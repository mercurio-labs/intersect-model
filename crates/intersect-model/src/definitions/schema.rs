//! Handwritten, serializable contracts for the bounded native compiler.
#[path = "schema_runtime_inputs.rs"]
mod runtime_inputs;
pub use runtime_inputs::{
    RuntimeAlternative, RuntimeArraySchemaInputs, RuntimeDomainSchemaInputs, RuntimeDomainSource,
    RuntimeFeatureDomainRead, RuntimeSchemaInputs, SharedRuntimeAlternatives,
    SharedRuntimeArrayTarget, SharedRuntimeDomain, SharedRuntimeDomainTarget, SharedRuntimeTarget,
};
pub use runtime_inputs::{
    RuntimeArrayViewSchemaInputs, RuntimeSourceArraySchemaInputs, SharedRuntimeArrayViewTarget,
    SharedRuntimeSourceArrayTarget,
};
pub(crate) use runtime_inputs::{RuntimeDomainInputs, RuntimeTargetInputs};
#[path = "borrowed_string_index.rs"]
mod borrowed_strings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[path = "schema_attribute_source.rs"]
mod attribute_source;
#[doc(hidden)]
pub use attribute_source::AttributeFieldOwner;
pub(crate) use attribute_source::{AttributeSourceInputs, NoAttributeSourceInputs};
#[path = "schema_attribute_preparation.rs"]
mod attribute_preparation;
pub use attribute_preparation::{
    AppliedDatatypeContractPreparation, AttributeConversionPreparation,
    AttributeValidationPreparation, RuntimeAttributeConversionPreparation,
    RuntimeAttributeValidationPreparation,
};
#[path = "schema_observation.rs"]
mod observation;
#[path = "schema_preparation.rs"]
mod preparation;
pub use observation::PreparationError;
pub(crate) use observation::{
    reserve as reserve_preparation, reserve_value as reserve_preparation_value,
};
pub use preparation::{
    ClassKeyPreparation, ConformancePreparation, EffectiveFeaturePreparation,
    FeatureDefaultPreparation, FeatureMapPreparation, PreparedFeatures,
    ReferenceConformancePreparation, ReferenceStructurePreparation, SelectedFeature,
};
pub(crate) use preparation::{
    DefaultSourceInputs, FeatureIdentityInput, RuntimeTypesPreparation, SelectedFeatureInput,
    ValidatedRuntimeTypes,
};
#[path = "schema_checked.rs"]
mod checked;
pub use checked::{CheckedRuntimeSchema, CheckedRuntimeSchemaIdentity, ScopeSchema};

#[path = "schema_diagnostic.rs"]
mod diagnostic;
pub use diagnostic::DiagnosticOrigin;

#[derive(Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub source: String,
    pub start: usize,
    pub end: usize,
    pub message: String,
    /// Native transient provenance; never trusted from serialized DTOs/packages.
    /// Boxed to keep compiler error returns small.
    #[serde(skip)]
    pub origin: Option<Box<DiagnosticOrigin>>,
}
// Preserve legacy diagnostic equality. Provenance qualification compares origin
// explicitly; equality of the four-field DTO is not a source-identity proof.
impl PartialEq for Diagnostic {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.start == other.start
            && self.end == other.end
            && self.message == other.message
    }
}
impl Diagnostic {
    pub fn new(source: &str, start: usize, end: usize, message: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            start,
            end,
            message: message.into(),
            origin: None,
        }
    }
}

/// Reserved structural reference domain; it is not a constructible EClass.
pub const EOBJECT_REFERENCE_TOP: &str = "__ecore_eobject_reference_top";

/// Resolve once against the exact bundled Ecore source, never a name supplied by a caller.
pub fn eobject_reference_top_declaration()
-> Result<crate::definitions::resources::DeclarationId, String> {
    eobject_reference_top_declaration_ref().cloned()
}
pub(crate) fn eobject_reference_top_declaration_ref()
-> Result<&'static crate::definitions::resources::DeclarationId, String> {
    static ID: std::sync::OnceLock<Result<crate::definitions::resources::DeclarationId, String>> =
        std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let source = crate::definitions::compiler::standard_datatype_source();
        let id = source.id.clone();
        let resources = crate::definitions::resources::ResourceSet::new_diagnostic(
            &crate::definitions::bundle::DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
        )
        .map_err(|e| e.to_string())?;
        resources.resolve(&id, "#//EObject")
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Unproven membership is retained separately from proven alternatives.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UnresolvedRuntimeFeatureCandidate {
    pub candidate: crate::definitions::generic_types::RuntimeType,
    pub target: String,
    pub reason: String,
}

/// Compiled member-domain alternatives within an explicit runtime materialization set.
/// Serialized/mutated schemas are data, not proof: authoritative consumers rebuild
/// from the source bundle and selected providers before trusting these alternatives.
pub type CheckedRuntimeFeatureDomain = CheckedRuntimeFeatureDomainRepresentation<String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CheckedRuntimeFeatureDomainRepresentation<Alternative: RuntimeAlternative = String> {
    pub source: Alternative::Source,
    pub alternatives: Alternative::Alternatives,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<UnresolvedRuntimeFeatureCandidate>,
    /// Source-established codec target only; never the first accepted alternative.
    pub lexical_target: Option<String>,
    pub default: Value,
}

