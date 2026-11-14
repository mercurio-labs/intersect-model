//! Retained resource assembly over the existing XML, package and XMI predicates.
//! Retained bundle admission shares the synchronous validator. Retained XML
//! ownership copies and Rcore lexical/syntax/tree traversal yield. XML events
//! and attributes retain their separate synchronous boundaries.
use super::*;
use crate::definitions::bundle::SourceCopyPreparation;
use crate::definitions::ecore::OwnedDocumentPreparation;
use crate::definitions::rcore_behavior::LexicalPreparation;
enum SourcePreparation<'source> {
    Copy(SourceCopyPreparation<'source>),
    Lexical(LexicalPreparation<'source>),
    Structural(Box<crate::definitions::rcore::DocumentPreparation<'source>>),
}
#[derive(Clone, Copy)]
enum Phase {
    Validation,
    Sources,
    Copy,
    RcoreLexical,
    RcoreParse,
    RcoreFrontend,
    Parse,
    Index,
    Store,
    Namespaces,
    Complete,
}
/// Build one immutable resource universe with event/node progress and fresh observers.
/// Index completion alone does not certify the semantics of referenced declarations.
pub struct ResourceSetPreparation<'bundle> {
    bundle: &'bundle DefinitionBundle,
    limits: crate::definitions::ecore::ParseLimits,
    span: SourceSpan,
    phase: Phase,
    index: usize,
    validation: Option<ResourceBundleValidation<'bundle>>,
    source_preparation: Option<SourcePreparation<'bundle>>,
    retain_source: bool,
    document: Option<OwnedDocumentPreparation>,
    indexing: Option<OwnedResourceIndexPreparation>,
    resource: Option<Resource>,
    resources: BTreeMap<SourceId, Arc<ParsedResource>>,
    namespaces: BTreeMap<String, DeclarationId>,
    failure: Option<ObservedImportError>,
}
const STATE_BYTES: usize = 2048;
const _: () = assert!(std::mem::size_of::<ResourceSetPreparation<'static>>() + 128 <= STATE_BYTES);
impl<'bundle> ResourceSetPreparation<'bundle> {
    /// Fix the exact borrowed bundle. Bundle/digest validation remains synchronous;
    /// documents and indexes are admitted only as subsequent advances request them.
    pub fn begin(
        bundle: &'bundle DefinitionBundle,
        limits: crate::definitions::ecore::ParseLimits,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        Self::begin_inner(bundle, limits, context, false)
    }
    /// Retain bundle/source admission, source ownership and the Rcore frontend.
    /// Validation errors arrive from advance; incomplete/failed work publishes no maps.
    pub fn begin_with_retained_bundle(
        bundle: &'bundle DefinitionBundle,
        limits: crate::definitions::ecore::ParseLimits,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        Self::begin_inner(bundle, limits, context, true)
    }
    fn begin_inner(
        bundle: &'bundle DefinitionBundle,
        limits: crate::definitions::ecore::ParseLimits,
        context: &InvocationContext<'_>,
        retain_bundle: bool,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        let (span, validation) = if retain_bundle {
            let (span, validation) =
                observation.finish(ResourceBundleValidation::begin(bundle, &observation))?;
            (span, Some(validation))
        } else {
            (
                observation.finish(validate_bundle_context(bundle, &observation))?,
                None,
            )
        };
        observation.finish(observation.reserve(STATE_BYTES, &span.source))?;
        Ok(Self {
            bundle,
            limits,
            span,
            phase: if retain_bundle {
                Phase::Validation
            } else {
                Phase::Sources
            },
            index: 0,
            validation,
            source_preparation: None,
            retain_source: retain_bundle,
            document: None,
            indexing: None,
            resource: None,
            resources: BTreeMap::new(),
            namespaces: BTreeMap::new(),
            failure: None,
        })
    }
    /// Advance one input chunk, source event, indexing transition or namespace binding.
    /// Failed work is terminal; no incomplete resource set can be published.
    pub fn advance(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<bool, ObservedImportError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let observation = ParserObservation::new(context);
        let result = context
            .checkpoint()
            .map_err(ObservedImportError::Observation)
            .and_then(|()| observation.finish(self.advance_inner(context, &observation)));
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn missing(&self, message: &str) -> ImportError {
        ImportError::new(ErrorKind::InvalidDefinition, &self.span, message)
    }
    fn advance_inner(
        &mut self,
        context: &InvocationContext<'_>,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(0, &self.span.source)?;
        match self.phase {
            Phase::Validation => {
                let validation = self.validation.as_mut().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.span,
                        "Missing resource bundle validation",
                    )
                })?;
                if validation.advance(&self.span, observation)? {
                    self.validation = None;
                    self.phase = Phase::Sources;
                }
            }
            Phase::Sources => {
                if let Some(source) = self.bundle.sources.get(self.index) {
                    observation.work(1, &source.id)?;
                    if source.kind == SourceKind::Rcore {
                        validate_resource_uri(source, observation)?;
                        if self.retain_source {
                            observation.admit_rcore_document(source)?;
                            crate::definitions::rcore::check_document_limits(source, self.limits)?;
                            self.source_preparation =
                                Some(SourcePreparation::Lexical(LexicalPreparation::new(source)));
                            self.phase = Phase::RcoreLexical;
                            return Ok(false);
                        }
                        // Ordinary constructors preserve their synchronous frontend entry.
                        let document = observation.document_with_limits(source, self.limits)?;
                        self.indexing =
                            Some(OwnedResourceIndexPreparation::new(document, observation)?);
                        self.phase = Phase::Index;
                    } else if source.kind == SourceKind::Ecore {
                        validate_resource_uri(source, observation)?;
                        if self.retain_source {
                            self.source_preparation = Some(SourcePreparation::Copy(
                                SourceCopyPreparation::new(source, context.has_observer()),
                            ));
                            self.phase = Phase::Copy;
                            return Ok(false);
                        }
                        self.document = Some(observation.imported_result(
                            &source.id,
                            OwnedDocumentPreparation::begin_with_retained_input(
                                source,
                                self.limits,
                                context,
                            ),
                        )?);
                        self.phase = Phase::Parse;
                    } else {
                        self.index += 1;
                    }
                } else {
                    self.index = 0;
                    self.phase = Phase::Namespaces;
                }
            }
            Phase::Copy => {
                let source = self
                    .bundle
                    .sources
                    .get(self.index)
                    .ok_or_else(|| self.missing("Missing copied source selection"))?;
                let copying = self
                    .source_preparation
                    .as_mut()
                    .and_then(|p| match p {
                        SourcePreparation::Copy(copying) => Some(copying),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        ImportError::new(
                            ErrorKind::InvalidDefinition,
                            &self.span,
                            "Missing source copy",
                        )
                    })?;
                let complete = observation.imported_result(
                    &source.id,
                    copying
                        .advance(context)
                        .map_err(ObservedImportError::Observation),
                )?;
                if complete {
                    let copied = observation.imported_result(
                        &source.id,
                        self.source_preparation
                            .take()
                            .and_then(|p| match p {
                                SourcePreparation::Copy(copying) => Some(copying),
                                _ => None,
                            })
                            .ok_or_else(|| self.missing("Missing completed source copy"))?
                            .finish()
                            .map_err(ObservedImportError::Observation),
                    )?;
                    self.document = Some(observation.imported_result(
                        &source.id,
                        OwnedDocumentPreparation::begin_with_prepared_source(
                            copied,
                            self.limits,
                            context,
                        ),
                    )?);
                    self.phase = Phase::Parse;
                }
            }
            Phase::RcoreLexical => {
                let source = self
                    .bundle
                    .sources
                    .get(self.index)
                    .ok_or_else(|| self.missing("Missing Rcore source selection"))?;
                let preparation = self
                    .source_preparation
                    .as_mut()
                    .and_then(|p| match p {
                        SourcePreparation::Lexical(preparation) => Some(preparation),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        ImportError::new(
                            ErrorKind::InvalidDefinition,
                            &self.span,
                            "Missing Rcore lexical preparation",
                        )
                    })?;
                if observation.imported_result(
                    &source.id,
                    preparation
                        .advance(context)
                        .map_err(ObservedImportError::Observation),
                )? {
                    self.phase = Phase::RcoreParse;
                }
            }
            Phase::RcoreParse => {
                let source = self
                    .bundle
                    .sources
                    .get(self.index)
                    .ok_or_else(|| self.missing("Missing parsed Rcore source"))?;
                let preparation = self
                    .source_preparation
                    .take()
                    .and_then(|p| match p {
                        SourcePreparation::Lexical(preparation) => Some(preparation),
                        _ => None,
                    })
                    .ok_or_else(|| self.missing("Missing completed Rcore lexical preparation"))?;
                let lexical = observation.imported_result(
                    &source.id,
                    preparation
                        .finish()
                        .map_err(ObservedImportError::Observation),
                )?;
                self.source_preparation = Some(SourcePreparation::Structural(Box::new(
                    crate::definitions::rcore::DocumentPreparation::from_lexical(
                        lexical,
                        self.limits,
                    )?,
                )));
                self.phase = Phase::RcoreFrontend;
            }
            Phase::RcoreFrontend => {
                let source = self
                    .bundle
                    .sources
                    .get(self.index)
                    .ok_or_else(|| self.missing("Missing structural Rcore source"))?;
                let preparation = self
                    .source_preparation
                    .as_mut()
                    .and_then(|p| match p {
                        SourcePreparation::Structural(preparation) => Some(preparation),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        ImportError::new(
                            ErrorKind::InvalidDefinition,
                            &self.span,
                            "Missing Rcore structural preparation",
                        )
                    })?;
                if observation.imported_result(&source.id, preparation.advance(context))? {
                    let preparation = self
                        .source_preparation
                        .take()
                        .and_then(|p| match p {
                            SourcePreparation::Structural(preparation) => Some(preparation),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            self.missing("Missing completed Rcore structural preparation")
                        })?;
                    let document = observation.imported_result(&source.id, preparation.finish())?;
                    self.indexing =
                        Some(OwnedResourceIndexPreparation::new(document, observation)?);
                    self.phase = Phase::Index;
                }
            }
            Phase::Parse => {
                let source = self
                    .bundle
                    .sources
                    .get(self.index)
                    .ok_or_else(|| self.missing("Missing source selection"))?;
                let preparation = self.document.as_mut().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.span,
                        "Missing XML preparation",
                    )
                })?;
                if observation.imported_result(&source.id, preparation.advance(context))? {
                    let preparation = self
                        .document
                        .take()
                        .ok_or_else(|| self.missing("Missing completed XML preparation"))?;
                    let document = observation.imported_result(&source.id, preparation.finish())?;
                    self.indexing =
                        Some(OwnedResourceIndexPreparation::new(document, observation)?);
                    self.phase = Phase::Index;
                }
            }
            Phase::Index => {
                let preparation = self.indexing.as_mut().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.span,
                        "Missing resource indexing",
                    )
                })?;
                if preparation.advance(observation)? {
                    self.resource = Some(
                        self.indexing
                            .take()
                            .ok_or_else(|| self.missing("Missing completed resource indexing"))?
                            .finish()?,
                    );
                    self.phase = Phase::Store;
                }
            }
            Phase::Store => {
                let source = self
                    .bundle
                    .sources
                    .get(self.index)
                    .ok_or_else(|| self.missing("Missing source publication"))?;
                observation.serialized(&source.id, &source.id)?;
                observation.reserve(128, &source.id)?;
                let resource = self
                    .resource
                    .take()
                    .ok_or_else(|| self.missing("Missing complete resource"))?;
                self.resources.insert(
                    source.id.clone(),
                    Arc::new(ParsedResource {
                        resource,
                        limits: self.limits,
                    }),
                );
                self.index += 1;
                self.phase = Phase::Sources;
            }
            Phase::Namespaces => {
                if let Some(binding) = self.bundle.namespaces.get(self.index) {
                    insert_namespace_binding(
                        &self.resources,
                        binding,
                        &self.span,
                        &mut self.namespaces,
                        observation,
                    )?;
                    self.index += 1;
                } else {
                    self.phase = Phase::Complete;
                }
            }
            Phase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
    /// Publish only complete indexes and owned exact source documents.
    pub fn finish(self) -> Result<ResourceSet, ObservedImportError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if !matches!(self.phase, Phase::Complete) {
            return Err(ObservedImportError::Import(
                self.missing("Resource-set preparation is not complete"),
            ));
        }
        Ok(ResourceSet {
            resources: self.resources,
            namespaces: self.namespaces,
        })
    }
}

