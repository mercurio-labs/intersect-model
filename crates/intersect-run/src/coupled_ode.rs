//! Handwritten coordination of one continuous value and a Run state machine.
//!
//! Chart transitions are driven by threshold crossings on the millisecond
//! clock. Adapters supply the derivative, crossing definitions and state IDs.

use crate::{
    RunError, Time,
    numerics::{first_crossing_tick, rk4},
    state_machine::{Chart, EventId},
};
use std::collections::BTreeSet;

const MAX_END_MS: Time = 86_400_000;
const MAX_OBSERVATIONS: usize = 8_192;
const MAX_INTEGRATIONS: Time = 262_144;
const MAX_CROSSINGS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub end_ms: Time,
    pub sample_ms: Time,
    pub max_step_ms: Time,
}

#[derive(Clone, Debug)]
pub struct Crossing {
    pub event: String,
    pub level: f64,
    pub rising: bool,
}

impl Crossing {
    fn crossed(&self, start: f64, end: f64) -> bool {
        if self.rising {
            start < self.level && end >= self.level
        } else {
            start > self.level && end <= self.level
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub at: Time,
    pub value: f64,
    pub state: String,
    pub kind: ObservationKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationKind {
    Sample,
    Crossing { event: String, transition: String },
}

/// Simulate one scalar value using fourth-order Runge–Kutta. The callback
/// receives seconds, the continuous value, and the active leaf state ID. It
/// returns a derivative in value units per second and may be called at trial
/// positions during crossing localization.
///
/// The first sample is at tick zero. A crossing brackets between integration
/// endpoints, then resolves to the first qualifying millisecond. At the same
/// tick, crossings dispatch in declaration order before the regular sample.
/// An event with no transition in the active state leaves the state unchanged
/// and contributes no crossing observation.
pub fn simulate(
    chart: &Chart,
    settings: Settings,
    initial: f64,
    crossings: &[Crossing],
    mut derivative: impl FnMut(f64, f64, &str) -> Result<f64, RunError>,
) -> Result<Vec<Observation>, RunError> {
    validate(settings, initial, crossings.len())?;
    let events = resolve_events(chart, crossings)?;
    let mut state = chart.initial();
    let mut at = 0;
    let mut value = initial;
    let mut next_sample = settings.sample_ms;
    let mut integrations = MAX_INTEGRATIONS;
    let mut observations = vec![Observation {
        at,
        value,
        state: chart.active_leaf(&state).to_owned(),
        kind: ObservationKind::Sample,
    }];

    while at < settings.end_ms {
        let boundary = next_sample.min(at + settings.max_step_ms);
        let leaf = chart.active_leaf(&state);
        let mut integrate_to =
            |until| integrate(at, until, value, leaf, &mut integrations, &mut derivative);
        let integrated = integrate_to(boundary)?;
        let mut earliest = boundary;
        let mut hits = Vec::new();
        for (index, crossing) in crossings.iter().enumerate() {
            if crossing.crossed(value, integrated) {
                let tick = first_crossing_tick(at, boundary, |tick| {
                    integrate_to(tick).map(|trial| crossing.crossed(value, trial))
                })?;
                if tick < earliest {
                    earliest = tick;
                    hits.clear();
                }
                if tick == earliest {
                    hits.push(index);
                }
            }
        }
        value = if earliest == boundary {
            integrated
        } else {
            integrate_to(earliest)?
        };
        at = earliest;

        for index in hits {
            if chart.dispatch(&mut state, events[index], at, |_, _, _| Ok(true))? {
                let transition = chart
                    .last_transition(&state)
                    .ok_or_else(|| error("Crossing applied without a transition identity"))?;
                record(
                    &mut observations,
                    at,
                    value,
                    chart.active_leaf(&state),
                    ObservationKind::Crossing {
                        event: crossings[index].event.clone(),
                        transition: transition.to_owned(),
                    },
                )?;
            }
        }
        if at == next_sample {
            record(
                &mut observations,
                at,
                value,
                chart.active_leaf(&state),
                ObservationKind::Sample,
            )?;
            next_sample += settings.sample_ms;
        }
    }
    Ok(observations)
}

fn validate(settings: Settings, initial: f64, crossings: usize) -> Result<(), RunError> {
    if !initial.is_finite()
        || settings.sample_ms == 0
        || settings.end_ms == 0
        || settings.end_ms > MAX_END_MS
        || !settings.end_ms.is_multiple_of(settings.sample_ms)
        || settings.max_step_ms == 0
        || settings.max_step_ms > MAX_END_MS
        || crossings == 0
        || crossings > MAX_CROSSINGS
    {
        return Err(error("Invalid coupled ODE controls or initial value"));
    }
    if settings.end_ms / settings.sample_ms >= MAX_OBSERVATIONS as Time
        || settings.end_ms.div_ceil(settings.max_step_ms) > MAX_INTEGRATIONS
    {
        return Err(error(
            "Coupled ODE scenario exceeds the integration or trace limit",
        ));
    }
    Ok(())
}

fn resolve_events(chart: &Chart, crossings: &[Crossing]) -> Result<Vec<EventId>, RunError> {
    let mut seen = BTreeSet::new();
    crossings
        .iter()
        .map(|crossing| {
            if !crossing.level.is_finite() || !seen.insert(&crossing.event) {
                return Err(error(format!(
                    "Invalid or duplicate crossing for event {}",
                    crossing.event
                )));
            }
            chart
                .event_id(&crossing.event)
                .ok_or_else(|| error(format!("Unbound crossing event {}", crossing.event)))
        })
        .collect()
}

fn integrate(
    at: Time,
    until: Time,
    value: f64,
    state: &str,
    remaining: &mut Time,
    derivative: &mut impl FnMut(f64, f64, &str) -> Result<f64, RunError>,
) -> Result<f64, RunError> {
    *remaining = remaining
        .checked_sub(1)
        .ok_or_else(|| error("Coupled ODE integration limit reached"))?;
    let span = (until - at) as f64;
    let mut callback_error = None;
    let result = rk4(0.0, span, &[value], span, 1, |offset, input, slopes| {
        let seconds = (at as f64 + offset) / 1_000.0;
        match derivative(seconds, input[0], state) {
            Ok(rate) => slopes[0] = rate / 1_000.0,
            Err(error) => callback_error = Some(error),
        }
    });
    match callback_error {
        Some(error) => Err(error),
        None => result
            .map(|values| values[0])
            .map_err(|issue| error(issue.to_string())),
    }
}

fn record(
    observations: &mut Vec<Observation>,
    at: Time,
    value: f64,
    state: &str,
    kind: ObservationKind,
) -> Result<(), RunError> {
    if observations.len() == MAX_OBSERVATIONS {
        return Err(error("Coupled ODE trace limit reached"));
    }
    observations.push(Observation {
        at,
        value,
        state: state.to_owned(),
        kind,
    });
    Ok(())
}

fn error(message: impl Into<String>) -> RunError {
    RunError::Semantics(message.into())
}
