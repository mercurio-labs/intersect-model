//! Immutable model-reference domains validated by the shared schema and core model.
use intersect_model::core::model::{Element, Model};
use intersect_model::definitions::{
    compiler,
    conversion::{ConversionRegistry, InvocationContext, ReferenceContext},
    resources::DeclarationId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};
#[path = "model_reference_policy_preparation.rs"]
mod policy_preparation;
use policy_preparation::PolicyPreparation;

/// Storage representation selection over the same fresh definition compiler.
/// RuntimeAlternative is sealed; these factories cache no semantic outcomes.
// The only retained compiler state has a portable preflight for its boxed record.
const RETAINED_COMPILATION_BYTES: usize = 2048;
const _: () = assert!(
    std::mem::size_of::<compiler::OwnedRuntimeSourceArrayPreparation>() + 128
        <= RETAINED_COMPILATION_BYTES
);
/// Owning schema state selected by the sealed runtime representation factories.
/// Only a completed definitions check can produce the immutable variant.
pub enum ReferenceSchema<A: intersect_model::definitions::schema::RuntimeAlternative> {
    Ordinary(intersect_model::definitions::schema::SchemaRepresentation<A>),
    Checked(intersect_model::definitions::schema::CheckedRuntimeSchema<A>),
}
impl<A: intersect_model::definitions::schema::RuntimeAlternative> ReferenceSchema<A> {
    fn schema(&self) -> &intersect_model::definitions::schema::SchemaRepresentation<A> {
        match self {
            Self::Ordinary(schema) => schema,
            Self::Checked(schema) => schema.schema(),
        }
    }
    fn begin_target(
        &self,
        target: Option<&intersect_model::definitions::generic_types::RuntimeType>,
        class: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<intersect_model::definitions::schema::ClassKeyPreparation, String> {
        use intersect_model::definitions::schema::ClassKeyPreparation;
        match (self, target) {
            (Self::Checked(schema), Some(target)) => {
                schema.class_key_preparation_observed(target, observation)
            }
            (Self::Checked(schema), None) => {
                schema.reference_declaration_preparation_observed(class, observation)
            }
            (Self::Ordinary(_), Some(target)) => {
                ClassKeyPreparation::new_observed(target, observation)
            }
            (Self::Ordinary(_), None) => {
                ClassKeyPreparation::reference_declaration_observed(class, observation)
            }
        }
        .map_err(|error| error.to_string())
    }
    fn advance_target(
        &self,
        pending: &mut intersect_model::definitions::schema::ClassKeyPreparation,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        match self {
            Self::Ordinary(schema) => pending.advance_observed(schema, observation),
            Self::Checked(schema) => pending.advance_checked_observed(schema, observation),
        }
        .map_err(|error| error.to_string())
    }
    fn advance_model(
        &self,
        pending: &mut intersect_model::core::storage::StructuralPreparation,
        model: &intersect_model::core::storage::OwnedModelView,
        registry: &ConversionRegistry<'_>,
        scoped: Option<&InvocationContext<'_>>,
    ) -> Result<bool, String> {
        match self {
            Self::Ordinary(schema) => pending.advance(model, schema, Some(registry), scoped),
            Self::Checked(schema) => {
                pending.advance_checked_schema(model, schema, Some(registry), scoped)
            }
        }
    }
}
pub trait ReferenceSchemaFactory: intersect_model::definitions::schema::RuntimeAlternative {
    type Compilation;
    /// Transfer a materialized source world into its existing retained compiler.
    /// None preserves the ordinary synchronous path and its receipt ordering.
    fn begin_compilation(
        _context: &mut ReferenceContext,
        _registry: &ConversionRegistry<'_>,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        Ok(None)
    }
    /// Request retained generic-agreement polling. Unsupported schema families
    /// reject explicitly rather than falling back to synchronous compilation.
    fn begin_compilation_with_retained_generic_agreement(
        _context: &mut ReferenceContext,
        _registry: &ConversionRegistry<'_>,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        Err("Retained generic agreement is unsupported by this schema factory".into())
    }
    /// Request source-inventory agreement polling with the mode fixed at creation.
    /// Unsupported schema families reject explicitly.
    fn begin_compilation_with_retained_source_inventory_agreement(
        _context: &mut ReferenceContext,
        _registry: &ConversionRegistry<'_>,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        Err("Retained source inventory agreement is unsupported by this schema factory".into())
    }
    /// Retain GenericType source decoding with polling fixed at construction.
    fn begin_compilation_with_retained_generic_source_decoding(
        _context: &mut ReferenceContext,
        _registry: &ConversionRegistry<'_>,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        Err("Retained generic source decoding is unsupported by this schema factory".into())
    }
    fn advance_compilation(
        _pending: &mut Self::Compilation,
        _registry: &ConversionRegistry<'_>,
        _observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        Err("No retained schema compilation".into())
    }
    fn finish_compilation(
        _pending: Self::Compilation,
        _context: &mut ReferenceContext,
        _observation: &InvocationContext<'_>,
    ) -> Result<ReferenceSchema<Self>, String> {
        Err("No retained schema compilation".into())
    }
    fn compile(
        context: &ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<intersect_model::definitions::schema::SchemaRepresentation<Self>, String>;
}
impl ReferenceSchemaFactory for String {
    type Compilation = ();
    fn compile(
        context: &ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<intersect_model::definitions::schema::Schema, String> {
        if context.materialized_types.is_empty() {
            compiler::compile_structure_bundle_with_conversions_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                registry,
                observation,
            )
        } else {
            compiler::materialize_runtime_bundle_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                &context.materialized_types,
                Some(registry),
                observation,
            )
        }
        .map_err(|error| error.to_string())
    }
}
impl ReferenceSchemaFactory for intersect_model::definitions::schema::SharedRuntimeArrayTarget {
    type Compilation = ();
    fn compile(
        context: &ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<intersect_model::definitions::schema::RuntimeArraySchemaInputs, String> {
        if context.materialized_types.is_empty() {
            compiler::compile_structure_bundle_with_conversions_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                registry,
                observation,
            )
            .map_err(|error| error.to_string())?
            .into_shared_array_inputs_observed(observation)
        } else {
            compiler::materialize_runtime_array_inputs_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                &context.materialized_types,
                Some(registry),
                observation,
            )
            .map_err(|error| error.to_string())
        }
    }
}
impl ReferenceSchemaFactory for intersect_model::definitions::schema::SharedRuntimeArrayViewTarget {
    type Compilation = ();
    fn compile(
        context: &ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<intersect_model::definitions::schema::RuntimeArrayViewSchemaInputs, String> {
        if context.materialized_types.is_empty() {
            compiler::compile_structure_bundle_with_conversions_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                registry,
                observation,
            )
            .map_err(|error| error.to_string())?
            .into_shared_array_view_inputs_observed(observation)
        } else {
            compiler::materialize_runtime_array_view_inputs_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                &context.materialized_types,
                Some(registry),
                observation,
            )
            .map_err(|error| error.to_string())
        }
    }
}

