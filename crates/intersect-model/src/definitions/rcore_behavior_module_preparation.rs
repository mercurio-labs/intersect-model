//! Retained module assembly; resource observations stay outside pure futures.
use super::*;
use crate::definitions::{
    bundle::{BundleValidation, SourceCopyPreparation},
    conversion::InvocationContext,
    ecore::{ObservedImportError, ParseLimits},
    resources::ResourceSetPreparation,
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModulePreparationError {
    Diagnostics(Vec<SyntaxDiagnostic>),
    Observation(String),
}
impl From<ObservedImportError> for ModulePreparationError {
    fn from(error: ObservedImportError) -> Self {
        match error {
            ObservedImportError::Import(e) => {
                Self::Diagnostics(vec![diagnostic(&e.span, e.message)])
            }
            ObservedImportError::Observation(e) => Self::Observation(e),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModulePreparationPhase {
    Validation,
    Resources,
    Selection,
    SourceCopy,
    Syntax,
    Interfaces,
    Complete,
}
type SelectionFuture<'b> =
    Pin<Box<dyn Future<Output = Result<ModuleInputs<'b>, Vec<SyntaxDiagnostic>>> + Send + 'b>>;
type SyntaxFuture = Pin<Box<dyn Future<Output = Document> + Send>>;
type InterfacesFuture<'b> = Pin<
    Box<dyn Future<Output = Result<(ModuleIndex, ResourceSet), Vec<SyntaxDiagnostic>>> + Send + 'b>,
>;
/// Only a complete index can be published. The exact input bundle is borrowed.
pub struct ModuleIndexPreparation<'b> {
    bundle: &'b DefinitionBundle,
    entry: SourceId,
    limits: ParseLimits,
    phase: ModulePreparationPhase,
    validation: Option<BundleValidation<'b>>,
    resources: Option<Box<ResourceSetPreparation<'b>>>,
    resource_set: Option<ResourceSet>,
    selection: Option<SelectionFuture<'b>>,
    inputs: Option<ModuleInputs<'b>>,
    selected: usize,
    copying: Option<SourceCopyPreparation<'b>>,
    syntax: Option<SyntaxFuture>,
    documents: Vec<(SourceId, Document)>,
    interfaces: Option<InterfacesFuture<'b>>,
    output: Option<(ModuleIndex, ResourceSet)>,
    failure: Option<ModulePreparationError>,
}
const STATE_BYTES: usize = 4096;
const _: () = assert!(std::mem::size_of::<ModuleIndexPreparation<'static>>() <= STATE_BYTES);
impl<'b> ModuleIndexPreparation<'b> {
    /// New observed admission profile; ordinary APIs drain with no observer.
    pub fn begin(
        bundle: &'b DefinitionBundle,
        entry: &SourceId,
        limits: ParseLimits,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ModulePreparationError> {
        context
            .checkpoint()
            .map_err(ModulePreparationError::Observation)?;
        context
            .retain_storage(STATE_BYTES)
            .map_err(ModulePreparationError::Observation)?;
        Ok(Self {
            bundle,
            entry: entry.clone(),
            limits,
            phase: ModulePreparationPhase::Validation,
            validation: Some(BundleValidation::new(bundle)),
            resources: None,
            resource_set: None,
            selection: None,
            inputs: None,
            selected: 0,
            copying: None,
            syntax: None,
            documents: Vec::new(),
            interfaces: None,
            output: None,
            failure: None,
        })
    }
    pub fn phase(&self) -> ModulePreparationPhase {
        self.phase
    }
    pub fn bundle(&self) -> &DefinitionBundle {
        self.bundle
    }
    pub fn advance(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<bool, ModulePreparationError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(context);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn invalid(&self, message: &str) -> ModulePreparationError {
        ModulePreparationError::Diagnostics(vec![diagnostic(
            &SourceSpan {
                source: self.entry.clone(),
                start: 0,
                end: 0,
            },
            message,
        )])
    }
    fn advance_inner(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<bool, ModulePreparationError> {
        context
            .checkpoint()
            .map_err(ModulePreparationError::Observation)?;
        if self.phase == ModulePreparationPhase::Complete {
            return Ok(true);
        }
        context
            .work(1)
            .map_err(ModulePreparationError::Observation)?;
        match self.phase {
            ModulePreparationPhase::Validation => {
                let validation = self.validation.as_mut().ok_or_else(|| {
                    ModulePreparationError::Diagnostics(vec![diagnostic(
                        &SourceSpan {
                            source: self.entry.clone(),
                            start: 0,
                            end: 0,
                        },
                        "Missing module bundle validation",
                    )])
                })?;
                let complete = validation
                    .advance(&|_| Ok(()))
                    .map_err(|e| self.invalid(&e))?;
                if complete {
                    self.validation = None;
                    self.resources = Some(Box::new(
                        ResourceSetPreparation::begin_with_retained_bundle(
                            self.bundle,
                            self.limits,
                            context,
                        )
                        .map_err(ModulePreparationError::from)?,
                    ));
                    self.phase = ModulePreparationPhase::Resources;
                }
            }
            ModulePreparationPhase::Resources => {
                let resources = self.resources.as_mut().ok_or_else(|| {
                    ModulePreparationError::Diagnostics(vec![diagnostic(
                        &SourceSpan {
                            source: self.entry.clone(),
                            start: 0,
                            end: 0,
                        },
                        "Missing module resources",
                    )])
                })?;
                if resources
                    .advance(context)
                    .map_err(ModulePreparationError::from)?
                {
                    self.resource_set = Some(
                        self.resources
                            .take()
                            .ok_or_else(|| self.invalid("Missing completed module resources"))?
                            .finish()
                            .map_err(ModulePreparationError::from)?,
                    );
                    self.selection =
                        Some(Box::pin(discover_sources(self.bundle, self.entry.clone())));
                    self.phase = ModulePreparationPhase::Selection;
                }
            }
            ModulePreparationPhase::Selection => {
                let future = self.selection.as_mut().ok_or_else(|| {
                    ModulePreparationError::Diagnostics(vec![diagnostic(
                        &SourceSpan {
                            source: self.entry.clone(),
                            start: 0,
                            end: 0,
                        },
                        "Missing module selection",
                    )])
                })?;
                if let Poll::Ready(inputs) = future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    self.inputs = Some(inputs.map_err(ModulePreparationError::Diagnostics)?);
                    self.selection = None;
                    self.phase = ModulePreparationPhase::SourceCopy;
                }
            }
            ModulePreparationPhase::SourceCopy => {
                let source = self
                    .inputs
                    .as_ref()
                    .ok_or_else(|| self.invalid("Missing selected module inputs"))?
                    .selected
                    .get(self.selected)
                    .copied();
                if let Some(source) = source {
                    if self.copying.is_none() {
                        // Conservative syntax admission before owned AST/source storage.
                        context
                            .work(source.text.len().max(1))
                            .map_err(ModulePreparationError::Observation)?;
                        let bytes =
                            source.text.len().checked_mul(256).ok_or_else(|| {
                                self.invalid("Module frontend storage size overflow")
                            })?;
                        context
                            .retain_storage(bytes)
                            .map_err(ModulePreparationError::Observation)?;
                        self.copying =
                            Some(SourceCopyPreparation::new(source, context.has_observer()));
                        return Ok(false);
                    }
                    let copying = self.copying.as_mut().ok_or_else(|| {
                        ModulePreparationError::Diagnostics(vec![diagnostic(
                            &SourceSpan {
                                source: self.entry.clone(),
                                start: 0,
                                end: 0,
                            },
                            "Missing module source copy",
                        )])
                    })?;
                    if copying
                        .advance(context)
                        .map_err(ModulePreparationError::Observation)?
                    {
                        let owned = self
                            .copying
                            .take()
                            .ok_or_else(|| self.invalid("Missing completed module source copy"))?
                            .finish()
                            .map_err(ModulePreparationError::Observation)?
                            .into_source();
                        self.syntax = Some(Box::pin(super::super::parser::owned_future(
                            owned,
                            self.limits,
                        )));
                        self.phase = ModulePreparationPhase::Syntax;
                    }
                } else {
                    let inputs = self
                        .inputs
                        .take()
                        .ok_or_else(|| self.invalid("Missing complete module inputs"))?;
                    let resources = self
                        .resource_set
                        .take()
                        .ok_or_else(|| self.invalid("Missing complete module resource set"))?;
                    self.interfaces = Some(Box::pin(finish_interfaces(
                        self.bundle,
                        self.entry.clone(),
                        inputs,
                        resources,
                        std::mem::take(&mut self.documents),
                    )));
                    self.phase = ModulePreparationPhase::Interfaces;
                }
            }
            ModulePreparationPhase::Syntax => {
                let syntax = self.syntax.as_mut().ok_or_else(|| {
                    ModulePreparationError::Diagnostics(vec![diagnostic(
                        &SourceSpan {
                            source: self.entry.clone(),
                            start: 0,
                            end: 0,
                        },
                        "Missing module syntax",
                    )])
                })?;
                if let Poll::Ready(document) = syntax
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    self.documents.push((document.source.id.clone(), document));
                    self.syntax = None;
                    self.selected += 1;
                    self.phase = ModulePreparationPhase::SourceCopy;
                }
            }
            ModulePreparationPhase::Interfaces => {
                let future = self.interfaces.as_mut().ok_or_else(|| {
                    ModulePreparationError::Diagnostics(vec![diagnostic(
                        &SourceSpan {
                            source: self.entry.clone(),
                            start: 0,
                            end: 0,
                        },
                        "Missing module interfaces",
                    )])
                })?;
                if let Poll::Ready(output) = future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    self.output = Some(output.map_err(ModulePreparationError::Diagnostics)?);
                    self.interfaces = None;
                    self.phase = ModulePreparationPhase::Complete;
                }
            }
            ModulePreparationPhase::Complete => {}
        }
        Ok(self.phase == ModulePreparationPhase::Complete)
    }
    pub fn finish(self) -> Result<ModuleIndex, ModulePreparationError> {
        self.finish_with_resources().map(|(index, _)| index)
    }
    /// Keep exact parsed ownership for later compiler phases. This is internal;
    /// arbitrary caller-supplied indexes/resources cannot enter the public path.
    pub(crate) fn finish_with_resources(
        self,
    ) -> Result<(ModuleIndex, ResourceSet), ModulePreparationError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.output.ok_or_else(|| {
            ModulePreparationError::Diagnostics(vec![diagnostic(
                &SourceSpan {
                    source: self.entry,
                    start: 0,
                    end: 0,
                },
                "Module index preparation is incomplete",
            )])
        })
    }
}