/// Owned schema DTO with String-based runtime alternatives.
pub type Schema = SchemaRepresentation<String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SchemaRepresentation<Alternative: RuntimeAlternative = String> {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime_feature_domains:
        BTreeMap<String, Vec<CheckedRuntimeFeatureDomainRepresentation<Alternative>>>,
    /// Exact pinned EObject identity for the reference-only top domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_top: Option<crate::definitions::resources::DeclarationId>,
    /// All materialized constraints on erased raw reference features, by owner/name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw_reference_bounds: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_warnings: Vec<crate::definitions::generic_resolution::Warning>,
    /// Ecore declared-erasure groups compiled alongside concrete projections.
    /// None preserves native instantiated-type overload selection. Serialized
    /// contracts are data, not proof of source/provider validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_operation_selections:
        Option<BTreeMap<String, crate::definitions::operations::DeclaredOperationSet>>,
    /// Closed instantiated identities keyed by runtime class key. Source declarations
    /// retain their original IDs; this table does not certify schema specialization.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub instantiations: BTreeMap<String, crate::definitions::generic_types::RuntimeType>,
    /// Checked generic source retained alongside concrete structural projections.
    /// Deserialization alone is not a definition validation certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic_signatures: Option<crate::definitions::generic_resolution::SignatureInventory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversion_contracts: Vec<crate::definitions::conversion::ConversionContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_conversion_contracts:
        Vec<crate::definitions::conversion::AppliedConversionContract>,
    /// Exact consumed constructor policies; these do not install payload codecs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constructor_contracts: Vec<crate::definitions::conversion::ConstructorContract>,
    /// Imported: SHA-256 of exact Ecore source. Programmatic: versioned definition
    /// digest (`dynamic-v1:`). Both are separate from namespace identity.
    pub revision: String,
    pub uri: String,
    pub name: String,
    pub classes: BTreeMap<String, Class>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub enumerations: BTreeMap<String, crate::definitions::enums::Enumeration>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub datatypes: BTreeMap<String, crate::definitions::datatypes::Datatype>,
    /// Exact declaration provenance; empty only for programmatically constructed schemas.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub declarations: BTreeMap<String, ClassifierDeclaration>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClassifierDeclaration {
    pub source: crate::definitions::bundle::SourceId,
    pub path: Vec<usize>,
    pub namespace: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Class {
    pub name: String,
    pub abstract_type: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interface_type: bool,
    /// Ordered direct supertypes. Required in the fresh-start schema format.
    pub parents: Vec<String>,
    pub features: BTreeMap<String, Feature>,
    /// Source declaration order. Programmatic classes may use deterministic name order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feature_order: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub feature_declarations: BTreeMap<String, crate::definitions::resources::DeclarationId>,
    /// Ordered declarations, including same-name overloads and base overrides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<crate::definitions::operations::Operation>,
}
/// Persistent identity is independent of a receiver's effective member position.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemberIdentity {
    Source {
        declaration: crate::definitions::resources::DeclarationId,
    },
    /// Identity for an explicitly constructed schema without source declarations.
    Programmatic {
        revision: String,
        namespace: String,
        owner: String,
        name: String,
    },
}
/// Effective features of each class, computed once per class while the schema
/// is borrowed; the borrow guarantees the schema cannot change meanwhile.
/// Answers equal `Schema::effective_feature`/`feature_by_id`.
pub struct FeatureIndex<'s> {
    schema: &'s Schema,
    classes:
        std::cell::RefCell<rustc_hash::FxHashMap<&'s str, std::rc::Rc<[EffectiveFeature<'s>]>>>,
    /// `Schema::prepared_features` per declared class, under the same borrow.
    maps: std::cell::RefCell<rustc_hash::FxHashMap<&'s str, FeatureMap>>,
}
/// A shared `Schema::prepared_features` result (`fields()` equals `Schema::features`).
pub type FeatureMap = std::rc::Rc<PreparedFeatures>;
impl<'s> FeatureIndex<'s> {
    pub fn new(schema: &'s Schema) -> Self {
        Self {
            schema,
            classes: Default::default(),
            maps: Default::default(),
        }
    }
    /// The features of a declared class; other kinds are not kept.
    fn class_features(
        &self,
        kind: &str,
    ) -> Option<Result<std::rc::Rc<[EffectiveFeature<'s>]>, String>> {
        if let Some(found) = self.classes.borrow().get(kind) {
            return Some(Ok(found.clone()));
        }
        // Keyed by the schema's own class key, which outlives this index.
        let (key, _) = self.schema.classes.get_key_value(kind)?;
        Some(self.schema.effective_features(key).map(|features| {
            let features: std::rc::Rc<[_]> = features.into();
            self.classes.borrow_mut().insert(key, features.clone());
            features
        }))
    }
}
/// Effective-feature lookups answered like `Schema::effective_feature` and
/// `Schema::feature_by_id`, possibly from a cache.
pub trait FeatureLookup {
    fn schema(&self) -> &Schema;
    fn effective_feature<'k>(
        &'k self,
        kind: &'k str,
        feature: &str,
    ) -> Result<EffectiveFeature<'k>, String>;
    fn feature_by_id<'k>(
        &'k self,
        kind: &'k str,
        identity: &MemberIdentity,
    ) -> Result<EffectiveFeature<'k>, String>;
    /// Equal to `Schema::prepared_features`, possibly shared from a cache.
    fn feature_map(&self, kind: &str) -> Result<FeatureMap, String> {
        self.schema().prepared_features(kind).map(std::rc::Rc::new)
    }
}
impl FeatureLookup for Schema {
    fn schema(&self) -> &Schema {
        self
    }
    fn effective_feature<'k>(
        &'k self,
        kind: &'k str,
        feature: &str,
    ) -> Result<EffectiveFeature<'k>, String> {
        Schema::effective_feature(self, kind, feature)
    }
    fn feature_by_id<'k>(
        &'k self,
        kind: &'k str,
        identity: &MemberIdentity,
    ) -> Result<EffectiveFeature<'k>, String> {
        Schema::feature_by_id(self, kind, identity)
    }
}
impl FeatureLookup for FeatureIndex<'_> {
    fn schema(&self) -> &Schema {
        self.schema
    }
    fn effective_feature<'k>(
        &'k self,
        kind: &'k str,
        feature: &str,
    ) -> Result<EffectiveFeature<'k>, String> {
        match self.class_features(kind) {
            Some(features) => features?
                .iter()
                .find(|member| member.feature.name == feature)
                .cloned()
                .ok_or_else(|| format!("Unknown feature {kind}.{feature}")),
            None => self.schema.effective_feature(kind, feature),
        }
    }
    fn feature_by_id<'k>(
        &'k self,
        kind: &'k str,
        identity: &MemberIdentity,
    ) -> Result<EffectiveFeature<'k>, String> {
        match self.class_features(kind) {
            Some(features) => features?
                .iter()
                .find(|member| &member.identity == identity)
                .cloned()
                .ok_or_else(|| format!("Unknown feature declaration in {kind}: {identity:?}")),
            None => self.schema.feature_by_id(kind, identity),
        }
    }
    fn feature_map(&self, kind: &str) -> Result<FeatureMap, String> {
        if let Some(found) = self.maps.borrow().get(kind) {
            return Ok(found.clone());
        }
        let map: FeatureMap = std::rc::Rc::new(self.schema.prepared_features(kind)?);
        // Only declared classes are kept, keyed by the schema's own class key;
        // failures are recomputed, so every answer equals `Schema::prepared_features`.
        if let Some((key, _)) = self.schema.classes.get_key_value(kind) {
            self.maps.borrow_mut().insert(key, map.clone());
        }
        Ok(map)
    }
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct EffectiveFeature<'a> {
    pub owner: &'a str,
    pub identity: MemberIdentity,
    pub position: usize,
    pub feature: &'a Feature,
}
impl Class {
    fn super_type_at(&self, index: usize) -> Option<&str> {
        self.parents.get(index).map(String::as_str)
    }
    pub fn super_types(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.parents.iter().map(String::as_str)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Feature {
    pub name: String,
    pub target: String,
    /// Exact declared datatype identity; absent for programmatic builtin scalars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datatype: Option<crate::definitions::resources::DeclarationId>,
    pub reference: bool,
    pub containment: bool,
    pub opposite: Option<String>,
    pub lower: usize,
    pub upper: i32,
    pub unique: bool,
    pub ordered: bool,
    pub derived: bool,
    /// Reflective replacement/reset permission; distinct from collection-view edits.
    #[serde(
        default = "feature_changeable_default",
        skip_serializing_if = "feature_is_changeable"
    )]
    pub changeable: bool,
    /// Runtime state retained in snapshots but omitted from persistent feature projection.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transient: bool,
    /// Independent Ecore flag. Dynamic storage does not imply a generated getter.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub volatile: bool,
    /// Explicit assignment history is independent of the effective default value.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsettable: bool,
    pub default: Value,
}
/// Canonical effective features for one receiver in an immutably borrowed schema.
///
/// Preparation avoids repeated effective-member resolution, not value checks or
/// provider calls. Only names from this context can be validated; caller-supplied
/// features cannot replace its private canonical fields. Provider availability and
/// results are checked on every call against the supplied registry.
///
/// A context cannot outlive mutation of its source schema:
/// ```compile_fail
/// use intersect_model::definitions::schema::Schema;
/// fn mutate(schema: &mut Schema) -> Result<(), String> {
///     let prepared = schema.prepare_runtime_features("C")?;
///     schema.classes.clear();
///     let _ = prepared.fields();
///     Ok(())
/// }
/// ```
pub struct PreparedRuntimeFeatures<'a, A: RuntimeAlternative = String> {
    schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
    receiver: String,
    fields: PreparedFeatures,
}
impl<A: RuntimeAlternative> PreparedRuntimeFeatures<'_, A> {
    pub fn fields(&self) -> &BTreeMap<String, Feature> {
        self.fields.fields()
    }
    fn field(&self, name: &str) -> Result<&Feature, String> {
        self.fields
            .get(name)
            .ok_or_else(|| "Attribute does not belong to the receiver's effective features".into())
    }
    pub fn require_attribute_conversion(
        &self,
        name: &str,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        let mut pending = RuntimeAttributeConversionPreparation::from_prepared(
            &self.receiver,
            self.field(name)?,
            &self.fields,
        )?;
        while !pending.advance(self.schema, conversions)? {}
        pending.finish()
    }
    pub fn validate_attribute(
        &self,
        name: &str,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        self.validate_attribute_observed(
            name,
            value,
            conversions,
            &crate::definitions::conversion::InvocationContext::default(),
        )
    }
    pub fn validate_attribute_observed(
        &self,
        name: &str,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<(), String> {
        let mut pending = RuntimeAttributeValidationPreparation::from_prepared_observed(
            &self.receiver,
            self.field(name)?,
            &self.fields,
            observation,
        )
        .map_err(|error| error.to_string())?;
        while !pending
            .advance_observed(self.schema, value, conversions, observation)
            .map_err(|error| error.to_string())?
        {}
        pending.finish()
    }
}
impl<A: RuntimeAlternative> crate::definitions::schema::SchemaRepresentation<A> {
    /// Resolve canonical members once while borrowing the same immutable schema.
    pub fn prepare_runtime_features(
        &self,
        receiver: &str,
    ) -> Result<PreparedRuntimeFeatures<'_, A>, String> {
        let mut pending = FeatureMapPreparation::new(receiver);
        while !pending.advance(self)? {}
        Ok(PreparedRuntimeFeatures {
            schema: self,
            receiver: receiver.to_owned(),
            fields: pending.finish()?,
        })
    }
}
fn feature_changeable_default() -> bool {
    true
}
fn feature_is_changeable(value: &bool) -> bool {
    *value
}

enum InstantiationValidationStage {
    References,
    ReferencePrepare,
    ReferenceTop,
    RuntimeDomains,
    RuntimePrepare,
    RuntimeNext,
    RuntimeClear,
    DomainDuplicates,
    DomainTargets,
    DomainClear,
    RawBounds,
    RawPrepare,
    RawNext,
    RawClear,
    RawTargets,
    Instantiations,
    InstantiationParameters,
    InstantiationIdentity,
    IdentityClear,
    Done,
}

// Complete immutable metadata only; all domain/identity decisions remain fresh.
pub use preparation::{
    FeatureInputCache, PreparedFeatureInputs, PreparedFeatureLookup, SharedFeatureInputCache,
};
enum ValidationFeatures {
    Owned(PreparedFeatureInputs),
    Shared(std::sync::Arc<PreparedFeatureInputs>),
}
impl std::ops::Deref for ValidationFeatures {
    type Target = PreparedFeatureInputs;
    fn deref(&self) -> &PreparedFeatureInputs {
        match self {
            Self::Owned(fields) => fields,
            Self::Shared(fields) => fields,
        }
    }
}
impl ValidationFeatures {
    fn clear_one(&mut self) -> bool {
        match self {
            Self::Owned(fields) => fields.clear_one(),
            Self::Shared(_) => true,
        }
    }
}

