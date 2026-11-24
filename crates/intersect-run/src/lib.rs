//! Handwritten language-neutral discrete-event execution kernel.
pub mod analysis;
pub mod coupled_ode;
pub mod numerics;
mod parametric;
pub mod state_machine;
mod trace;

pub use parametric::{CaseError, CaseOutcome, Parameters, RunCase, RunCases, run_cases};
pub use trace::{Observer, Trace, TraceEntry, TraceHeader, TraceKind, TraceMetadata, run_traced};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt};

/// Simulation time in ticks of the profile's declared resolution.
pub type Time = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    First,
    Seeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    pub steps_per_instant: u64,
    pub total_steps: u64,
    pub events: u64,
    /// Last tick at which events and steps may run, inclusive.
    pub end_time: Time,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub language_profile: String,
    pub nanos_per_tick: u64,
    pub policy: Policy,
    pub seed: u64,
    pub budgets: Budgets,
}

impl Profile {
    pub const NANOS_PER_MILLISECOND: u64 = 1_000_000;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    InvalidResolution,
    PastEvent { now: Time, requested: Time },
    TimeOverflow { now: Time, delay: Time },
    SequenceOverflow,
    Semantics(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidResolution => f.write_str("time resolution must be positive"),
            Self::PastEvent { now, requested } => {
                write!(f, "cannot schedule tick {requested} from tick {now}")
            }
            Self::TimeOverflow { now, delay } => {
                write!(f, "tick {now} plus delay {delay} exceeds the time range")
            }
            Self::SequenceOverflow => f.write_str("run sequence counter exhausted"),
            Self::Semantics(message) => f.write_str(message),
        }
    }
}

impl Error for RunError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Quiescence,
    EndTime,
    StepsPerInstant,
    TotalSteps,
    Events,
}

#[derive(Debug)]
pub struct RunOutcome<S> {
    pub state: S,
    pub time: Time,
    pub steps: u64,
    pub events: u64,
    pub reason: EndReason,
}

#[derive(Debug, Clone)]
pub struct Input<E> {
    pub time: Time,
    pub event: E,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventEffect {
    Applied,
    Rejected(String),
}

/// Semantics supplies deterministic behavior, including step ordering, labels
/// and observations. Keep the model immutable and store changes in State.
pub trait Semantics {
    type State;
    type Step: Ord + fmt::Debug;
    type Event: fmt::Debug;

    fn initial(&self) -> Result<Self::State, RunError>;
    fn enabled(&self, state: &Self::State, now: Time) -> Result<Vec<Self::Step>, RunError>;
    fn fire(
        &self,
        state: &mut Self::State,
        step: &Self::Step,
        ctx: &mut StepContext<'_, Self::Event>,
    ) -> Result<(), RunError>;
    fn on_event(
        &self,
        state: &mut Self::State,
        event: &Self::Event,
        ctx: &mut StepContext<'_, Self::Event>,
    ) -> Result<EventEffect, RunError>;
    fn observe(&self, state: &Self::State) -> Result<Value, RunError>;
}

struct Queue<E> {
    entries: BTreeMap<(Time, i32, u64), E>,
    next_sequence: u64,
}

impl<E> Queue<E> {
    fn push(&mut self, time: Time, priority: i32, event: E) -> Result<(), RunError> {
        let sequence = self.next_sequence;
        self.next_sequence = sequence.checked_add(1).ok_or(RunError::SequenceOverflow)?;
        self.entries.insert((time, priority, sequence), event);
        Ok(())
    }

    fn next_time(&self) -> Option<Time> {
        self.entries.first_key_value().map(|(key, _)| key.0)
    }
}

pub struct StepContext<'a, E> {
    now: Time,
    queue: &'a mut Queue<E>,
}

