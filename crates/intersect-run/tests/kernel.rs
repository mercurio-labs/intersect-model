use intersect_run::{
    Budgets, EndReason, EventEffect, Input, Policy, Profile, RunError, Semantics, StepContext,
    Time, Trace, TraceKind, TraceMetadata, run, run_traced,
};
use serde_json::{Value, json};

#[derive(Debug)]
enum Event {
    Prime,
    Record(&'static str),
    Reject,
    Overflow,
    Past,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    A,
    B,
}

#[derive(Clone, Copy)]
enum Mode {
    Timer,
    Conflict,
    Repeat,
}

impl Semantics for Mode {
    type State = Vec<&'static str>;
    type Step = Step;
    type Event = Event;

    fn initial(&self) -> Result<Self::State, RunError> {
        Ok(Vec::new())
    }

    fn enabled(&self, state: &Self::State, _: Time) -> Result<Vec<Step>, RunError> {
        Ok(match self {
            Self::Conflict if state.is_empty() => vec![Step::B, Step::A],
            Self::Repeat => vec![Step::A],
            _ => vec![],
        })
    }

    fn fire(
        &self,
        state: &mut Self::State,
        step: &Step,
        _: &mut StepContext<'_, Event>,
    ) -> Result<(), RunError> {
        state.push(match step {
            Step::A => "a",
            Step::B => "b",
        });
        Ok(())
    }

    fn on_event(
        &self,
        state: &mut Self::State,
        event: &Event,
        ctx: &mut StepContext<'_, Event>,
    ) -> Result<EventEffect, RunError> {
        match event {
            Event::Prime => {
                ctx.schedule_after(3, 1, Event::Record("late"))?;
                ctx.schedule_after(3, 0, Event::Record("first tie"))?;
                ctx.schedule_after(3, -1, Event::Record("early"))?;
                ctx.schedule_at(3, 0, Event::Record("second tie"))?;
                ctx.schedule_at(ctx.now(), 0, Event::Record("same tick"))?;
            }
            Event::Record(label) => state.push(label),
            Event::Reject => return Ok(EventEffect::Rejected("input disabled".into())),
            Event::Overflow => ctx.schedule_after(u64::MAX, 0, Event::Reject)?,
            Event::Past => ctx.schedule_at(0, 0, Event::Reject)?,
        }
        Ok(EventEffect::Applied)
    }

    fn observe(&self, state: &Self::State) -> Result<Value, RunError> {
        Ok(json!(state))
    }
}

fn profile() -> Profile {
    Profile {
        name: "test".into(),
        language_profile: "counter.v1".into(),
        nanos_per_tick: Profile::NANOS_PER_MILLISECOND,
        policy: Policy::First,
        seed: 0,
        budgets: Budgets {
            steps_per_instant: 10,
            total_steps: 20,
            events: 20,
            end_time: 100,
        },
    }
}

#[test]
fn timers_order_by_time_priority_and_insertion() -> Result<(), RunError> {
    let outcome = run(
        &Mode::Timer,
        &profile(),
        [
            Input {
                time: 3,
                event: Event::Record("scenario"),
            },
            Input {
                time: 0,
                event: Event::Prime,
            },
            Input {
                time: 2,
                event: Event::Record("tick two"),
            },
        ],
        &mut (),
    )?;
    assert_eq!(
        outcome.state,
        [
            "same tick",
            "tick two",
            "early",
            "scenario",
            "first tie",
            "second tie",
            "late"
        ]
    );
    assert_eq!(outcome.time, 3);
    assert_eq!(outcome.reason, EndReason::Quiescence);
    Ok(())
}

#[test]
fn seeded_conflict_is_repeatable_and_differs_from_first() -> Result<(), Box<dyn std::error::Error>>
{
    let mut profile = profile();
    let first = run(&Mode::Conflict, &profile, [], &mut ())?;
    assert_eq!(first.state, ["a"]);
    profile.policy = Policy::Seeded;
    let metadata = TraceMetadata {
        model_digest: Some("test-model".into()),
        language_data: None,
    };
    let (seeded, trace) = run_traced(&Mode::Conflict, &profile, [], metadata.clone())?;
    let (_, repeated) = run_traced(&Mode::Conflict, &profile, [], metadata)?;
    assert_eq!(seeded.state, ["b"]);
    let bytes = serde_json::to_vec(&trace)?;
    assert_eq!(bytes, serde_json::to_vec(&repeated)?);
    assert_eq!(serde_json::from_slice::<Trace>(&bytes)?, trace);
    assert_eq!(trace.header.format, Trace::FORMAT);
    assert_eq!(trace.header.profile, profile);
    assert_eq!(
        trace.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    Ok(())
}

#[test]
fn each_budget_stops_before_the_next_action() -> Result<(), RunError> {
    let mut profile = profile();
    profile.budgets.steps_per_instant = 2;
    let outcome = run(&Mode::Repeat, &profile, [], &mut ())?;
    assert_eq!(
        (outcome.reason, outcome.steps),
        (EndReason::StepsPerInstant, 2)
    );

    profile.budgets.total_steps = 1;
    let outcome = run(&Mode::Repeat, &profile, [], &mut ())?;
    assert_eq!((outcome.reason, outcome.steps), (EndReason::TotalSteps, 1));

    profile.budgets.events = 1;
    let outcome = run(
        &Mode::Timer,
        &profile,
        [Input {
            time: 0,
            event: Event::Prime,
        }],
        &mut (),
    )?;
    assert_eq!((outcome.reason, outcome.events), (EndReason::Events, 1));

    profile.budgets.events = 10;
    profile.budgets.end_time = 2;
    let outcome = run(
        &Mode::Timer,
        &profile,
        [
            Input {
                time: 3,
                event: Event::Record("past end"),
            },
            Input {
                time: 2,
                event: Event::Record("at end"),
            },
        ],
        &mut (),
    )?;
    assert_eq!((outcome.reason, outcome.time), (EndReason::EndTime, 2));
    assert_eq!(outcome.state, ["at end"]);
    Ok(())
}

#[test]
fn rejected_input_is_recorded_without_changing_state() -> Result<(), RunError> {
    let (outcome, trace) = run_traced(
        &Mode::Timer,
        &profile(),
        [Input {
            time: 4,
            event: Event::Reject,
        }],
        TraceMetadata::default(),
    )?;
    assert_eq!(outcome.events, 1);
    assert!(outcome.state.is_empty());
    assert_eq!(trace.entries[1].kind, TraceKind::TimeAdvanced);
    let entry = &trace.entries[2];
    assert_eq!(entry.kind, TraceKind::InputRejected);
    assert_eq!(entry.time, 4);
    assert_eq!(entry.label, "Reject: input disabled");
    assert_eq!(entry.observation, Some(json!([])));
    Ok(())
}

#[test]
fn rejects_impossible_event_times() {
    for (event, expected) in [
        (
            Event::Past,
            RunError::PastEvent {
                now: 1,
                requested: 0,
            },
        ),
        (
            Event::Overflow,
            RunError::TimeOverflow {
                now: 1,
                delay: u64::MAX,
            },
        ),
    ] {
        let result = run(
            &Mode::Timer,
            &profile(),
            [Input { time: 1, event }],
            &mut (),
        );
        assert_eq!(result.err(), Some(expected));
    }
}
