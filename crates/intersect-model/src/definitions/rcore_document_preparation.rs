//! Handwritten continuation of the shared structural grammar and declaration builder.
//! Futures retain Rust control flow only; hosts explicitly advance them with fresh
//! observers. No runtime, scheduler, wakeup, clock or external resource is required.
use super::*;
use crate::definitions::{
    bundle::SourceCopyPreparation,
    conversion::InvocationContext,
    ecore::{Document, ErrorKind, ImportError, Node, ObservedImportError, ParseLimits},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};

#[derive(Default)]
struct Counts {
    tokens: AtomicUsize,
    tree: AtomicUsize,
    stage: AtomicUsize,
}
#[derive(Clone, Default)]
pub(super) struct Traversal(Arc<Counts>);
impl Traversal {
    pub(super) async fn token(&self) {
        self.0.tokens.fetch_add(1, Ordering::Relaxed);
        pause().await;
    }
    pub(super) async fn tree(&self) {
        self.0.tree.fetch_add(1, Ordering::Relaxed);
        pause().await;
    }
}
pub(super) use crate::definitions::continuation::complete;
use crate::definitions::continuation::pause;

/// Progress of structural traversal; individual spellings/allocations still have
/// separate physical latency and cleanup obligations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentStage {
    Syntax,
    Tree,
    SourceCopy,
    Complete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentProgress {
    pub stage: DocumentStage,
    pub consumed_tokens: usize,
    pub tree_transitions: usize,
    pub copied_text_bytes: usize,
}
type RootFuture<'source> =
    Pin<Box<dyn Future<Output = Result<Node, ImportError>> + Send + 'source>>;
/// Opaque retained structural document. Only a completed document can be published.
/// It borrows the exact immutable input and never retains an invocation observer.
pub struct DocumentPreparation<'source> {
    source: &'source DefinitionSource,
    traversal: Traversal,
    future: Option<RootFuture<'source>>,
    root: Option<Node>,
    copying: Option<SourceCopyPreparation<'source>>,
    output: Option<Document>,
    failure: Option<ObservedImportError>,
}
impl<'source> DocumentPreparation<'source> {
    /// Accept only complete lexical output. Limits and lexical-error priority are
    /// unchanged; syntax and tree construction begin on subsequent advances.
    pub fn from_lexical(
        output: rcore_behavior::LexicalOutput<'source>,
        limits: ParseLimits,
    ) -> Result<Self, ImportError> {
        let (source, tokens, diagnostics) = output.into_input();
        direct::check_document_limits(source, limits)?;
        if let Some(error) = diagnostics.into_iter().next() {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &error.span,
                error.message,
            ));
        }
        let traversal = Traversal::default();
        let counts = traversal.clone();
        let future = Box::pin(async move {
            let parsed = parse_structure_tokens_retained(tokens, counts.clone())
                .await
                .map_err(|errors| {
                    errors.into_iter().next().map_or_else(
                        || {
                            ImportError::new(
                                ErrorKind::InvalidDefinition,
                                &SourceSpan {
                                    source: source.id.clone(),
                                    start: 0,
                                    end: source.text.len(),
                                },
                                "Invalid Rcore structure",
                            )
                        },
                        |e| ImportError::new(ErrorKind::InvalidDefinition, &e.span, e.message),
                    )
                })?;
            counts.0.stage.store(1, Ordering::Relaxed);
            direct::build_root(&parsed, &counts).await
        });
        Ok(Self {
            source,
            traversal,
            future: Some(future),
            root: None,
            copying: None,
            output: None,
            failure: None,
        })
    }
    pub fn source(&self) -> &DefinitionSource {
        self.source
    }
    pub fn progress(&self) -> DocumentProgress {
        DocumentProgress {
            stage: if self.output.is_some() {
                DocumentStage::Complete
            } else if self.copying.is_some() {
                DocumentStage::SourceCopy
            } else if self.traversal.0.stage.load(Ordering::Relaxed) == 0 {
                DocumentStage::Syntax
            } else {
                DocumentStage::Tree
            },
            consumed_tokens: self.traversal.0.tokens.load(Ordering::Relaxed),
            tree_transitions: self.traversal.0.tree.load(Ordering::Relaxed),
            copied_text_bytes: if self.output.is_some() {
                self.source.text.len()
            } else {
                self.copying
                    .as_ref()
                    .map_or(0, SourceCopyPreparation::copied_text_bytes)
            },
        }
    }
    /// Advance one token, declaration transition or UTF-8-aligned ownership chunk.
    /// Errors and cancellation are terminal; no partial declaration is exposed.
    pub fn advance(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<bool, ObservedImportError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(context);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn advance_inner(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<bool, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        if let Some(future) = &mut self.future {
            let mut polling = Context::from_waker(Waker::noop());
            if let Poll::Ready(root) = future.as_mut().poll(&mut polling) {
                self.root = Some(root.map_err(ObservedImportError::Import)?);
                self.future = None;
                // Existing conservative Rcore frontend admission covers ownership.
                self.copying = Some(SourceCopyPreparation::new(self.source, false));
            }
            return Ok(false);
        }
        if let Some(copying) = &mut self.copying
            && copying
                .advance(context)
                .map_err(ObservedImportError::Observation)?
        {
            let copying = self
                .copying
                .take()
                .ok_or_else(|| self.missing("Missing Rcore source copy"))?;
            let source = copying
                .finish()
                .map_err(ObservedImportError::Observation)?
                .into_source();
            let root = self
                .root
                .take()
                .ok_or_else(|| self.missing("Missing Rcore declaration root"))?;
            self.output = Some(Document {
                source,
                root,
                misc: vec![],
            });
        }
        Ok(self.output.is_some())
    }
    fn missing(&self, message: &str) -> ObservedImportError {
        ObservedImportError::Import(ImportError::new(
            ErrorKind::InvalidDefinition,
            &SourceSpan {
                source: self.source.id.clone(),
                start: 0,
                end: self.source.text.len(),
            },
            message,
        ))
    }
    pub fn finish(self) -> Result<Document, ObservedImportError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.output.ok_or_else(|| {
            ObservedImportError::Import(ImportError::new(
                ErrorKind::InvalidDefinition,
                &SourceSpan {
                    source: self.source.id.clone(),
                    start: 0,
                    end: self.source.text.len(),
                },
                "Rcore document preparation is incomplete",
            ))
        })
    }
}