impl<E> StepContext<'_, E> {
    pub fn now(&self) -> Time {
        self.now
    }

    /// Schedule at or after the current tick. Lower priorities come first.
    pub fn schedule_at(&mut self, time: Time, priority: i32, event: E) -> Result<(), RunError> {
        if time < self.now {
            return Err(RunError::PastEvent {
                now: self.now,
                requested: time,
            });
        }
        self.queue.push(time, priority, event)
    }

    pub fn schedule_after(&mut self, delay: Time, priority: i32, event: E) -> Result<(), RunError> {
        let time = self.now.checked_add(delay).ok_or(RunError::TimeOverflow {
            now: self.now,
            delay,
        })?;
        self.schedule_at(time, priority, event)
    }
}

// SplitMix64 with rejection sampling for a uniform index.
fn seeded_index(seed: &mut u64, len: usize) -> usize {
    let bound = len as u64;
    let threshold = bound.wrapping_neg() % bound;
    loop {
        *seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = *seed;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        if value >= threshold {
            return (value % bound) as usize;
        }
    }
}

/// Execute one run. Events at the current tick take precedence over step
/// selection, including events scheduled for the current tick during a step.
pub fn run<S: Semantics>(
    semantics: &S,
    profile: &Profile,
    inputs: impl IntoIterator<Item = Input<S::Event>>,
    observer: &mut impl Observer,
) -> Result<RunOutcome<S::State>, RunError> {
    if profile.nanos_per_tick == 0 {
        return Err(RunError::InvalidResolution);
    }

    let mut queue = Queue {
        entries: BTreeMap::new(),
        next_sequence: 0,
    };
    for input in inputs {
        queue.push(input.time, 0, input.event)?;
    }

    let mut now = 0;
    let mut steps = 0;
    let mut events = 0;
    let mut instant_steps = 0;
    let mut seed = profile.seed;
    let budgets = profile.budgets;
    let mut state = semantics.initial()?;
    let mut log = trace::Log::new(observer);
    log.emit(
        now,
        TraceKind::RunStart,
        &profile.name,
        Some(semantics.observe(&state)?),
    )?;

    let reason = loop {
        if queue.next_time() == Some(now) {
            if events == budgets.events {
                break EndReason::Events;
            }
            if let Some((_, event)) = queue.entries.pop_first() {
                let mut ctx = StepContext {
                    now,
                    queue: &mut queue,
                };
                let effect = semantics.on_event(&mut state, &event, &mut ctx)?;
                events += 1;
                let (kind, label) = match effect {
                    EventEffect::Applied => (TraceKind::EventDelivered, format!("{event:?}")),
                    EventEffect::Rejected(reason) => {
                        (TraceKind::InputRejected, format!("{event:?}: {reason}"))
                    }
                };
                log.emit(now, kind, label, Some(semantics.observe(&state)?))?;
            }
            continue;
        }

        let mut enabled = semantics.enabled(&state, now)?;
        if !enabled.is_empty() {
            if instant_steps == budgets.steps_per_instant {
                break EndReason::StepsPerInstant;
            }
            if steps == budgets.total_steps {
                break EndReason::TotalSteps;
            }
            enabled.sort();
            let index = match profile.policy {
                Policy::First => 0,
                Policy::Seeded => seeded_index(&mut seed, enabled.len()),
            };
            let step = &enabled[index];
            let mut ctx = StepContext {
                now,
                queue: &mut queue,
            };
            semantics.fire(&mut state, step, &mut ctx)?;
            steps += 1;
            instant_steps += 1;
            log.emit(
                now,
                TraceKind::StepFired,
                format!("{step:?}"),
                Some(semantics.observe(&state)?),
            )?;
            continue;
        }

        if now == budgets.end_time {
            break EndReason::EndTime;
        }
        let Some(next) = queue.next_time() else {
            break EndReason::Quiescence;
        };
        now = next.min(budgets.end_time);
        instant_steps = 0;
        log.emit(now, TraceKind::TimeAdvanced, "", None)?;
    };

    log.emit(now, TraceKind::RunEnd, format!("{reason:?}"), None)?;
    Ok(RunOutcome {
        state,
        time: now,
        steps,
        events,
        reason,
    })
}
