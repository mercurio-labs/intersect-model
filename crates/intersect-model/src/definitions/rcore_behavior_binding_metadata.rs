//! Source-dependent metadata stays outside the pure AST future.
use super::*;
use crate::definitions::{
    conversion::InvocationContext,
    ecore::{ImportError, ObservedImportError, ParseLimits, ParserObservation},
    generic_resolution::{DeclaredFeaturePreparation, Resolver, ResolverPreparation},
    generic_types::Type,
};

struct ContractQuery {
    receiver: Type,
    name: String,
    span: SourceSpan,
}
struct FeatureOwner {
    resolver: Box<Resolver>,
    query: Box<ContractQuery>,
}
struct FeatureCursor<'a> {
    preparation: Option<DeclaredFeaturePreparation<'a>>,
}
self_cell::self_cell!(
    struct FeatureInputs {
        owner: FeatureOwner,
        #[covariant]
        dependent: FeatureCursor,
    }
);
enum ContractPhase<'a> {
    Resolver {
        preparation: Box<ResolverPreparation<'a>>,
        query: Option<Box<ContractQuery>>,
    },
    Feature(FeatureInputs),
    Complete,
}
struct ContractPreparation<'a> {
    phase: ContractPhase<'a>,
    span: SourceSpan,
    output: Option<DeclarationId>,
    failure: Option<ObservedImportError>,
}
const QUERY_BYTES: usize = 2048;
const FEATURE_BYTES: usize = 2048;
const _: () = assert!(std::mem::size_of::<ContractPreparation<'static>>() <= QUERY_BYTES);
const _: () = assert!(
    std::mem::size_of::<FeatureOwner>() + std::mem::size_of::<FeatureCursor<'static>>() + 128
        <= FEATURE_BYTES
);
impl<'a> ContractPreparation<'a> {
    fn begin(
        bundle: &'a DefinitionBundle,
        resources: ResourceSet,
        limits: ParseLimits,
        query: ContractQuery,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        context
            .retain_storage(QUERY_BYTES)
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        observation.finish(observation.serialized(&query.span, &query.span.source))?;
        let span = query.span.clone();
        let preparation = Box::new(ResolverPreparation::begin(
            bundle,
            Some(resources),
            limits,
            context,
        )?);
        Ok(Self {
            phase: ContractPhase::Resolver {
                preparation,
                query: Some(Box::new(query)),
            },
            span,
            output: None,
            failure: None,
        })
    }
    fn invalid(&self, message: &str) -> ObservedImportError {
        ObservedImportError::Import(ImportError::new(
            crate::definitions::ecore::ErrorKind::InvalidDefinition,
            &self.span,
            message,
        ))
    }
    fn feature_inputs(
        owner: FeatureOwner,
        context: &InvocationContext<'_>,
    ) -> Result<FeatureInputs, ObservedImportError> {
        context
            .retain_storage(FEATURE_BYTES)
            .map_err(ObservedImportError::Observation)?;
        Ok(FeatureInputs::new(owner, |owner| FeatureCursor {
            preparation: Some(DeclaredFeaturePreparation::begin(
                &owner.resolver,
                &owner.query.receiver,
                &owner.query.name,
                &owner.query.span,
            )),
        }))
    }
    fn advance(&mut self, context: &InvocationContext<'_>) -> Result<bool, ObservedImportError> {
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
        if matches!(self.phase, ContractPhase::Complete) {
            return Ok(true);
        }
        context.work(1).map_err(ObservedImportError::Observation)?;
        match &mut self.phase {
            ContractPhase::Resolver { preparation, .. } => {
                if preparation.advance(context)? {
                    let prior = std::mem::replace(&mut self.phase, ContractPhase::Complete);
                    let ContractPhase::Resolver {
                        preparation,
                        mut query,
                    } = prior
                    else {
                        return Err(self.invalid("Missing completed contract resolver"));
                    };
                    let resolver = preparation.finish()?;
                    let query = query
                        .take()
                        .ok_or_else(|| self.invalid("Missing contract feature request"))?;
                    self.phase = ContractPhase::Feature(Self::feature_inputs(
                        FeatureOwner {
                            resolver: Box::new(resolver),
                            query,
                        },
                        context,
                    )?);
                }
            }
            ContractPhase::Feature(inputs) => {
                let observation = ParserObservation::new(context);
                let output = inputs.with_dependent_mut(|owner, cursor| {
                    let preparation = cursor.preparation.as_mut().ok_or_else(|| {
                        ObservedImportError::Import(ImportError::new(
                            crate::definitions::ecore::ErrorKind::InvalidDefinition,
                            &owner.query.span,
                            "Missing contract feature cursor",
                        ))
                    })?;
                    if observation.finish(preparation.advance(&observation))? {
                        let preparation = cursor.preparation.take().ok_or_else(|| {
                            ObservedImportError::Import(ImportError::new(
                                crate::definitions::ecore::ErrorKind::InvalidDefinition,
                                &owner.query.span,
                                "Missing completed contract feature cursor",
                            ))
                        })?;
                        observation.finish(preparation.finish()).map(Some)
                    } else {
                        Ok(None)
                    }
                })?;
                if let Some(output) = output {
                    context
                        .checkpoint()
                        .map_err(ObservedImportError::Observation)?;
                    self.output = Some(output);
                    self.phase = ContractPhase::Complete;
                }
            }
            ContractPhase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, ContractPhase::Complete))
    }
    fn finish(mut self) -> Result<DeclarationId, ObservedImportError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if !matches!(self.phase, ContractPhase::Complete) {
            return Err(self.invalid("Contract feature preparation is incomplete"));
        }
        self.output
            .take()
            .ok_or_else(|| self.invalid("Missing complete contract feature"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum QueryError {
    Import(ImportError),
    Observation(String),
    Diagnostic(String),
}
impl From<ObservedImportError> for QueryError {
    fn from(error: ObservedImportError) -> Self {
        match error {
            ObservedImportError::Import(error) => Self::Import(error),
            ObservedImportError::Observation(message) => Self::Observation(message),
        }
    }
}
struct EnumQueryPreparation<'a> {
    preparation: Option<Box<crate::definitions::compiler::EnumerationPreparation<'a>>>,
    enumeration: Option<crate::definitions::enums::Enumeration>,
    classifier: Option<DeclarationId>,
    name: String,
    output: Option<Target>,
    failure: Option<QueryError>,
}
impl<'a> EnumQueryPreparation<'a> {
    fn begin(
        resources: ResourceSet,
        classifier: DeclarationId,
        name: String,
        context: &InvocationContext<'_>,
    ) -> Result<Self, QueryError> {
        context.checkpoint().map_err(QueryError::Observation)?;
        context
            .retain_storage(QUERY_BYTES)
            .map_err(QueryError::Observation)?;
        let observation = ParserObservation::new(context);
        observation
            .finish(observation.serialized(&classifier, &classifier.source))
            .map_err(QueryError::from)?;
        let preparation = Box::new(
            crate::definitions::compiler::EnumerationPreparation::begin_with_resources(
                resources,
                classifier.clone(),
                context,
            )
            .map_err(QueryError::from)?,
        );
        Ok(Self {
            preparation: Some(preparation),
            enumeration: None,
            classifier: Some(classifier),
            name,
            output: None,
            failure: None,
        })
    }
    fn advance(&mut self, context: &InvocationContext<'_>) -> Result<bool, QueryError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(context);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn advance_inner(&mut self, context: &InvocationContext<'_>) -> Result<bool, QueryError> {
        context.checkpoint().map_err(QueryError::Observation)?;
        if self.output.is_some() {
            return Ok(true);
        }
        if let Some(preparation) = self.preparation.as_mut() {
            if preparation.advance(context).map_err(QueryError::from)? {
                self.enumeration = Some(
                    self.preparation
                        .take()
                        .ok_or_else(|| {
                            QueryError::Diagnostic("Missing completed enum query input".into())
                        })?
                        .finish()
                        .map_err(QueryError::from)?,
                );
            }
            return Ok(false);
        }
        context.work(1).map_err(QueryError::Observation)?;
        let enumeration = self
            .enumeration
            .as_ref()
            .ok_or_else(|| QueryError::Diagnostic("Missing enum query input".into()))?;
        let literal = enumeration
            .by_name(&self.name)
            .ok_or_else(|| QueryError::Diagnostic(format!("Unknown enum literal {}", self.name)))?;
        let observation = ParserObservation::new(context);
        observation
            .finish(observation.serialized(&literal.declaration, &literal.declaration.source))
            .map_err(QueryError::from)?;
        let literal = literal.declaration.clone();
        context.checkpoint().map_err(QueryError::Observation)?;
        self.output = Some(Target::EnumLiteral {
            enumeration: self
                .classifier
                .take()
                .ok_or_else(|| QueryError::Diagnostic("Missing enum query identity".into()))?,
            literal,
        });
        self.enumeration = None;
        Ok(true)
    }
    fn finish(mut self) -> Result<Target, QueryError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        self.output
            .take()
            .ok_or_else(|| QueryError::Diagnostic("Enum query is incomplete".into()))
    }
}
impl From<super::super::model_types::ModelPreparationError> for QueryError {
    fn from(error: super::super::model_types::ModelPreparationError) -> Self {
        match error {
            super::super::model_types::ModelPreparationError::Diagnostic(message) => {
                Self::Diagnostic(message)
            }
            super::super::model_types::ModelPreparationError::Observation(message) => {
                Self::Observation(message)
            }
        }
    }
}
enum PendingQuery<'a> {
    Contract(Box<ContractPreparation<'a>>),
    Enumeration(Box<EnumQueryPreparation<'a>>),
    Model(Box<super::super::model_types::ModelFeaturePreparation<'a>>),
}
impl PendingQuery<'_> {
    fn advance(&mut self, context: &InvocationContext<'_>) -> Result<bool, QueryError> {
        match self {
            Self::Contract(preparation) => preparation.advance(context).map_err(QueryError::from),
            Self::Enumeration(preparation) => preparation.advance(context),
            Self::Model(preparation) => preparation.advance(context).map_err(QueryError::from),
        }
    }
    fn finish(self) -> Result<Target, QueryError> {
        match self {
            Self::Contract(preparation) => preparation
                .finish()
                .map(Target::Feature)
                .map_err(QueryError::from),
            Self::Enumeration(preparation) => preparation.finish(),
            Self::Model(preparation) => preparation
                .finish()
                .map(|projection| Target::Feature(projection.declaration))
                .map_err(QueryError::from),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MetadataDriverError {
    Observation(String),
    Bridge(String),
}
impl std::fmt::Display for MetadataDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Observation(message) | Self::Bridge(message) => f.write_str(message),
        }
    }
}
// Source-dependent queries retain no observer or executable registry lease.
// Model-feature sources keep the original independent default source policy.
pub(super) struct MetadataDriver<'a> {
    bundle: &'a DefinitionBundle,
    resources: ResourceSet,
    limits: ParseLimits,
    pending: Option<PendingQuery<'a>>,
    failure: Option<MetadataDriverError>,
}
impl<'a> MetadataDriver<'a> {
    pub(super) fn new(
        bundle: &'a DefinitionBundle,
        resources: ResourceSet,
        limits: ParseLimits,
    ) -> Self {
        Self {
            bundle,
            resources,
            limits,
            pending: None,
            failure: None,
        }
    }
    pub(super) fn advance(
        &mut self,
        bridge: &MetadataBridge,
        context: &InvocationContext<'_>,
    ) -> Result<(), MetadataDriverError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(bridge, context);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn publish(
        &mut self,
        bridge: &MetadataBridge,
        result: Result<Target, QueryError>,
    ) -> Result<(), MetadataDriverError> {
        let reply = match result {
            Ok(target) => Ok(target),
            Err(QueryError::Import(error)) => Err(error.to_string()),
            Err(QueryError::Diagnostic(message)) => Err(message),
            Err(QueryError::Observation(message)) => {
                return Err(MetadataDriverError::Observation(message));
            }
        };
        bridge.publish(reply).map_err(MetadataDriverError::Bridge)
    }
    fn advance_inner(
        &mut self,
        bridge: &MetadataBridge,
        context: &InvocationContext<'_>,
    ) -> Result<(), MetadataDriverError> {
        context
            .checkpoint()
            .map_err(MetadataDriverError::Observation)?;
        if let Some(preparation) = self.pending.as_mut() {
            match preparation.advance(context) {
                Ok(false) => return Ok(()),
                Ok(true) => {
                    let preparation = self.pending.take().ok_or_else(|| {
                        MetadataDriverError::Bridge("Missing completed binding query".into())
                    })?;
                    return self.publish(bridge, preparation.finish());
                }
                Err(error) => {
                    self.pending = None;
                    return self.publish(bridge, Err(error));
                }
            }
        }
        let Some(request) = bridge.take_request().map_err(MetadataDriverError::Bridge)? else {
            return Ok(());
        };
        match request {
            MetadataQuery::ContractFeature {
                receiver,
                name,
                span,
            } => {
                match ContractPreparation::begin(
                    self.bundle,
                    self.resources.clone(),
                    self.limits,
                    ContractQuery {
                        receiver,
                        name,
                        span,
                    },
                    context,
                ) {
                    Ok(preparation) => {
                        self.pending = Some(PendingQuery::Contract(Box::new(preparation)));
                        Ok(())
                    }
                    Err(error) => self.publish(bridge, Err(QueryError::from(error))),
                }
            }
            MetadataQuery::EnumLiteral { classifier, name } => {
                // The literal is read from the resources this compile already
                // loaded under its limits, as contract features are.
                match EnumQueryPreparation::begin(self.resources.clone(), classifier, name, context)
                {
                    Ok(preparation) => {
                        self.pending = Some(PendingQuery::Enumeration(Box::new(preparation)));
                        Ok(())
                    }
                    Err(error) => self.publish(bridge, Err(error)),
                }
            }
            MetadataQuery::ModelFeature { expression, name } => {
                match super::super::model_types::ModelFeaturePreparation::begin(
                    self.bundle,
                    expression,
                    name,
                    context,
                ) {
                    Ok(preparation) => {
                        self.pending = Some(PendingQuery::Model(Box::new(preparation)));
                        Ok(())
                    }
                    Err(error) => self.publish(bridge, Err(QueryError::from(error))),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "rcore_behavior_binding_metadata_tests.rs"]
mod tests;
