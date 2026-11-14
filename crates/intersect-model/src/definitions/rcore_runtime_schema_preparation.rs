//! Shared model closure and binder query orchestration. Nested report algorithms,
//! owned-schema source initialization and individual copies remain synchronous.
use super::*;
use crate::definitions::{
    compiler::OwnedSchemaPreparation,
    conversion::InvocationContext,
    ecore::{ObservedImportError, ParserObservation},
    generic_resolution::{
        Resolver, ResolverPreparation, RuntimeMembers, RuntimeMembersPreparation,
    },
    generic_types::{RuntimeType, Type as ModelType},
    resources::ResourceSetPreparation,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelPreparationError {
    Diagnostic(String),
    Observation(String),
}
impl From<ObservedImportError> for ModelPreparationError {
    fn from(error: ObservedImportError) -> Self {
        match error {
            ObservedImportError::Import(error) => Self::Diagnostic(error.to_string()),
            ObservedImportError::Observation(message) => Self::Observation(message),
        }
    }
}
impl From<String> for ModelPreparationError {
    fn from(message: String) -> Self {
        Self::Diagnostic(message)
    }
}
impl std::fmt::Display for ModelPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Diagnostic(message) | Self::Observation(message) => f.write_str(message),
        }
    }
}
type Result<T> = std::result::Result<T, ModelPreparationError>;