// Private keys resolve against the same complete immutable prepared map.
// Public continuations still retain independently owned feature inputs.
enum ValidationDomainFeature {
    Owned(Feature),
    Prepared(String),
}
impl ValidationDomainFeature {
    fn resolve<'a>(
        &'a self,
        fields: Option<&'a ValidationFeatures>,
    ) -> Result<&'a Feature, String> {
        match self {
            Self::Owned(feature) => Ok(feature),
            Self::Prepared(name) => fields
                .and_then(|fields| fields.get(name))
                .ok_or_else(|| "Missing runtime-domain feature".into()),
        }
    }
}

/// Resumable validation for retained runtime-schema projections. Callers must
/// keep the schema immutable until completion. One declaration, domain, raw
/// bound target, signature parameter, or instantiated identity is validated per
/// advance. Successful completion releases retained identity keys incrementally.
pub struct InstantiationValidation {
    reference_views: bool,
    stage: InstantiationValidationStage,
    outer: Option<String>,
    inner: Option<String>,
    index: usize,
    target: usize,
    identities: BTreeSet<String>,
    alternatives: borrowed_strings::BorrowedStringIndex,
    domain_field: Option<ValidationDomainFeature>,
    feature_preparation: Option<FeatureMapPreparation>,
    fields: Option<ValidationFeatures>,
    shared_fields: BTreeMap<String, std::sync::Arc<PreparedFeatureInputs>>,
    input_cache: Option<SharedFeatureInputCache>,
    reference: Option<ReferenceStructurePreparation>,
    failed: bool,
}

impl Default for InstantiationValidation {
    fn default() -> Self {
        Self::new()
    }
}

impl InstantiationValidation {
    pub fn new() -> Self {
        Self {
            reference_views: false,
            stage: InstantiationValidationStage::References,
            outer: None,
            inner: None,
            index: 0,
            target: 0,
            identities: BTreeSet::new(),
            alternatives: borrowed_strings::BorrowedStringIndex::default(),
            domain_field: None,
            feature_preparation: None,
            fields: None,
            shared_fields: BTreeMap::new(),
            input_cache: None,
            reference: None,
            failed: false,
        }
    }

