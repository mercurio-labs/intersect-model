use intersect_run::{
    Budgets, Policy, Profile, RunError, TraceKind, TraceMetadata,
    state_machine::{Chart, ChartSpec, Node, TimedEvent, Transition},
};
use serde_json::json;

fn node(id: &str, parent: Option<&str>, initial: Option<&str>) -> Node {
    Node {
        id: id.into(),
        parent: parent.map(str::to_owned),
        initial: initial.map(str::to_owned),
    }
}

fn transition(id: &str, from: &str, event: &str, to: &str) -> Transition {
    Transition {
        id: Some(id.into()),
        from: from.into(),
        event: event.into(),
        to: to.into(),
    }
}

fn chart(immediate: bool) -> Result<Chart, RunError> {
    Chart::new(ChartSpec {
        initial: "normal".into(),
        states: vec![
            node("normal", None, None),
            node("boosted", None, Some("running")),
            node("running", Some("boosted"), None),
            node("stopped", None, None),
        ],
        transitions: vec![
            transition("boost", "normal", "Boost", "boosted"),
            transition("reset-child", "running", "Reset", "stopped"),
            transition("reset-parent", "boosted", "Reset", "normal"),
        ],
        immediate,
        clock_state: Some("boosted".into()),
    })
}

fn profile(end_time: u64) -> Profile {
    Profile {
        name: "chart".into(),
        language_profile: "chart.v1".into(),
        nanos_per_tick: Profile::NANOS_PER_MILLISECOND,
        policy: Policy::First,
        seed: 0,
        budgets: Budgets {
            steps_per_instant: 10,
            total_steps: 20,
            events: 20,
            end_time,
        },
    }
}

fn input(at: u64, event: &str) -> TimedEvent {
    TimedEvent {
        at,
        event: event.into(),
    }
}

#[test]
fn deferred_trace_uses_parent_fallback_and_tracks_time_in_the_active_path() -> Result<(), RunError>
{
    let chart = chart(false)?;
    let scenario = chart.prepare_scenario(vec![
        input(1, "Reset"),
        input(2, "Boost"),
        input(3, "Boost"),
        input(4, "Reset"),
        input(7, "Boost"),
        input(9, "Unknown"),
    ])?;
    assert_eq!(scenario.end(), 10);
    let trace = chart.run(
        &scenario,
        &profile(scenario.end()),
        TraceMetadata::default(),
        |source, event, target| Ok((source, event, target) != ("running", "Reset", "stopped")),
    )?;
    let observed = trace
        .entries
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                TraceKind::RunStart | TraceKind::StepFired | TraceKind::InputRejected
            )
        })
        .map(|entry| (entry.time, entry.kind, entry.observation))
        .collect::<Vec<_>>();
    use TraceKind::{InputRejected, RunStart, StepFired};
    assert_eq!(
        observed,
        [
            (
                0,
                RunStart,
                Some(json!({"active": ["normal"], "elapsed": 0, "transition": null}))
            ),
            (
                1,
                InputRejected,
                Some(json!({"active": ["normal"], "elapsed": 0, "transition": null}))
            ),
            (
                2,
                StepFired,
                Some(
                    json!({"active": ["boosted", "running"], "elapsed": 0, "transition": "boost"})
                )
            ),
            (
                3,
                InputRejected,
                Some(json!({"active": ["boosted", "running"], "elapsed": 1, "transition": null}))
            ),
            (
                4,
                StepFired,
                Some(json!({"active": ["normal"], "elapsed": 2, "transition": "reset-parent"}))
            ),
            (
                7,
                StepFired,
                Some(
                    json!({"active": ["boosted", "running"], "elapsed": 2, "transition": "boost"})
                )
            ),
            (
                9,
                InputRejected,
                Some(json!({"active": ["boosted", "running"], "elapsed": 4, "transition": null}))
            ),
        ]
    );
    Ok(())
}

#[test]
fn immediate_events_at_one_tick_follow_input_order() -> Result<(), RunError> {
    let chart = chart(true)?;
    let scenario = chart.prepare_scenario(vec![input(3, "Boost"), input(3, "Reset")])?;
    let trace = chart.run(
        &scenario,
        &profile(scenario.end()),
        TraceMetadata::default(),
        |_, _, _| Ok(true),
    )?;
    assert!(
        !trace
            .entries
            .iter()
            .any(|entry| entry.kind == TraceKind::StepFired)
    );
    let observed: Vec<_> = trace
        .entries
        .iter()
        .filter(|entry| entry.kind == TraceKind::EventDelivered)
        .map(|entry| (entry.time, entry.label.as_str(), entry.observation.clone()))
        .collect();
    assert_eq!(
        observed,
        [
            (
                3,
                "Boost",
                Some(
                    json!({"active": ["boosted", "running"], "elapsed": 0, "transition": "boost"})
                )
            ),
            (
                3,
                "Reset",
                Some(json!({"active": ["stopped"], "elapsed": 0, "transition": "reset-child"}))
            ),
        ]
    );
    Ok(())
}

#[test]
fn direct_dispatch_validates_event_time_and_preserves_state_after_a_guard_error()
-> Result<(), RunError> {
    let chart = chart(false)?;
    let mut state = chart.initial();
    let boost = chart.event_id("Boost").unwrap();
    let reset = chart.event_id("Reset").unwrap();
    assert!(chart.dispatch(&mut state, boost, 2, |_, _, _| Ok(true))?);
    assert_eq!(chart.active_leaf(&state), "running");
    assert!(Chart::active(&state, chart.state_id("boosted").unwrap()));
    assert_eq!(chart.last_transition(&state), Some("boost"));
    let error = RunError::Semantics("guard failed".into());
    assert_eq!(
        chart.dispatch(&mut state, reset, 5, |_, _, _| Err(error.clone())),
        Err(error)
    );
    assert_eq!(chart.last_transition(&state), Some("boost"));
    assert!(!chart.dispatch(&mut state, boost, 3, |_, _, _| Ok(true))?);
    assert_eq!(chart.active_leaf(&state), "running");
    assert_eq!(chart.last_transition(&state), None);
    assert_eq!(
        chart.dispatch(&mut state, reset, 2, |_, _, _| Ok(true)),
        Err(RunError::PastEvent {
            now: 3,
            requested: 2
        })
    );
    Ok(())
}
