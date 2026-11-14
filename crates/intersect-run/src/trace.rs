//! Handwritten observer and versioned trace format.
use crate::{Input, Profile, RunError, RunOutcome, Semantics, Time, run};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceHeader {
    pub format: String,
    pub profile: Profile,
    #[serde(flatten)]
    pub metadata: TraceMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    RunStart,
    EventDelivered,
    InputRejected,
    StepFired,
    TimeAdvanced,
    RunEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEntry {
    pub seq: u64,
    pub time: Time,
    pub kind: TraceKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub header: TraceHeader,
    pub entries: Vec<TraceEntry>,
}

impl Trace {
    pub const FORMAT: &'static str = "intersect.run.trace.v1";
}

pub trait Observer {
    fn record(&mut self, entry: TraceEntry);
}

impl Observer for () {
    fn record(&mut self, _: TraceEntry) {}
}

pub(crate) struct Log<'a, O> {
    observer: &'a mut O,
    seq: u64,
}

impl<'a, O: Observer> Log<'a, O> {
    pub(crate) fn new(observer: &'a mut O) -> Self {
        Self { observer, seq: 0 }
    }

    pub(crate) fn emit(
        &mut self,
        time: Time,
        kind: TraceKind,
        label: impl Into<String>,
        observation: Option<Value>,
    ) -> Result<(), RunError> {
        let seq = self.seq;
        self.seq = seq.checked_add(1).ok_or(RunError::SequenceOverflow)?;
        self.observer.record(TraceEntry {
            seq,
            time,
            kind,
            label: label.into(),
            observation,
        });
        Ok(())
    }
}

struct Recorder(Vec<TraceEntry>);

impl Observer for Recorder {
    fn record(&mut self, entry: TraceEntry) {
        self.0.push(entry);
    }
}

pub fn run_traced<S: Semantics>(
    semantics: &S,
    profile: &Profile,
    inputs: impl IntoIterator<Item = Input<S::Event>>,
    metadata: TraceMetadata,
) -> Result<(RunOutcome<S::State>, Trace), RunError> {
    let mut recorder = Recorder(Vec::new());
    let outcome = run(semantics, profile, inputs, &mut recorder)?;
    let trace = Trace {
        header: TraceHeader {
            format: Trace::FORMAT.into(),
            profile: profile.clone(),
            metadata,
        },
        entries: recorder.0,
    };
    Ok((outcome, trace))
}