// No borrowed report cursor or registry lease is needed: this is the same complete
// owned RuntimeMembers report the ordinary closure used, including cache evidence.
enum Scan {
    Parents(usize),
    Members {
        member: usize,
        signature: usize,
        ty: usize,
    },
    Bounds {
        bound: usize,
        ty: usize,
    },
    Domains(usize),
    Complete,
}
struct ReportScan {
    report: RuntimeMembers,
    scan: Scan,
}
impl ReportScan {
    // Exactly the former parents -> member signatures -> reference bounds ->
    // reference erasures order. Finish this scan before popping the next LIFO type.
    fn advance(&mut self, pending: &mut Vec<ModelType>, resources: &ResourceSet) -> Result<bool> {
        let target = match &mut self.scan {
            Scan::Parents(index) => {
                if let Some(parent) = self.report.parents.get(*index) {
                    pending.push(parent.clone());
                    *index += 1;
                } else {
                    self.scan = Scan::Members {
                        member: 0,
                        signature: 0,
                        ty: 0,
                    };
                }
                return Ok(false);
            }
            Scan::Members {
                member,
                signature,
                ty,
            } => {
                let Some(current) = self.report.members.get(*member) else {
                    self.scan = Scan::Bounds { bound: 0, ty: 0 };
                    return Ok(false);
                };
                if !current.formals.is_empty() {
                    *member += 1;
                    *signature = 0;
                    *ty = 0;
                    return Ok(false);
                }
                let Some(current) = current.signatures.get(*signature) else {
                    *member += 1;
                    *signature = 0;
                    *ty = 0;
                    return Ok(false);
                };
                let Some(target) = current.specialized.get(*ty) else {
                    *signature += 1;
                    *ty = 0;
                    return Ok(false);
                };
                *ty += 1;
                target
            }
            Scan::Bounds { bound, ty } => {
                let Some(current) = self.report.reference_bounds.get(*bound) else {
                    self.scan = Scan::Domains(0);
                    return Ok(false);
                };
                let Some(target) = current.bounds.get(*ty) else {
                    *bound += 1;
                    *ty = 0;
                    return Ok(false);
                };
                *ty += 1;
                target
            }
            Scan::Domains(index) => {
                let Some(domain) = self.report.feature_domains.get(*index) else {
                    self.scan = Scan::Complete;
                    return Ok(false);
                };
                *index += 1;
                if !domain.reference {
                    return Ok(false);
                }
                domain.erasure.expression()
            }
            Scan::Complete => return Ok(true),
        };
        let ModelType::Classifier { declaration, .. } = target else {
            return Ok(false);
        };
        if declaration.source == *crate::definitions::compiler::standard_datatype_source_id() {
            return Ok(false);
        }
        let node = resources.node(declaration)?;
        if node.attributes.iter().any(|attribute| {
            attribute.name.namespace.as_deref() == Some(crate::definitions::ecore::XSI)
                && attribute.name.local == "type"
                && attribute.type_name.as_ref().is_some_and(|name| {
                    name.namespace.as_deref() == Some(crate::definitions::ecore::ECORE)
                        && name.local == "EClass"
                })
        }) {
            pending.push(target.clone());
        }
        Ok(false)
    }
}
struct MemberCursor<'source> {
    preparation: Option<RuntimeMembersPreparation<'source, 'source>>,
}
self_cell::self_cell!(
    struct ClosureResolver {
        owner: Resolver,
        #[covariant]
        dependent: MemberCursor,
    }
);
impl ClosureResolver {
    fn idle(resolver: Resolver) -> Self {
        Self::new(resolver, |_| MemberCursor { preparation: None })
    }
}
enum Phase<'bundle> {
    Resolver(Box<ResolverPreparation<'bundle>>),
    Closure,
    Requested,
    Materialize(Box<OwnedSchemaPreparation<'bundle>>),
    // A preparation of the same key completed by another context sharing this
    // context's store: import its results instead of preparing them again.
    Shared(std::sync::Arc<super::runtime_schema_store::StoreEntry>),
    // Another thread sharing the store is preparing this key: await its entry
    // in bounded steps (at most `MAX_AWAIT_STEPS`), or prepare the key itself
    // if that thread releases it or the wait runs out.
    Awaiting(u32),
    Complete,
}
pub(super) struct RuntimeSchemaPreparation<'bundle> {
    phase: Phase<'bundle>,
    key: Vec<ModelType>,
    resolver: Option<ClosureResolver>,
    pending: Vec<ModelType>,
    visited: BTreeSet<ModelType>,
    requested: Vec<RuntimeType>,
    // `requested` as model types: the exact closure the schema materializes.
    closure: Vec<ModelType>,
    report: Option<ReportScan>,
    failure: Option<ModelPreparationError>,
    // This context's claim on preparing `key` cold, released on publication.
    claim: Option<super::runtime_schema_store::Claim>,
}
const STATE_BYTES: usize = 4096;
/// Bound on awaiting another thread's preparation of a key (about a minute of
/// bounded steps) before preparing it independently.
const MAX_AWAIT_STEPS: u32 = 12_000;
const _: () = assert!(
    std::mem::size_of::<RuntimeSchemaPreparation<'static>>()
        + std::mem::size_of::<Resolver>()
        + std::mem::size_of::<MemberCursor<'static>>()
        + 128
        <= STATE_BYTES
);
impl<'bundle> RuntimeSchemaPreparation<'bundle> {
    // Private to one fixed ModelTypes attempt. Each advance receives that model
    // and its current operation; no observer or executable registry is retained.
    pub(super) fn begin(
        models: &ModelTypes<'bundle>,
        expressions: &[ModelType],
        context: &InvocationContext<'_>,
    ) -> Result<Self> {
        context
            .checkpoint()
            .map_err(ModelPreparationError::Observation)?;
        context
            .retain_storage(STATE_BYTES)
            .map_err(ModelPreparationError::Observation)?;
        let mut key = expressions.to_vec();
        key.sort();
        key.dedup();
        let mut claim = None;
        let phase = if models.runtime_schemas.contains_key(&key) {
            Phase::Complete
        } else {
            // A metered context's steps must not depend on other threads, so
            // only unobserved contexts await another thread's preparation.
            let lookup = models
                .schema_store
                .as_ref()
                .map(|store| store.lookup(&key, !context.has_observer()));
            Self::obtain(lookup, &mut claim, models, context)?
        };
        Ok(Self {
            phase,
            pending: key.clone(),
            key,
            resolver: None,
            visited: BTreeSet::new(),
            requested: vec![],
            closure: vec![],
            report: None,
            failure: None,
            claim,
        })
    }
    /// The phase that obtains a key this context has not prepared.
    fn obtain(
        lookup: Option<super::runtime_schema_store::Lookup>,
        claim: &mut Option<super::runtime_schema_store::Claim>,
        models: &ModelTypes<'bundle>,
        context: &InvocationContext<'_>,
    ) -> Result<Phase<'bundle>> {
        use super::runtime_schema_store::Lookup;
        Ok(match lookup {
            Some(Lookup::Ready(entry)) => Phase::Shared(entry),
            Some(Lookup::Busy) => Phase::Awaiting(0),
            Some(Lookup::Prepare(owned)) => {
                *claim = owned;
                Self::cold(models, context)?
            }
            None => Self::cold(models, context)?,
        })
    }
    fn cold(
        models: &ModelTypes<'bundle>,
        context: &InvocationContext<'_>,
    ) -> Result<Phase<'bundle>> {
        Ok(Phase::Resolver(Box::new(ResolverPreparation::begin(
            models.bundle,
            Some(models.resources.clone()),
            models.source_limits,
            context,
        )?)))
    }
    pub(super) fn advance(
        &mut self,
        models: &mut ModelTypes<'bundle>,
        context: &InvocationContext<'_>,
    ) -> Result<bool> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(models, context);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn advance_inner(
        &mut self,
        models: &mut ModelTypes<'bundle>,
        context: &InvocationContext<'_>,
    ) -> Result<bool> {
        context
            .checkpoint()
            .map_err(ModelPreparationError::Observation)?;
        if matches!(self.phase, Phase::Complete) {
            return Ok(true);
        }
        context
            .work(1)
            .map_err(ModelPreparationError::Observation)?;
        match &mut self.phase {
            Phase::Resolver(preparation) => {
                if preparation.advance(context)? {
                    let Phase::Resolver(preparation) =
                        std::mem::replace(&mut self.phase, Phase::Closure)
                    else {
                        return Err("Missing model closure resolver".to_string().into());
                    };
                    let resolver = preparation.finish()?;
                    // Existing conversion-policy setup is a synchronous boundary;
                    // it stores relation data, never the executable registry.
                    self.resolver = Some(ClosureResolver::idle(match models.conversions {
                        Some(registry) => resolver
                            .with_conversions(registry)
                            .map_err(|e| e.to_string())?,
                        None => resolver,
                    }));
                }
            }
            Phase::Closure => {
                let holder = self
                    .resolver
                    .as_mut()
                    .ok_or_else(|| "Missing model closure resolver".to_string())?;
                let observation = ParserObservation::new(context);
                let progress = holder.with_dependent_mut(|_, cursor| {
                    let Some(preparation) = cursor.preparation.as_mut() else {
                        return Ok(None);
                    };
                    if !observation.finish(preparation.advance(&observation))? {
                        return Ok(Some(None));
                    }
                    let preparation = cursor.preparation.take().ok_or_else(|| {
                        ModelPreparationError::Diagnostic(
                            "Missing completed model member preparation".into(),
                        )
                    })?;
                    observation
                        .finish(preparation.finish_owned(&observation))
                        .map(|report| Some(Some(report)))
                        .map_err(ModelPreparationError::from)
                })?;
                if let Some(report) = progress {
                    if let Some(report) = report {
                        if matches!(
                            report.receiver.expression(),
                            ModelType::Classifier { raw: true, .. }
                        ) || matches!(report.receiver.expression(),ModelType::Classifier{arguments,..} if !arguments.is_empty())
                        {
                            models
                                .generic_checks
                                .insert(report.receiver.expression().clone(), report.clone());
                        }
                        self.report = Some(ReportScan {
                            report,
                            scan: Scan::Parents(0),
                        });
                    }
                    return Ok(false);
                }
                if let Some(report) = &mut self.report {
                    if report.advance(&mut self.pending, &models.resources)? {
                        self.report = None;
                    }
                    return Ok(false);
                }
                let Some(expression) = self.pending.pop() else {
                    if let [root] = self.key.as_slice() {
                        models
                            .runtime_closures
                            .insert(root.clone(), self.visited.clone());
                    }
                    self.resolver = None;
                    self.phase = Phase::Requested;
                    return Ok(false);
                };
                if self.visited.contains(&expression) {
                    return Ok(false);
                }
                // A closure is transitive: it holds the closure of each of its
                // members, so a type whose closure this model context already
                // walked adds that closure instead of walking it again.
                if let Some(closure) = models.runtime_closures.get(&expression) {
                    self.visited.extend(closure.iter().cloned());
                    if self.visited.len() > 1024 {
                        return Err("Rcore model application closure exceeds 1024 types"
                            .to_string()
                            .into());
                    }
                    return Ok(false);
                }
                self.visited.insert(expression.clone());
                if self.visited.len() > 1024 {
                    return Err("Rcore model application closure exceeds 1024 types"
                        .to_string()
                        .into());
                }
                let runtime = RuntimeType::new(expression)?;
                let holder = self
                    .resolver
                    .as_mut()
                    .ok_or_else(|| "Missing model closure resolver".to_string())?;
                holder.with_dependent_mut(|resolver, cursor| {
                    cursor.preparation = Some(RuntimeMembersPreparation::begin(
                        resolver,
                        std::borrow::Cow::Owned(runtime),
                        true,
                    ));
                });
            }

            Phase::Requested => {
                if let Some(expression) = self.visited.pop_first() {
                    self.requested.push(RuntimeType::new(expression.clone())?);
                    self.closure.push(expression);
                } else if let Some(schema) = models
                    .materialized
                    .get(&self.closure)
                    .and_then(|key| models.runtime_schemas.get(key))
                    .cloned()
                {
                    // The owned schema depends only on the requested closure and
                    // this model context's fixed bundle, resources and
                    // conversions: reuse the one materialized for that closure.
                    self.publish(models, &schema);
                    models.runtime_schemas.insert(self.key.clone(), schema);
                    self.phase = Phase::Complete;
                } else {
                    self.phase = Phase::Materialize(Box::new(
                        OwnedSchemaPreparation::begin_with_operation_cache(
                            models.bundle,
                            &self.requested,
                            models.conversions,
                            models.resources.clone(),
                            models
                                .conversions
                                .is_none()
                                .then(|| models.runtime_operations.clone()),
                            context,
                        )?,
                    ));
                }
            }
            Phase::Materialize(preparation) => {
                if preparation.advance(models.conversions, context)? {
                    let Phase::Materialize(preparation) =
                        std::mem::replace(&mut self.phase, Phase::Complete)
                    else {
                        return Err("Missing model schema materializer".to_string().into());
                    };
                    let schema = std::sync::Arc::new(preparation.finish_checked(context)?);
                    context
                        .checkpoint()
                        .map_err(ModelPreparationError::Observation)?;
                    self.publish(models, &schema);
                    models.runtime_schemas.insert(self.key.clone(), schema);
                    models
                        .materialized
                        .insert(std::mem::take(&mut self.closure), self.key.clone());
                }
            }
            Phase::Shared(entry) => {
                // Exactly what the cold phases record for this key: its schema,
                // the closure of a single requested root, the closure's
                // materialized key and the generic checks of its members.
                let entry = entry.clone();
                if let [root] = self.key.as_slice() {
                    models
                        .runtime_closures
                        .insert(root.clone(), entry.closure.iter().cloned().collect());
                }
                for (expression, report) in &entry.generic_checks {
                    models
                        .generic_checks
                        .entry(expression.clone())
                        .or_insert_with(|| report.clone());
                }
                models
                    .materialized
                    .entry(entry.closure.clone())
                    .or_insert_with(|| self.key.clone());
                models
                    .runtime_schemas
                    .insert(self.key.clone(), entry.schema.clone());
                self.phase = Phase::Complete;
            }
            Phase::Awaiting(steps) => {
                *steps += 1;
                let patient = *steps <= MAX_AWAIT_STEPS;
                let lookup = models.schema_store.as_ref().map(|store| {
                    if patient {
                        store.await_step(&self.key)
                    } else {
                        store.lookup(&self.key, false)
                    }
                });
                if !matches!(lookup, Some(super::runtime_schema_store::Lookup::Busy)) {
                    self.phase = Self::obtain(lookup, &mut self.claim, models, context)?;
                }
            }
            Phase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
    fn publish(&mut self, models: &ModelTypes<'bundle>, schema: &std::sync::Arc<CheckedSchema>) {
        if let Some(store) = &models.schema_store {
            store.publish(&self.key, schema, &self.closure, &models.generic_checks);
        }
        self.claim = None;
    }
    pub(super) fn finish(mut self) -> Result<Vec<ModelType>> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if !matches!(self.phase, Phase::Complete) {
            return Err("Runtime schema preparation is incomplete"
                .to_string()
                .into());
        }
        Ok(self.key)
    }
}