    // Only the synchronous immutable-schema proof selects this retention policy.
    // All validation stages and outcomes remain in the shared continuation.
    fn with_reference_views() -> Self {
        Self {
            reference_views: true,
            ..Self::new()
        }
    }
    fn completed_reference_array<'schema, A: RuntimeAlternative>(
        &self,
        schema: &'schema SchemaRepresentation<A>,
    ) -> Option<&'schema [A]> {
        if self.failed || !matches!(self.stage, InstantiationValidationStage::DomainClear) {
            return None;
        }
        let domain = schema
            .runtime_feature_domains
            .get(self.outer.as_ref()?)?
            .get(self.index)?;
        domain
            .source
            .view()
            .reference
            .then_some(&domain.alternatives)
    }
    fn pending_reference_array<'schema, A: RuntimeAlternative>(
        &self,
        schema: &'schema SchemaRepresentation<A>,
    ) -> Option<&'schema [A]> {
        if self.failed
            || self.target != 0
            || self.alternatives.is_pending()
            || !matches!(self.stage, InstantiationValidationStage::DomainDuplicates)
        {
            return None;
        }
        let domain = schema
            .runtime_feature_domains
            .get(self.outer.as_ref()?)?
            .get(self.index)?;
        domain
            .source
            .view()
            .reference
            .then_some(&domain.alternatives)
    }
    fn use_reference_array_certificate<A: RuntimeAlternative>(
        &mut self,
        schema: &SchemaRepresentation<A>,
        certificate: &preparation::ReferenceArrayCertificate<'_, A>,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        let result = (|| {
            observation
                .checkpoint()
                .map_err(PreparationError::Observation)?;
            let values = self
                .pending_reference_array(schema)
                .ok_or("No pending reference array")?;
            if !certificate.belongs_to(schema, values) {
                return Err(
                    "Reference array certificate belongs to another immutable schema or input"
                        .into(),
                );
            }
            self.stage = InstantiationValidationStage::DomainClear;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    // Each duplicate-index transition observes its actual comparison/movement.
    pub(super) fn duplicate_transition(&self) -> bool {
        matches!(self.stage, InstantiationValidationStage::DomainDuplicates)
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(
            schema,
            &crate::definitions::conversion::InvocationContext::default(),
        )
        .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl RuntimeAlternative>,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Schema validation preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner<A: RuntimeAlternative>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<A>,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        use self::observation::{reserve, reserve_value};
        use std::ops::Bound::{Excluded, Unbounded};
        if self.reference_views
            && self.input_cache.is_none()
            && !matches!(self.stage, InstantiationValidationStage::Done)
        {
            reserve(observation, 128 + 32, &[])?;
            self.input_cache = Some(std::sync::Arc::new(std::sync::Mutex::new(
                FeatureInputCache::default(),
            )));
        }
        match self.stage {
            InstantiationValidationStage::References => {
                let current = match &self.outer {
                    Some(name) => schema.classes.get_key_value(name),
                    None => schema.classes.iter().next(),
                };
                let Some((receiver, class)) = current else {
                    self.stage = InstantiationValidationStage::ReferenceTop;
                    return Ok(false);
                };
                let next = match &self.inner {
                    Some(name) => class
                        .features
                        .range::<str, _>((Excluded(name.as_str()), Unbounded))
                        .next(),
                    None => class.features.iter().next(),
                };
                if let Some((name, feature)) = next {
                    let preparation = if self.reference_views && self.shared_fields.len() < 4096 {
                        ReferenceStructurePreparation::declared_observed(
                            receiver,
                            name,
                            observation,
                        )?
                    } else {
                        ReferenceStructurePreparation::new_observed(receiver, feature, observation)?
                    };
                    reserve(observation, 0, &[name])?;
                    self.reference = Some(preparation);
                    self.inner = Some(name.clone());
                    self.stage = InstantiationValidationStage::ReferencePrepare;
                } else {
                    let next_class = schema
                        .classes
                        .range::<str, _>((Excluded(receiver.as_str()), Unbounded))
                        .next();
                    if let Some((name, _)) = next_class {
                        reserve(observation, 0, &[name])?;
                    }
                    self.outer = next_class.map(|(name, _)| name.clone());
                    self.inner = None;
                    if self.outer.is_none() {
                        self.stage = InstantiationValidationStage::ReferenceTop;
                    }
                }
            }
            InstantiationValidationStage::ReferencePrepare => {
                let reference = self
                    .reference
                    .as_mut()
                    .ok_or("Missing reference structure preparation")?;
                let complete = if self.reference_views {
                    reference.advance_shared_observed(
                        schema,
                        self.input_cache
                            .as_ref()
                            .ok_or("Missing reference input cache")?,
                        &mut self.shared_fields,
                        observation,
                    )?
                } else {
                    reference.advance_observed(schema, observation)?
                };
                if complete {
                    self.reference
                        .take()
                        .ok_or("Missing reference structure preparation")?
                        .finish()?;
                    self.stage = InstantiationValidationStage::References;
                }
            }
            InstantiationValidationStage::ReferenceTop => {
                schema.validate_reference_top()?;
                self.outer = None;
                self.inner = None;
                self.index = 0;
                self.stage = InstantiationValidationStage::RuntimeDomains;
            }
            InstantiationValidationStage::RuntimeDomains => {
                let next = match &self.outer {
                    Some(receiver) => schema.runtime_feature_domains.get_key_value(receiver),
                    None => schema.runtime_feature_domains.iter().next(),
                };
                let Some((receiver, _)) = next else {
                    self.outer = None;
                    self.index = 0;
                    self.stage = InstantiationValidationStage::RawBounds;
                    return Ok(false);
                };
                if self.reference_views
                    && let Some(fields) = self.shared_fields.get(receiver)
                {
                    reserve(observation, 0, &[receiver])?;
                    self.fields = Some(ValidationFeatures::Shared(fields.clone()));
                    self.outer = Some(receiver.clone());
                    self.stage = InstantiationValidationStage::RuntimeNext;
                } else {
                    let preparation = if self.reference_views {
                        FeatureMapPreparation::with_source_inputs_observed(
                            receiver,
                            self.input_cache
                                .as_ref()
                                .ok_or("Missing feature input cache")?
                                .clone(),
                            observation,
                        )?
                    } else {
                        FeatureMapPreparation::new_observed(receiver, observation)?
                    };
                    reserve(observation, 0, &[receiver])?;
                    self.feature_preparation = Some(preparation);
                    self.outer = Some(receiver.clone());
                    self.stage = InstantiationValidationStage::RuntimePrepare;
                }
            }
            InstantiationValidationStage::RuntimePrepare
            | InstantiationValidationStage::RawPrepare => {
                if self
                    .feature_preparation
                    .as_mut()
                    .ok_or("Missing runtime feature preparation")?
                    .advance_observed(schema, observation)?
                {
                    let fields = self
                        .feature_preparation
                        .take()
                        .ok_or("Missing runtime feature preparation")?
                        .finish_inputs()?;
                    self.fields =
                        Some(if self.reference_views && self.shared_fields.len() < 4096 {
                            let receiver =
                                self.outer.as_ref().ok_or("Missing feature-map receiver")?;
                            // Adopt admitted contents; precharge the new shared payload object, record and header.
                            reserve(observation, 128 + 128 + 32, &[receiver])?;
                            let fields = std::sync::Arc::new(fields);
                            self.shared_fields.insert(receiver.clone(), fields.clone());
                            ValidationFeatures::Shared(fields)
                        } else {
                            ValidationFeatures::Owned(fields)
                        });
                    self.stage =
                        if matches!(self.stage, InstantiationValidationStage::RuntimePrepare) {
                            InstantiationValidationStage::RuntimeNext
                        } else {
                            InstantiationValidationStage::RawNext
                        };
                }
            }
            InstantiationValidationStage::RuntimeNext => {
                let receiver = self
                    .outer
                    .as_ref()
                    .ok_or("Missing runtime-domain receiver")?;
                let domains = schema
                    .runtime_feature_domains
                    .get(receiver)
                    .ok_or("Missing runtime-domain policies")?;
                if let Some(domain) = domains.get(self.index) {
                    let field = schema.validate_runtime_domain_header(
                        receiver,
                        domain,
                        self.fields
                            .as_ref()
                            .ok_or("Missing runtime-domain fields")?,
                    )?;
                    self.domain_field = Some(if self.reference_views {
                        reserve(observation, 0, &[&field.name])?;
                        ValidationDomainFeature::Prepared(field.name.clone())
                    } else {
                        reserve_value(field, observation)?;
                        ValidationDomainFeature::Owned(field.clone())
                    });
                    self.target = 0;
                    self.stage = InstantiationValidationStage::DomainDuplicates;
                } else {
                    self.stage = InstantiationValidationStage::RuntimeClear;
                }
            }
            InstantiationValidationStage::RuntimeClear => {
                if self
                    .fields
                    .as_mut()
                    .ok_or("Missing runtime-domain fields")?
                    .clear_one()
                {
                    self.fields = None;
                    let receiver = self
                        .outer
                        .as_ref()
                        .ok_or("Missing runtime-domain receiver")?;
                    let next = schema
                        .runtime_feature_domains
                        .range::<str, _>((Excluded(receiver.as_str()), Unbounded))
                        .next();
                    if let Some((name, _)) = next {
                        reserve(observation, 0, &[name])?;
                    }
                    self.outer = next.map(|(name, _)| name.clone());
                    self.index = 0;
                    self.stage = if self.outer.is_none() {
                        InstantiationValidationStage::RawBounds
                    } else {
                        InstantiationValidationStage::RuntimeDomains
                    };
                }
            }
            InstantiationValidationStage::DomainDuplicates
            | InstantiationValidationStage::DomainTargets => {
                let domain = schema
                    .runtime_feature_domains
                    .get(
                        self.outer
                            .as_ref()
                            .ok_or("Missing runtime-domain receiver")?,
                    )
                    .and_then(|domains| domains.get(self.index))
                    .ok_or("Missing runtime domain")?;
                if let Some(target) = domain.alternatives.get(self.target).map(AsRef::as_ref) {
                    if matches!(self.stage, InstantiationValidationStage::DomainDuplicates) {
                        use borrowed_strings::{IndexError, IndexEvent, IndexStep};
                        let index_error = |error| match error {
                            IndexError::Observation(error) => error,
                            IndexError::InvalidState(message) => {
                                PreparationError::Schema(message.into())
                            }
                            IndexError::Allocation => PreparationError::Observation(
                                "Schema preparation allocation failed".into(),
                            ),
                        };
                        if !self.alternatives.is_pending() {
                            if A::STREAM_ORDERED_ALTERNATIVES {
                                self.alternatives
                                    .begin_streaming(self.target)
                                    .map_err(index_error)?;
                            } else {
                                self.alternatives.begin(self.target).map_err(index_error)?;
                            }
                        }
                        match self
                            .alternatives
                            .advance(&domain.alternatives, |event| match event {
                                IndexEvent::Work(units) => observation
                                    .work(units)
                                    .map_err(PreparationError::Observation),
                                IndexEvent::Retain(bytes) => reserve(observation, bytes, &[]),
                            })
                            .map_err(index_error)?
                        {
                            IndexStep::Pending => return Ok(false),
                            IndexStep::Inserted => {}
                            IndexStep::Duplicate => {
                                return Err("Duplicate runtime domain alternative".into());
                            }
                        }
                    } else if domain.source.view().reference {
                        if !schema.classes.contains_key(target) {
                            return Err("Unknown runtime domain class".into());
                        }
                    } else {
                        schema.domain_alternative(
                            self.domain_field
                                .as_ref()
                                .ok_or("Missing runtime-domain feature")?
                                .resolve(self.fields.as_ref())?,
                            target,
                        )?;
                    }
                    self.target += 1;
                } else if matches!(self.stage, InstantiationValidationStage::DomainDuplicates) {
                    self.target = 0;
                    self.stage = InstantiationValidationStage::DomainTargets;
                } else {
                    self.stage = InstantiationValidationStage::DomainClear;
                }
            }
            InstantiationValidationStage::DomainClear => {
                if self.alternatives.clear_one() {
                    self.domain_field = None;
                    self.index += 1;
                    self.stage = InstantiationValidationStage::RuntimeNext;
                }
            }
            InstantiationValidationStage::RawBounds => {
                let owner = match &self.outer {
                    Some(name) => schema.raw_reference_bounds.get_key_value(name),
                    None => schema.raw_reference_bounds.iter().next(),
                };
                let Some((owner, _)) = owner else {
                    self.outer = None;
                    self.inner = None;
                    self.stage = InstantiationValidationStage::Instantiations;
                    return Ok(false);
                };
                if self.reference_views
                    && let Some(fields) = self.shared_fields.get(owner)
                {
                    reserve(observation, 0, &[owner])?;
                    self.fields = Some(ValidationFeatures::Shared(fields.clone()));
                    self.outer = Some(owner.clone());
                    self.stage = InstantiationValidationStage::RawNext;
                } else {
                    let preparation = if self.reference_views {
                        FeatureMapPreparation::with_source_inputs_observed(
                            owner,
                            self.input_cache
                                .as_ref()
                                .ok_or("Missing feature input cache")?
                                .clone(),
                            observation,
                        )?
                    } else {
                        FeatureMapPreparation::new_observed(owner, observation)?
                    };
                    reserve(observation, 0, &[owner])?;
                    self.feature_preparation = Some(preparation);
                    self.outer = Some(owner.clone());
                    self.stage = InstantiationValidationStage::RawPrepare;
                }
            }
            InstantiationValidationStage::RawNext => {
                let owner = self.outer.as_ref().ok_or("Missing raw-bound owner")?;
                let features = schema
                    .raw_reference_bounds
                    .get(owner)
                    .ok_or("Missing raw-bound policies")?;
                let next = match &self.inner {
                    Some(name) => features
                        .range::<str, _>((Excluded(name.as_str()), Unbounded))
                        .next(),
                    None => features.iter().next(),
                };
                if let Some((name, targets)) = next {
                    let features_in_scope =
                        self.fields.as_ref().ok_or("Missing raw-bound fields")?;
                    if !features_in_scope
                        .get(name)
                        .is_some_and(|field| field.reference)
                        || targets.is_empty()
                    {
                        return Err("Invalid raw reference bound projection".into());
                    }
                    reserve(observation, 0, &[name])?;
                    self.inner = Some(name.clone());
                    self.target = 0;
                    self.stage = InstantiationValidationStage::RawTargets;
                } else {
                    self.stage = InstantiationValidationStage::RawClear;
                }
            }
            InstantiationValidationStage::RawClear => {
                if self
                    .fields
                    .as_mut()
                    .ok_or("Missing raw-bound fields")?
                    .clear_one()
                {
                    self.fields = None;
                    let owner = self.outer.as_ref().ok_or("Missing raw-bound owner")?;
                    let next = schema
                        .raw_reference_bounds
                        .range::<str, _>((Excluded(owner.as_str()), Unbounded))
                        .next();
                    if let Some((name, _)) = next {
                        reserve(observation, 0, &[name])?;
                    }
                    self.outer = next.map(|(name, _)| name.clone());
                    self.inner = None;
                    self.stage = if self.outer.is_none() {
                        InstantiationValidationStage::Instantiations
                    } else {
                        InstantiationValidationStage::RawBounds
                    };
                }
            }
            InstantiationValidationStage::RawTargets => {
                let targets = schema
                    .raw_reference_bounds
                    .get(self.outer.as_ref().ok_or("Missing raw-bound owner")?)
                    .and_then(|fields| fields.get(self.inner.as_ref()?))
                    .ok_or("Missing raw-bound feature")?;
                if let Some(target) = targets.get(self.target) {
                    if !schema.is_reference_target(target) {
                        return Err("Invalid raw reference bound projection".into());
                    }
                    self.target += 1;
                } else {
                    self.stage = InstantiationValidationStage::RawNext;
                }
            }
            InstantiationValidationStage::Instantiations => {
                let next = match &self.outer {
                    Some(key) => schema
                        .instantiations
                        .range::<str, _>((Excluded(key.as_str()), Unbounded))
                        .next(),
                    None => schema.instantiations.iter().next(),
                };
                if let Some((key, ty)) = next {
                    let raw = schema.validate_instantiation_header(key, ty)?;
                    reserve(observation, 0, &[key])?;
                    self.outer = Some(key.clone());
                    self.target = 0;
                    self.stage = if raw {
                        InstantiationValidationStage::InstantiationParameters
                    } else {
                        InstantiationValidationStage::InstantiationIdentity
                    };
                } else {
                    self.stage = InstantiationValidationStage::IdentityClear;
                }
            }
            InstantiationValidationStage::InstantiationParameters => {
                let ty = schema
                    .instantiations
                    .get(
                        self.outer
                            .as_ref()
                            .ok_or("Missing instantiated class key")?,
                    )
                    .ok_or("Missing instantiated class")?;
                let crate::definitions::generic_types::Type::Classifier { declaration, .. } =
                    ty.expression()
                else {
                    return Err("Instantiated class requires a classifier type".into());
                };
                let inventory = schema
                    .generic_signatures
                    .as_ref()
                    .ok_or("Raw runtime identity requires source signature inventory")?;
                let parameter = inventory
                    .parameters
                    .get(self.target)
                    .ok_or("Raw runtime identity requires a generic classifier")?;
                let id = &parameter.syntax.declaration;
                if id.source == declaration.source
                    && id.path.len() == declaration.path.len() + 1
                    && id.path.starts_with(&declaration.path)
                {
                    self.stage = InstantiationValidationStage::InstantiationIdentity;
                } else {
                    self.target += 1;
                }
            }
            InstantiationValidationStage::InstantiationIdentity => {
                let ty = schema
                    .instantiations
                    .get(
                        self.outer
                            .as_ref()
                            .ok_or("Missing instantiated class key")?,
                    )
                    .ok_or("Missing instantiated class")?;
                if self.identities.contains(ty.identity()) {
                    return Err("Duplicate instantiated class identity".into());
                }
                reserve(observation, 0, &[ty.identity()])?;
                self.identities.insert(ty.identity().to_owned());
                self.stage = InstantiationValidationStage::Instantiations;
            }
            InstantiationValidationStage::IdentityClear => {
                self.identities.pop_first();
                if self.identities.is_empty() {
                    if let Some(mut entry) = self.shared_fields.first_entry() {
                        // Release one complete map's bookkeeping entry per advance.
                        // An aliased Rc can be dropped without destroying its payload.
                        let complete = std::sync::Arc::get_mut(entry.get_mut())
                            .is_none_or(|fields| fields.clear_one());
                        if complete {
                            entry.remove_entry();
                        }
                    } else if let Some(cache) = &mut self.input_cache {
                        let complete = match std::sync::Arc::get_mut(cache) {
                            Some(cache) => cache
                                .get_mut()
                                .map_err(|_| {
                                    PreparationError::Schema(
                                        "Feature input metadata cache is poisoned during release"
                                            .into(),
                                    )
                                })?
                                .clear_one()?,
                            None => true, // Releasing an aliased handle does not destroy its retained table.
                        };
                        if complete {
                            self.input_cache = None;
                            self.stage = InstantiationValidationStage::Done;
                        }
                    } else {
                        self.stage = InstantiationValidationStage::Done;
                    }
                }
            }
            InstantiationValidationStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, InstantiationValidationStage::Done))
    }
}

