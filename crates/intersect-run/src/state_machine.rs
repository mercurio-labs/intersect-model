//! Handwritten deterministic hierarchical state machines over the Run kernel.
//!
//! Adapters supply linked identities, guard evaluation, and the time and budget
//! policy. State paths, event selection, and clock accounting are shared here.

use crate::{
    EventEffect, Input, Profile, RunError, Semantics, StepContext, Time, Trace, TraceMetadata,
    run_traced,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Source specification for a state machine. A single transition may be
/// selected per active state and event, with a leaf checked before its parent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartSpec {
    pub initial: String,
    pub states: Vec<Node>,
    pub transitions: Vec<Transition>,
    /// During a Run scenario, apply a transition as part of event delivery.
    /// Otherwise Run records it as a discrete step after delivery.
    #[serde(default)]
    pub immediate: bool,
    /// Accumulate elapsed ticks while this state belongs to the active path.
    #[serde(default)]
    pub clock_state: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub initial: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    #[serde(default)]
    pub id: Option<String>,
    pub from: String,
    pub event: String,
    pub to: String,
}

/// A state-machine scenario event in ticks of the caller's Run clock.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimedEvent {
    pub at: Time,
    pub event: String,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StateId(usize);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventId(usize);

type Guard<'a> = dyn Fn(&str, &str, &str) -> Result<bool, RunError> + 'a;

struct ResolvedTransition {
    id: String,
    event: String,
    to: StateId,
}

pub struct Chart {
    owner: Arc<()>,
    initial: StateId,
    immediate: bool,
    clock_state: Option<StateId>,
    names: Vec<String>,
    states: BTreeMap<String, StateId>,
    paths: Vec<Vec<StateId>>,
    events: BTreeMap<String, EventId>,
    transitions: Vec<ResolvedTransition>,
    lookup: BTreeMap<(StateId, EventId), usize>,
}

pub struct Scenario {
    owner: Arc<()>,
    end: Time,
    inputs: Vec<Input<Event>>,
}

impl Scenario {
    /// The first tick after the last scheduled event, or one for an empty scenario.
    pub fn end(&self) -> Time {
        self.end
    }
}

pub struct State {
    owner: Arc<()>,
    active: Vec<StateId>,
    pending: Option<usize>,
    last: Option<usize>,
    since: Time,
    elapsed: Time,
    now: Time,
}

impl Chart {
    pub fn new(spec: ChartSpec) -> Result<Self, RunError> {
        if spec.states.len() > 128 || spec.transitions.len() > 512 {
            return Err(error("State-machine profile exceeds its size limit"));
        }
        let mut states = BTreeMap::new();
        for (index, node) in spec.states.iter().enumerate() {
            if node.id.is_empty() || states.insert(node.id.clone(), StateId(index)).is_some() {
                return Err(error(format!("Duplicate or empty state {}", node.id)));
            }
        }
        let initial = states
            .get(&spec.initial)
            .copied()
            .ok_or_else(|| error("Unknown initial state"))?;
        if spec.states[initial.0].parent.is_some() {
            return Err(error("Top-level initial state must be a root"));
        }
        let clock_state = spec
            .clock_state
            .as_ref()
            .map(|id| {
                states
                    .get(id)
                    .copied()
                    .ok_or_else(|| error("Unknown clock state"))
            })
            .transpose()?;
        let paths = resolve_paths(&spec.states, &states)?;

        let mut lookup = BTreeMap::new();
        let mut events = BTreeMap::new();
        let mut transition_ids = BTreeSet::new();
        let mut transitions = Vec::with_capacity(spec.transitions.len());
        for (index, transition) in spec.transitions.into_iter().enumerate() {
            let from = states
                .get(&transition.from)
                .copied()
                .ok_or_else(|| error(format!("Unknown source state {}", transition.from)))?;
            let to = states
                .get(&transition.to)
                .copied()
                .ok_or_else(|| error(format!("Unknown target state {}", transition.to)))?;
            let next_event_id = EventId(events.len());
            let event = *events
                .entry(transition.event.clone())
                .or_insert(next_event_id);
            let id = transition
                .id
                .unwrap_or_else(|| format!("transition-{index}"));
            if transition.event.is_empty()
                || id.is_empty()
                || !transition_ids.insert(id.clone())
                || lookup.insert((from, event), index).is_some()
            {
                return Err(error(format!(
                    "Invalid or ambiguous transition {}",
                    transition.event
                )));
            }
            transitions.push(ResolvedTransition {
                id,
                event: transition.event,
                to,
            });
        }
        Ok(Self {
            owner: Arc::new(()),
            initial,
            immediate: spec.immediate,
            clock_state,
            names: spec.states.into_iter().map(|node| node.id).collect(),
            states,
            paths,
            events,
            transitions,
            lookup,
        })
    }