// Binder model features deliberately use ModelTypes::new's DEFAULT source policy,
// independently of the enclosing binder's limits or already-admitted resources.
// The full source -> receiver -> closure -> owned schema -> projection path is
// retained here, outside the pure AST future and without executable providers.
pub(crate) struct ModelFeaturePreparation<'bundle> {
    bundle: &'bundle DefinitionBundle,
    sources: Option<Box<ResourceSetPreparation<'bundle>>>,
    models: Option<ModelTypes<'bundle>>,
    expression: ModelType,
    name: String,
    runtime: Option<RuntimeType>,
    schema: Option<RuntimeSchemaPreparation<'bundle>>,
    key: Option<Vec<ModelType>>,
    output: Option<FeatureProjection>,
    failure: Option<ModelPreparationError>,
}
const FEATURE_BYTES: usize = 4096;
const _: () =
    assert!(std::mem::size_of::<ModelFeaturePreparation<'static>>() + 128 <= FEATURE_BYTES);
impl<'bundle> ModelFeaturePreparation<'bundle> {
    pub(crate) fn begin(
        bundle: &'bundle DefinitionBundle,
        expression: ModelType,
        name: String,
        context: &InvocationContext<'_>,
    ) -> Result<Self> {
        context
            .checkpoint()
            .map_err(ModelPreparationError::Observation)?;
        context
            .retain_storage(FEATURE_BYTES)
            .map_err(ModelPreparationError::Observation)?;
        let sources = ResourceSetPreparation::begin_with_retained_bundle(
            bundle,
            Default::default(),
            context,
        )?;
        Ok(Self {
            bundle,
            sources: Some(Box::new(sources)),
            models: None,
            expression,
            name,
            runtime: None,
            schema: None,
            key: None,
            output: None,
            failure: None,
        })
    }
    pub(crate) fn advance(&mut self, context: &InvocationContext<'_>) -> Result<bool> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(context);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn advance_inner(&mut self, context: &InvocationContext<'_>) -> Result<bool> {
        context
            .checkpoint()
            .map_err(ModelPreparationError::Observation)?;
        if self.output.is_some() {
            return Ok(true);
        }
        if let Some(sources) = self.sources.as_mut() {
            if sources.advance(context)? {
                let sources = self
                    .sources
                    .take()
                    .ok_or_else(|| "Missing model query sources".to_string())?
                    .finish()?;
                self.models = Some(ModelTypes::with_source_resources(
                    self.bundle,
                    None,
                    Default::default(),
                    sources,
                ));
            }
            return Ok(false);
        }
        let models = self
            .models
            .as_mut()
            .ok_or_else(|| "Missing model query context".to_string())?;
        if self.runtime.is_none() {
            self.runtime = Some(RuntimeType::new(self.expression.clone())?);
            self.schema = Some(RuntimeSchemaPreparation::begin(
                models,
                std::slice::from_ref(&self.expression),
                context,
            )?);
            return Ok(false);
        }
        if let Some(schema) = self.schema.as_mut() {
            if schema.advance(models, context)? {
                self.key = Some(
                    self.schema
                        .take()
                        .ok_or_else(|| "Missing model query schema".to_string())?
                        .finish()?,
                );
            }
            return Ok(false);
        }
        context
            .work(1)
            .map_err(ModelPreparationError::Observation)?;
        let key = self
            .key
            .as_ref()
            .ok_or_else(|| "Missing model query schema key".to_string())?;
        let schema = models
            .runtime_schemas
            .get(key)
            .ok_or_else(|| "Missing checked model application context".to_string())?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| "Missing model query receiver".to_string())?;
        let projection = project_feature(schema, runtime, &self.name)?;
        context
            .checkpoint()
            .map_err(ModelPreparationError::Observation)?;
        self.output = Some(projection);
        Ok(true)
    }
    pub(crate) fn finish(mut self) -> Result<FeatureProjection> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        self.output
            .take()
            .ok_or_else(|| "Model feature preparation is incomplete".to_string().into())
    }
}

#[cfg(test)]
#[path = "rcore_runtime_schema_preparation_tests.rs"]
mod tests;
