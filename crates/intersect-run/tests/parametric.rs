use intersect_run::{
    Budgets, EventEffect, Input, Parameters, Policy, Profile, RunCase, RunCases, RunError,
    Semantics, StepContext, TraceKind, TraceMetadata, run_cases,
};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};

struct Sensor(u64);
impl Semantics for Sensor {
    type State = u64;
    type Step = ();
    type Event = u64;

    fn initial(&self) -> Result<u64, RunError> {
        Ok(0)
    }
    fn enabled(&self, _: &u64, _: u64) -> Result<Vec<()>, RunError> {
        Ok(vec![])
    }
    fn fire(&self, _: &mut u64, _: &(), _: &mut StepContext<'_, u64>) -> Result<(), RunError> {
        Err(RunError::Semantics("Unexpected internal step".into()))
    }
    fn on_event(
        &self,
        state: &mut u64,
        event: &u64,
        _: &mut StepContext<'_, u64>,
    ) -> Result<EventEffect, RunError> {
        *state += event * self.0;
        Ok(EventEffect::Applied)
    }
    fn observe(&self, state: &u64) -> Result<Value, RunError> {
        Ok(json!(state))
    }
}

#[test]
fn lazy_cases_isolate_failures_and_preserve_inputs_trace_and_seed()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = RunCases::explicit(
        [
            ("baseline", json!(2)),
            ("bad", json!("bad")),
            ("comparison", json!(3)),
        ]
        .into_iter()
        .map(|(id, gain)| RunCase {
            id: id.into(),
            parameters: [("gain".into(), gain)].into(),
        })
        .collect(),
        3,
    )?;
    let profile = Profile {
        name: "Sensor".into(),
        language_profile: "sensor.v1".into(),
        nanos_per_tick: 1000,
        policy: Policy::Seeded,
        seed: 82,
        budgets: Budgets {
            steps_per_instant: 1,
            total_steps: 1,
            events: 2,
            end_time: 5,
        },
    };
    let preparations = Cell::new(0);
    let prepare = |parameters: &Parameters| {
        preparations.set(preparations.get() + 1);
        let gain = parameters
            .get("gain")
            .and_then(Value::as_u64)
            .ok_or_else(|| RunError::Semantics("Gain must be an integer".into()))?;
        Ok((Sensor(gain), [Input { time: 1, event: 5 }]))
    };
    let mut runs = run_cases(&cases, &profile, TraceMetadata::default(), prepare);
    assert_eq!(preparations.get(), 0);
    let first = runs.next().ok_or("Missing baseline")?;
    assert_eq!(first.case.id, "baseline");
    let (baseline, trace) = first.result?;
    assert_eq!(baseline.state, 10);
    assert_eq!(trace.header.profile.seed, 82);
    assert_eq!(trace.entries[0].kind, TraceKind::RunStart);
    let bad = runs.next().ok_or("Missing rejected case")?;
    assert_eq!(bad.case.id, "bad");
    assert!(matches!(bad.result, Err(RunError::Semantics(_))));
    let comparison = runs.next().ok_or("Missing comparison")?;
    assert_eq!(comparison.case.parameters["gain"], json!(3));
    assert_eq!(comparison.result?.0.state, 15);
    assert!(runs.next().is_none());
    assert_eq!(preparations.get(), 3);
    Ok(())
}

#[test]
fn grids_are_ordered_and_validate_limits_and_names_before_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let axes = BTreeMap::from([
        ("power".into(), vec![json!(120), json!(250)]),
        ("loss".into(), vec![json!(5), json!(10)]),
    ]);
    let grid = RunCases::grid(Parameters::new(), axes.clone(), 4)?;
    let pairs: Vec<_> = grid
        .as_slice()
        .iter()
        .map(|case| (&case.parameters["loss"], &case.parameters["power"]))
        .collect();
    assert_eq!(
        pairs,
        [
            (&json!(5), &json!(120)),
            (&json!(5), &json!(250)),
            (&json!(10), &json!(120)),
            (&json!(10), &json!(250))
        ]
    );
    assert!(RunCases::grid(Parameters::new(), axes.clone(), 3).is_err());
    assert!(RunCases::grid(BTreeMap::from([("power".into(), json!(120))]), axes, 4).is_err());
    assert!(
        RunCases::grid(
            Parameters::new(),
            BTreeMap::from([("empty".into(), vec![])]),
            4
        )
        .is_err()
    );
    let one = RunCases::grid(Parameters::new(), BTreeMap::new(), 1)?;
    assert_eq!(one.as_slice().len(), 1);
    assert!(RunCases::explicit(vec![one.as_slice()[0].clone(); 2], 2).is_err());
    Ok(())
}