impl ReferenceSchemaFactory
    for intersect_model::definitions::schema::SharedRuntimeSourceArrayTarget
{
    type Compilation = compiler::OwnedRuntimeSourceArrayPreparation;
    fn begin_compilation(
        context: &mut ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        begin_source_array_compilation(
            context,
            registry,
            observation,
            ReferenceCompilationPolling::Compatibility,
        )
    }
    fn begin_compilation_with_retained_generic_agreement(
        context: &mut ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        begin_source_array_compilation(
            context,
            registry,
            observation,
            ReferenceCompilationPolling::RetainedGenericAgreement,
        )
    }
    fn begin_compilation_with_retained_source_inventory_agreement(
        context: &mut ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        begin_source_array_compilation(
            context,
            registry,
            observation,
            ReferenceCompilationPolling::RetainedSourceInventoryAgreement,
        )
    }
    /// Retain GenericType source decoding with polling fixed at construction.
    fn begin_compilation_with_retained_generic_source_decoding(
        context: &mut ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<Self::Compilation>, String> {
        begin_source_array_compilation(
            context,
            registry,
            observation,
            ReferenceCompilationPolling::RetainedGenericSourceDecoding,
        )
    }
    fn advance_compilation(
        pending: &mut Self::Compilation,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        pending
            .advance(Some(registry), observation)
            .map_err(|error| error.to_string())
    }
    fn finish_compilation(
        pending: Self::Compilation,
        context: &mut ReferenceContext,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceSchema<Self>, String> {
        let finished = pending.finish_checked(observation);
        context.bundle = finished.bundle;
        if finished.entry_source != context.entry || finished.namespace != context.namespace {
            return Err("Retained schema selection changed".into());
        }
        finished
            .result
            .map(ReferenceSchema::Checked)
            .map_err(|error| error.to_string())
    }
    fn compile(
        context: &ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<intersect_model::definitions::schema::RuntimeSourceArraySchemaInputs, String> {
        if context.materialized_types.is_empty() {
            compiler::compile_structure_bundle_with_conversions_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                registry,
                observation,
            )
            .map_err(|error| error.to_string())?
            .into_shared_source_array_inputs_observed(observation)
        } else {
            compiler::materialize_runtime_source_array_inputs_observed(
                &context.bundle,
                &context.entry,
                &context.namespace,
                &context.materialized_types,
                Some(registry),
                observation,
            )
            .map_err(|error| error.to_string())
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReferenceCompilationPolling {
    Compatibility,
    RetainedGenericAgreement,
    RetainedSourceInventoryAgreement,
    RetainedGenericSourceDecoding,
}
impl ReferenceCompilationPolling {
    fn missing_types(self) -> &'static str {
        match self {
            Self::RetainedSourceInventoryAgreement => {
                "Retained source inventory agreement requires materialized runtime types"
            }
            Self::RetainedGenericSourceDecoding => {
                "Retained generic source decoding requires materialized runtime types"
            }
            _ => "Retained generic agreement requires materialized runtime types",
        }
    }
    fn missing_compilation(self) -> &'static str {
        match self {
            Self::RetainedSourceInventoryAgreement => {
                "Retained source inventory agreement requires retained schema compilation"
            }
            Self::RetainedGenericSourceDecoding => {
                "Retained generic source decoding requires retained schema compilation"
            }
            _ => "Retained generic agreement requires retained schema compilation",
        }
    }
}
// All public host entries share source transfer, original admissions and failure
// recovery. The chosen compiler constructor fixes polling policy before returning.
fn begin_source_array_compilation(
    context: &mut ReferenceContext,
    registry: &ConversionRegistry<'_>,
    observation: &InvocationContext<'_>,
    polling: ReferenceCompilationPolling,
) -> Result<Option<compiler::OwnedRuntimeSourceArrayPreparation>, String> {
    if context.materialized_types.is_empty() {
        return if polling != ReferenceCompilationPolling::Compatibility {
            Err(polling.missing_types().into())
        } else {
            Ok(None)
        };
    }
    intersect_model::definitions::retained_storage::reserve_serialized(
        &context.entry,
        observation,
    )?;
    observation.retain_storage(
        64usize
            .checked_add(context.namespace.len())
            .ok_or("Schema selection storage overflow")?,
    )?;
    let empty = intersect_model::definitions::bundle::DefinitionBundle {
        version: context.bundle.version,
        sources: vec![],
        namespaces: vec![],
    };
    let bundle = std::mem::replace(&mut context.bundle, empty);
    let begin = match polling {
        ReferenceCompilationPolling::Compatibility => compiler::OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources,
        ReferenceCompilationPolling::RetainedGenericAgreement => compiler::OwnedRuntimeSourceArrayPreparation::begin_with_retained_generic_agreement,
        ReferenceCompilationPolling::RetainedSourceInventoryAgreement => compiler::OwnedRuntimeSourceArrayPreparation::begin_with_retained_source_inventory_agreement,
        ReferenceCompilationPolling::RetainedGenericSourceDecoding => compiler::OwnedRuntimeSourceArrayPreparation::begin_with_retained_generic_source_decoding,
    };
    match begin(
        bundle,
        context.entry.clone(),
        context.namespace.clone(),
        &context.materialized_types,
        Some(registry),
        observation,
    ) {
        Ok(pending) => Ok(Some(pending)),
        Err(failure) => {
            context.bundle = failure.materialization.bundle;
            match failure.materialization.result {
                Err(error) => Err(error.to_string()),
                Ok(_) => Err("Schema compilation failed without an error".into()),
            }
        }
    }
}
pub type ReferenceDomain = ReferenceDomainState<String>;
pub type ReferenceDomainPreparation = ReferenceDomainPreparationState<String>;
pub type ReferenceAdmission = ReferenceAdmissionState<String>;
pub(crate) type RuntimeReferenceDomain =
    ReferenceDomainState<intersect_model::definitions::schema::SharedRuntimeSourceArrayTarget>;
pub(crate) type RuntimeReferenceDomainPreparation = ReferenceDomainPreparationState<
    intersect_model::definitions::schema::SharedRuntimeSourceArrayTarget,
>;
pub(crate) type RuntimeReferenceAdmission =
    ReferenceAdmissionState<intersect_model::definitions::schema::SharedRuntimeSourceArrayTarget>;

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReferenceValue {
    ModelReference { context: String, element: String },
}

#[derive(Clone)]
pub struct ReferenceDomainState<A: ReferenceSchemaFactory = String> {
    class: DeclarationId,
    runtime_target: Option<intersect_model::definitions::generic_types::RuntimeType>,
    target_key: String,
    admitted: Arc<AdmittedContext<A>>,
    allowed: BTreeSet<usize>,
}
/// Immutable admission authority shared only by domains derived from this exact
/// snapshot. Target membership remains separate from model/schema validation.
struct AdmittedContext<A: ReferenceSchemaFactory> {
    context: ReferenceContext,
    digest: String,
    model: intersect_model::core::storage::OwnedModelView,
    ownership: intersect_model::core::storage::OwnershipIndex,
    schema: ReferenceSchema<A>,
}
impl<A: ReferenceSchemaFactory> ReferenceDomainState<A> {
    pub fn new(
        class: DeclarationId,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
    ) -> Result<Self, String> {
        Self::new_context(class, None, context, registry, None)
    }
    pub fn new_runtime(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
    ) -> Result<Self, String> {
        Self::new_runtime_context(target, context, registry, None)
    }
    /// Build a domain with operation-scoped cancellation/work observation.
    pub fn new_runtime_observed(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, String> {
        Self::new_runtime_context(target, context, registry, Some(observation))
    }
    fn new_runtime_context(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: Option<&InvocationContext<'_>>,
    ) -> Result<Self, String> {
        let intersect_model::definitions::generic_types::Type::Classifier { declaration, .. } =
            target.expression()
        else {
            return Err("Reference runtime target must be a classifier".into());
        };
        if let Some(observation) = observation.filter(|context| context.has_observer()) {
            intersect_model::definitions::retained_storage::reserve_serialized(
                declaration,
                observation,
            )?;
        }
        Self::new_context(
            declaration.clone(),
            Some(target),
            context,
            registry,
            observation,
        )
    }
    fn new_context(
        class: DeclarationId,
        runtime_target: Option<intersect_model::definitions::generic_types::RuntimeType>,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        scoped: Option<&InvocationContext<'_>>,
    ) -> Result<Self, String> {
        let unobserved = InvocationContext::default();
        let observation = scoped.unwrap_or(&unobserved);
        let mut pending = Self::begin_context(class, runtime_target, context, registry, scoped)?;
        while !pending.advance(registry, observation)? {}
        pending.finish(observation)
    }
    /// Prepare a runtime reference context in stages. Materialized source-array
    /// compilation advances through the retained definitions compiler. Initialization
    /// and individual report/class stages remain synchronous; other schema families
    /// retain their synchronous compilation path. Policy comparison, target selection,
    /// indexing and membership advance incrementally. Snapshot decoding yields between objects; each
    /// object's stored payload and canonical comparison remain synchronous.
    /// Structural validation yields between objects and containment edges. Use the same registry
    /// for every advance; no domain is published before all stages succeed.
    pub(crate) fn begin_prepared_runtime_observed(
        target: intersect_model::definitions::generic_types::RuntimeType,
        input: crate::rcore_behavior_functions::context::PreparedReferenceInput,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        let (context, digest) = input.into_parts();
        let mut pending = Self::begin_runtime_observed(target, context, registry, observation)?;
        pending
            .spec
            .as_mut()
            .ok_or("Missing prepared reference specification")?
            .prepared_digest = Some(digest);
        Ok(pending)
    }
    pub fn begin_runtime_observed(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        Self::begin_runtime_observed_with_polling(
            target,
            context,
            registry,
            observation,
            ReferenceCompilationPolling::Compatibility,
        )
    }
    /// Opt in to retained generic-agreement steps for a materialized source-array
    /// context. Existing entries preserve their original polling and callback
    /// boundaries. Construction fixes this mode; advances still require the
    /// selected current registry and fresh resources. Source-inventory nodes,
    /// repeated policy comparison and other documented compiler tails remain
    /// synchronous. Only finish publishes a usable domain.
    pub fn begin_runtime_observed_with_retained_generic_agreement(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        Self::begin_runtime_observed_with_polling(
            target,
            context,
            registry,
            observation,
            ReferenceCompilationPolling::RetainedGenericAgreement,
        )
    }
    /// Retain source-inventory legacy agreement as well as report agreement for
    /// a materialized source-array context. Construction fixes this additive
    /// polling profile; existing entries keep their original callback boundaries.
    /// Every advance still validates the current registry with fresh resources.
    /// Individual resolution, repeated policy comparison and other documented
    /// compiler tails remain synchronous. Only finish publishes a usable domain.
    pub fn begin_runtime_observed_with_retained_source_inventory_agreement(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        Self::begin_runtime_observed_with_polling(
            target,
            context,
            registry,
            observation,
            ReferenceCompilationPolling::RetainedSourceInventoryAgreement,
        )
    }
    /// Retain GenericType source decoding and the earlier report/inventory
    /// agreement phases. Every retained compilation advance checks the current
    /// registry with fresh resources; only finish publishes a usable domain. Target resolution,
    /// policy comparison and other documented compiler tails remain synchronous.
    pub fn begin_runtime_observed_with_retained_generic_source_decoding(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        Self::begin_runtime_observed_with_polling(
            target,
            context,
            registry,
            observation,
            ReferenceCompilationPolling::RetainedGenericSourceDecoding,
        )
    }
    fn begin_runtime_observed_with_polling(
        target: intersect_model::definitions::generic_types::RuntimeType,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
        polling: ReferenceCompilationPolling,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        let intersect_model::definitions::generic_types::Type::Classifier { declaration, .. } =
            target.expression()
        else {
            return Err("Reference runtime target must be a classifier".into());
        };
        if observation.has_observer() {
            intersect_model::definitions::retained_storage::reserve_serialized(
                declaration,
                observation,
            )?;
        }
        Self::begin_context_with_polling(
            declaration.clone(),
            Some(target),
            context,
            registry,
            Some(observation),
            polling,
        )
    }
    fn begin_context(
        class: DeclarationId,
        runtime_target: Option<intersect_model::definitions::generic_types::RuntimeType>,
        context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        scoped: Option<&InvocationContext<'_>>,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        Self::begin_context_with_polling(
            class,
            runtime_target,
            context,
            registry,
            scoped,
            ReferenceCompilationPolling::Compatibility,
        )
    }
    fn begin_context_with_polling(
        class: DeclarationId,
        runtime_target: Option<intersect_model::definitions::generic_types::RuntimeType>,
        mut context: ReferenceContext,
        registry: &ConversionRegistry<'_>,
        scoped: Option<&InvocationContext<'_>>,
        polling: ReferenceCompilationPolling,
    ) -> Result<ReferenceAdmissionState<A>, String> {
        let unobserved = InvocationContext::default();
        let observation = scoped.unwrap_or(&unobserved);
        observation.work(1)?;
        let pending = match polling {
            ReferenceCompilationPolling::Compatibility => {
                A::begin_compilation(&mut context, registry, observation)?
            }
            ReferenceCompilationPolling::RetainedGenericAgreement => {
                A::begin_compilation_with_retained_generic_agreement(
                    &mut context,
                    registry,
                    observation,
                )?
            }
            ReferenceCompilationPolling::RetainedSourceInventoryAgreement => {
                A::begin_compilation_with_retained_source_inventory_agreement(
                    &mut context,
                    registry,
                    observation,
                )?
            }
            ReferenceCompilationPolling::RetainedGenericSourceDecoding => {
                A::begin_compilation_with_retained_generic_source_decoding(
                    &mut context,
                    registry,
                    observation,
                )?
            }
        };
        let (schema, stage) = match pending {
            Some(pending) => {
                observation.retain_storage(RETAINED_COMPILATION_BYTES)?;
                (None, AdmissionStage::Compilation(Box::new(pending)))
            }
            None if polling != ReferenceCompilationPolling::Compatibility => {
                return Err(polling.missing_compilation().into());
            }
            None => (
                Some(ReferenceSchema::Ordinary(A::compile(
                    &context,
                    registry,
                    observation,
                )?)),
                AdmissionStage::Policies(PolicyPreparation::new()),
            ),
        };
        Ok(ReferenceAdmissionState {
            spec: Some(AdmissionSpec {
                class,
                runtime_target,
                context,
                schema,
                target: String::new(),
                observed: scoped.is_some(),
                prepared_digest: None,
            }),
            stage: Some(stage),
        })
    }
    /// Shared validated stored ownership for this immutable admitted snapshot.
    pub fn ownership(&self) -> &intersect_model::core::storage::OwnershipIndex {
        &self.admitted.ownership
    }
    /// Select another exact runtime target over this already admitted snapshot.
    /// This reuses schema, provider validation, indexes and context identity. It
    /// still observes every object inspected for the new target membership.
    pub fn retarget_runtime_observed(
        &self,
        target: intersect_model::definitions::generic_types::RuntimeType,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, String> {
        let mut pending = self.begin_retarget_runtime(target, observation)?;
        while !pending.advance(observation)? {}
        pending.finish(observation)
    }
    /// Start selecting another runtime target without scanning the admitted model.
    /// Each advance inspects at most one object. Only finish exposes a usable domain.
    pub fn begin_retarget_runtime(
        &self,
        target: intersect_model::definitions::generic_types::RuntimeType,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceDomainPreparationState<A>, String> {
        observation.work(1)?;
        let intersect_model::definitions::generic_types::Type::Classifier { declaration, .. } =
            target.expression()
        else {
            return Err("Reference runtime target must be a classifier".into());
        };
        if observation.has_observer() {
            intersect_model::definitions::retained_storage::reserve_serialized(
                declaration,
                observation,
            )?;
        }
        let key = self
            .admitted
            .schema
            .begin_target(Some(&target), declaration, observation)?;
        let class = declaration.clone();
        Ok(ReferenceDomainPreparationState {
            domain: Self {
                class,
                runtime_target: Some(target),
                target_key: String::new(),
                admitted: Arc::clone(&self.admitted),
                allowed: BTreeSet::new(),
            },
            key: Some(Box::new(key)),
            conformance: None,
            next: 0,
            done: false,
            failed: false,
        })
    }
    /// The admitted immutable schema and read view share the reference context.
    pub fn schema(&self) -> &intersect_model::definitions::schema::SchemaRepresentation<A> {
        self.admitted.schema.schema()
    }
    /// Checked selection for this exact immutable schema/runtime target. This is
    /// retained only after complete domain preparation, never caller evidence.
    pub(crate) fn target_key(&self) -> &str {
        &self.target_key
    }
    pub(crate) fn contains_object(&self, element: &str) -> bool {
        self.admitted
            .model
            .row_for_id(element)
            .is_some_and(|row| self.allowed.contains(&row))
    }
    pub fn read(&self) -> &impl intersect_model::core::storage::ModelRead {
        &self.admitted.model
    }
    pub fn stored_feature<'a>(
        &'a self,
        value: &Value,
        receiver: &str,
        member: &intersect_model::definitions::schema::MemberIdentity,
    ) -> Result<intersect_model::core::storage::StoredFeatureProjection<'a>, String> {
        let element = self.resolve(value)?;
        intersect_model::core::storage::StoredFeatureProjection::new(
            &self.admitted.model,
            self.schema(),
            receiver,
            member,
            &element.id,
        )
    }
    pub fn class(&self) -> &DeclarationId {
        &self.class
    }
    pub fn runtime_target(
        &self,
    ) -> Option<&intersect_model::definitions::generic_types::RuntimeType> {
        self.runtime_target.as_ref()
    }
    pub fn context(&self) -> &ReferenceContext {
        &self.admitted.context
    }
    pub fn bind(&self, element: &str) -> Result<Value, String> {
        let value = serde_json::to_value(ReferenceValue::ModelReference {
            context: self.admitted.digest.clone(),
            element: element.into(),
        })
        .map_err(|e| e.to_string())?;
        self.resolve(&value)?;
        Ok(value)
    }
    pub fn resolve(&self, value: &Value) -> Result<&Element, String> {
        let (context, element) = reference_identity(value)?;
        if context != self.admitted.digest {
            return Err("Reference belongs to another model context revision".into());
        }
        let index = self
            .admitted
            .model
            .row_for_id(element)
            .filter(|index| self.allowed.contains(index))
            .ok_or("Reference object is absent or does not conform to the selected EClass")?;
        self.admitted
            .model
            .model()
            .elements
            .get(index)
            .ok_or_else(|| "Invalid checked reference index".into())
    }
}

/// The canonical context-qualified model identity shared by resolution, equality
/// and graph traversal. This validates wire shape, not domain membership.
pub(crate) fn reference_identity(value: &Value) -> Result<(&str, &str), String> {
    let object = value
        .as_object()
        .ok_or("Reference requires an object value")?;
    if object.len() != 3 || object.get("kind").and_then(Value::as_str) != Some("model_reference") {
        return Err("Reference value requires exact explicit fields".into());
    }
    let context = object
        .get("context")
        .and_then(Value::as_str)
        .ok_or("Reference requires context identity")?;
    let element = object
        .get("element")
        .and_then(Value::as_str)
        .ok_or("Reference requires element identity")?;
    Ok((context, element))
}

/// Resumable membership selection over one immutable, already validated context.
/// Dropping this value cancels preparation without changing any existing domain.
/// A failed observation poisons preparation; restart explicitly to recover.
pub struct ReferenceDomainPreparationState<A: ReferenceSchemaFactory = String> {
    domain: ReferenceDomainState<A>,
    key: Option<Box<intersect_model::definitions::schema::ClassKeyPreparation>>,
    conformance: Option<Box<intersect_model::definitions::schema::ConformancePreparation>>,
    next: usize,
    done: bool,
    failed: bool,
}
impl<A: ReferenceSchemaFactory> ReferenceDomainPreparationState<A> {
    /// Advance one schema selection, object setup or class-conformance transition.
    pub fn advance(&mut self, observation: &InvocationContext<'_>) -> Result<bool, String> {
        if self.failed {
            return Err("Reference membership preparation has failed".into());
        }
        let result = self.advance_inner(observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(&mut self, observation: &InvocationContext<'_>) -> Result<bool, String> {
        crate::core::storage::advance_target_membership(
            crate::core::storage::TargetMembership {
                key: &mut self.key,
                conformance: &mut self.conformance,
                next: &mut self.next,
                done: &mut self.done,
                target_key: &mut self.domain.target_key,
                allowed: &mut self.domain.allowed,
            },
            self.domain.admitted.schema.schema(),
            &self.domain.admitted.model.model().elements,
            |key, observation| self.domain.admitted.schema.advance_target(key, observation),
            |_| Ok(()),
            observation,
        )
    }
    /// Publish only complete, successful membership. No partial domain escapes.
    pub fn finish(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceDomainState<A>, String> {
        if self.failed
            || !self.done
            || self.key.is_some()
            || self.conformance.is_some()
            || self.next != self.domain.admitted.model.model().elements.len()
        {
            return Err("Reference membership preparation is incomplete or failed".into());
        }
        observation.checkpoint()?;
        Ok(self.domain)
    }
}

struct AdmissionSpec<A: ReferenceSchemaFactory> {
    class: DeclarationId,
    runtime_target: Option<intersect_model::definitions::generic_types::RuntimeType>,
    context: ReferenceContext,
    schema: Option<ReferenceSchema<A>>,
    target: String,
    observed: bool,
    prepared_digest: Option<String>,
}
impl<A: ReferenceSchemaFactory> AdmissionSpec<A> {
    fn schema(
        &self,
    ) -> Result<&intersect_model::definitions::schema::SchemaRepresentation<A>, String> {
        Ok(self.schema_state()?.schema())
    }
    fn schema_state(&self) -> Result<&ReferenceSchema<A>, String> {
        self.schema
            .as_ref()
            .ok_or_else(|| "Reference schema compilation is incomplete".into())
    }
}
enum AdmissionStage<A: ReferenceSchemaFactory> {
    Compilation(Box<A::Compilation>),
    Policies(PolicyPreparation),
    Target(Box<intersect_model::definitions::schema::ClassKeyPreparation>),
    ConvertModel,
    DecodeModel(intersect_model::core::model::SnapshotPreparation),
    Index(Box<intersect_model::core::storage::IndexedModelPreparation<Model>>),
    Validate(
        intersect_model::core::storage::OwnedModelView,
        Box<intersect_model::core::storage::StructuralPreparation>,
    ),
    Membership(ReferenceDomainPreparationState<A>),
    Ready(ReferenceDomainState<A>),
}
/// Private staged admission; failed or unfinished preparation cannot expose a domain.
pub struct ReferenceAdmissionState<A: ReferenceSchemaFactory = String> {
    spec: Option<AdmissionSpec<A>>,
    stage: Option<AdmissionStage<A>>,
}
impl<A: ReferenceSchemaFactory> ReferenceAdmissionState<A> {
    #[cfg(test)]
    pub(crate) fn diagnostic_phase(&self) -> &'static str {
        match &self.stage {
            Some(AdmissionStage::Compilation(_)) => "compiler",
            Some(AdmissionStage::Policies(_)) => "policies",
            Some(AdmissionStage::Target(_)) => "target",
            Some(AdmissionStage::ConvertModel) => "convert_model",
            Some(AdmissionStage::DecodeModel(_)) => "decode_model",
            Some(AdmissionStage::Index(_)) => "index",
            Some(AdmissionStage::Validate(_, _)) => "validate_model",
            Some(AdmissionStage::Membership(_)) => "membership",
            Some(AdmissionStage::Ready(_)) => "ready",
            None => "absent",
        }
    }

    /// Advance one policy, target-selection, conversion, indexing, validation or membership transition.
    pub fn advance(
        &mut self,
        registry: &ConversionRegistry<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        let stage = self.stage.take().ok_or("Reference admission has failed")?;
        observation.checkpoint()?;
        self.stage = Some(match stage {
            AdmissionStage::Compilation(mut pending) => {
                // Registered owning inputs can group a bounded number of the
                // existing transitions. Each original transition retains its
                // observations, allocations and terminal failure behavior.
                let registered = self
                    .spec
                    .as_ref()
                    .is_some_and(|spec| spec.prepared_digest.is_some());
                let mut complete = false;
                for _ in 0..if registered { 1024 } else { 1 } {
                    if A::advance_compilation(&mut pending, registry, observation)? {
                        complete = true;
                        break;
                    }
                }
                if complete {
                    let spec = self
                        .spec
                        .as_mut()
                        .ok_or("Missing compiling reference specification")?;
                    spec.schema = Some(A::finish_compilation(
                        *pending,
                        &mut spec.context,
                        observation,
                    )?);
                    AdmissionStage::Policies(PolicyPreparation::new())
                } else {
                    AdmissionStage::Compilation(pending)
                }
            }
            AdmissionStage::Policies(mut policies) => {
                let spec = self
                    .spec
                    .as_ref()
                    .ok_or("Missing reference admission specification")?;
                observation.work(1)?;
                if policies.advance(&spec.context, spec.schema()?)? {
                    policies.finish()?;
                    let selection = spec.schema_state()?.begin_target(
                        spec.runtime_target.as_ref(),
                        &spec.class,
                        observation,
                    )?;
                    AdmissionStage::Target(Box::new(selection))
                } else {
                    AdmissionStage::Policies(policies)
                }
            }
            AdmissionStage::Target(mut selection) => {
                let spec = self
                    .spec
                    .as_mut()
                    .ok_or("Missing reference admission specification")?;
                observation.work(1)?;
                let mut complete = false;
                for _ in 0..if spec.prepared_digest.is_some() {
                    16
                } else {
                    1
                } {
                    if spec
                        .schema_state()?
                        .advance_target(&mut selection, observation)?
                    {
                        complete = true;
                        break;
                    }
                }
                if complete {
                    spec.target = selection.finish()?;
                    AdmissionStage::ConvertModel
                } else {
                    AdmissionStage::Target(selection)
                }
            }
            AdmissionStage::ConvertModel => {
                observation.work(1)?;
                AdmissionStage::DecodeModel(Default::default())
            }
            AdmissionStage::DecodeModel(mut decoding) => {
                let spec = self
                    .spec
                    .as_ref()
                    .ok_or("Missing reference admission specification")?;
                let error = |error: intersect_model::core::model::SnapshotError| match error {
                    intersect_model::core::model::SnapshotError::NonCanonical => {
                        "Reference model snapshot requires exact explicit fields".to_owned()
                    }
                    error => error.to_string(),
                };
                if decoding
                    .advance(&spec.context.model, observation)
                    .map_err(error)?
                {
                    let model = decoding.finish().map_err(error)?;
                    AdmissionStage::Index(Box::new(
                        intersect_model::core::storage::OwnedModelView::begin_observed(
                            model,
                            observation,
                        )?,
                    ))
                } else {
                    AdmissionStage::DecodeModel(decoding)
                }
            }
            AdmissionStage::Index(mut index) => {
                if index.advance(observation)? {
                    AdmissionStage::Validate(
                        index.finish(observation)?,
                        Box::new(
                            intersect_model::core::storage::StructuralPreparation::ownership(),
                        ),
                    )
                } else {
                    AdmissionStage::Index(index)
                }
            }
            AdmissionStage::Validate(model, mut validation) => {
                let spec = self
                    .spec
                    .as_ref()
                    .ok_or("Missing reference admission specification")?;
                let mut complete = false;
                for _ in 0..if spec.prepared_digest.is_some() {
                    512
                } else {
                    1
                } {
                    if spec.schema_state()?.advance_model(
                        &mut validation,
                        &model,
                        registry,
                        spec.observed.then_some(observation),
                    )? {
                        complete = true;
                        break;
                    }
                }
                if !complete {
                    self.stage = Some(AdmissionStage::Validate(model, validation));
                    return Ok(false);
                }
                let ownership = validation.finish()?;
                let spec = self
                    .spec
                    .take()
                    .ok_or("Missing reference admission specification")?;
                observation.work(1)?;
                // The owning evaluator has already streamed and checked this
                // exact context identity. Compilation transfers its source bundle
                // out and restores the exact owned inputs before reaching here.
                let digest = match spec.prepared_digest {
                    Some(digest) => digest,
                    None => spec
                        .context
                        .digest_with_work(&|units| observation.work(units))?,
                };
                AdmissionStage::Membership(ReferenceDomainPreparationState {
                    domain: ReferenceDomainState {
                        class: spec.class,
                        runtime_target: spec.runtime_target,
                        target_key: spec.target,
                        admitted: Arc::new(AdmittedContext {
                            context: spec.context,
                            digest,
                            model,
                            ownership,
                            schema: spec.schema.ok_or("Missing completed reference schema")?,
                        }),
                        allowed: BTreeSet::new(),
                    },
                    key: None,
                    conformance: None,
                    next: 0,
                    done: false,
                    failed: false,
                })
            }
            AdmissionStage::Membership(mut membership) => {
                if membership.advance(observation)? {
                    AdmissionStage::Ready(membership.finish(observation)?)
                } else {
                    AdmissionStage::Membership(membership)
                }
            }
            AdmissionStage::Ready(domain) => AdmissionStage::Ready(domain),
        });
        Ok(matches!(self.stage, Some(AdmissionStage::Ready(_))))
    }
    pub fn finish(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<ReferenceDomainState<A>, String> {
        observation.checkpoint()?;
        match self.stage {
            Some(AdmissionStage::Ready(domain)) => Ok(domain),
            _ => Err("Reference admission is incomplete or failed".into()),
        }
    }
}
