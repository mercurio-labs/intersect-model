//! One continuous volume with a constant rate and a one-shot rising threshold.
use super::{AnalysisError, ErrorCode, Quantity, Source, Unit, finite};
use crate::{
    Budgets, EndReason, EventEffect, Input, Policy, Profile, RunError, Semantics, StepContext,
    Time, Trace, TraceMetadata, run_traced,
};
use serde::{Deserialize, Serialize};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const MAX_SAMPLES: u64 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Simulation {
    pub initial: Quantity,
    pub rate: Quantity,
    pub threshold: Quantity,
    pub next_rate: Quantity,
    pub end: Quantity,
    pub interval: Quantity,
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub time: Quantity,
    pub volume: Quantity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationResult {
    pub samples: Vec<Sample>,
    pub crossing: Option<Quantity>,
    pub trace: Trace,
}

pub fn simulate(request: &Simulation) -> Result<SimulationResult, AnalysisError> {
    let source = request.source.as_ref();
    let invalid = |message| AnalysisError::new(ErrorCode::Invalid, message, source);
    let initial = request.initial.in_unit(Unit::Litre, source)?;
    let threshold = request.threshold.in_unit(Unit::Litre, source)?;
    let rate = request.rate.in_unit(Unit::LitresPerSecond, source)?;
    let next_rate = request.next_rate.in_unit(Unit::LitresPerSecond, source)?;
    let end = ticks(request.end.in_unit(Unit::Second, source)?, source)?;
    let interval = ticks(request.interval.in_unit(Unit::Second, source)?, source)?;
    if interval == 0 {
        return Err(invalid("sample interval must be at least one nanosecond"));
    }
    let sample_count = end / interval + 1 + u64::from(!end.is_multiple_of(interval));
    if sample_count > MAX_SAMPLES {
        return Err(AnalysisError::new(
            ErrorCode::Budget,
            "sample count limit exceeded",
            source,
        ));
    }
    let crossing_time = if initial >= threshold {
        Some(0.0)
    } else if rate > 0.0 {
        Some((threshold - initial) / rate)
    } else {
        None
    }
    .filter(|time| *time <= seconds(end));
    let crossing = crossing_time.map(|time| ticks(time, source)).transpose()?;
    let mut inputs = Vec::new();
    // A threshold event at the same tick as a sample is delivered first.
    if let Some(time) = crossing {
        inputs.push(Input {
            time,
            event: Event::Crossing,
        });
    }
    inputs.extend((0..=end / interval).map(|index| Input {
        time: index * interval,
        event: Event::Sample,
    }));
    if !end.is_multiple_of(interval) {
        inputs.push(Input {
            time: end,
            event: Event::Sample,
        });
    }
    let semantics = Rate {
        initial,
        rate,
        next_rate,
        threshold,
        crossing_time,
    };
    let profile = Profile {
        name: "constant-rate threshold".into(),
        language_profile: "intersect.analysis.constant-rate.v1".into(),
        nanos_per_tick: 1,
        policy: Policy::First,
        seed: 0,
        budgets: Budgets {
            steps_per_instant: 1,
            total_steps: 1,
            events: sample_count + 1,
            end_time: end,
        },
    };
    let (outcome, trace) = run_traced(&semantics, &profile, inputs, TraceMetadata::default())
        .map_err(|error| AnalysisError::new(ErrorCode::Invalid, error.to_string(), source))?;
    if outcome.reason != EndReason::EndTime {
        return Err(AnalysisError::new(
            ErrorCode::Budget,
            "simulation stopped before the end time",
            source,
        ));
    }
    Ok(SimulationResult {
        samples: outcome.state.samples,
        crossing: crossing.map(time_quantity),
        trace,
    })
}

fn ticks(time: f64, source: Option<&Source>) -> Result<Time, AnalysisError> {
    let ticks = (time * NANOS_PER_SECOND as f64).round();
    // Above this bound an f64 cannot retain individual nanosecond ticks.
    if !ticks.is_finite() || time < 0.0 || ticks >= (1_u64 << 53) as f64 {
        return Err(AnalysisError::new(
            ErrorCode::Invalid,
            "time falls outside the supported simulation range",
            source,
        ));
    }
    Ok(ticks as Time)
}

fn seconds(time: Time) -> f64 {
    time as f64 / NANOS_PER_SECOND as f64
}

fn time_quantity(time: Time) -> Quantity {
    Quantity {
        value: seconds(time),
        unit: Unit::Second,
    }
}

#[derive(Debug)]
enum Event {
    Crossing,
    Sample,
}

struct Rate {
    initial: f64,
    rate: f64,
    next_rate: f64,
    threshold: f64,
    crossing_time: Option<f64>,
}

#[derive(Default)]
struct State {
    samples: Vec<Sample>,
    last: Option<Sample>,
    crossed: bool,
}

impl Semantics for Rate {
    type State = State;
    type Step = ();
    type Event = Event;

    fn initial(&self) -> Result<State, RunError> {
        Ok(State::default())
    }

    fn enabled(&self, _: &State, _: Time) -> Result<Vec<()>, RunError> {
        Ok(Vec::new())
    }

    fn fire(&self, _: &mut State, _: &(), _: &mut StepContext<'_, Event>) -> Result<(), RunError> {
        Err(RunError::Semantics(
            "constant-rate simulation has no autonomous steps".into(),
        ))
    }

    fn on_event(
        &self,
        state: &mut State,
        event: &Event,
        ctx: &mut StepContext<'_, Event>,
    ) -> Result<EventEffect, RunError> {
        let time = ctx.now();
        if matches!(event, Event::Crossing) {
            state.crossed = true;
        }
        let value = if state.crossed {
            let elapsed = (seconds(time) - self.crossing_time.unwrap_or_default()).max(0.0);
            self.initial.max(self.threshold) + elapsed * self.next_rate
        } else {
            self.initial + seconds(time) * self.rate
        };
        let value = finite(value, None).map_err(|error| RunError::Semantics(error.to_string()))?;
        let sample = Sample {
            time: time_quantity(time),
            volume: Quantity {
                value,
                unit: Unit::Litre,
            },
        };
        state.last = Some(sample.clone());
        if matches!(event, Event::Sample) {
            state.samples.push(sample);
        }
        Ok(EventEffect::Applied)
    }

    fn observe(&self, state: &State) -> Result<serde_json::Value, RunError> {
        Ok(serde_json::json!({ "crossed": state.crossed, "sample": state.last }))
    }
}