    pub fn initial(&self) -> State {
        State {
            owner: Arc::clone(&self.owner),
            active: self.paths[self.initial.0].clone(),
            pending: None,
            last: None,
            since: 0,
            elapsed: 0,
            now: 0,
        }
    }

    pub fn active(state: &State, id: StateId) -> bool {
        state.active.contains(&id)
    }

    pub fn active_leaf(&self, state: &State) -> &str {
        if !Arc::ptr_eq(&self.owner, &state.owner) {
            return "";
        }
        state.active.last().map_or("", |id| &self.names[id.0])
    }

    /// The transition applied by the most recent event, if one was selected.
    pub fn last_transition(&self, state: &State) -> Option<&str> {
        if !Arc::ptr_eq(&self.owner, &state.owner) {
            return None;
        }
        state.last.map(|index| self.transitions[index].id.as_str())
    }

    pub fn state_id(&self, name: &str) -> Option<StateId> {
        self.states.get(name).copied()
    }

    pub fn event_id(&self, name: &str) -> Option<EventId> {
        self.events.get(name).copied()
    }

    fn select(
        &self,
        state: &State,
        event: Option<EventId>,
        guard: &Guard<'_>,
    ) -> Result<Option<usize>, RunError> {
        let Some(event) = event else { return Ok(None) };
        for id in state.active.iter().rev() {
            let Some(&index) = self.lookup.get(&(*id, event)) else {
                continue;
            };
            let transition = &self.transitions[index];
            if guard(
                &self.names[id.0],
                &transition.event,
                &self.names[transition.to.0],
            )? {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn apply(&self, state: &mut State, index: usize, now: Time) {
        let was_timing = self.is_timing(state);
        state
            .active
            .clone_from(&self.paths[self.transitions[index].to.0]);
        match (was_timing, self.is_timing(state)) {
            (true, false) => state.elapsed += now - state.since,
            (false, true) => state.since = now,
            _ => {}
        }
        state.last = Some(index);
    }

    /// Deliver an event directly under an external coordinator. Use a state
    /// and event ID from this chart and supply ticks in chronological order.
    /// The guard receives the source state, event name, and target state.
    /// A disabled event leaves the active state unchanged.
    pub fn dispatch(
        &self,
        state: &mut State,
        event: EventId,
        at: Time,
        guard: impl Fn(&str, &str, &str) -> Result<bool, RunError>,
    ) -> Result<bool, RunError> {
        if !Arc::ptr_eq(&self.owner, &state.owner) {
            return Err(error("State belongs to a different state machine"));
        }
        if event.0 >= self.events.len() {
            return Err(error("Unknown state-machine event ID"));
        }
        if at < state.now {
            return Err(RunError::PastEvent {
                now: state.now,
                requested: at,
            });
        }
        let selected = self.select(state, Some(event), &guard)?;
        state.now = at;
        state.last = None;
        let Some(index) = selected else {
            return Ok(false);
        };
        self.apply(state, index, at);
        Ok(true)
    }

    fn is_timing(&self, state: &State) -> bool {
        self.clock_state.is_some_and(|id| Self::active(state, id))
    }

    pub fn prepare_scenario(&self, events: Vec<TimedEvent>) -> Result<Scenario, RunError> {
        if events.len() > 1_000 || events.iter().any(|event| event.at > 86_400_000) {
            return Err(error("Chart scenario exceeds time or event limits"));
        }
        let end = events.iter().map(|event| event.at).max().unwrap_or(0) + 1;
        let inputs = events
            .into_iter()
            .map(|item| Input {
                time: item.at,
                event: Event {
                    id: self.event_id(&item.event),
                    name: item.event,
                },
            })
            .collect();
        Ok(Scenario {
            owner: Arc::clone(&self.owner),
            end,
            inputs,
        })
    }

    pub fn run(
        &self,
        scenario: &Scenario,
        profile: &Profile,
        metadata: TraceMetadata,
        guard: impl Fn(&str, &str, &str) -> Result<bool, RunError>,
    ) -> Result<Trace, RunError> {
        if !Arc::ptr_eq(&self.owner, &scenario.owner) {
            return Err(error("Scenario belongs to a different state machine"));
        }
        let (_, trace) = run_traced(
            &Run {
                chart: self,
                guard: &guard,
            },
            profile,
            scenario.inputs.iter().cloned(),
            metadata,
        )?;
        Ok(trace)
    }
}

fn error(message: impl Into<String>) -> RunError {
    RunError::Semantics(message.into())
}

fn resolve_paths(
    nodes: &[Node],
    states: &BTreeMap<String, StateId>,
) -> Result<Vec<Vec<StateId>>, RunError> {
    let index = |id: &str| {
        states
            .get(id)
            .copied()
            .ok_or_else(|| error(format!("Unknown state {id}")))
    };
    let parents = nodes
        .iter()
        .map(|node| node.parent.as_deref().map(index).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let children = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let id = StateId(index);
            match node
                .initial
                .as_deref()
                .and_then(|name| states.get(name).copied())
            {
                Some(child) if parents[child.0] == Some(id) => Ok(Some(child)),
                None if node.initial.is_none() && !parents.contains(&Some(id)) => Ok(None),
                _ => Err(error(format!(
                    "State {} needs a direct initial child",
                    node.id
                ))),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut paths = Vec::with_capacity(nodes.len());
    for index in 0..nodes.len() {
        let id = StateId(index);
        let mut seen = BTreeSet::from([id]);
        let mut path = vec![id];
        let mut parent = parents[index];
        while let Some(id) = parent {
            if !seen.insert(id) {
                return Err(error("Cycle in state hierarchy"));
            }
            path.push(id);
            parent = parents[id.0];
        }
        path.reverse();
        let mut child = children[index];
        while let Some(id) = child {
            path.push(id);
            child = children[id.0];
        }
        paths.push(path);
    }
    Ok(paths)
}

struct Run<'a> {
    chart: &'a Chart,
    guard: &'a Guard<'a>,
}

#[derive(Clone)]
struct Event {
    id: Option<EventId>,
    name: String,
}

impl std::fmt::Debug for Event {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.name)
    }
}

impl Semantics for Run<'_> {
    type State = State;
    type Step = usize;
    type Event = Event;

    fn initial(&self) -> Result<State, RunError> {
        Ok(self.chart.initial())
    }

    fn enabled(&self, state: &State, _: Time) -> Result<Vec<usize>, RunError> {
        Ok(state.pending.into_iter().collect())
    }

    fn fire(
        &self,
        state: &mut State,
        step: &usize,
        context: &mut StepContext<'_, Event>,
    ) -> Result<(), RunError> {
        self.chart.apply(state, *step, context.now());
        state.pending = None;
        Ok(())
    }

    fn on_event(
        &self,
        state: &mut State,
        event: &Event,
        context: &mut StepContext<'_, Event>,
    ) -> Result<EventEffect, RunError> {
        state.now = context.now();
        state.last = None;
        let Some(index) = self.chart.select(state, event.id, self.guard)? else {
            return Ok(EventEffect::Rejected(
                "no transition from an active state".into(),
            ));
        };
        if self.chart.immediate {
            self.chart.apply(state, index, context.now());
        } else {
            if state.pending.is_some() {
                return Err(error("Deferred chart events need distinct ticks"));
            }
            state.pending = Some(index);
        }
        Ok(EventEffect::Applied)
    }

    fn observe(&self, state: &State) -> Result<Value, RunError> {
        let elapsed = state.elapsed
            + if self.chart.is_timing(state) {
                state.now - state.since
            } else {
                0
            };
        let active = state
            .active
            .iter()
            .map(|id| &self.chart.names[id.0])
            .collect::<Vec<_>>();
        let transition = state.last.map(|index| &self.chart.transitions[index].id);
        Ok(json!({ "active": active, "elapsed": elapsed, "transition": transition }))
    }
}
