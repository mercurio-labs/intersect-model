//! Checked generic identities, lexical scope and signature shape. Substitution is E08.
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceKind, SourceSpan},
    compiler::standard_datatype_source_observed,
    ecore::{ECORE, ErrorKind, ImportError, Node, XMI, XSI},
    generics::{GenericKind, GenericType, TypeParameter, TypeReference},
    resources::{DeclarationId, ResourceSet},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[path = "generic_resolver_preparation.rs"]
mod resolver_preparation;
pub use resolver_preparation::{ResolverPreparation, ResolverPreparationPhase};
#[path = "generic_agreement.rs"]
mod agreement;
pub(crate) use agreement::AgreementPreparation;
pub use agreement::RawSignature;
#[path = "generic_reference_inputs.rs"]
mod reference_inputs;
#[path = "generic_source_inventory.rs"]
mod source_inventory;
pub(crate) use source_inventory::SourceInventoryPreparation;
#[path = "generic_inheritance.rs"]
mod inheritance;
pub use inheritance::{InheritedType, InheritedTypes};
#[path = "generic_conformance.rs"]
mod conformance;
#[path = "generic_constructor_relations.rs"]
mod constructor_relations;
#[path = "generic_scalar_relations.rs"]
mod scalar_relations;
#[path = "generic_symbolic.rs"]
mod symbolic;
pub(crate) use conformance::nominal_rules::NominalRule;
pub use conformance::{ApplicationBounds, BoundCheck};
pub(crate) use conformance::{CheckedRuntimeFeatureDomain, PreparedRuntimeMembers};
#[path = "generic_datatype_applications.rs"]
mod datatype_applications;
pub use datatype_applications::CheckedDatatypeApplication;
#[path = "generic_operation_override.rs"]
mod operation_override;
pub use operation_override::{
    GenericOperation, GenericOperationResolution, GenericOperationSet, OperationOverride,
};
#[path = "generic_declared_feature_preparation.rs"]
mod declared_feature_preparation;
pub(crate) use declared_feature_preparation::DeclaredFeaturePreparation;
#[path = "generic_members.rs"]
mod members;
pub(crate) use members::RuntimeMembersPreparation;
pub use members::{
    AppliedOperation, MemberTypes, RequestedMembers, RuntimeAppliedOperation, RuntimeFeatureDomain,
    RuntimeMemberProjection, RuntimeMembers, SpecializedFormal, SpecializedMember,
    SpecializedSignature,
};
pub use members::{MemberSourceRef, RuntimeDomainRef, RuntimeProjectionRef};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Class,
    Datatype,
    Enumeration,
    Parameter,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub usage: DeclarationId,
    pub target: DeclarationId,
    pub target_kind: TargetKind,
    pub raw: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Warning {
    pub code: String,
    pub message: String,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedType {
    pub syntax: GenericType,
    pub bindings: Vec<Binding>,
    pub warnings: Vec<Warning>,
    /// Ecore raw classifier identity, not generic substitution or conformance.
    pub raw_type: DeclarationId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedParameter {
    pub syntax: TypeParameter,
    pub bounds: Vec<ResolvedType>,
}

/// All generic signatures in an explicit resource context. Not a runtime schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureInventory {
    pub sources: Vec<SourceId>,
    pub parameters: Vec<ResolvedParameter>,
    pub types: Vec<ResolvedType>,
    pub raw_signatures: Vec<RawSignature>,
    /// Annotation/extension data preserved without generic interpretation.
    pub retained_payloads: Vec<SourceSpan>,
}

#[derive(Clone, Debug)]
pub struct Resolver {
    // Ownership policy only; package/classifier checks still execute freshly.
    borrow_owner_inputs: bool,
    prepared_reference_inputs: std::cell::RefCell<Option<Box<reference_inputs::ReferenceInputs>>>,
    // Scheduling policy only; the local checks themselves live in one predicate.
    coalesce_runtime_classifiers: std::cell::Cell<bool>,
    // Complete immutable parameter syntax/resolution/expression inputs only.
    prepared_parameter_bound_inputs: std::cell::RefCell<
        Option<std::rc::Rc<std::cell::RefCell<conformance::ParameterBoundInputTable>>>,
    >,
    prepared_runtime_inputs: std::cell::OnceCell<
        std::rc::Rc<std::cell::RefCell<crate::definitions::generic_types::RuntimeInputCache>>,
    >,
    // Checked structural metadata; never bound/conformance or provider results.
    prepared_ancestry: std::cell::OnceCell<std::rc::Rc<inheritance::StructuralAncestryCache>>,
    prepared_feature_signatures: std::cell::RefCell<
        Option<std::rc::Rc<std::cell::RefCell<members::CompiledFeatureSignatureInputs>>>,
    >,
    prepared_erased_signatures:
        std::cell::OnceCell<std::rc::Rc<std::cell::RefCell<members::ErasedSignatureCache>>>,
    // Enabled only during one materialization over this immutable source context.
    prepared_formals: std::cell::OnceCell<
        std::rc::Rc<std::collections::BTreeMap<DeclarationId, Vec<TypeParameter>>>,
    >,
    rigid_parameters: std::collections::BTreeMap<
        crate::definitions::generic_types::RigidParameterId,
        Vec<crate::definitions::generic_types::Type>,
    >,
    semantic_sources: BTreeSet<SourceId>,
    sources: Vec<SourceId>,
    resources: ResourceSet,
    standard: ResourceSet,
    standard_id: SourceId,
    consumed_constructor_relations: std::cell::RefCell<BTreeSet<DeclarationId>>,
    constructor_contracts: std::collections::BTreeMap<
        DeclarationId,
        crate::definitions::conversion::ConstructorContract,
    >,
    constructor_relations: std::collections::BTreeMap<
        (DeclarationId, DeclarationId),
        crate::definitions::conversion::ClassifierRelation,
    >,
    consumed_provider_relations: std::cell::RefCell<BTreeSet<DeclarationId>>,
    provider_relations: std::collections::BTreeMap<
        (DeclarationId, DeclarationId),
        crate::definitions::conversion::ClassifierRelation,
    >,
    // Resolved runtime operation sets shared by resolvers over one model-type
    // context's fixed inputs; consulted only without conversions or rigid
    // parameters, where a set depends on nothing but the receiver.
    runtime_operations: Option<RuntimeOperationCache>,
}
/// Resolved runtime operation sets by receiver, shared by the materialization
/// resolvers of one model-type context.
pub(crate) type RuntimeOperationCache = std::rc::Rc<
    std::cell::RefCell<
        std::collections::BTreeMap<crate::definitions::generic_types::Type, GenericOperationSet>,
    >,
>;
#[derive(Clone, Copy)]
enum Position {
    Typed,
    Super,
    Argument { immediate_super: bool },
    Bound,
}
fn error(span: &SourceSpan, message: impl Into<String>) -> ImportError {
    ImportError::new(ErrorKind::InvalidDefinition, span, message)
}
fn fallback(id: &DeclarationId) -> SourceSpan {
    SourceSpan {
        source: id.source.clone(),
        start: 0,
        end: 0,
    }
}
fn parent(id: &DeclarationId) -> Result<DeclarationId, ImportError> {
    parent_observed(
        id,
        &crate::definitions::ecore::ParserObservation::new(
            &crate::definitions::conversion::InvocationContext::default(),
        ),
    )
}
fn parent_observed(
    id: &DeclarationId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<DeclarationId, ImportError> {
    observation.work(1, &id.source)?;
    if id.path.is_empty() {
        return Err(error(&fallback(id), "Declaration requires an owner"));
    }
    observation.serialized(id, &id.source)?;
    let mut result = id.clone();
    result.path.pop();
    Ok(result)
}
// A cursor into the original immutable declaration or a legacy owned parent.
// Borrowing shortens a slice; it retains no parent identity or validation result.
struct DeclarationSelection<'a> {
    original: &'a DeclarationId,
    owned: Option<DeclarationId>,
    prefix: usize,
}
impl<'a> DeclarationSelection<'a> {
    fn new(original: &'a DeclarationId) -> Self {
        Self {
            original,
            owned: None,
            prefix: original.path.len(),
        }
    }
    fn source(&self) -> &SourceId {
        self.owned
            .as_ref()
            .map_or(&self.original.source, |id| &id.source)
    }
    fn path(&self) -> &[usize] {
        self.owned
            .as_ref()
            .map_or(&self.original.path[..self.prefix], |id| id.path.as_slice())
    }
    fn parent_observed(
        &mut self,
        borrowed: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        if borrowed {
            observation.work(1, &self.original.source)?;
            self.prefix = self
                .prefix
                .checked_sub(1)
                .ok_or_else(|| error(&fallback(self.original), "Declaration requires an owner"))?;
        } else {
            self.owned = Some(parent_observed(
                self.owned.as_ref().unwrap_or(self.original),
                observation,
            )?);
        }
        Ok(())
    }
}
fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.attributes
        .iter()
        .find(|a| a.name.namespace.is_none() && a.name.local == name)
        .map(|a| a.value.as_str())
}
fn named(node: &Node, name: &str) -> bool {
    node.name.namespace.is_none() && node.name.local == name
}
fn kind(node: &Node) -> Result<&str, ImportError> {
    kind_observed(
        node,
        &crate::definitions::ecore::ParserObservation::new(
            &crate::definitions::conversion::InvocationContext::default(),
        ),
    )
}
fn kind_observed<'a>(
    node: &'a Node,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<&'a str, ImportError> {
    let mut selected = None;
    for attribute in &node.attributes {
        observation.work(1, &node.span.source)?;
        if attribute.name.namespace.as_deref() == Some(XSI) && attribute.name.local == "type" {
            selected = Some(attribute);
            break;
        }
    }
    let a = selected.ok_or_else(|| error(&node.opening, "Missing Ecore declaration kind"))?;
    let name = a
        .type_name
        .as_ref()
        .filter(|n| n.namespace.as_deref() == Some(ECORE))
        .ok_or_else(|| error(&a.span, "Invalid Ecore declaration kind"))?;
    Ok(&name.local)
}
impl Resolver {
    fn primitive_classifier(&self, declaration: &DeclarationId) -> Result<bool, ImportError> {
        self.primitive_classifier_observed(
            declaration,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn primitive_classifier_observed(
        &self,
        declaration: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let node = self.node_observed(declaration, observation)?;
        let mut selected = None;
        for name in ["instanceClassName", "instanceTypeName"] {
            for attribute in &node.attributes {
                observation.work(1, &declaration.source)?;
                if attribute.name.namespace.is_none() && attribute.name.local == name {
                    selected = Some(attribute.value.as_str());
                    break;
                }
            }
            if selected.is_some() {
                break;
            }
        }
        Ok(selected.is_some_and(|name| {
            matches!(
                name,
                "boolean" | "byte" | "char" | "double" | "float" | "int" | "long" | "short"
            )
        }))
    }
    pub fn new(bundle: &DefinitionBundle) -> Result<Self, ImportError> {
        Self::new_context(
            bundle,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    /// Explicit policy applies to caller-supplied Ecore sources. The built-in
    /// standard datatype source retains its existing trusted default policy.
    pub fn new_with_limits(
        bundle: &DefinitionBundle,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, ImportError> {
        Self::new_context_with_limits(
            bundle,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
            limits,
        )
    }
    /// Ordinary fresh resolver over source-derived parsed inputs. Policy tables
    /// and the standard resource are rebuilt; parsed inputs alone grant no authority.
    /// Observed compiler jobs must use prepared inputs under their real context.
    pub(crate) fn with_source_resources(
        bundle: &DefinitionBundle,
        resources: ResourceSet,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, ImportError> {
        let context = crate::definitions::conversion::InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        let span = match bundle.sources.first() {
            Some(source) => observation.span(&source.id, 0, 0)?,
            None => observation.empty_span()?,
        };
        let mut preparation = observation.imported(&span.source, |context| {
            ResolverPreparation::begin(bundle, Some(resources), limits, context)
        })?;
        while !observation.imported(&span.source, |context| preparation.advance(context))? {}
        observation.imported_result(&span.source, preparation.finish())
    }

    pub fn new_observed_with_limits(
        bundle: &DefinitionBundle,
        context: &crate::definitions::conversion::InvocationContext<'_>,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, crate::definitions::ecore::ObservedImportError> {
        context
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        let observation = crate::definitions::ecore::ParserObservation::new(context);
        observation.finish(Self::new_context_with_limits(bundle, &observation, limits))
    }
    pub(crate) fn new_context(
        bundle: &DefinitionBundle,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let source = standard_datatype_source_observed(observation)?;
        Self::with_standard_source(bundle, source, observation)
    }
    pub(crate) fn new_context_with_limits(
        bundle: &DefinitionBundle,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, ImportError> {
        let source = standard_datatype_source_observed(observation)?;
        Self::with_standard_source_and_inputs_with_limits(
            bundle,
            source,
            None,
            observation,
            limits,
            false,
        )
    }
    pub(crate) fn with_standard_source(
        bundle: &DefinitionBundle,
        source: crate::definitions::bundle::DefinitionSource,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::with_standard_source_and_inputs(bundle, source, None, observation)
    }
    pub(crate) fn with_standard_source_and_inputs(
        bundle: &DefinitionBundle,
        source: crate::definitions::bundle::DefinitionSource,
        parsed: Option<ResourceSet>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::with_standard_source_and_inputs_with_limits(
            bundle,
            source,
            parsed,
            observation,
            crate::definitions::ecore::ParseLimits::default(),
            false,
        )
    }
    pub(crate) fn with_standard_source_and_borrowed_cursors(
        bundle: &DefinitionBundle,
        source: crate::definitions::bundle::DefinitionSource,
        parsed: Option<ResourceSet>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::with_standard_source_and_inputs_with_limits(
            bundle,
            source,
            parsed,
            observation,
            crate::definitions::ecore::ParseLimits::default(),
            true,
        )
    }
    fn with_standard_source_and_inputs_with_limits(
        bundle: &DefinitionBundle,
        source: crate::definitions::bundle::DefinitionSource,
        parsed: Option<ResourceSet>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        limits: crate::definitions::ecore::ParseLimits,
        borrowed_cursors: bool,
    ) -> Result<Self, ImportError> {
        let (sources, semantic_sources) =
            Self::source_identity_inputs(bundle, &source.id, observation)?;
        let resources = match parsed {
            // Legacy owner-prepared inputs carry the admission policy of their
            // owning compilation. Explicit re-admission under a requested policy
            // belongs to ResolverPreparation, not an implicit default clamp here.
            Some(resources) if resources.matches_bundle_observed(bundle, observation)? => resources,
            _ => ResourceSet::new_context_with_cursor_policy(
                bundle,
                observation,
                limits,
                borrowed_cursors,
            )?,
        };
        observation.serialized(&source.id, &source.id)?;
        let standard_id = source.id.clone();
        observation.reserve(128 + 32, &source.id)?;
        let standard = ResourceSet::new_context_with_cursor_policy(
            &DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
            observation,
            crate::definitions::ecore::ParseLimits::default(),
            borrowed_cursors,
        )?;
        Ok(Self::from_resource_inputs(
            resources,
            standard,
            standard_id,
            sources,
            semantic_sources,
        ))
    }
    fn source_identity_inputs(
        bundle: &DefinitionBundle,
        standard_id: &SourceId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(Vec<SourceId>, BTreeSet<SourceId>), ImportError> {
        observation.reserve(128, standard_id)?;
        let mut sources = Vec::new();
        for candidate in &bundle.sources {
            observation.work(1, &candidate.id)?;
            if matches!(candidate.kind, SourceKind::Ecore | SourceKind::Rcore) {
                observation.serialized(&candidate.id, &candidate.id)?;
                observation.reserve(32, &candidate.id)?;
                sources.push(candidate.id.clone());
            }
        }
        observation.work(sources.len(), standard_id)?;
        let sort_bytes = observation.size(sources.len().checked_mul(32), standard_id)?;
        observation.reserve(sort_bytes, standard_id)?;
        sources.sort();
        let mut semantic_sources = BTreeSet::new();
        for candidate in &bundle.sources {
            observation.work(1, &candidate.id)?;
            if matches!(
                candidate.kind,
                SourceKind::RcoreBehavior | SourceKind::Rcore
            ) {
                observation.serialized(&candidate.id, &candidate.id)?;
                observation.reserve(128, &candidate.id)?;
                semantic_sources.insert(candidate.id.clone());
            }
        }
        Ok((sources, semantic_sources))
    }
    // Exact immutable parsed inputs only. Neither parsing nor input reuse grants
    // generic, provider or conformance authority; all resolver policies start fresh.
    pub(crate) fn with_prepared_resource_inputs(
        bundle: &DefinitionBundle,
        standard_bundle: &DefinitionBundle,
        resources: ResourceSet,
        standard: ResourceSet,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let Some(source) = standard_bundle.sources.first() else {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &observation.empty_span()?,
                "Missing prepared standard source",
            ));
        };
        let (sources, semantic_sources) =
            Self::source_identity_inputs(bundle, &source.id, observation)?;
        if standard_bundle.sources.len() != 1
            || !standard_bundle.namespaces.is_empty()
            || !resources.matches_bundle_observed(bundle, observation)?
            || !standard.matches_bundle_observed(standard_bundle, observation)?
        {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &observation.span(&source.id, 0, 0)?,
                "Prepared resource inputs do not match their exact bundles",
            ));
        }
        observation.serialized(&source.id, &source.id)?;
        Ok(Self::from_resource_inputs(
            resources,
            standard,
            source.id.clone(),
            sources,
            semantic_sources,
        ))
    }
    fn from_resource_inputs(
        resources: ResourceSet,
        standard: ResourceSet,
        standard_id: SourceId,
        sources: Vec<SourceId>,
        semantic_sources: BTreeSet<SourceId>,
    ) -> Self {
        Self {
            borrow_owner_inputs: false,
            prepared_reference_inputs: Default::default(),
            coalesce_runtime_classifiers: std::cell::Cell::new(false),
            prepared_parameter_bound_inputs: Default::default(),
            prepared_runtime_inputs: Default::default(),
            prepared_ancestry: Default::default(),
            prepared_feature_signatures: Default::default(),
            prepared_erased_signatures: Default::default(),
            prepared_formals: Default::default(),
            rigid_parameters: Default::default(),
            semantic_sources,
            sources,
            constructor_contracts: Default::default(),
            constructor_relations: Default::default(),
            consumed_constructor_relations: Default::default(),
            provider_relations: Default::default(),
            consumed_provider_relations: Default::default(),
            runtime_operations: None,
            resources,
            standard_id,
            standard,
        }
    }
    /// Share resolved runtime operation sets through `cache`, which must only
    /// hold sets resolved over this resolver's exact sources and resources.
    pub(crate) fn with_runtime_operation_cache(mut self, cache: RuntimeOperationCache) -> Self {
        self.runtime_operations = Some(cache);
        self
    }
    pub(crate) fn with_borrowed_owner_inputs(mut self) -> Self {
        self.borrow_owner_inputs = true;
        self
    }
    pub(crate) fn enable_attempt_classifiers(&self) {
        self.coalesce_runtime_classifiers.set(true);
    }
    pub(crate) fn resource_sets(&self) -> (&ResourceSet, &ResourceSet, &SourceId) {
        (&self.resources, &self.standard, &self.standard_id)
    }
    /// Snapshot ordinary datatype relations and separately selected constructor policies.
    /// No wire-supplied relation table is accepted. Generic arguments are checked
    /// by the appropriate conformance consumer after constructor comparison.
    pub fn with_conversions(
        mut self,
        registry: &crate::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<Self, ImportError> {
        let contracts = registry.registered_contracts().map_err(|message| {
            error(
                &fallback(&DeclarationId {
                    source: self.standard_id.clone(),
                    path: vec![],
                }),
                message,
            )
        })?;
        if contracts.len() > 128 {
            return Err(error(
                &fallback(&DeclarationId {
                    source: self.standard_id.clone(),
                    path: vec![],
                }),
                "Classifier relation provider budget exceeded (128)",
            ));
        }
        let mut eligible = Vec::new();
        for contract in &contracts {
            let id = &contract.datatype;
            if self.classifier(id)? != TargetKind::Datatype {
                return Err(error(
                    &fallback(id),
                    "Classifier relation provider requires an EDataType",
                ));
            }
            let node = self.node(id)?;
            let serializable = match attr(node, "serializable") {
                None | Some("true" | "1") => true,
                Some("false" | "0") => false,
                _ => return Err(error(&node.span, "Invalid datatype serializable flag")),
            };
            if contract.serializable != serializable {
                return Err(error(
                    &node.span,
                    "Classifier relation provider serializability differs from source datatype",
                ));
            }
            if self.formals(id)?.is_empty() {
                eligible.push(id);
            }
        }
        self.prepared_runtime_inputs.take();
        self.prepared_ancestry.take();
        self.prepared_erased_signatures.take();
        self.provider_relations.clear();
        self.consumed_provider_relations.get_mut().clear();
        for a in &eligible {
            for b in &eligible {
                if a >= b {
                    continue;
                }
                if let Some(relation) = registry
                    .classifier_relation(a, b)
                    .map_err(|message| error(&fallback(a), message))?
                {
                    use crate::definitions::conversion::ClassifierRelation as R;
                    let reverse = match relation {
                        R::Equivalent => R::Equivalent,
                        R::Subtype => R::Supertype,
                        R::Supertype => R::Subtype,
                        R::Unrelated => R::Unrelated,
                    };
                    self.provider_relations
                        .insert(((*a).clone(), (*b).clone()), relation);
                    self.provider_relations
                        .insert(((*b).clone(), (*a).clone()), reverse);
                }
            }
        }
        registry.registered_contracts().map_err(|message| {
            error(
                &fallback(&DeclarationId {
                    source: self.standard_id.clone(),
                    path: vec![],
                }),
                message,
            )
        })?;
        self.with_constructors(registry.constructors())
    }

    /// Exact providers consumed by relation checks, for retaining reconstruction dependencies.
    pub fn consumed_relation_datatypes(&self) -> Vec<DeclarationId> {
        self.consumed_provider_relations
            .borrow()
            .iter()
            .cloned()
            .collect()
    }

    pub(crate) fn canonical_operation_source(
        &self,
        member: &SpecializedMember,
    ) -> Result<crate::definitions::operations::Operation, ImportError> {
        let node = self.node(&member.declaration)?;
        let document = self
            .resources(&member.declaration)
            .document(&member.declaration.source)
            .map_err(|e| error(&member.span, e))?;
        crate::definitions::compiler::canonical_operation_from_source(
            node,
            &document.source.text,
            member,
        )
    }

    pub(crate) fn pinned_operation_builtin(&self, name: &str) -> Result<DeclarationId, String> {
        if crate::definitions::scalars::Builtin::named(name).is_none() {
            return Err("Unsupported operation builtin".into());
        }
        self.standard
            .resolve(&self.standard_id, &format!("#//{name}"))
    }

    pub(crate) fn resources(&self, id: &DeclarationId) -> &ResourceSet {
        if id.source == self.standard_id {
            &self.standard
        } else {
            &self.resources
        }
    }
    fn node(&self, id: &DeclarationId) -> Result<&Node, ImportError> {
        self.node_observed(
            id,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn node_observed(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<&Node, ImportError> {
        self.resources(id).node_context(id, observation)
    }
    fn package_observed(
        &self,
        mut selected: DeclarationSelection<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        loop {
            observation.work(1, selected.source())?;
            let resources = if selected.source() == &self.standard_id {
                &self.standard
            } else {
                &self.resources
            };
            let node =
                resources.node_path_context(selected.source(), selected.path(), observation)?;
            if node.name.namespace.as_deref() == Some(ECORE) && node.name.local == "EPackage" {
                if selected.path().is_empty() {
                    return Ok(());
                }
                selected.parent_observed(self.borrow_owner_inputs, observation)?;
                let outer =
                    resources.node_path_context(selected.source(), selected.path(), observation)?;
                if selected.path().is_empty()
                    && outer.name.namespace.as_deref() == Some(XMI)
                    && outer.name.local == "XMI"
                {
                    return Ok(());
                }
            } else if named(node, "eSubpackages") {
                selected.parent_observed(self.borrow_owner_inputs, observation)?;
                continue;
            }
            return Err(error(
                &node.opening,
                "Classifier requires an actual EPackage owner",
            ));
        }
    }
    fn classifier(&self, id: &DeclarationId) -> Result<TargetKind, ImportError> {
        self.classifier_observed(
            id,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn classifier_observed(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<TargetKind, ImportError> {
        observation.work(1, &id.source)?;
        let node = self.node_observed(id, observation)?;
        self.classifier_node_observed(id, node, observation)
    }
    // The node is selected from this exact immutable resolver by the caller.
    fn classifier_node_observed(
        &self,
        id: &DeclarationId,
        node: &Node,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<TargetKind, ImportError> {
        if !named(node, "eClassifiers") {
            return Err(error(
                &node.opening,
                "Reference does not select a classifier declaration",
            ));
        }
        let mut owner = DeclarationSelection::new(id);
        owner.parent_observed(self.borrow_owner_inputs, observation)?;
        self.package_observed(owner, observation)?;
        match kind_observed(node, observation)? {
            "EClass" => Ok(TargetKind::Class),
            "EDataType" => Ok(TargetKind::Datatype),
            "EEnum" => Ok(TargetKind::Enumeration),
            _ => Err(error(&node.opening, "Unsupported classifier kind")),
        }
    }
    fn operation(&self, id: &DeclarationId) -> Result<(), ImportError> {
        self.operation_observed(
            id,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn operation_observed(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &id.source)?;
        let node = self.node_observed(id, observation)?;
        if !named(node, "eOperations")
            || self.classifier_observed(&parent_observed(id, observation)?, observation)?
                != TargetKind::Class
        {
            return Err(error(&node.opening, "Operation requires an EClass owner"));
        }
        Ok(())
    }
    pub(crate) fn parameter_owner_observed(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        observation.work(1, &id.source)?;
        let node = self.node_observed(id, observation)?;
        if !named(node, "eTypeParameters") {
            return Err(error(
                &node.opening,
                "Reference does not select a type parameter",
            ));
        }
        let owner = parent_observed(id, observation)?;
        observation.work(1, &id.source)?;
        self.parameter_owner_role_observed(&owner, observation)?;
        Ok(owner)
    }
    fn parameter_owner_role_observed(
        &self,
        owner: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        if named(self.node_observed(owner, observation)?, "eOperations") {
            self.operation_observed(owner, observation)?;
        } else {
            self.classifier_observed(owner, observation)?;
        }
        Ok(())
    }
    pub(super) fn parameter_owner_input_observed(
        &self,
        id: &DeclarationId,
        owner: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &id.source)?;
        let node = self.node_observed(id, observation)?;
        if !named(node, "eTypeParameters") {
            return Err(error(
                &node.opening,
                "Reference does not select a type parameter",
            ));
        }
        observation.work(id.path.len().saturating_add(1), &id.source)?;
        if owner.source != id.source
            || id.path.split_last().map(|(_, path)| path) != Some(owner.path.as_slice())
        {
            return Err(error(
                &node.opening,
                "Type parameter owner differs from exact source identity",
            ));
        }
        observation.work(1, &id.source)?;
        self.parameter_owner_role_observed(owner, observation)
    }
    fn formals(&self, owner: &DeclarationId) -> Result<Vec<TypeParameter>, ImportError> {
        self.formals_observed(
            owner,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(super) fn formal_info_observed<'a>(
        &'a self,
        owner: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<std::borrow::Cow<'a, [TypeParameter]>, ImportError> {
        if let Some(formals) = self
            .prepared_formals
            .get()
            .and_then(|prepared| prepared.get(owner))
        {
            observation.work(1, &owner.source)?;
            return Ok(std::borrow::Cow::Borrowed(formals));
        }
        self.formals_observed(owner, observation)
            .map(std::borrow::Cow::Owned)
    }

    // Construct the syntax cache only from a fresh checked inventory. Caller
    // supplied inventory rows cannot grant source authority. Its ownership is
    // charged before copying; later reads borrow the immutable syntax.
    pub(crate) fn resolve_materialization_inventory_observed(
        &mut self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<SignatureInventory, ImportError> {
        let inventory = self.resolve_all()?;
        self.prepared_runtime_inputs.take();
        self.prepared_ancestry.take();
        self.prepared_erased_signatures.take();
        self.prepared_formals.take();
        self.install_materialization_inventory(&inventory, observation)?;
        Ok(inventory)
    }
    pub(crate) fn install_materialization_inventory(
        &self,
        inventory: &SignatureInventory,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        if self.prepared_formals.get().is_some()
            || self.prepared_ancestry.get().is_some()
            || self.prepared_erased_signatures.get().is_some()
            || self.prepared_runtime_inputs.get().is_some()
        {
            return Err(error(
                &fallback(&DeclarationId {
                    source: self.standard_id.clone(),
                    path: Vec::new(),
                }),
                "Materialization inventory is already installed",
            ));
        }
        observation.reserve(128, &self.standard_id)?;
        let mut prepared: std::collections::BTreeMap<DeclarationId, Vec<TypeParameter>> =
            std::collections::BTreeMap::new();
        for parameter in &inventory.parameters {
            let syntax = &parameter.syntax;
            observation.work(1, &syntax.declaration.source)?;
            let owner = parent_observed(&syntax.declaration, observation)?;
            if !prepared.contains_key(&owner) {
                observation.reserve(128, &owner.source)?;
            }
            observation.serialized(syntax, &syntax.declaration.source)?;
            observation.reserve(32, &syntax.declaration.source)?;
            prepared.entry(owner).or_default().push(syntax.clone());
        }
        // Finish every reservation before publishing any prepared input family.
        observation.reserve(128 + 32, &self.standard_id)?;
        observation.reserve(128 + 32, &self.standard_id)?;
        observation.reserve(128 + 32, &self.standard_id)?;
        let duplicate = || {
            error(
                &fallback(&DeclarationId {
                    source: self.standard_id.clone(),
                    path: Vec::new(),
                }),
                "Materialization inputs are already installed",
            )
        };
        self.prepared_formals
            .set(std::rc::Rc::new(prepared))
            .map_err(|_| duplicate())?;
        self.prepared_ancestry
            .set(std::rc::Rc::new(
                std::cell::RefCell::new(Default::default()),
            ))
            .map_err(|_| duplicate())?;
        self.prepared_erased_signatures
            .set(std::rc::Rc::new(
                std::cell::RefCell::new(Default::default()),
            ))
            .map_err(|_| duplicate())?;
        self.prepared_runtime_inputs
            .set(std::rc::Rc::new(
                std::cell::RefCell::new(Default::default()),
            ))
            .map_err(|_| duplicate())?;
        Ok(())
    }

    pub(crate) fn runtime_input_prepared_observed(
        &self,
        expression: std::borrow::Cow<'_, crate::definitions::generic_types::Type>,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<
        crate::definitions::generic_types::RuntimeType,
        crate::definitions::schema::PreparationError,
    > {
        match self.prepared_runtime_inputs.get() {
            Some(cache) => {
                let inputs = self
                    .prepared_parameter_bound_inputs
                    .try_borrow()
                    .map_err(|_| {
                        crate::definitions::schema::PreparationError::Schema(
                            "Runtime input ownership context is already borrowed".into(),
                        )
                    })?;
                let index = match inputs.as_ref() {
                    Some(inputs) => Some(
                        inputs
                            .try_borrow()
                            .map_err(|_| {
                                crate::definitions::schema::PreparationError::Schema(
                                    "Runtime input ownership metadata is already borrowed".into(),
                                )
                            })?
                            .runtime_types
                            .clone(),
                    ),
                    None => None,
                };
                drop(inputs);
                match index {
                    Some(index) => {
                        crate::definitions::generic_types::RuntimeType::from_input_indexed_observed(
                            expression, cache, &index, context,
                        )
                    }
                    None => crate::definitions::generic_types::RuntimeType::from_input_observed(
                        expression, cache, context,
                    ),
                }
            }
            None => {
                if matches!(&expression, std::borrow::Cow::Borrowed(_)) {
                    crate::definitions::schema::reserve_preparation_value(
                        expression.as_ref(),
                        context,
                    )?;
                }
                crate::definitions::generic_types::RuntimeType::new_observed(
                    expression.into_owned(),
                    context,
                )
            }
        }
    }
    pub(crate) fn runtime_input_observed(
        &self,
        expression: std::borrow::Cow<'_, crate::definitions::generic_types::Type>,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<crate::definitions::generic_types::RuntimeType, ImportError> {
        let expression = match (self.prepared_runtime_inputs.get(), expression) {
            (None, std::borrow::Cow::Borrowed(value)) => {
                // Preserve the original public parser copy/work policy.
                observation.serialized(value, &span.source)?;
                std::borrow::Cow::Owned(value.clone())
            }
            (_, expression) => expression,
        };
        observation.preparation_at(span, |context| {
            self.runtime_input_prepared_observed(expression, context)
        })
    }

    pub(crate) fn formals_observed(
        &self,
        owner: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<TypeParameter>, ImportError> {
        let mut names = BTreeSet::new();
        let mut result = Vec::new();
        for (index, node) in self
            .node_observed(owner, observation)?
            .children
            .iter()
            .enumerate()
        {
            observation.work(1, &owner.source)?;
            if !named(node, "eTypeParameters") {
                continue;
            }
            observation.serialized(owner, &owner.source)?;
            observation.reserve(32, &owner.source)?;
            let mut id = owner.clone();
            id.path.push(index);
            self.parameter_owner_observed(&id, observation)?;
            let parameter = crate::definitions::generics::parse_parameter_observed(
                self.resources(&id),
                &id,
                observation,
            )?;
            observation.work(1, &owner.source)?;
            if names.contains(&parameter.name) {
                return Err(error(
                    &parameter.span,
                    "Duplicate type parameter name in the same owner",
                ));
            }
            observation.string(parameter.name.len(), &owner.source)?;
            observation.reserve(128, &owner.source)?;
            names.insert(parameter.name.clone());
            observation.reserve(32, &owner.source)?;
            result.push(parameter);
        }
        Ok(result)
    }
    #[cfg(test)]
    fn reference(
        &self,
        usage: &DeclarationId,
        reference: &TypeReference,
    ) -> Result<DeclarationId, ImportError> {
        self.reference_observed(
            usage,
            reference,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn reference_observed(
        &self,
        usage: &DeclarationId,
        reference: &TypeReference,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        observation.work(1, &usage.source)?;
        let standard_prefix = "http://www.eclipse.org/emf/2002/Ecore#";
        let result = if reference.href.starts_with(standard_prefix) {
            // Borrow the existing #fragment rather than allocating a second href.
            self.reference_input_context(
                true,
                &self.standard_id,
                &reference.href[standard_prefix.len() - 1..],
                &reference.span,
                observation,
            )
        } else {
            self.reference_input_context(
                usage.source == self.standard_id,
                &usage.source,
                &reference.href,
                &reference.span,
                observation,
            )
        };
        // Keep semantic diagnostics at the retained reference span. Observer
        // failures remain typed in ParserObservation's separate failure channel.
        result.map_err(|failure| error(&reference.span, failure.message))
    }
    #[cfg(test)]
    fn position(&self, id: &DeclarationId) -> Result<Position, ImportError> {
        self.position_observed(
            id,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn position_observed(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Position, ImportError> {
        observation.work(1, &id.source)?;
        let node = self.node_observed(id, observation)?;
        let owner = parent_observed(id, observation)?;
        let container = self.node_observed(&owner, observation)?;
        match node.name.local.as_str() {
            "eGenericType" => {
                let mut generic_types = 0;
                for child in &container.children {
                    observation.work(1, &id.source)?;
                    generic_types += usize::from(named(child, "eGenericType"));
                }
                if generic_types != 1 {
                    return Err(error(
                        &node.opening,
                        "Typed element has duplicate generic types",
                    ));
                }
                if named(container, "eStructuralFeatures") {
                    if !matches!(
                        kind_observed(container, observation)?,
                        "EAttribute" | "EReference"
                    ) || self
                        .classifier_observed(&parent_observed(&owner, observation)?, observation)?
                        != TargetKind::Class
                    {
                        return Err(error(&container.opening, "Invalid generic feature owner"));
                    }
                } else if named(container, "eOperations") {
                    self.operation_observed(&owner, observation)?;
                } else if named(container, "eParameters") {
                    self.operation_observed(&parent_observed(&owner, observation)?, observation)?;
                } else {
                    return Err(error(
                        &container.opening,
                        "Generic type requires a typed element owner",
                    ));
                }
                Ok(Position::Typed)
            }
            "eGenericSuperTypes" => {
                if self.classifier_observed(&owner, observation)? != TargetKind::Class {
                    return Err(error(
                        &node.opening,
                        "Generic supertype requires an EClass owner",
                    ));
                }
                Ok(Position::Super)
            }
            "eGenericExceptions" => {
                self.operation_observed(&owner, observation)?;
                Ok(Position::Bound)
            }
            "eBounds" => {
                self.parameter_owner_observed(&owner, observation)?;
                Ok(Position::Bound)
            }
            "eTypeArguments" | "eUpperBound" | "eLowerBound" => {
                self.position_observed(&owner, observation)?;
                if node.name.local == "eTypeArguments" {
                    Ok(Position::Argument {
                        immediate_super: named(container, "eGenericSuperTypes"),
                    })
                } else {
                    Ok(Position::Bound)
                }
            }
            _ => Err(error(&node.opening, "Unknown generic position")),
        }
    }
    fn walk_observed(
        &self,
        syntax: &GenericType,
        bindings: &mut Vec<Binding>,
        warnings: &mut Vec<Warning>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &syntax.span.source)?;
        let position = self.position_observed(&syntax.declaration, observation)?;
        match &syntax.value {
            GenericKind::Classifier {
                reference,
                arguments,
            } => {
                let target =
                    self.reference_observed(&syntax.declaration, reference, observation)?;
                let target_kind = self.classifier_observed(&target, observation)?;
                let hint_agrees = match reference.kind_hint.as_deref() {
                    None | Some("EClassifier") => true,
                    Some("EClass") => target_kind == TargetKind::Class,
                    Some("EDataType") => {
                        matches!(target_kind, TargetKind::Datatype | TargetKind::Enumeration)
                    }
                    Some("EEnum") => target_kind == TargetKind::Enumeration,
                    _ => false,
                };
                if !hint_agrees {
                    return Err(error(
                        &reference.span,
                        "Generic kind hint disagrees with resolved classifier",
                    ));
                }
                if matches!(position, Position::Super) && target_kind != TargetKind::Class {
                    return Err(error(
                        &reference.span,
                        "Generic supertype must select an EClass",
                    ));
                }
                if matches!(position, Position::Typed) {
                    let owner = parent_observed(&syntax.declaration, observation)?;
                    let container = self.node_observed(&owner, observation)?;
                    if named(container, "eStructuralFeatures") {
                        let reference_feature =
                            kind_observed(container, observation)? == "EReference";
                        if reference_feature != (target_kind == TargetKind::Class) {
                            return Err(error(
                                &reference.span,
                                "Generic feature classifier disagrees with attribute/reference kind",
                            ));
                        }
                    }
                } else if self.primitive_classifier_observed(&target, observation)? {
                    return Err(error(
                        &reference.span,
                        "Primitive classifier is only allowed as a typed-element generic type",
                    ));
                }
                let formals = self.formals_observed(&target, observation)?;
                if !arguments.is_empty() && arguments.len() != formals.len() {
                    return Err(error(
                        &syntax.span,
                        format!(
                            "Generic arity mismatch: expected {}, found {}",
                            formals.len(),
                            arguments.len()
                        ),
                    ));
                }
                let raw = arguments.is_empty() && !formals.is_empty();
                if raw {
                    observation.string(8, &syntax.span.source)?;
                    let message = observation.formatted(
                        &format_args!("Raw classifier omits {} type arguments", formals.len()),
                        &syntax.span.source,
                    )?;
                    observation.serialized(&syntax.span, &syntax.span.source)?;
                    observation.reserve(128 + 32, &syntax.span.source)?;
                    warnings.push(Warning {
                        code: "raw_type".into(),
                        message,
                        span: syntax.span.clone(),
                    });
                }
                observation.serialized(&syntax.declaration, &syntax.span.source)?;
                observation.reserve(128 + 32, &syntax.span.source)?;
                bindings.push(Binding {
                    usage: syntax.declaration.clone(),
                    target,
                    target_kind,
                    raw,
                });
                for argument in arguments {
                    self.walk_observed(argument, bindings, warnings, observation)?;
                }
            }
            GenericKind::Parameter { reference } => {
                if matches!(position, Position::Super) {
                    return Err(error(
                        &reference.span,
                        "Generic supertype must select an EClass",
                    ));
                }
                let target =
                    self.reference_observed(&syntax.declaration, reference, observation)?;
                let owner = self.parameter_owner_observed(&target, observation)?;
                self.formals_observed(&owner, observation)?;
                if syntax.declaration.source != owner.source
                    || !syntax.declaration.path.starts_with(&owner.path)
                {
                    return Err(error(
                        &reference.span,
                        "Type parameter is outside its lexical owner scope",
                    ));
                }
                if named(
                    self.node_observed(&syntax.declaration, observation)?,
                    "eBounds",
                ) {
                    let bound_parameter = parent_observed(&syntax.declaration, observation)?;
                    if parent_observed(&bound_parameter, observation)? == owner
                        && target.path.last() >= bound_parameter.path.last()
                    {
                        return Err(error(
                            &reference.span,
                            "Direct parameter bound cannot reference itself or a later parameter",
                        ));
                    }
                }
                observation.serialized(&syntax.declaration, &syntax.span.source)?;
                observation.reserve(128 + 32, &syntax.span.source)?;
                bindings.push(Binding {
                    usage: syntax.declaration.clone(),
                    target,
                    target_kind: TargetKind::Parameter,
                    raw: false,
                });
            }
            GenericKind::Wildcard { upper, lower } => {
                if !matches!(
                    position,
                    Position::Argument {
                        immediate_super: false
                    }
                ) {
                    return Err(error(
                        &syntax.span,
                        "Wildcard is not permitted in this generic position",
                    ));
                }
                for bound in [upper, lower].into_iter().flatten() {
                    self.walk_observed(bound, bindings, warnings, observation)?;
                }
            }
        }
        Ok(())
    }
    /// Check every generic declaration, including signatures not referenced elsewhere.
    /// Other Ecore semantics and opaque annotation/extension payloads are not certified.
    pub fn resolve_all(&self) -> Result<SignatureInventory, ImportError> {
        self.resolve_all_context(&crate::definitions::ecore::ParserObservation::new(
            &crate::definitions::conversion::InvocationContext::default(),
        ))
    }
    /// Observe the same complete source inventory, including legacy/generic agreement.
    /// Drives the retained source walker to completion; per-declaration checks remain synchronous.
    pub fn resolve_all_observed(
        &self,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<SignatureInventory, crate::definitions::ecore::ObservedImportError> {
        context
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        let observation = crate::definitions::ecore::ParserObservation::new(context);
        observation.finish(self.resolve_all_context(&observation))
    }
    fn resolve_all_context(
        &self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<SignatureInventory, ImportError> {
        let mut preparation = SourceInventoryPreparation::new(self, observation)?;
        while !preparation.advance(observation)? {}
        preparation.finish()
    }
    /// Add only the exact pinned EObject source subtree needed by its runtime metadata.
    /// Existing inventory order is preserved; collecting an already included source is idempotent.
    pub(crate) fn extend_pinned_eobject_inventory(
        &self,
        inventory: &mut SignatureInventory,
    ) -> Result<(), ImportError> {
        let declaration =
            crate::definitions::schema::eobject_reference_top_declaration().map_err(|message| {
                error(
                    &fallback(&DeclarationId {
                        source: self.standard_id.clone(),
                        path: vec![],
                    }),
                    message,
                )
            })?;
        if declaration.source != self.standard_id {
            return Err(error(
                &fallback(&declaration),
                "EObject inventory source differs from pinned standard context",
            ));
        }
        let mut added = SignatureInventory {
            sources: vec![self.standard_id.clone()],
            parameters: vec![],
            types: vec![],
            raw_signatures: vec![],
            retained_payloads: vec![],
        };
        self.collect(&declaration, &mut added)?;
        fn append_checked<T: PartialEq>(
            target: &mut Vec<T>,
            added: Vec<T>,
            same_identity: impl Fn(&T, &T) -> bool,
        ) -> Result<(), String> {
            for value in added {
                if let Some(existing) = target
                    .iter()
                    .find(|existing| same_identity(existing, &value))
                {
                    if existing != &value {
                        return Err(
                            "Pinned EObject inventory conflicts with retained source entry".into(),
                        );
                    }
                } else {
                    target.push(value);
                }
            }
            Ok(())
        }
        // Do not partially extend an inventory if a retained identity conflicts.
        let mut merged = inventory.clone();
        append_checked(&mut merged.parameters, added.parameters, |a, b| {
            a.syntax.declaration == b.syntax.declaration
        })
        .map_err(|message| error(&fallback(&declaration), message))?;
        append_checked(&mut merged.types, added.types, |a, b| {
            a.syntax.declaration == b.syntax.declaration
        })
        .map_err(|message| error(&fallback(&declaration), message))?;
        append_checked(&mut merged.raw_signatures, added.raw_signatures, |a, b| {
            a.owner == b.owner && a.feature == b.feature
        })
        .map_err(|message| error(&fallback(&declaration), message))?;
        for payload in added.retained_payloads {
            if !merged.retained_payloads.contains(&payload) {
                merged.retained_payloads.push(payload);
            }
        }
        if !merged.sources.contains(&self.standard_id) {
            merged.sources.push(self.standard_id.clone());
            merged.sources.sort();
        }
        *inventory = merged;
        Ok(())
    }

    fn collect(
        &self,
        id: &DeclarationId,
        inventory: &mut SignatureInventory,
    ) -> Result<(), ImportError> {
        let context = crate::definitions::conversion::InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        let mut preparation = SourceInventoryPreparation::subtree(self, id, &observation)?;
        while !preparation.advance(&observation)? {}
        let mut added = preparation.finish()?;
        inventory.parameters.append(&mut added.parameters);
        inventory.types.append(&mut added.types);
        inventory.raw_signatures.append(&mut added.raw_signatures);
        inventory
            .retained_payloads
            .append(&mut added.retained_payloads);
        Ok(())
    }
    fn collect_node_prefix_context(
        &self,
        id: &DeclarationId,
        inventory: &mut SignatureInventory,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<&Node>, ImportError> {
        observation.work(1, &id.source)?;
        let node = self.node_observed(id, observation)?;
        let resource_root = id.path.is_empty();
        let package =
            node.name.namespace.as_deref() == Some(ECORE) && node.name.local == "EPackage";
        if named(node, "eAnnotations")
            || (node.name.namespace.is_some() && !resource_root && !package)
        {
            let span = observation.span(&node.span.source, node.span.start, node.span.end)?;
            observation.reserve(32, &id.source)?;
            inventory.retained_payloads.push(span);
            return Ok(None);
        }
        if named(node, "eClassifiers") || named(node, "eOperations") {
            self.formals_observed(id, observation)?;
        }
        Ok(Some(node))
    }
    fn collect_node_suffix_context(
        &self,
        id: &DeclarationId,
        node: &Node,
        inventory: &mut SignatureInventory,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<usize, ImportError> {
        if named(node, "eTypeParameters") {
            let parameter = self.resolve_parameter_context(id, observation)?;
            for child in &node.children {
                observation.work(1, &id.source)?;
                if named(child, "eAnnotations") {
                    let span =
                        observation.span(&child.span.source, child.span.start, child.span.end)?;
                    observation.reserve(32, &id.source)?;
                    inventory.retained_payloads.push(span);
                }
            }
            observation.reserve(32, &id.source)?;
            inventory.parameters.push(parameter);
            return Ok(0);
        }
        if crate::definitions::generics::type_position(node) {
            let resolved = self.resolve_type_context(id, observation)?;
            observation.reserve(32, &id.source)?;
            inventory.types.push(resolved);
            return Ok(0);
        }
        Ok(node.children.len())
    }

    /// Adapter projection uses the same signature and legacy agreement checks as schemas.
    pub(crate) fn concrete_feature_target(
        &self,
        id: &DeclarationId,
    ) -> Result<DeclarationId, ImportError> {
        let node = self.node(id)?;
        let index = node
            .children
            .iter()
            .position(|child| named(child, "eGenericType"))
            .ok_or_else(|| error(&node.opening, "Missing generic feature type"))?;
        let mut selected = id.clone();
        selected.path.push(index);
        let checked = self.resolve_type(&selected)?;
        self.check_legacy(id)?;
        if !matches!(&checked.syntax.value, GenericKind::Classifier {arguments,..} if arguments.is_empty())
        {
            return Err(ImportError::new(
                ErrorKind::Unsupported,
                &checked.syntax.span,
                "Generic feature arguments require E08 substitution/conformance",
            ));
        }
        Ok(checked.raw_type)
    }

    /// Reparse retained source before checking; deserialized ASTs are not trusted as proof.
    pub fn resolve_type(&self, id: &DeclarationId) -> Result<ResolvedType, ImportError> {
        self.resolve_type_context(
            id,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    /// Observe the shared source checker and its retained results. This guards
    /// synchronous work; it is not a host-yield continuation.
    pub fn resolve_type_observed(
        &self,
        id: &DeclarationId,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<ResolvedType, crate::definitions::ecore::ObservedImportError> {
        context
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        let observation = crate::definitions::ecore::ParserObservation::new(context);
        observation.finish(self.resolve_type_context(id, &observation))
    }
    pub(crate) fn resolve_type_context(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<ResolvedType, ImportError> {
        let syntax =
            crate::definitions::generics::parse_type_observed(self.resources(id), id, observation)?;
        self.finish_type_syntax_context(id, syntax, observation)
    }
    // The source decoder hands its sole fresh AST to the SAME checked walk and
    // erasure. These individual algorithms remain explicit synchronous tails.
    pub(crate) fn finish_type_syntax_context(
        &self,
        id: &DeclarationId,
        syntax: crate::definitions::generics::GenericType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<ResolvedType, ImportError> {
        let mut bindings = Vec::new();
        let mut warnings = Vec::new();
        self.walk_observed(&syntax, &mut bindings, &mut warnings, observation)?;
        let raw_type = self.erasure_observed(&syntax, &mut BTreeSet::new(), observation)?;
        observation.reserve(128, &id.source)?;
        Ok(ResolvedType {
            raw_type,
            syntax,
            bindings,
            warnings,
        })
    }
    pub fn resolve_parameter(&self, id: &DeclarationId) -> Result<ResolvedParameter, ImportError> {
        self.resolve_parameter_context(
            id,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    /// Observe the shared parameter owner, formal list and bound source checker.
    pub fn resolve_parameter_observed(
        &self,
        id: &DeclarationId,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<ResolvedParameter, crate::definitions::ecore::ObservedImportError> {
        context
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        let observation = crate::definitions::ecore::ParserObservation::new(context);
        observation.finish(self.resolve_parameter_context(id, &observation))
    }
    pub(crate) fn resolve_parameter_context(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<ResolvedParameter, ImportError> {
        let owner = self.parameter_owner_observed(id, observation)?;
        self.formals_observed(&owner, observation)?;
        let syntax = crate::definitions::generics::parse_parameter_observed(
            self.resources(id),
            id,
            observation,
        )?;
        let mut bounds = Vec::new();
        for bound in &syntax.bounds {
            observation.work(1, &id.source)?;
            let resolved = self.resolve_type_context(&bound.declaration, observation)?;
            observation.reserve(32, &id.source)?;
            bounds.push(resolved);
        }
        observation.reserve(128, &id.source)?;
        Ok(ResolvedParameter { syntax, bounds })
    }
}

#[cfg(test)]
mod source_scope_controls {
    use super::*;
    use crate::definitions::{
        bundle::DefinitionSource,
        compiler::xml_projection_controls::assert_lowering_receipt,
        conversion::InvocationContext,
        ecore::{Document, ObservedImportError, ParserObservation},
    };

    fn fixture(body: &str) -> (DefinitionSource, Document, Resolver) {
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{body}</e:EPackage>"
            ),
        );
        let document = Document::parse(source.clone()).unwrap();
        let resolver = Resolver::new(&DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        })
        .unwrap();
        (source, document, resolver)
    }

    #[test]
    fn observed_reference_resolution_keeps_exact_target_and_reference_span() {
        let (source, document, resolver) = fixture(
            "<eClassifiers x:type='e:EClass' name='Owner'><eGenericSuperTypes eClassifier='#//@eClassifiers.0'/></eClassifiers>",
        );
        let usage = DeclarationId {
            source: source.id.clone(),
            path: vec![0, 0],
        };
        let span = document.root.children[0].children[0].attributes[0]
            .span
            .clone();
        let reference = TypeReference {
            href: "#//@eClassifiers.0".into(),
            kind_hint: None,
            span: span.clone(),
        };
        let expected = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        // Shared resource resolution2091/28 plus one reference-selection visit.
        assert_lowering_receipt(&expected, 2091, 29, &[727, 1980, 2059, 2091], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(resolver.reference_observed(&usage, &reference, &observation))
        });
        assert_eq!(resolver.reference(&usage, &reference).unwrap(), expected);
        let missing = TypeReference {
            href: "#//@eClassifiers.99".into(),
            ..reference
        };
        let expected = error(&span, "Missing structural fragment @eClassifiers.99");
        assert_lowering_receipt(&expected, 2060, 29, &[727, 1980, 2060], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            match observation.finish(resolver.reference_observed(&usage, &missing, &observation)) {
                Err(ObservedImportError::Import(error)) => Ok(error),
                Err(error) => Err(error),
                Ok(_) => panic!("missing ordinal was accepted"),
            }
        });
        assert_eq!(resolver.reference(&usage, &missing).unwrap_err(), expected);
    }

    #[test]
    fn observed_generic_position_preserves_super_owner_and_duplicate_type_errors() {
        for kind in ["EClass", "EDataType"] {
            let (source, document, resolver) = fixture(&format!(
                "<eClassifiers x:type='e:{kind}' name='Owner'><eGenericSuperTypes eClassifier='#//Owner'/></eClassifiers>"
            ));
            let selected = DeclarationId {
                source: source.id.clone(),
                path: vec![0, 0],
            };
            let expected = if kind == "EClass" {
                Ok(true)
            } else {
                Err(error(
                    &document.root.children[0].children[0].opening,
                    "Generic supertype requires an EClass owner",
                ))
            };
            // Node3 + entry1 + parent1573/18 and its visit1 + owner node2 + classifier1413/23.
            assert_lowering_receipt(&expected, 2986, 48, &[1573, 2986], |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(resolver.position_observed(&selected, &observation)) {
                    Ok(position) => Ok(Ok(matches!(position, Position::Super))),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error)),
                    Err(error) => Err(error),
                }
            });
        }
        let (source, document, resolver) = fixture(
            "<eClassifiers x:type='e:EClass' name='Owner'><eStructuralFeatures x:type='e:EReference' name='value'><eGenericType eClassifier='#//Owner'/><eGenericType eClassifier='#//Owner'/></eStructuralFeatures></eClassifiers>",
        );
        let selected = DeclarationId {
            source: source.id,
            path: vec![0, 0, 0],
        };
        let expected = error(
            &document.root.children[0].children[0].children[0].opening,
            "Typed element has duplicate generic types",
        );
        // Entry1 + node4 + parent1733/19 and its visit1 + owner node3 + two counted children.
        assert_lowering_receipt(&expected, 1733, 30, &[1733], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            match observation.finish(resolver.position_observed(&selected, &observation)) {
                Err(ObservedImportError::Import(error)) => Ok(error),
                Err(error) => Err(error),
                Ok(_) => panic!("duplicate generic type was accepted"),
            }
        });
        assert_eq!(resolver.position(&selected).err().unwrap(), expected);
    }

    #[test]
    fn observed_complete_source_type_resolution_preserves_bindings_and_raw_warnings() {
        let payload = "<eGenericSuperTypes eClassifier='#//@eClassifiers.0'/>";
        for raw in [false, true] {
            let parameter = if raw {
                "<eTypeParameters name='T'/>"
            } else {
                ""
            };
            let (source, document, resolver) = fixture(&format!(
                "<eClassifiers x:type='e:EClass' name='Owner'>{parameter}{payload}</eClassifiers>"
            ));
            let child = usize::from(raw);
            let usage = DeclarationId {
                source: source.id.clone(),
                path: vec![0, child],
            };
            let target = DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            };
            let node = &document.root.children[0].children[child];
            let warning_message = "Raw classifier omits 1 type arguments";
            let expected = ResolvedType {
                syntax: GenericType {
                    declaration: usage.clone(),
                    span: node.span.clone(),
                    source_payload: payload.into(),
                    value: GenericKind::Classifier {
                        reference: TypeReference {
                            href: "#//@eClassifiers.0".into(),
                            kind_hint: None,
                            span: node.attributes[0].span.clone(),
                        },
                        arguments: vec![],
                    },
                },
                bindings: vec![Binding {
                    usage: usage.clone(),
                    target: target.clone(),
                    target_kind: TargetKind::Class,
                    raw,
                }],
                warnings: if raw {
                    vec![Warning {
                        code: "raw_type".into(),
                        message: warning_message.into(),
                        span: node.span.clone(),
                    }]
                } else {
                    vec![]
                },
                raw_type: target,
            };
            // Parser5253+payload bytes/63; position2986/48, reference2091/29,
            // classifier1413/23, borrowed primitive6, empty formals3 or
            // source formal8088/114 including the super child. Binding1733/18,
            // erasure3792/48 and resolved record128. Raw warning adds
            // code72, formatted64+message bytes/two work, span1575/18 and160.
            let parser_bytes = 5253 + payload.len();
            let (bytes, work, prefixes) = if raw {
                (
                    27355 + payload.len() + warning_message.len(),
                    370,
                    vec![
                        parser_bytes,
                        parser_bytes + 2986,
                        parser_bytes + 6490,
                        parser_bytes + 14578,
                        parser_bytes + 18182 + warning_message.len(),
                        27227 + payload.len() + warning_message.len(),
                        27355 + payload.len() + warning_message.len(),
                    ],
                )
            } else {
                (
                    17396 + payload.len(),
                    239,
                    vec![
                        parser_bytes,
                        parser_bytes + 2986,
                        parser_bytes + 6490,
                        parser_bytes + 8223,
                        17268 + payload.len(),
                        17396 + payload.len(),
                    ],
                )
            };
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                resolver.resolve_type_observed(&usage, &InvocationContext::new(probe.clone()))
            });
            assert_eq!(resolver.resolve_type(&usage).unwrap(), expected);
        }
    }

    #[test]
    fn observed_complete_source_failures_preserve_semantic_spans_and_observer_precedence() {
        let payload = "<eGenericSuperTypes eClassifier='#//@eClassifiers.0'/>";
        let (source, document, resolver) = fixture(&format!(
            "<eClassifiers x:type='e:EDataType' name='Owner'>{payload}</eClassifiers>"
        ));
        let selected = DeclarationId {
            source: source.id,
            path: vec![0, 0],
        };
        let expected = error(
            &document.root.children[0].children[0].opening,
            "Generic supertype requires an EClass owner",
        );
        // Parser5253+payload/63 + walk visit1 + rejected position2986/48.
        assert_lowering_receipt(
            &expected,
            8239 + payload.len(),
            112,
            &[
                5253 + payload.len(),
                6826 + payload.len(),
                8239 + payload.len(),
            ],
            |probe| match resolver
                .resolve_type_observed(&selected, &InvocationContext::new(probe.clone()))
            {
                Err(ObservedImportError::Import(error)) => Ok(error),
                Err(error) => Err(error),
                Ok(_) => panic!("datatype generic super owner was accepted"),
            },
        );
        assert_eq!(resolver.resolve_type(&selected).unwrap_err(), expected);
        let (source, document, resolver) = fixture(
            "<eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'/><eTypeParameters name='T'/></eClassifiers>",
        );
        let selected = DeclarationId {
            source: source.id,
            path: vec![0, 0],
        };
        let expected = error(
            &document.root.children[0].children[1].span,
            "Duplicate type parameter name in the same owner",
        );
        // Parameter owner2986/49 precedes formal duplicate15951/224.
        assert_lowering_receipt(
            &expected,
            18937,
            273,
            &[1573, 2986, 11074, 18937],
            |probe| match resolver
                .resolve_parameter_observed(&selected, &InvocationContext::new(probe.clone()))
            {
                Err(ObservedImportError::Import(error)) => Ok(error),
                Err(error) => Err(error),
                Ok(_) => panic!("duplicate source formal was accepted"),
            },
        );
        assert_eq!(resolver.resolve_parameter(&selected).unwrap_err(), expected);
    }

    #[test]
    fn observed_source_normalization_preserves_exact_type_use_and_raw_projection() {
        use crate::definitions::generic_types::{Type, TypeUse};
        let payload = "<eGenericSuperTypes eClassifier='#//@eClassifiers.0'/>";
        for raw in [false, true] {
            let parameter = if raw {
                "<eTypeParameters name='T'/>"
            } else {
                ""
            };
            let (source, document, resolver) = fixture(&format!(
                "<eClassifiers x:type='e:EClass' name='Owner'>{parameter}{payload}</eClassifiers>"
            ));
            let child = usize::from(raw);
            let selected = DeclarationId {
                source: source.id.clone(),
                path: vec![0, child],
            };
            let expected = TypeUse {
                declaration: selected.clone(),
                span: document.root.children[0].children[child].span.clone(),
                expression: Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id,
                        path: vec![0],
                    },
                    arguments: vec![],
                    raw,
                },
            };
            // Source type17450/239 or27446/370, borrowed binding map128/1,
            // normalization record128 + copied target1413/16 + two visits,
            // shared shape scratch32/1 and TypeUse record128: +1829/20.
            let (source_bytes, bytes, work) = if raw {
                (27446, 29275, 390)
            } else {
                (17450, 19279, 259)
            };
            assert_lowering_receipt(
                &expected,
                bytes,
                work,
                &[
                    source_bytes,
                    source_bytes + 128,
                    source_bytes + 256,
                    source_bytes + 1669,
                    source_bytes + 1701,
                    bytes,
                ],
                |probe| {
                    resolver
                        .type_expression_observed(&selected, &InvocationContext::new(probe.clone()))
                },
            );
            assert_eq!(resolver.type_expression(&selected).unwrap(), expected);
        }
    }

    #[test]
    fn observed_complete_source_parameter_resolution_preserves_owner_and_empty_bounds() {
        let payload = "<eTypeParameters name='T'/>";
        let (source, document, resolver) = fixture(&format!(
            "<eClassifiers x:type='e:EClass' name='Owner'>{payload}</eClassifiers>"
        ));
        let selected = DeclarationId {
            source: source.id,
            path: vec![0, 0],
        };
        let expected = ResolvedParameter {
            syntax: TypeParameter {
                declaration: selected.clone(),
                span: document.root.children[0].children[0].span.clone(),
                source_payload: payload.into(),
                name: "T".into(),
                bounds: vec![],
            },
            bounds: vec![],
        };
        // Owner2986/49 + one formal8088/113 + fresh parser3432/44 + record128.
        assert_lowering_receipt(
            &expected,
            14634,
            206,
            &[
                1573, 2986, 4399, 4431, 6004, 7417, 11074, 11165, 12738, 14313, 14378, 14506, 14634,
            ],
            |probe| {
                resolver
                    .resolve_parameter_observed(&selected, &InvocationContext::new(probe.clone()))
            },
        );
        assert_eq!(resolver.resolve_parameter(&selected).unwrap(), expected);
    }

    #[test]
    fn observed_primitive_metadata_keeps_class_name_precedence_and_borrowed_storage() {
        for (attributes, expected, work) in [
            (
                "instanceClassName='int' instanceTypeName='foreign'",
                true,
                5,
            ),
            (
                "instanceClassName='foreign' instanceTypeName='long'",
                false,
                5,
            ),
            ("instanceTypeName='long'", true, 8),
            ("", false, 6),
        ] {
            let (source, _, resolver) = fixture(&format!(
                "<eClassifiers x:type='e:EDataType' name='Value' {attributes}/>"
            ));
            let selected = DeclarationId {
                source: source.id,
                path: vec![0],
            };
            // Two borrowed node visits, then ordered attribute scans for the
            // preferred class name and only when absent the type name.
            assert_lowering_receipt(&expected, 0, work, &[], |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.primitive_classifier_observed(&selected, &observation))
            });
            assert_eq!(resolver.primitive_classifier(&selected).unwrap(), expected);
        }
    }
}

#[cfg(test)]
#[path = "generic_owner_input_tests.rs"]
mod owner_input_tests;

#[cfg(test)]
#[path = "generic_source_inventory_tests.rs"]
mod source_inventory_tests;

#[cfg(test)]
mod resource_sharing_policy_tests {
    use super::*;
    use crate::definitions::{bundle::DefinitionSource, ecore::ParseLimits};

    #[test]
    fn resource_sharing_preserves_stricter_policy_errors_and_fresh_resolver_inputs()
    -> Result<(), String> {
        for kind in [SourceKind::Ecore, SourceKind::Rcore] {
            let padding = "x".repeat(ParseLimits::default().source_bytes + 17);
            let text = if kind == SourceKind::Ecore {
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:sharing'><eClassifiers xsi:type='e:EClass' name='Node'/><!--{padding}--></e:EPackage>"
                )
            } else {
                format!(
                    "rcore 1;package p version \"1\";metamodel \"urn:sharing\" prefix p {{class Node {{}}}}//{padding}\n"
                )
            };
            let source = DefinitionSource::new("memory:resource-policy", kind, text);
            let id = source.id.clone();
            let limits = ParseLimits {
                source_bytes: source.text.len(),
            };
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            };
            let resources =
                ResourceSet::new_with_limits(&bundle, limits).map_err(|error| error.to_string())?;
            assert!(resources.admitted_for(limits));
            assert!(!resources.admitted_for(ParseLimits::default()));
            let pointer = resources.document(&id)? as *const _;
            for requested in [
                limits,
                ParseLimits {
                    source_bytes: limits.source_bytes + 1,
                },
            ] {
                let fresh = Resolver::with_source_resources(&bundle, resources.clone(), requested)
                    .map_err(|error| error.to_string())?;
                assert_eq!(fresh.resource_sets().0.document(&id)? as *const _, pointer);
                let next = Resolver::with_source_resources(&bundle, resources.clone(), requested)
                    .map_err(|error| error.to_string())?;
                assert_eq!(next.resource_sets().0.document(&id)? as *const _, pointer);
                // Standard resources and semantic policy state are rebuilt on each
                // resolver construction; sharing input trees grants no authority.
                assert!(!std::ptr::eq(
                    fresh.resource_sets().1,
                    next.resource_sets().1
                ));
                assert_ne!(
                    fresh
                        .resource_sets()
                        .1
                        .document(&fresh.standard_id)?
                        .source
                        .text
                        .as_ptr(),
                    next.resource_sets()
                        .1
                        .document(&next.standard_id)?
                        .source
                        .text
                        .as_ptr()
                );
                assert!(fresh.rigid_parameters.is_empty());
                assert!(fresh.constructor_contracts.is_empty());
                assert!(fresh.provider_relations.is_empty());
            }
            for requested in [
                ParseLimits::default(),
                ParseLimits {
                    source_bytes: limits.source_bytes - 1,
                },
            ] {
                let expected = Resolver::new_with_limits(&bundle, requested)
                    .err()
                    .ok_or("Original parser unexpectedly accepted a smaller limit")?;
                let observed =
                    Resolver::with_source_resources(&bundle, resources.clone(), requested)
                        .err()
                        .ok_or("Prepared inputs bypassed stricter source policy")?;
                assert_eq!(observed, expected);
            }
            let mut invalid = bundle.clone();
            invalid.sources[0].text.push(' ');
            let expected = Resolver::new_with_limits(&invalid, limits)
                .err()
                .ok_or("Invalid source accepted")?;
            let observed = Resolver::with_source_resources(&invalid, resources.clone(), limits)
                .err()
                .ok_or("Prepared inputs bypassed identity validation")?;
            assert_eq!(observed, expected);
            assert_eq!(resources.document(&id)? as *const _, pointer);
        }
        Ok(())
    }
}

#[cfg(test)]
mod prepared_semantic_admission_regression {
    use super::*;
    use crate::definitions::bundle::DefinitionSource;
    #[test]
    fn prepared_resolver_rejects_unadmitted_semantic_source_metadata() -> Result<(), String> {
        let model = DefinitionSource::new(
            "memory:reuse-model.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='p' nsURI='urn:p'/>",
        );
        let original = DefinitionBundle {
            version: 1,
            sources: vec![model],
            namespaces: vec![],
        };
        let resources = ResourceSet::new(&original)?;
        let mut semantic = DefinitionSource::new(
            "memory:bad-behavior.rcore",
            SourceKind::RcoreBehavior,
            "semantic metadata belongs to this exact source",
        );
        semantic.id.sha256 = "0".repeat(64);
        let mut bundle = original;
        bundle.sources.push(semantic);
        let expected = Resolver::new(&bundle)
            .err()
            .ok_or("Ordinary resolver accepted a bad digest")?;
        let observed = Resolver::with_source_resources(&bundle, resources, Default::default())
            .err()
            .ok_or("Prepared resolver bypassed semantic-source admission")?;
        assert_eq!(observed, expected);
        Ok(())
    }
}
