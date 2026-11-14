//! Bounded opt-in evidence at actual semantic read sites. Not a dependency proof.
use super::preflight;
use intersect_model::definitions::{
    bundle::SourceSpan, rcore_behavior::ir_v2::TypeId, resources::DeclarationId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationLimits {
    pub enabled: bool,
    pub max_events: usize,
    /// Aggregate encoded event payload bytes, excluding fixed report metadata.
    pub max_event_bytes: usize,
}
impl Default for ObservationLimits {
    fn default() -> Self {
        Self {
            enabled: false,
            max_events: 4096,
            max_event_bytes: 65536,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadKind {
    StoredSingular,
    StoredPage,
    IncomingStart,
    IncomingMatch,
    IncomingEnd,
}
#[derive(Clone, Debug, Serialize)]
pub struct SemanticRead {
    pub kind: ReadKind,
    pub context: String,
    pub element: String,
    pub member: DeclarationId,
    pub source: SourceSpan,
    pub cursor: Option<usize>,
    pub source_type: Option<TypeId>,
    pub matched: Option<String>,
    pub value: Option<serde_json::Value>,
}
#[derive(Serialize)]
pub(super) struct ReadInput<'a> {
    pub kind: ReadKind,
    pub context: &'a str,
    pub element: &'a str,
    pub member: &'a DeclarationId,
    pub source: &'a SourceSpan,
    pub cursor: Option<usize>,
    pub source_type: Option<TypeId>,
    pub matched: Option<&'a str>,
    pub value: Option<&'a serde_json::Value>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ValidationAdmission {
    pub context: String,
    pub source: Option<SourceSpan>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReadReport {
    pub coverage: &'static str,
    pub cache_dependencies_complete: bool,
    pub execution_complete: bool,
    pub events_complete: bool,
    pub truncated: bool,
    pub event_bytes: usize,
    pub max_events: usize,
    pub max_event_bytes: usize,
    pub semantic_reads: Vec<SemanticRead>,
    pub validation_admissions: Vec<ValidationAdmission>,
}
pub(super) struct Recorder {
    report: Option<ReadReport>,
}
impl Recorder {
    pub fn new(limits: &ObservationLimits) -> Self {
        Self {
            report: limits.enabled.then(|| ReadReport {
                coverage: "stored_feature_values_and_incoming_queries_v1",
                cache_dependencies_complete: false,
                execution_complete: false,
                events_complete: true,
                truncated: false,
                event_bytes: 0,
                max_events: limits.max_events,
                max_event_bytes: limits.max_event_bytes,
                semantic_reads: Vec::new(),
                validation_admissions: Vec::new(),
            }),
        }
    }
    fn admit<T: Serialize>(report: &mut ReadReport, input: &T) -> bool {
        if report.truncated {
            return false;
        }
        if report
            .semantic_reads
            .len()
            .saturating_add(report.validation_admissions.len())
            >= report.max_events
        {
            report.truncated = true;
            report.events_complete = false;
            return false;
        }
        let remaining = report.max_event_bytes.saturating_sub(report.event_bytes);
        match preflight::serialized_size(input, remaining) {
            Ok(bytes) => {
                report.event_bytes += bytes;
                true
            }
            Err(_) => {
                report.truncated = true;
                report.events_complete = false;
                false
            }
        }
    }
    pub fn enabled(&self) -> bool {
        self.report.is_some()
    }
    pub fn read(&mut self, input: ReadInput<'_>) -> Option<usize> {
        let report = self.report.as_mut()?;
        if !Self::admit(report, &input) {
            return None;
        }
        let index = report.semantic_reads.len();
        report.semantic_reads.push(SemanticRead {
            kind: input.kind,
            context: input.context.into(),
            element: input.element.into(),
            member: input.member.clone(),
            source: input.source.clone(),
            cursor: input.cursor,
            source_type: input.source_type,
            matched: input.matched.map(str::to_owned),
            value: input.value.cloned(),
        });
        Some(index)
    }
    pub fn incoming_followup(
        &mut self,
        index: Option<usize>,
        kind: ReadKind,
        matched: Option<&str>,
        cursor: usize,
    ) {
        let (Some(index), Some(report)) = (index, self.report.as_mut()) else {
            return;
        };
        if report.truncated {
            return;
        }
        let Some(start) = report.semantic_reads.get(index) else {
            report.truncated = true;
            report.events_complete = false;
            return;
        };
        let input = ReadInput {
            kind,
            context: &start.context,
            element: &start.element,
            member: &start.member,
            source: &start.source,
            cursor: Some(cursor),
            source_type: start.source_type,
            matched,
            value: None,
        };
        let remaining = report.max_event_bytes.saturating_sub(report.event_bytes);
        let Ok(bytes) = preflight::serialized_size(&input, remaining) else {
            report.truncated = true;
            report.events_complete = false;
            return;
        };
        if report
            .semantic_reads
            .len()
            .saturating_add(report.validation_admissions.len())
            >= report.max_events
        {
            report.truncated = true;
            report.events_complete = false;
            return;
        }
        let event = SemanticRead {
            kind,
            context: start.context.clone(),
            element: start.element.clone(),
            member: start.member.clone(),
            source: start.source.clone(),
            cursor: Some(cursor),
            source_type: start.source_type,
            matched: matched.map(str::to_owned),
            value: None,
        };
        report.event_bytes += bytes;
        report.semantic_reads.push(event);
    }
    pub fn validation(&mut self, context: &str, source: Option<&SourceSpan>) {
        #[derive(Serialize)]
        struct Input<'a> {
            context: &'a str,
            source: Option<&'a SourceSpan>,
        }
        let Some(report) = self.report.as_mut() else {
            return;
        };
        if !Self::admit(report, &Input { context, source }) {
            return;
        }
        report.validation_admissions.push(ValidationAdmission {
            context: context.into(),
            source: source.cloned(),
        });
    }
    pub fn snapshot(&self) -> Option<ReadReport> {
        self.report.clone()
    }
    pub fn finish(&mut self) -> Option<ReadReport> {
        self.report.take().map(|mut report| {
            report.execution_complete = true;
            report
        })
    }
}