#[cfg(test)]
mod bundle_admission_controls {
    use super::*;
    #[test]
    fn retained_rcore_lexing_finishes_before_syntax_indexing_or_resource_publication() {
        let source = crate::definitions::bundle::DefinitionSource::new(
            "memory:lexical-private.rcore",
            SourceKind::Rcore,
            format!(
                "rcore 1;package lexical version \"1\"; /*{}*/ metamodel \"urn:lexical-private\" prefix l {{ class Item {{}} }}",
                "λ🚀".repeat(1000)
            ),
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let operation = InvocationContext::default();
        let mut pending = ResourceSetPreparation::begin_with_retained_bundle(
            &bundle,
            Default::default(),
            &operation,
        )
        .unwrap();
        let mut advances = 0;
        while !matches!(pending.phase, Phase::RcoreParse) {
            if matches!(pending.phase, Phase::RcoreLexical) {
                assert!(pending.document.is_none());
                assert!(pending.indexing.is_none());
                assert!(pending.resource.is_none());
                assert!(pending.resources.is_empty());
                advances += 1;
            }
            assert!(!pending.advance(&operation).unwrap());
        }
        assert!(advances > source.text.len() / 256);
        let mut structural_stages = [false; 3];
        while !matches!(pending.phase, Phase::Index) {
            assert!(pending.document.is_none());
            assert!(pending.indexing.is_none());
            assert!(pending.resource.is_none());
            assert!(pending.resources.is_empty());
            if let Some(SourcePreparation::Structural(preparation)) = &pending.source_preparation {
                use crate::definitions::rcore::DocumentStage;
                match preparation.progress().stage {
                    DocumentStage::Syntax => structural_stages[0] = true,
                    DocumentStage::Tree => structural_stages[1] = true,
                    DocumentStage::SourceCopy => structural_stages[2] = true,
                    DocumentStage::Complete => {}
                }
            }
            assert!(!pending.advance(&operation).unwrap());
        }
        assert!(structural_stages.into_iter().all(|seen| seen));
        while !pending.advance(&operation).unwrap() {}
        let output = pending.finish().unwrap();
        assert_eq!(output.document(&source.id).unwrap().source, source);
        assert_eq!(
            format!("{output:?}"),
            format!("{:?}", ResourceSet::new(&bundle).unwrap())
        );
    }
    #[test]
    fn retained_xml_source_copy_yields_before_parser_creation_and_preserves_exact_documents() {
        let source = crate::definitions::bundle::DefinitionSource::new(
            format!("memory:{}.ecore", "α".repeat(300)),
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='copied' nsURI='urn:copied'><!--{}--></e:EPackage>",
                "λ🚀".repeat(1200)
            ),
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let context = InvocationContext::default();
        let mut pending = ResourceSetPreparation::begin_with_retained_bundle(
            &bundle,
            crate::definitions::ecore::ParseLimits::default(),
            &context,
        )
        .unwrap();
        let mut copy_steps = 0;
        while !matches!(pending.phase, Phase::Parse) {
            if matches!(pending.phase, Phase::Copy) {
                copy_steps += 1;
                assert!(pending.document.is_none());
                assert!(pending.indexing.is_none());
                assert!(pending.resources.is_empty());
            }
            assert!(!pending.advance(&context).unwrap());
        }
        assert!(copy_steps > source.text.len() / 256);
        while !pending.advance(&context).unwrap() {}
        let resources = pending.finish().unwrap();
        assert_eq!(resources.document(&source.id).unwrap().source, source);
        assert_eq!(
            format!("{resources:?}"),
            format!("{:?}", ResourceSet::new(&bundle).unwrap())
        );
    }
    #[test]
    fn bundle_admission_precedes_every_document_parse_and_partial_map() {
        let bundle: DefinitionBundle = serde_json::from_str(include_str!(
            "../../assets/fixtures/ecore-generics/resource-preparation/bundle.json"
        ))
        .unwrap();
        let context = InvocationContext::default();
        let mut pending = ResourceSetPreparation::begin_with_retained_bundle(
            &bundle,
            crate::definitions::ecore::ParseLimits::default(),
            &context,
        )
        .unwrap();
        let mut steps = 0;
        while pending.validation.is_some() {
            assert!(pending.resources.is_empty());
            assert!(pending.document.is_none());
            assert!(pending.indexing.is_none());
            assert!(pending.resource.is_none());
            assert!(!pending.advance(&context).unwrap());
            steps += 1;
        }
        assert!(steps > 20);
        assert!(pending.finish().is_err());
    }
}
