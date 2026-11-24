use intersect_run::{
    RunError,
    coupled_ode::{Crossing, ObservationKind, Settings, simulate},
    state_machine::{Chart, ChartSpec, Node, Transition},
};

fn chart(states: &[&str], transitions: &[(&str, &str, &str)]) -> Result<Chart, RunError> {
    Chart::new(ChartSpec {
        initial: states[0].into(),
        states: states
            .iter()
            .map(|&id| Node {
                id: id.into(),
                parent: None,
                initial: None,
            })
            .collect(),
        transitions: transitions
            .iter()
            .map(|&(from, event, to)| Transition {
                id: Some(format!("{from}-{to}")),
                from: from.into(),
                event: event.into(),
                to: to.into(),
            })
            .collect(),
        immediate: true,
        clock_state: None,
    })
}

fn crossing(event: &str, level: f64, rising: bool) -> Crossing {
    Crossing {
        event: event.into(),
        level,
        rising,
    }
}

#[test]
fn thermostat_preserves_sample_grid_and_matches_closed_form_crossing_intervals()
-> Result<(), RunError> {
    let thermostat = chart(
        &["Heating", "Idle"],
        &[
            ("Heating", "tooHot", "Idle"),
            ("Idle", "tooCool", "Heating"),
        ],
    )?;
    let settings = Settings {
        end_ms: 1_800_000,
        sample_ms: 15_000,
        max_step_ms: 2_000,
    };
    let crossings = [
        crossing("tooHot", 40.0, true),
        crossing("tooCool", 38.0, false),
    ];
    for (power, expected_first, expected_crossings) in [(120.0, 430_023, 23), (250.0, 122_599, 83)]
    {
        let observations = simulate(
            &thermostat,
            settings,
            20.0,
            &crossings,
            |_, value, state| {
                let applied = if state == "Heating" { power } else { 0.0 };
                Ok((applied - 5.0 * (value - 20.0)) / 1_200.0)
            },
        )?;
        let samples = observations
            .iter()
            .filter(|observation| observation.kind == ObservationKind::Sample)
            .map(|observation| observation.at)
            .collect::<Vec<_>>();
        assert_eq!(samples, (0..=1_800_000).step_by(15_000).collect::<Vec<_>>());
        let switches = observations
            .iter()
            .filter(|observation| matches!(observation.kind, ObservationKind::Crossing { .. }))
            .collect::<Vec<_>>();
        assert_eq!(switches.len(), expected_crossings);
        assert_eq!(switches[0].at, expected_first);

        let (mut previous_time, mut previous_value) = (0, 20.0);
        for (index, observation) in switches.into_iter().enumerate() {
            let (event, from, to, level, applied) = if index % 2 == 0 {
                ("tooHot", "Heating", "Idle", 40.0, power)
            } else {
                ("tooCool", "Idle", "Heating", 38.0, 0.0)
            };
            assert_eq!(observation.state, to);
            assert_eq!(
                observation.kind,
                ObservationKind::Crossing {
                    event: event.into(),
                    transition: format!("{from}-{to}"),
                },
            );
            assert!((observation.value - level).abs() < 0.0002);
            let equilibrium = 20.0 + applied / 5.0;
            let expected_seconds =
                -240.0 * ((level - equilibrium) / (previous_value - equilibrium)).ln();
            let actual_seconds = (observation.at - previous_time) as f64 / 1_000.0;
            assert!(
                (actual_seconds - expected_seconds).abs() <= 0.001,
                "crossing {index} at {} for {power} W: {actual_seconds} s vs {expected_seconds} s",
                observation.at,
            );
            (previous_time, previous_value) = (observation.at, observation.value);
        }
    }
    Ok(())
}

#[test]
fn same_tick_crossings_dispatch_in_declaration_order_before_sample() -> Result<(), RunError> {
    let pump = chart(
        &["Filling", "Held", "Stopped"],
        &[("Filling", "hold", "Held"), ("Held", "stop", "Stopped")],
    )?;
    let observations = simulate(
        &pump,
        Settings {
            end_ms: 4_000,
            sample_ms: 2_000,
            max_step_ms: 500,
        },
        0.0,
        &[crossing("hold", 2.0, true), crossing("stop", 2.0, true)],
        |_, _, state| Ok(if state == "Filling" { 1.0 } else { 0.0 }),
    )?;
    let coincident = observations
        .iter()
        .filter(|observation| observation.at == 2_000)
        .collect::<Vec<_>>();
    assert_eq!(coincident.len(), 3);
    assert_eq!(coincident[0].state, "Held");
    assert_eq!(coincident[1].state, "Stopped");
    assert_eq!(coincident[2].state, "Stopped");
    assert_eq!(coincident[2].kind, ObservationKind::Sample);
    assert!(
        coincident
            .iter()
            .all(|observation| observation.value == 2.0)
    );
    assert_eq!(observations.last().unwrap().value, 2.0);
    Ok(())
}

#[test]
fn validates_crossings_and_preserves_derivative_failures() -> Result<(), RunError> {
    let pump = chart(&["Filling", "Stopped"], &[("Filling", "stop", "Stopped")])?;
    let settings = Settings {
        end_ms: 4_000,
        sample_ms: 2_000,
        max_step_ms: 500,
    };
    let invokes = |crossings: &[Crossing]| {
        let mut calls = 0;
        let result = simulate(&pump, settings, 0.0, crossings, |_, _, _| {
            calls += 1;
            Err(RunError::Semantics(
                "Derivative refused the supplied input".into(),
            ))
        });
        (result, calls)
    };
    for invalid in [
        vec![crossing("absent", 2.0, true)],
        vec![crossing("stop", 2.0, true), crossing("stop", 3.0, true)],
        vec![crossing("stop", f64::NAN, true)],
    ] {
        let (result, calls) = invokes(&invalid);
        assert!(result.is_err());
        assert_eq!(calls, 0);
    }
    let (result, calls) = invokes(&[crossing("stop", 2.0, true)]);
    assert_eq!(calls, 1);
    assert_eq!(
        result.unwrap_err().to_string(),
        "Derivative refused the supplied input",
    );
    Ok(())
}