impl<A: RuntimeAlternative> crate::definitions::schema::SchemaRepresentation<A> {
    /// A container back-reference is identified by its opposite's containment.
    /// Its set state follows container presence, including when unsettable=true.
    pub fn is_container_feature(&self, feature: &Feature) -> Result<bool, String> {
        let Some(opposite) = feature.opposite.as_deref() else {
            return Ok(false);
        };
        let (owner, name) = opposite
            .split_once('/')
            .ok_or("Invalid opposite identity")?;
        let fields = self.features(owner)?;
        Ok(fields
            .get(name)
            .ok_or("Unknown opposite feature")?
            .containment)
    }
    /// Check EReference uniqueness/container bounds and resolve aliases before
    /// comparing declaration owners: an inherited spelling
    /// must not turn a self-opposite into two apparently distinct references.
    pub(crate) fn validate_reference_structure(
        &self,
        receiver: &str,
        feature: &Feature,
    ) -> Result<(), String> {
        let mut pending = ReferenceStructurePreparation::new(receiver, feature);
        while !pending.advance(self)? {}
        pending.finish()
    }
    /// Validate the shared bounded structural profile, independently of input syntax.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.uri.trim().is_empty() {
            return Err("Schema name and namespace must be nonempty".into());
        }
        if self.classes.is_empty() {
            return Err("At least one EClass is required".into());
        }
        for (name, class) in &self.classes {
            if name.trim().is_empty() || name != &class.name || name.contains('/') {
                return Err(format!("Invalid class name {name}"));
            }
            self.features(name)?;
            for (key, feature) in &class.features {
                let invalid = |message: &str| format!("{name}.{key}: {message}");
                if key.trim().is_empty() || key != &feature.name || key.contains('/') {
                    return Err(invalid("Invalid feature name"));
                }
                if feature.upper < -1
                    || feature.upper == 0
                    || (feature.upper >= 0 && feature.lower > feature.upper as usize)
                {
                    return Err(invalid("Invalid or unsupported multiplicity"));
                }
                if feature.reference {
                    if !self.classes.contains_key(&feature.target) {
                        return Err(invalid("Unknown reference target"));
                    }
                } else {
                    if feature.containment || feature.opposite.is_some() {
                        return Err(invalid("Containment/opposites require EReference"));
                    }
                    if !matches!(feature.target.as_str(), "EString" | "EInt" | "EBoolean") {
                        return Err(invalid("Unsupported attribute type"));
                    }
                }
                let default = &feature.default;
                let valid_default = if feature.upper != 1 {
                    default.as_array().is_some_and(Vec::is_empty)
                } else if feature.reference {
                    default.is_null()
                } else {
                    default.is_null()
                        || match feature.target.as_str() {
                            "EString" => default.is_string(),
                            "EBoolean" => default.is_boolean(),
                            "EInt" => default.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
                            _ => false,
                        }
                };
                if !valid_default {
                    return Err(invalid("Invalid or unsupported default"));
                }
                self.validate_reference_structure(name, feature)?;
                if let Some(opposite) = &feature.opposite {
                    let (owner, other_name) = opposite
                        .split_once('/')
                        .ok_or_else(|| invalid("Invalid opposite identity"))?;
                    let fields = self.features(owner)?;
                    let other = fields
                        .get(other_name)
                        .ok_or_else(|| invalid("Unknown opposite"))?;
                    if !other.reference
                        || other.opposite.as_deref() != Some(&format!("{name}/{}", feature.name))
                        || !self.conforms(&feature.target, owner)
                        || !self.conforms(name, &other.target)
                        || (feature.containment && other.containment)
                    {
                        return Err(invalid("Inconsistent opposite"));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn feature_domains(
        &self,
        receiver: &str,
        name: &str,
    ) -> Vec<&CheckedRuntimeFeatureDomainRepresentation<A>> {
        self.runtime_feature_domains
            .get(receiver)
            .into_iter()
            .flatten()
            .filter(|domain| domain.source.view().projection.source.name == name)
            .collect()
    }

    /// An open domain can use only a codec established by source/receiver typing.
    pub fn runtime_attribute_lexical_target(
        &self,
        receiver: &str,
        name: &str,
    ) -> Result<Option<&str>, String> {
        self.attribute_lexical_target(receiver, name, || self.feature_owner(receiver, name))
    }
    /// `runtime_attribute_lexical_target` for a caller that already holds
    /// `effective_features(receiver)`: the feature's owner is selected from
    /// `effective` (the first member of that name, as `feature_owner`
    /// selects) instead of recomputing the receiver's effective features for
    /// every attribute asked about.
    pub fn runtime_attribute_lexical_target_among(
        &self,
        receiver: &str,
        name: &str,
        effective: &[EffectiveFeature<'_>],
    ) -> Result<Option<&str>, String> {
        self.attribute_lexical_target(receiver, name, || {
            effective
                .iter()
                .find(|member| member.feature.name == name)
                .map(|member| member.owner)
                .ok_or_else(|| format!("Unknown feature {receiver}.{name}"))
        })
    }
    fn attribute_lexical_target<'o>(
        &self,
        receiver: &str,
        name: &str,
        owner: impl FnOnce() -> Result<&'o str, String>,
    ) -> Result<Option<&str>, String> {
        let domains = self.feature_domains(receiver, name);
        if domains.is_empty() {
            let owner = owner()?;
            let field = self
                .classes
                .get(owner)
                .and_then(|class| class.features.get(name))
                .ok_or("Unknown attribute feature")?;
            if field.reference {
                return Err("Reference is not an attribute".into());
            }
            return Ok(Some(field.target.as_str()));
        }
        let first = domains[0];
        if first.source.view().reference {
            return Err("Reference is not an attribute".into());
        }
        if domains
            .iter()
            .any(|domain| domain.lexical_target != first.lexical_target)
        {
            return Err("Conflicting source-established attribute lexical codecs".into());
        }
        Ok(first.lexical_target.as_deref())
    }

    pub fn parse_runtime_attribute(
        &self,
        receiver: &str,
        name: &str,
        text: &str,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<Value, String> {
        let fields = self.features(receiver)?;
        let field = fields.get(name).ok_or("Unknown attribute feature")?;
        let target = self
            .runtime_attribute_lexical_target(receiver, name)?
            .ok_or("Open generic attribute requires a source-established exact lexical codec")?;
        let alternative = self.domain_alternative(field, target)?;
        self.require_attribute_conversion(&alternative, conversions)?;
        let value = if let Some(datatype) = self.datatypes.get(target) {
            if datatype.codec().is_ok() {
                serde_json::to_value(datatype.parse(text)?).map_err(|e| e.to_string())?
            } else {
                conversions
                    .ok_or("Explicit attribute conversion registry required")?
                    .parse_datatype(datatype, text)?
            }
        } else if let Some(enumeration) = self.enumerations.get(target) {
            serde_json::to_value(enumeration.parse(text)?).map_err(|e| e.to_string())?
        } else {
            crate::definitions::scalars::Builtin::named(target)
                .ok_or("Unknown attribute lexical codec")?
                .parse(text)?
        };
        self.validate_runtime_attribute(receiver, field, &value, conversions)?;
        Ok(value)
    }

    fn domain_alternative(&self, field: &Feature, target: &str) -> Result<Feature, String> {
        self.domain_alternative_observed(
            field,
            target,
            &crate::definitions::conversion::InvocationContext::default(),
        )
        .map_err(|error| error.to_string())
    }
    pub(crate) fn domain_alternative_observed(
        &self,
        field: &Feature,
        target: &str,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Feature, PreparationError> {
        use self::observation::{reserve, reserve_value};
        let declaration = self
            .datatypes
            .get(target)
            .map(|d| d.declaration())
            .or_else(|| self.enumerations.get(target).map(|d| d.declaration()));
        if declaration.is_none() && crate::definitions::scalars::Builtin::named(target).is_none() {
            return Err("Unknown runtime attribute domain codec".into());
        }
        reserve_value(field, observation)?;
        reserve(observation, 0, &[target])?;
        if let Some(declaration) = declaration {
            reserve_value(declaration, observation)?;
        }
        let mut alternative = field.clone();
        alternative.target = target.to_owned();
        alternative.datatype = declaration.cloned();
        Ok(alternative)
    }

    fn require_attribute_receiver(&self, field: &Feature) -> Result<(), String> {
        for receiver in self.runtime_feature_domains.keys() {
            if !self.feature_domains(receiver, &field.name).is_empty() {
                let effective = self.effective_feature(receiver, &field.name)?;
                let mut candidate = field.clone();
                candidate.default = effective.feature.default.clone();
                if &candidate == effective.feature {
                    return Err("Receiver is required to validate a constrained attribute".into());
                }
            }
        }
        Ok(())
    }

    pub fn require_runtime_attribute_conversion(
        &self,
        receiver: &str,
        field: &Feature,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        let mut pending = RuntimeAttributeConversionPreparation::new(receiver, field);
        while !pending.advance(self, conversions)? {}
        pending.finish()
    }

    pub fn validate_runtime_attribute(
        &self,
        receiver: &str,
        field: &Feature,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        self.validate_runtime_attribute_context(receiver, field, value, conversions, None)
    }
    /// `require_runtime_attribute_conversion` reusing `fields`, a completed
    /// member/default selection of `receiver` from THIS schema (for example
    /// `prepared_features`); the receiver and full field equality are checked.
    pub fn require_runtime_attribute_conversion_prepared(
        &self,
        receiver: &str,
        field: &Feature,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        fields: &dyn PreparedFeatureLookup,
    ) -> Result<(), String> {
        let mut pending =
            RuntimeAttributeConversionPreparation::from_prepared(receiver, field, fields)?;
        while !pending.advance(self, conversions)? {}
        pending.finish()
    }
    /// `validate_runtime_attribute` reusing `fields`, a completed member/default
    /// selection of `receiver` from THIS schema; see
    /// `require_runtime_attribute_conversion_prepared`.
    pub fn validate_runtime_attribute_prepared(
        &self,
        receiver: &str,
        field: &Feature,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        fields: &dyn PreparedFeatureLookup,
    ) -> Result<(), String> {
        let mut pending =
            RuntimeAttributeValidationPreparation::from_prepared(receiver, field, fields)?;
        while !pending.advance(self, value, conversions, None)? {}
        pending.finish()
    }

    /// Validate with a borrowed operation observer shared by nested providers.
    /// Terminal observer failures are not rejected domain alternatives.
    pub fn validate_runtime_attribute_observed(
        &self,
        receiver: &str,
        field: &Feature,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<(), String> {
        self.validate_runtime_attribute_context(
            receiver,
            field,
            value,
            conversions,
            Some(observation),
        )
    }

    fn validate_runtime_attribute_context(
        &self,
        receiver: &str,
        field: &Feature,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: Option<&crate::definitions::conversion::InvocationContext<'_>>,
    ) -> Result<(), String> {
        let unobserved = crate::definitions::conversion::InvocationContext::default();
        let observation = observation.unwrap_or(&unobserved);
        let mut pending =
            RuntimeAttributeValidationPreparation::new_observed(receiver, field, observation)
                .map_err(|error| error.to_string())?;
        while !pending
            .advance_observed(self, value, conversions, observation)
            .map_err(|error| error.to_string())?
        {}
        pending.finish()
    }

    pub fn is_reference_target(&self, target: &str) -> bool {
        self.classes.contains_key(target)
            || (target == EOBJECT_REFERENCE_TOP
                && self.reference_top.as_ref().is_some_and(|id| {
                    eobject_reference_top_declaration().is_ok_and(|expected| *id == expected)
                }))
    }

    /// Install only the canonical EObject reference context, retaining the exact
    /// schema revision derivation used by native text consumers.
    pub fn prepare_eobject_reference_context(&mut self) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        self.validate_reference_top()?;
        if self.reference_top.is_none() {
            self.reference_top = Some(eobject_reference_top_declaration()?);
            let encoded = serde_json::to_vec(&(&self.revision, &self.reference_top))
                .map_err(|error| error.to_string())?;
            self.revision = format!("{:x}", Sha256::digest(encoded));
        }
        Ok(())
    }

    /// Check the retained universal-reference identity and classifier collisions.
    /// This read-only check neither installs a context nor grants an owner lease.
    pub fn validate_reference_top(&self) -> Result<(), String> {
        if let Some(id) = &self.reference_top {
            if *id != eobject_reference_top_declaration()? {
                return Err("Reference top domain differs from pinned EObject declaration".into());
            }
            if self.classes.contains_key(EOBJECT_REFERENCE_TOP)
                || self.enumerations.contains_key(EOBJECT_REFERENCE_TOP)
                || self.datatypes.contains_key(EOBJECT_REFERENCE_TOP)
            {
                return Err("Reference top domain collides with a classifier".into());
            }
        }
        Ok(())
    }

    /// Resolve a language-visible class name to one exact runtime key.
    pub fn language_class_key(&self, name: &str) -> Result<&str, String> {
        if let Some((key, _)) = self.classes.get_key_value(name) {
            return Ok(key);
        }
        let mut found = self.declarations.iter().filter(|(key, declaration)| {
            self.classes.contains_key(*key)
                && declaration.namespace == self.uri
                && declaration.name == name
        });
        let (key, _) = found
            .next()
            .ok_or_else(|| format!("Unknown rule class {name}"))?;
        if found.next().is_some() {
            return Err(format!("Ambiguous materialized class {name}"));
        }
        Ok(key)
    }

    pub fn applied_datatype_contract(
        &self,
        datatype: &crate::definitions::datatypes::Datatype,
    ) -> Result<&crate::definitions::conversion::AppliedConversionContract, String> {
        let mut pending = AppliedDatatypeContractPreparation::new(datatype);
        while !pending.advance(self)? {}
        pending.finish(self)
    }
    /// Require a custom attribute's provider independently of current multiplicity.
    pub fn require_attribute_conversion(
        &self,
        field: &Feature,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        let mut pending = AttributeConversionPreparation::new(field);
        while !pending.advance(self, conversions)? {}
        pending.finish()
    }
    /// Validate one present attribute value. Cardinality/nullability belongs to
    /// the model validator; enum aliases are checked by exact declaration ID.
    pub fn validate_attribute(&self, field: &Feature, value: &Value) -> Result<(), String> {
        self.require_attribute_receiver(field)?;
        self.validate_attribute_context(field, value, None)
    }
    /// Explicit custom conversion context. Existing callers never gain implicit providers.
    pub fn validate_attribute_with_conversions(
        &self,
        field: &Feature,
        value: &Value,
        conversions: &crate::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<(), String> {
        self.require_attribute_receiver(field)?;
        self.validate_attribute_context(field, value, Some(conversions))
    }
    fn validate_attribute_context(
        &self,
        field: &Feature,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        self.validate_attribute_observation(field, value, conversions, None)
    }
    fn validate_attribute_observation(
        &self,
        field: &Feature,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: Option<&crate::definitions::conversion::InvocationContext<'_>>,
    ) -> Result<(), String> {
        let unobserved = crate::definitions::conversion::InvocationContext::default();
        let observation = observation.unwrap_or(&unobserved);
        let mut pending = AttributeValidationPreparation::new_observed(field, observation)
            .map_err(|error| error.to_string())?;
        while !pending
            .advance_observed(self, value, conversions, observation)
            .map_err(|error| error.to_string())?
        {}
        pending.finish()
    }
    /// One present payload atom, after the shared codec preparation completes.
    /// JSON decoding, datatype equality and native provider calls remain atomic.
    pub(crate) fn validate_attribute_payload(
        &self,
        field: &Feature,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: Option<&crate::definitions::conversion::InvocationContext<'_>>,
    ) -> Result<(), PreparationError> {
        if field.reference {
            return Err("Reference is not an attribute".into());
        }
        if value.is_null() {
            return Err("Null is absence, not a present attribute value".into());
        }
        if let Some(id) = &field.datatype {
            if let Some(datatype) = self
                .datatypes
                .get(&field.target)
                .filter(|datatype| datatype.declaration() == id)
            {
                if let Some(observation) = observation {
                    self::observation::reserve_value(value, observation)?;
                }
                if let Some(application) = datatype.application() {
                    let converted: crate::definitions::conversion::AppliedConvertedValue =
                        serde::Deserialize::deserialize(value)
                            .map_err(|e| format!("Invalid applied value: {e}"))?;
                    let registry = conversions
                        .ok_or("Explicit applied conversion registry required")?
                        .applied();
                    if let Some(observation) = observation {
                        registry.validate_for_observed(application, &converted, observation)?;
                    } else {
                        registry.validate_for(application, &converted)?;
                    }
                    return Ok(());
                }
                if matches!(
                    datatype.binding(),
                    crate::definitions::datatypes::Binding::Unsupported { .. }
                ) {
                    let conversions = conversions
                        .ok_or("Explicit conversion registry required for custom datatype")?;
                    let converted: crate::definitions::conversion::ConvertedValue =
                        serde::Deserialize::deserialize(value)
                            .map_err(|error| format!("Invalid converted value: {error}"))?;
                    if let Some(observation) = observation {
                        conversions.validate_for_observed(datatype, &converted, observation)?;
                    } else {
                        conversions.validate_for(datatype, &converted)?;
                    }
                    return Ok(());
                }
                let value: crate::definitions::datatypes::DatatypeValue =
                    serde::Deserialize::deserialize(value)
                        .map_err(|error| format!("Invalid datatype value: {error}"))?;
                datatype.validate_value(&value)?;
                return Ok(());
            }
            let enumeration = self
                .enumerations
                .get(&field.target)
                .filter(|enumeration| enumeration.declaration() == id)
                .ok_or_else(|| format!("Unknown declared datatype {}", field.target))?;
            if let Some(observation) = observation {
                self::observation::reserve_value(value, observation)?;
            }
            let value: crate::definitions::enums::EnumValue =
                serde::Deserialize::deserialize(value)
                    .map_err(|error| format!("Invalid enum value: {error}"))?;
            enumeration.validate_value(&value)?;
            return Ok(());
        }
        crate::definitions::scalars::Builtin::named(&field.target)
            .ok_or_else(|| format!("Unsupported attribute type {}", field.target))?
            .validate(value)
            .map_err(PreparationError::Schema)
    }
    /// Parent-first declaration order; shared diamond ancestors occur once.
    /// Explicit frames keep deeply nested input off the native/WASM call stack.
    pub fn ancestry<'a>(&'a self, kind: &'a str) -> Result<Vec<&'a str>, String> {
        self.ancestry_observed(
            kind,
            &crate::definitions::conversion::InvocationContext::default(),
        )
        .map_err(|error| error.to_string())
    }
    pub(crate) fn ancestry_observed<'a>(
        &'a self,
        kind: &'a str,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Vec<&'a str>, PreparationError> {
        let mut pending = preparation::AncestryPreparation::new_observed(kind, observation)?;
        loop {
            observation.work(1).map_err(PreparationError::Observation)?;
            if pending.advance_observed(self, observation)? {
                break;
            }
        }
        let mut result = Vec::new();
        for name in pending.finish()? {
            observation.work(1).map_err(PreparationError::Observation)?;
            let key = self
                .classes
                .get_key_value(&name)
                .map(|(key, _)| key.as_str())
                .ok_or_else(|| format!("Unknown class {name}"))?;
            observation::reserve(observation, 32, &[])?;
            result.push(key);
        }
        Ok(result)
    }
    /// Return the declaring class rather than the receiver of an inherited feature.
    pub fn effective_feature<'a>(
        &'a self,
        kind: &'a str,
        feature: &str,
    ) -> Result<EffectiveFeature<'a>, String> {
        self.effective_features(kind)?
            .into_iter()
            .find(|member| member.feature.name == feature)
            .ok_or_else(|| format!("Unknown feature {kind}.{feature}"))
    }
    pub fn feature_owner<'a>(&'a self, kind: &'a str, feature: &str) -> Result<&'a str, String> {
        Ok(self.effective_feature(kind, feature)?.owner)
    }
    /// Exact identity lookup; a source edit or legacy revision cannot alias a member.
    pub fn feature_by_id<'a>(
        &'a self,
        kind: &'a str,
        identity: &MemberIdentity,
    ) -> Result<EffectiveFeature<'a>, String> {
        self.effective_features(kind)?
            .into_iter()
            .find(|member| &member.identity == identity)
            .ok_or_else(|| format!("Unknown feature declaration in {kind}: {identity:?}"))
    }
    pub fn reference_conforms(
        &self,
        owner: &str,
        name: &str,
        actual: &str,
    ) -> Result<bool, String> {
        let mut pending = ReferenceConformancePreparation::new(owner, name, actual);
        while !pending.advance(self)? {}
        pending.finish()
    }
    pub fn conforms(&self, actual: &str, expected: &str) -> bool {
        self.conforms_observed(
            actual,
            expected,
            &crate::definitions::conversion::InvocationContext::default(),
        )
        .unwrap_or(false)
    }
    pub(crate) fn conforms_observed(
        &self,
        actual: &str,
        expected: &str,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        let mut pending = ConformancePreparation::new_observed(actual, expected, observation)?;
        loop {
            observation.work(1).map_err(PreparationError::Observation)?;
            match pending.advance_observed(self, observation) {
                Ok(false) => {}
                Ok(true) => return Ok(matches!(pending.finish(), Ok(true))),
                Err(PreparationError::Schema(_)) => return Ok(false),
                Err(error @ PreparationError::Observation(_)) => return Err(error),
            }
        }
    }
    pub fn features(&self, kind: &str) -> Result<BTreeMap<String, Feature>, String> {
        Ok(self.prepared_features(kind)?.into_fields())
    }
    /// The completed member/default selection behind `features`, reusable as
    /// a `PreparedFeatureLookup` of this same schema.
    pub fn prepared_features(&self, kind: &str) -> Result<PreparedFeatures, String> {
        let mut pending = FeatureMapPreparation::new(kind);
        while !pending.advance(self)? {}
        pending.finish()
    }
    /// Shared feature selection for construction, grammar and semantic consumers.
    pub fn effective_features<'a>(
        &'a self,
        kind: &'a str,
    ) -> Result<Vec<EffectiveFeature<'a>>, String> {
        let mut pending = EffectiveFeaturePreparation::new(kind);
        while !pending.advance(self)? {}
        pending
            .finish()?
            .into_iter()
            .map(|member| member.resolve(self))
            .collect()
    }
    pub(crate) fn validate_member_declaration(
        &self,
        owner: &str,
        member: &crate::definitions::resources::DeclarationId,
    ) -> Result<(), String> {
        if !self.classes.contains_key(owner) {
            return Err(format!("Unknown member owner {owner}"));
        }
        if member.path.is_empty() {
            return Err("Member declaration cannot be a resource root".into());
        }
        if let Some(classifier) = self.declarations.get(owner)
            && (member.source != classifier.source
                || member.path.len() != classifier.path.len() + 1
                || !member.path.starts_with(&classifier.path))
        {
            return Err(format!(
                "Member declaration has wrong owner {owner}: {member:?}"
            ));
        }
        Ok(())
    }
    fn validate_runtime_domain_header<'a>(
        &self,
        receiver: &str,
        domain: &CheckedRuntimeFeatureDomainRepresentation<A>,
        fields: &'a PreparedFeatureInputs,
    ) -> Result<&'a Feature, String> {
        let projection = domain.source.view().projection;
        let receiver_matches = if let Some(instantiation) = self.instantiations.get(receiver) {
            instantiation == projection.receiver
        } else if let crate::definitions::generic_types::Type::Classifier {
            declaration,
            arguments,
            raw: false,
        } = projection.receiver.expression()
        {
            arguments.is_empty()
                && self.classes.contains_key(receiver)
                && self.declarations.get(receiver).is_some_and(|source| {
                    source.source == declaration.source && source.path == declaration.path
                })
        } else {
            false
        };
        if !receiver_matches {
            return Err("Runtime domain receiver identity mismatch".into());
        }
        let field = fields
            .get(projection.source.name)
            .ok_or("Missing runtime domain feature")?;
        let owner = fields
            .owner(&field.name)
            .ok_or("Missing runtime domain feature owner")?;
        if self
            .classes
            .get(owner)
            .and_then(|class| class.feature_declarations.get(&field.name))
            != Some(projection.source.declaration)
            || field.reference != domain.source.view().reference
        {
            return Err("Runtime domain feature identity mismatch".into());
        }
        Ok(field)
    }

    fn validate_instantiation_header(
        &self,
        key: &str,
        ty: &crate::definitions::generic_types::RuntimeType,
    ) -> Result<bool, String> {
        if !self.classes.contains_key(key) {
            return Err(format!("Unknown instantiated class {key}"));
        }
        let declared = self
            .declarations
            .get(key)
            .ok_or("Instantiated class requires exact source provenance")?;
        let crate::definitions::generic_types::Type::Classifier {
            declaration,
            arguments,
            ..
        } = ty.expression()
        else {
            return Err("Instantiated class requires a classifier type".into());
        };
        if declaration.source != declared.source || declaration.path != declared.path {
            return Err("Instantiated class declaration identity mismatch".into());
        }
        if arguments.is_empty()
            && !matches!(
                ty.expression(),
                crate::definitions::generic_types::Type::Classifier { raw: true, .. }
            )
        {
            return Err(
                "Instantiated class requires explicit type arguments or a raw identity".into(),
            );
        }
        let raw = matches!(
            ty.expression(),
            crate::definitions::generic_types::Type::Classifier { raw: true, .. }
        );
        Ok(raw)
    }

    pub fn validate_instantiations(&self) -> Result<(), String> {
        let mut pending = InstantiationValidation::new();
        while !pending.advance(self)? {}
        Ok(())
    }
    /// `class_key_for_type` for a schema that already passed
    /// `validate_instantiations` and cannot have changed since: the caller
    /// owns it immutably. Each `class_key_for_type` call repeats that
    /// whole-schema validation; this selects the key only.
    pub(crate) fn class_key_for_validated_type(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
    ) -> Result<&str, String> {
        let mut pending = ClassKeyPreparation::validated(ty);
        while !pending.advance(self)? {}
        let key = pending.finish()?;
        self.classes
            .get_key_value(&key)
            .map(|(key, _)| key.as_str())
            .ok_or_else(|| "Missing prepared runtime class key".into())
    }
    pub fn instantiated_class_key(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
    ) -> Result<&str, String> {
        let mut pending = ClassKeyPreparation::instantiated(ty);
        while !pending.advance(self)? {}
        let key = pending.finish()?;
        self.instantiations
            .get_key_value(&key)
            .map(|(key, _)| key.as_str())
            .ok_or_else(|| "Missing prepared instantiated class key".into())
    }
    /// Select a concrete runtime class by its full closed type expression.
    pub fn class_key_for_type(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
    ) -> Result<&str, String> {
        self.class_key_for_type_observed(
            ty,
            &crate::definitions::conversion::InvocationContext::default(),
        )
        .map_err(|error| error.to_string())
    }
    pub(crate) fn class_key_for_type_observed(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<&str, PreparationError> {
        let mut pending = ClassKeyPreparation::new_observed(ty, observation)?;
        loop {
            observation.work(1).map_err(PreparationError::Observation)?;
            if pending.advance_observed(self, observation)? {
                break;
            }
        }
        let key = pending.finish()?;
        self.classes
            .get_key_value(&key)
            .map(|(key, _)| key.as_str())
            .ok_or_else(|| "Missing prepared runtime class key".into())
    }
    pub fn identity(&self, class: &str) -> String {
        self.formatted_identity(class)
    }

    /// Select by exact source revision and namespace, never by an ambiguous short name.
    pub fn class_key(
        &self,
        source: &crate::definitions::bundle::SourceId,
        namespace: &str,
        name: &str,
    ) -> Result<&str, String> {
        let mut matches = self
            .declarations
            .iter()
            .filter(|(_, d)| &d.source == source && d.namespace == namespace && d.name == name);
        let (key, _) = matches
            .next()
            .ok_or_else(|| format!("Unknown classifier {namespace}::{name} in {}", source.uri))?;
        if matches.next().is_some() {
            return Err("Ambiguous classifier declaration".into());
        }
        Ok(key)
    }
}

