//! Native transient diagnostic provenance. The legacy DTO stays unchanged.
use super::Diagnostic;
use crate::definitions::bundle::{SourceId, SourceSpan};
use serde::{Deserialize, Serialize};

/// Producer-owned input identity, distinct from admission or support evidence.
/// A missing span leaves the historical display range unqualified. Contexts
/// identify a request/world, not an authored culprit. Generated correspondence
/// retains every original location rather than selecting an arbitrary one.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticOrigin {
    Source {
        source: SourceId,
        #[serde(skip_serializing_if = "Option::is_none")]
        span: Option<SourceSpan>,
    },
    Context {
        source: SourceId,
    },
    Generated {
        generated: SourceSpan,
        originals: Vec<SourceSpan>,
    },
    Unavailable {
        reason: String,
    },
}
// Legacy APIs embed Debug diagnostics in error strings. Metadata remains opt-in.
impl std::fmt::Debug for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Diagnostic")
            .field("source", &self.source)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("message", &self.message)
            .finish()
    }
}
impl Diagnostic {
    pub fn at_source(
        source: &SourceId,
        start: usize,
        end: usize,
        message: impl Into<String>,
    ) -> Self {
        Self::new(&source.uri, start, end, message).with_source(source)
    }
    pub fn from_span(span: SourceSpan, message: impl Into<String>) -> Self {
        Self::new(&span.source.uri, span.start, span.end, message).with_span(span)
    }
    pub fn with_source(mut self, source: &SourceId) -> Self {
        self.origin = Some(Box::new(DiagnosticOrigin::Source {
            source: source.clone(),
            span: None,
        }));
        self
    }
    pub fn with_span(mut self, span: SourceSpan) -> Self {
        self.origin = Some(Box::new(DiagnosticOrigin::Source {
            source: span.source.clone(),
            span: Some(span),
        }));
        self
    }
    pub fn with_context(mut self, source: &SourceId) -> Self {
        self.origin = Some(Box::new(DiagnosticOrigin::Context {
            source: source.clone(),
        }));
        self
    }
    pub fn with_generated(mut self, generated: SourceSpan, originals: Vec<SourceSpan>) -> Self {
        self.origin = Some(Box::new(DiagnosticOrigin::Generated {
            generated,
            originals,
        }));
        self
    }
}