/// Ordered children must identify the same parent and retain source ordinal order.
pub(crate) fn validate_declaration_order(
    previous: &crate::definitions::resources::DeclarationId,
    next: &crate::definitions::resources::DeclarationId,
) -> Result<(), String> {
    let (Some((previous_index, previous_parent)), Some((next_index, next_parent))) =
        (previous.path.split_last(), next.path.split_last())
    else {
        return Err("Member declaration cannot be a resource root".into());
    };
    if previous.source != next.source
        || previous_parent != next_parent
        || previous_index >= next_index
    {
        return Err(format!(
            "Invalid member declaration order: {previous:?} then {next:?}"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Semantics {
    /// Retained source selections, never serialized executable authority.
    /// Consumers must explicitly compile and install these reference policies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope_bindings: Vec<crate::definitions::rcore_scope::ScopeBindingDescriptor>,
    /// Explicit per-rule conversion policy; no schema-wide codec replacement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_conversions: Vec<crate::definitions::rule_conversion::RuleConversion>,
    pub version: u32,
    pub scope: String,
    pub derived: Vec<Derivation>,
    /// Explicit per-concrete-class constructor dependencies, separate from defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initializers: Vec<ClassInitialization>,
    pub validations: Vec<Validation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassInitialization {
    pub class: String,
    pub provider: crate::definitions::conversion::ProviderId,
    pub members: Vec<String>,
    /// Canonical values for the installed constant-attribute provider only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<BTreeMap<String, Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derivation {
    pub class: String,
    pub feature: String,
    pub expression: Expression,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expression {
    /// Selector data only. Fresh source compilation must install its getter.
    Source {
        source: crate::definitions::bundle::SourceId,
        declaration: usize,
    },
    /// A checked, direct call to an external declaration in the retained source.
    External {
        function: String,
        arguments: Vec<DerivedArgument>,
    },
    Stored {
        feature: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        writable: bool,
    },
    Count {
        feature: String,
    },
    NoIncoming {
        class: String,
        feature: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DerivedArgument {
    Receiver,
    Stored { feature: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    pub class: String,
    pub feature: String,
    pub max_incoming: usize,
    pub message: String,
}

/// Version of the neutral model metadata DTO (legacy text package format 2 is frontend-owned).
pub const MODEL_PACKAGE_VERSION: u32 = 3;

fn model_package_version<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != MODEL_PACKAGE_VERSION {
        return Err(serde::de::Error::custom(
            "Unsupported model metadata package version; expected 3",
        ));
    }
    Ok(version)
}

/// Retained modeling metadata and neutral structural/behavior selections.
/// This is data, not a checked model owner or executable provider installation.
/// Text packages belong to their frontend; text-only fields are refused here.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    #[serde(deserialize_with = "model_package_version")]
    pub version: u32,
    pub schema: Schema,
    pub semantics: Semantics,
    pub sources: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compilation: Option<super::model_definition::ModelCompilation>,
}
impl Package {
    pub fn model_definition(&self) -> super::model_definition::ModelDefinitionView<'_> {
        super::model_definition::ModelDefinitionView::new(&self.schema, &self.semantics)
    }
    pub fn model_package(&self) -> super::model_definition::ModelPackageView<'_> {
        super::model_definition::ModelPackageView {
            definition: self.model_definition(),
            version: if self.version == MODEL_PACKAGE_VERSION {
                self.version
            } else {
                0
            },
            structure_text: self.sources.get("structure").map(String::as_str),
            behavior_text: self.sources.get("behavior").map(String::as_str),
            structure: self.compilation.as_ref().map(|recipe| &recipe.structure),
            behavior: self.compilation.as_ref().map(|recipe| &recipe.behavior),
            bundle: self.compilation.as_ref().map(|recipe| &recipe.bundle),
        }
    }
}

#[path = "schema_identity_observation.rs"]
mod identity_observation;
pub(crate) use identity_observation::{
    admit_feature_lookup_observed, member_identity_equal_observed,
};
