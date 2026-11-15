//! Staged shared structural validation. Stores/schema must remain immutable.
use super::construction_shapes::{
    BudgetConstructionShapes, ConstructionShapes, LeasedConstructionShapes, NoConstructionShapes,
};
use super::*;
use crate::definitions::schema::NoAttributeSourceInputs;
use std::ops::Bound::{Excluded, Unbounded};
#[path = "completed_references.rs"]
mod completed_references;
use completed_references::CompletedReferences;
// Full audits retain their owned projection without an additional Arc allocation.
// Eligible local edits borrow the facade's immutable shared metadata through Arc.
/// Reference requirements during an incomplete construction transaction.
/// Provisional references still require linking and strict final validation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DraftReferencePolicy {
    #[default]
    Resolved,
    /// Permit only canonical unresolved non-containment references. Compatibility
    /// JSON markers, unavailable identities and resolved targets remain checked.
    PendingNonContainment,
}

enum CachedFeatures {
    Owned(intersect_model::definitions::schema::PreparedFeatures),
    Shared(std::sync::Arc<intersect_model::definitions::schema::PreparedFeatures>),
    Inputs(intersect_model::definitions::schema::PreparedFeatureInputs),
}
// Ephemeral checked declaration selection from this audit's OWN completed row.
// It is never public, retained in a holder, serialized or reused after a write.
pub(super) struct PendingReferenceSource<'a> {
    pub(super) owner: &'a str,
    pub(super) declared: &'a intersect_model::definitions::schema::Feature,
}
// Canonical source access only; implementations are private completed rows.
pub(super) trait ReferenceSourceLookup {
    fn receiver(&self) -> &str;
    fn get(&self, name: &str) -> Option<&intersect_model::definitions::schema::Feature>;
    fn identity(&self, name: &str)
    -> Option<&intersect_model::definitions::schema::MemberIdentity>;
    fn owner(&self, name: &str) -> Option<&str>;
}
pub(super) fn checked_reference_source<'a, F: ReferenceSourceLookup + ?Sized>(
    fields: &'a F,
    receiver: &str,
    name: &str,
    field: &intersect_model::definitions::schema::Feature,
    schema: &'a intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    observation: &InvocationContext<'_>,
) -> StorageResult<Option<PendingReferenceSource<'a>>> {
    observation.work(1)?;
    if fields.receiver() != receiver
        || !fields
            .get(name)
            .is_some_and(|selected| std::ptr::eq(selected, field))
        || field.name != name
    {
        return Err("Pending reference source receiver/member changed".into());
    }
    observation.work(1)?;
    let identity = fields
        .identity(name)
        .ok_or("Missing pending reference source identity")?;
    let intersect_model::definitions::schema::MemberIdentity::Source { declaration } = identity
    else {
        // The new route is bounded to imported source. Preserve old discovery
        // for programmatic inputs; a caller cannot assert a source owner.
        return Ok(None);
    };
    observation.work(1)?;
    let owner = fields
        .owner(name)
        .ok_or("Missing pending reference source owner")?;
    observation.work(1)?;
    let class = schema
        .classes
        .get(owner)
        .ok_or("Missing pending reference source class")?;
    if class.feature_declarations.get(name) != Some(declaration) {
        return Err("Pending reference source declaration changed".into());
    }
    observation.work(1)?;
    schema.validate_member_declaration(owner, declaration)?;
    observation.work(1)?;
    let declared = class
        .features
        .get(name)
        .ok_or("Missing pending reference source field")?;
    if declared.name != name || !declared.reference || !field.reference {
        return Err("Pending reference source is not the named reference declaration".into());
    }
    Ok(Some(PendingReferenceSource { owner, declared }))
}
impl CachedFeatures {
    fn selected_reference<'a>(
        &'a self,
        receiver: &str,
        name: &str,
        field: &intersect_model::definitions::schema::Feature,
        schema: &'a intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<PendingReferenceSource<'a>>> {
        checked_reference_source(self, receiver, name, field, schema, observation)
    }
    fn lookup(&self) -> &dyn intersect_model::definitions::schema::PreparedFeatureLookup {
        match self {
            Self::Owned(fields) => fields,
            Self::Shared(fields) => fields.as_ref(),
            Self::Inputs(fields) => fields,
        }
    }
    fn get(&self, name: &str) -> Option<&intersect_model::definitions::schema::Feature> {
        self.lookup().get(name)
    }
    fn has_no_unsettable_fields(&self) -> bool {
        let fields = match self {
            Self::Owned(fields) => fields,
            Self::Shared(fields) => fields.as_ref(),
            _ => return false,
        };
        !fields
            .fields()
            .values()
            .any(|field| field.unsettable && !field.derived)
    }
    fn ordered_name(&self, index: usize) -> Option<&str> {
        match self {
            Self::Owned(fields) => fields.order().get(index).map(String::as_str),
            Self::Shared(fields) => fields.order().get(index).map(String::as_str),
            Self::Inputs(fields) => fields.ordered_name(index),
        }
    }
}
impl ReferenceSourceLookup for CachedFeatures {
    fn receiver(&self) -> &str {
        self.lookup().receiver()
    }
    fn get(&self, name: &str) -> Option<&intersect_model::definitions::schema::Feature> {
        self.get(name)
    }
    fn identity(
        &self,
        name: &str,
    ) -> Option<&intersect_model::definitions::schema::MemberIdentity> {
        initialization::MemberLookup::member_identity(self, name)
    }
    fn owner(&self, name: &str) -> Option<&str> {
        match self {
            Self::Owned(fields) => fields.owner(name),
            Self::Shared(fields) => fields.owner(name),
            Self::Inputs(fields) => fields.owner(name),
        }
    }
}
impl ReferenceSourceLookup for intersect_model::definitions::schema::PreparedFeatureInputs {
    fn receiver(&self) -> &str {
        self.receiver()
    }
    fn get(&self, name: &str) -> Option<&intersect_model::definitions::schema::Feature> {
        self.get(name)
    }
    fn identity(
        &self,
        name: &str,
    ) -> Option<&intersect_model::definitions::schema::MemberIdentity> {
        self.member_identity(name)
    }
    fn owner(&self, name: &str) -> Option<&str> {
        self.owner(name)
    }
}
impl assignment::FieldLookup for CachedFeatures {
    fn get(&self, name: &str) -> Option<&intersect_model::definitions::schema::Feature> {
        self.get(name)
    }
    fn next_field(
        &self,
        after: Option<&str>,
    ) -> Option<(&str, &intersect_model::definitions::schema::Feature)> {
        match self {
            Self::Inputs(fields) => fields.next_field(after),
            Self::Owned(fields) => assignment::FieldLookup::next_field(fields.fields(), after),
            Self::Shared(fields) => assignment::FieldLookup::next_field(fields.fields(), after),
        }
    }
}
impl initialization::MemberLookup for CachedFeatures {
    fn member_identity(
        &self,
        name: &str,
    ) -> Option<&intersect_model::definitions::schema::MemberIdentity> {
        match self {
            Self::Inputs(fields) => fields.member_identity(name),
            Self::Owned(fields) => fields.member_identities().get(name),
            Self::Shared(fields) => fields.member_identities().get(name),
        }
    }
}

/// The prepared stages of one synchronous audit, as `StructureFacts`.
struct AuditFacts<'a, A: intersect_model::definitions::schema::RuntimeAlternative> {
    schema: &'a intersect_model::definitions::schema::SchemaRepresentation<A>,
    features: &'a BTreeMap<String, CachedFeatures>,
    order: &'a BTreeMap<String, Vec<String>>,
    audit: &'a crate::core::resources::ResourceAudit,
}
impl<A: intersect_model::definitions::schema::RuntimeAlternative> StructureFacts
    for AuditFacts<'_, A>
{
    fn class(&self, class: &str, identity: &str) -> Option<Vec<FeatureRule<'_>>> {
        let observation = InvocationContext::default();
        if self.schema.classes.get(class)?.abstract_type
            || !self
                .schema
                .identity_matches_observed(class, identity, &observation)
                .ok()?
            || !self
                .schema
                .runtime_feature_domains
                .get(class)
                .is_none_or(Vec::is_empty)
        {
            return None;
        }
        let prepared = self.features.get(class)?;
        if !prepared.has_no_unsettable_fields() {
            return None;
        }
        self.order
            .get(class)?
            .iter()
            .map(|name| {
                let field = prepared.get(name)?;
                // Declared datatypes and opposites take the staged conversion
                // and reciprocity checks.
                let kind = if field.opposite.is_some() || field.datatype.is_some() {
                    return None;
                } else if field.derived {
                    if field.containment {
                        return None;
                    }
                    FeatureRuleKind::Derived
                } else if field.reference {
                    FeatureRuleKind::Reference {
                        containment: field.containment,
                    }
                } else if field.containment
                    || crate::definitions::scalars::Builtin::named(&field.target)
                        != Some(crate::definitions::scalars::Builtin::EString)
                {
                    return None;
                } else {
                    FeatureRuleKind::Text
                };
                Some(FeatureRule {
                    name,
                    lower: field.lower,
                    upper: field.upper,
                    unique: field.unique,
                    kind,
                })
            })
            .collect()
    }
    fn conforms(&self, class: &str, feature: &str, target: &str) -> bool {
        let observation = InvocationContext::default();
        (|| -> StorageResult<bool> {
            let prepared = self.features.get(class).ok_or("Missing class")?;
            let field = prepared.get(feature).ok_or("Missing feature")?;
            let source =
                prepared.selected_reference(class, feature, field, self.schema, &observation)?;
            let mut preparation = match source {
                Some(source) => intersect_model::definitions::schema::ReferenceConformancePreparation::from_selected_source_observed(
                    class, feature, target, source.owner, source.declared, &observation,
                ),
                None => intersect_model::definitions::schema::ReferenceConformancePreparation::new_observed(
                    class, feature, target, &observation,
                ),
            }
            .map_err(|error| error.to_string())?;
            while !preparation
                .advance_observed(self.schema, &observation)
                .map_err(|error| error.to_string())?
            {}
            preparation.completed_result()
        })()
        .unwrap_or(false)
    }
    fn resource_loaded(&self, uri: &str) -> bool {
        self.audit.loaded(uri)
    }
    fn roots(&self, visit: &mut dyn FnMut(&str, &str) -> bool) -> bool {
        self.audit.declared_roots().all(|(id, uri)| visit(id, uri))
    }
}

enum Stage {
    Start,
    Revision,
    Identity,
    Cache,
    CachePrepare,
    CacheOrder,
    Objects,
    Assignment,
    Header,
    Unknown,
    Feature,
    Conversion,
    FeatureReady,
    Shape,
    Value,
    Attribute,
    Reference,
    CycleStart,
    CycleEdge,
    CycleClear,
    Done,
}
/// Read-only one-to-one projection of the structural continuation's stage.
/// This carries no graph, selected member or validation authority.
pub struct StructuralPreparation {
    stage: Stage,
    after: Option<u64>,
    object: Option<(String, String)>,
    feature: usize,
    atom: usize,
    shape: Option<FeatureShape>,
    assignment: assignment::AssignmentPreparation,
    receipt: Option<initialization::ReceiptPreparation>,
    reference: Option<(
        String,
        intersect_model::definitions::schema::ReferenceConformancePreparation,
    )>,
    conversion:
        Option<Box<intersect_model::definitions::schema::RuntimeAttributeConversionPreparation>>,
    attribute: Option<(
        Value,
        Box<intersect_model::definitions::schema::RuntimeAttributeValidationPreparation>,
    )>,
    unknown: views::FieldNames,
    schema_validation: intersect_model::definitions::schema::InstantiationValidation,
    checked_schema: Option<intersect_model::definitions::schema::CheckedRuntimeSchemaIdentity>,
    feature_inputs: Option<intersect_model::definitions::schema::SharedFeatureInputCache>,
    ordinary_schema_started: bool,
    class: Option<String>,
    member_preparation: Option<Box<intersect_model::definitions::schema::FeatureMapPreparation>>,
    cache_after: Option<String>,
    cache_index: usize,
    features: BTreeMap<String, CachedFeatures>,
    feature_order: BTreeMap<String, Vec<String>>,
    owners: OwnershipIndex,
    resource_audit: Option<crate::core::resources::ResourceAudit>,
    audit_resources: bool,
    synchronous: bool,
    require_lower_bounds: bool,
    references: DraftReferencePolicy,
    retain_ownership: bool,
    last_cycle: Option<String>,
    current: Option<String>,
    seen: BTreeSet<String>,
    failed: bool,
}
impl StructuralPreparation {
    pub fn ownership() -> Self {
        Self::new(true, true)
    }
    pub(super) fn new(require_lower_bounds: bool, retain_ownership: bool) -> Self {
        Self::new_typed(require_lower_bounds, retain_ownership)
    }
    pub(super) fn draft(references: DraftReferencePolicy) -> Self {
        let mut pending = Self::new(false, false);
        pending.references = references;
        pending
    }
    // Eligible isolated draft edits reuse immutable canonical metadata and the
    // same object/assignment/value validation stages as complete audits.
    pub(super) fn selected(
        features: &BTreeMap<
            String,
            std::sync::Arc<intersect_model::definitions::schema::PreparedFeatures>,
        >,
    ) -> Self {
        let mut pending = Self::with_prepared_features(false, features);
        pending.audit_resources = false;
        pending
    }
    /// Only the private facade plan can supply these features. Its immutable
    /// schema has already passed the same instantiation validation.
    pub(super) fn with_prepared_features(
        require_lower_bounds: bool,
        features: &BTreeMap<
            String,
            std::sync::Arc<intersect_model::definitions::schema::PreparedFeatures>,
        >,
    ) -> Self {
        let mut pending = Self::new(require_lower_bounds, false);
        pending.stage = Stage::Revision;
        pending.features = features
            .iter()
            .map(|(class, fields)| (class.clone(), CachedFeatures::Shared(fields.clone())))
            .collect();
        pending.feature_order = features
            .iter()
            .map(|(class, fields)| (class.clone(), fields.fields().keys().cloned().collect()))
            .collect();
        pending
    }
    /// Reuse actual immutable runtime-type validation, retaining every model,
    /// provider, revision, value, relationship and ownership check.
    pub fn advance_checked_schema<A: intersect_model::definitions::schema::RuntimeAlternative>(
        &mut self,
        store: &(impl ModelRead + ?Sized),
        schema: &intersect_model::definitions::schema::CheckedRuntimeSchema<A>,
        conversions: Option<&ConversionRegistry<'_>>,
        scoped: Option<&InvocationContext<'_>>,
    ) -> StorageResult<bool> {
        if self.failed {
            return Err("Structural preparation has failed".into());
        }
        let unobserved = InvocationContext::default();
        let observation = scoped.unwrap_or(&unobserved);
        let result = (|| {
            observation.checkpoint()?;
            if let Some(identity) = &self.checked_schema {
                if !schema.matches_identity(identity) {
                    return Err("Checked structural schema changed".into());
                }
            } else {
                if self.ordinary_schema_started || !matches!(self.stage, Stage::Start) {
                    return Err(
                        "Structural schema authority cannot change during preparation".into(),
                    );
                }
                self.checked_schema = Some(
                    schema
                        .identity_observed(observation)
                        .map_err(|e| e.to_string())?,
                );
                // Only an immutable checked-schema audit selects this retention
                // policy. Every class/default/model/provider check still runs.
                observation.retain_storage(128 + 32)?;
                self.feature_inputs = Some(std::sync::Arc::new(std::sync::Mutex::new(
                    intersect_model::definitions::schema::FeatureInputCache::default(),
                )));
                self.stage = Stage::Revision;
            }
            self.advance_inner(
                store,
                schema.schema(),
                conversions,
                scoped,
                &mut NoConstructionShapes,
                None,
                None,
            )
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Drain a private audit without retaining codec continuations for fields
    /// whose validation is entirely schema-local.
    pub(super) fn run(
        mut self,
        store: &(impl ModelRead + ?Sized),
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        conversions: Option<&ConversionRegistry<'_>>,
        scoped: Option<&InvocationContext<'_>>,
    ) -> StorageResult<OwnershipIndex> {
        self.synchronous = scoped.is_none() && conversions.is_none();
        if self.synchronous
            && self.require_lower_bounds
            && !self.retain_ownership
            && self.audit_resources
            && self.references == DraftReferencePolicy::Resolved
            && matches!(self.stage, Stage::Start)
            && self.features.is_empty()
            && store.may_accept_structure()
            && Self::accepted(store, schema)
        {
            return Ok(self.owners);
        }
        let mut completed = CompletedReferences::default();
        while !self.advance_cached(
            store,
            schema,
            conversions,
            scoped,
            self.synchronous.then_some(&mut completed),
        )? {}
        self.finish()
    }

    /// Whether the store itself establishes that an unobserved complete
    /// audit passes. A separate preparation runs the canonical schema,
    /// revision, resource-registry and per-class feature stages; the store
    /// then checks its objects against those prepared facts
    /// (`ModelRead::accepts_structure`). Any failure or doubt returns false
    /// and the caller's own audit starts from the beginning, so every error
    /// and its precedence stay the staged audit's.
    fn accepted(
        store: &(impl ModelRead + ?Sized),
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
    ) -> bool {
        let mut probe = Self::new(true, false);
        probe.synchronous = true;
        while matches!(probe.stage, Stage::Start | Stage::Revision) {
            if probe
                .advance_cached(store, schema, None, None, None)
                .is_err()
            {
                return false;
            }
        }
        if !matches!(probe.stage, Stage::Identity) {
            return false;
        }
        // The per-class feature stages read only the schema.
        probe.stage = Stage::Cache;
        while matches!(
            probe.stage,
            Stage::Cache | Stage::CachePrepare | Stage::CacheOrder
        ) {
            if probe
                .advance_cached(store, schema, None, None, None)
                .is_err()
            {
                return false;
            }
        }
        let Some(audit) = &probe.resource_audit else {
            return false;
        };
        store.accepts_structure(&AuditFacts {
            schema,
            features: &probe.features,
            order: &probe.feature_order,
            audit,
        })
    }

    pub fn advance(
        &mut self,
        store: &(impl ModelRead + ?Sized),
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        conversions: Option<&ConversionRegistry<'_>>,
        scoped: Option<&InvocationContext<'_>>,
    ) -> StorageResult<bool> {
        self.advance_cached(store, schema, conversions, scoped, None)
    }
    fn advance_cached(
        &mut self,
        store: &(impl ModelRead + ?Sized),
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        conversions: Option<&ConversionRegistry<'_>>,
        scoped: Option<&InvocationContext<'_>>,
        completed: Option<&mut CompletedReferences>,
    ) -> StorageResult<bool> {
        if self.checked_schema.is_some() {
            self.failed = true;
            return Err(
                "Checked structural preparation requires its immutable checked schema".into(),
            );
        }
        if self.failed {
            return Err("Structural preparation has failed".into());
        }
        self.ordinary_schema_started = true;
        let result = self.advance_inner(
            store,
            schema,
            conversions,
            scoped,
            &mut NoConstructionShapes,
            None,
            completed,
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub(super) fn advance_with_structural_values(
        &mut self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        values: &mut super::structural_values::StructuralValueCursor<'_, '_, '_>,
    ) -> StorageResult<bool> {
        if self.failed || self.checked_schema.is_some() {
            return Err(
                "Structural value preparation has failed or changed schema authority".into(),
            );
        }
        if !std::ptr::eq(store, values.store) || !std::ptr::eq(schema, values.schema) {
            self.failed = true;
            return Err("Structural values belong to another actual store/schema".into());
        }
        self.ordinary_schema_started = true;
        let result = self.advance_inner(
            store,
            schema,
            conversions,
            Some(observation),
            &mut NoConstructionShapes,
            Some(values),
            None,
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    // This borrowed input is never retained in the public preparation holder.
    // Only the checked factory supplies it, inside its atomic transaction. It
    // grants no ordinary validation, ownership, semantic-read or getter proof.
    pub(super) fn advance_construction(
        &mut self,
        store: &impl ModelStore,
        schema: &Schema,
        construction: &DraftConstructionContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        self.advance_construction_mode(store, schema, None, construction, budget)
    }

    pub(super) fn advance_construction_with_conversions(
        &mut self,
        store: &impl ModelStore,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        construction: &DraftConstructionContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        self.advance_construction_mode(store, schema, conversions, construction, budget)
    }

    pub(super) fn advance_construction_mode(
        &mut self,
        store: &impl ModelStore,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        construction: &DraftConstructionContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        if self.failed {
            return Err("Structural preparation has failed".into());
        }
        let result = (|| {
            self.construction_step_boundary(store, schema, construction, budget)?;
            self.advance_inner(
                store,
                schema,
                conversions,
                None,
                &mut BudgetConstructionShapes {
                    context: construction,
                    budget,
                },
                None,
                None,
            )
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn construction_step_boundary(
        &mut self,
        store: &impl ModelStore,
        schema: &Schema,
        construction: &DraftConstructionContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<()> {
        // New association/store callback work is charged on the caller's
        // original ledger immediately before the feature boundary. Earlier
        // unchanged structural traversal remains the legacy atomic gap.
        if matches!(self.stage, Stage::FeatureReady) {
            budget.charge()?;
            if !store.is_transaction_active() {
                return Err("Storage write requires a transaction".into());
            }
            budget.charge()?;
            construction.check_construction_schema(schema)?;
        }
        if self.require_lower_bounds || self.retain_ownership {
            return Err("Physical construction cannot publish strict structural authority".into());
        }
        if self.checked_schema.is_some() {
            return Err(
                "Checked structural preparation requires its immutable checked schema".into(),
            );
        }
        self.ordinary_schema_started = true;
        Ok(())
    }
    pub(super) fn advance_construction_with_sources(
        &mut self,
        store: &impl ModelStore,
        schema: &Schema,
        construction: &DraftConstructionContext<'_>,
        source: &DraftConstructionSource<'_, '_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        self.advance_construction_with_sources_and_conversions(
            store,
            schema,
            None,
            construction,
            source,
            budget,
        )
    }
    pub(super) fn advance_construction_with_sources_and_conversions(
        &mut self,
        store: &impl ModelStore,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        construction: &DraftConstructionContext<'_>,
        source: &DraftConstructionSource<'_, '_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        if self.failed {
            return Err("Structural preparation has failed".into());
        }
        let result = (|| {
            self.construction_step_boundary(store, schema, construction, budget)?;
            self.advance_inner(
                store,
                schema,
                conversions,
                None,
                &mut LeasedConstructionShapes {
                    context: construction,
                    source,
                    budget,
                },
                None,
                None,
            )
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl StructuralPreparation {
    pub(super) fn new_typed(require_lower_bounds: bool, retain_ownership: bool) -> Self {
        Self {
            stage: Stage::Start,
            after: None,
            object: None,
            feature: 0,
            atom: 0,
            shape: None,
            assignment: assignment::AssignmentPreparation::new(),
            receipt: None,
            reference: None,
            conversion: None,
            attribute: None,
            unknown: views::FieldNames::default(),
            schema_validation: intersect_model::definitions::schema::InstantiationValidation::new(),
            checked_schema: None,
            feature_inputs: None,
            ordinary_schema_started: false,
            class: None,
            member_preparation: None,
            cache_after: None,
            cache_index: 0,
            features: BTreeMap::new(),
            feature_order: BTreeMap::new(),
            owners: OwnershipIndex::new(retain_ownership),
            resource_audit: None,
            audit_resources: true,
            synchronous: false,
            require_lower_bounds,
            references: DraftReferencePolicy::Resolved,
            retain_ownership,
            last_cycle: None,
            current: None,
            seen: BTreeSet::new(),
            failed: false,
        }
    }
    #[allow(clippy::too_many_arguments)] // One shared algorithm with an optional operation-local value cursor.
    fn advance_inner<D: ConstructionShapes>(
        &mut self,
        store: &(impl ModelRead + ?Sized),
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        conversions: Option<&ConversionRegistry<'_>>,
        scoped: Option<&InvocationContext<'_>>,
        shapes: &mut D,
        values: Option<&mut super::structural_values::StructuralValueCursor<'_, '_, '_>>,
        completed: Option<&mut CompletedReferences>,
    ) -> StorageResult<bool> {
        let unobserved = InvocationContext::default();
        let observation = scoped.unwrap_or(&unobserved);
        observation.checkpoint()?;
        match self.stage {
            Stage::Start => {
                observation.work(1)?;
                if self
                    .schema_validation
                    .advance_observed(schema, observation)
                    .map_err(|error| error.to_string())?
                {
                    self.stage = Stage::Revision;
                }
            }
            Stage::Revision => {
                observation.work(1)?;
                if self.audit_resources && self.resource_audit.is_none() {
                    self.resource_audit = Some(crate::core::resources::ResourceAudit::new(store)?);
                }
                let has_objects = !store.id_page_observed(None, 1, observation)?.is_empty();
                let mut wrong_revision = None;
                store.inspect_revision(&mut |revision| {
                    wrong_revision =
                        Some((has_objects || !revision.is_empty()) && revision != schema.revision);
                    Ok(())
                })?;
                if wrong_revision.ok_or("Missing model revision projection")? {
                    return Err("Model belongs to a different schema revision".into());
                }
                self.stage = Stage::Identity;
            }
            Stage::Identity | Stage::Objects => {
                observation.work(1)?;
                let page = store.id_page_observed(self.after, 1, observation)?;
                if page.len() > 1 {
                    return Err("Structural page exceeds requested bound".into());
                }
                if let Some((cursor, id)) = page.into_iter().next() {
                    if self.after.is_some_and(|previous| cursor <= previous) {
                        return Err("Structural page did not advance".into());
                    }
                    observation.work(1)?;
                    let identity = matches!(self.stage, Stage::Identity);
                    let found = store.inspect(&id, &mut |element| {
                        if identity {
                            if let Some(audit) = &mut self.resource_audit {
                                audit.element(element)?;
                            }
                            let identity_matches = if self.synchronous {
                                schema.identity_matches_observed(
                                    element.class(),
                                    element.type_identity(),
                                    observation,
                                )?
                            } else {
                                element.type_identity() == schema.identity(element.class())
                            };
                            if !identity_matches {
                                return Err("Model type identity does not match schema".into());
                            }
                            if schema
                                .classes
                                .get(element.class())
                                .ok_or("Unknown class")?
                                .abstract_type
                            {
                                return Err("Abstract instance".into());
                            }
                            Ok(())
                        } else {
                            self.begin_object(element, observation)
                        }
                    })?;
                    if !found {
                        return Err("Missing paged element".into());
                    }
                    self.after = Some(cursor);
                } else {
                    self.after = None;
                    self.stage = if matches!(self.stage, Stage::Identity) {
                        if self.features.is_empty() {
                            Stage::Cache
                        } else {
                            Stage::Objects
                        }
                    } else {
                        Stage::CycleStart
                    };
                }
            }
            Stage::Cache => {
                let next = match &self.class {
                    Some(last) => schema
                        .classes
                        .range::<str, _>((Excluded(last.as_str()), Unbounded))
                        .next(),
                    None => schema.classes.iter().next(),
                };
                if let Some((name, _)) = next {
                    observation.work(1)?;
                    let preparation = match &self.feature_inputs {
                        Some(cache) => intersect_model::definitions::schema::FeatureMapPreparation::with_source_inputs_observed(name, cache.clone(), observation),
                        None => intersect_model::definitions::schema::FeatureMapPreparation::new_observed(name, observation),
                    }.map_err(|error| error.to_string())?;
                    if self.feature_inputs.is_some() && self.retain_ownership {
                        // The complete prepared input already owns this exact
                        // declaration order. Keep one class cursor, not a second
                        // map/vector of copied order names.
                        reserve_names(observation, 0, &[name])?;
                    } else {
                        reserve_names(observation, 0, &[name, name])?;
                        self.feature_order.insert(name.clone(), Vec::new());
                    }
                    self.member_preparation = Some(Box::new(preparation));
                    self.class = Some(name.clone());
                    self.stage = Stage::CachePrepare;
                } else {
                    self.stage = Stage::Objects;
                }
            }
            Stage::CachePrepare => {
                observation.work(1)?;
                let prepared = self
                    .member_preparation
                    .as_mut()
                    .ok_or("Missing member preparation")?;
                if prepared
                    .advance_observed(schema, observation)
                    .map_err(|error| error.to_string())?
                {
                    let class = self.class.as_ref().ok_or("Missing cached class")?;
                    reserve_names(observation, 0, &[class])?;
                    self.features.insert(class.clone(), {
                        let prepared = self
                            .member_preparation
                            .take()
                            .ok_or("Missing member preparation")?;
                        if self.feature_inputs.is_some() {
                            CachedFeatures::Inputs(prepared.finish_inputs()?)
                        } else {
                            CachedFeatures::Owned(prepared.finish()?)
                        }
                    });
                    self.cache_after = None;
                    self.cache_index = 0;
                    self.stage = Stage::CacheOrder;
                }
            }
            Stage::CacheOrder => {
                observation.work(1)?;
                let class = self.class.as_ref().ok_or("Missing cached class")?;
                let prepared = self.features.get(class).ok_or("Missing cached features")?;
                let next = if self.retain_ownership {
                    prepared.ordered_name(self.cache_index)
                } else {
                    assignment::FieldLookup::next_field(prepared, self.cache_after.as_deref())
                        .map(|(name, _)| name)
                };
                if let Some(name) = next {
                    let indexed_input_order =
                        self.feature_inputs.is_some() && self.retain_ownership;
                    if !indexed_input_order {
                        reserve_names(observation, 32, &[name, name])?;
                        let order = self
                            .feature_order
                            .get_mut(class)
                            .ok_or("Missing cached order")?;
                        order
                            .try_reserve(1)
                            .map_err(|_| "Structural feature order allocation failed")?;
                        order.push(name.to_owned());
                        self.cache_after = Some(name.to_owned());
                    }
                    self.cache_index += 1;
                } else {
                    self.cache_after = None;
                    self.stage = Stage::Cache;
                }
            }
            Stage::Assignment | Stage::Header | Stage::Unknown => {
                let (id, class) = self.object.as_ref().ok_or("Missing structural object")?;
                let prepared = self.features.get(class).ok_or("Unknown class")?;
                let fields = prepared;
                let member_identities = prepared;
                let found = store.inspect(id, &mut |element| {
                    if matches!(self.stage, Stage::Assignment) {
                        if self.synchronous && prepared.has_no_unsettable_fields() {
                            if element.assignment_state().is_some() {
                                return Err(
                                    "Explicit assignment state requires unsettable schema support"
                                        .into(),
                                );
                            }
                            self.stage = Stage::Header;
                        } else {
                            if self
                                .assignment
                                .advance(schema, element, fields, || observation.work(1))?
                            {
                                self.stage = Stage::Header;
                            }
                            return Ok(());
                        }
                    }
                    if matches!(self.stage, Stage::Header) {
                        if self.synchronous && element.initialization().is_none() {
                            self.stage = Stage::Unknown;
                        } else {
                            if self.receipt.is_none() {
                                self.receipt =
                                    Some(initialization::ReceiptPreparation::new(schema, element)?);
                            }
                            let receipt_done = self
                                .receipt
                                .as_mut()
                                .ok_or("Missing initialization receipt preparation")?
                                .advance(schema, element, fields, member_identities, || {
                                    observation.work(1)
                                })?;
                            if receipt_done {
                                self.unknown = views::FieldNames::default();
                                self.stage = Stage::Unknown;
                            }
                            return Ok(());
                        }
                    }
                    loop {
                        observation.work(1)?;
                        let (key, done) = self.unknown.advance(element);
                        if let Some(key) = key
                            && fields.get(key).is_none()
                        {
                            return Err(format!("Unknown feature {}.{key}", element.class()));
                        }
                        if done {
                            self.stage = Stage::Feature;
                            break;
                        }
                        if !self.synchronous {
                            break;
                        }
                    }
                    Ok(())
                })?;
                if !found {
                    return Err("Missing structural object".into());
                }
            }
            Stage::Feature
            | Stage::Conversion
            | Stage::FeatureReady
            | Stage::Shape
            | Stage::Value
            | Stage::Attribute
            | Stage::Reference => self.advance_member(
                store,
                schema,
                conversions,
                scoped,
                shapes,
                values,
                completed,
            )?,
            Stage::CycleStart => {
                let next = match &self.last_cycle {
                    Some(last) => self
                        .owners
                        .parents
                        .range::<str, _>((Excluded(last.as_str()), Unbounded))
                        .next(),
                    None => self.owners.parents.iter().next(),
                };
                if let Some((id, _)) = next {
                    observation.work(1)?;
                    reserve_cycle_pair(observation, id, id)?;
                    self.last_cycle = Some(id.clone());
                    self.current = Some(id.clone());
                    self.stage = Stage::CycleEdge;
                } else {
                    self.stage = Stage::Done;
                }
            }
            Stage::CycleEdge => {
                let current = self
                    .current
                    .as_ref()
                    .ok_or("Missing containment traversal position")?;
                if let Some(owner) = self
                    .owners
                    .cycle_next(current, &mut self.seen, observation)?
                {
                    self.current = Some(owner);
                } else {
                    self.current = None;
                    self.stage = Stage::CycleClear;
                }
            }
            Stage::CycleClear => {
                if !self.seen.is_empty() {
                    observation.work(1)?;
                    self.seen.pop_first();
                }
                if self.seen.is_empty() {
                    self.stage = Stage::CycleStart;
                }
            }
            Stage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, Stage::Done))
    }
    pub fn finish(self) -> StorageResult<OwnershipIndex> {
        if self.failed || !matches!(self.stage, Stage::Done) {
            return Err("Structural preparation is incomplete or failed".into());
        }
        if let Some(audit) = self.resource_audit {
            audit.finish()?;
        }
        Ok(self.owners)
    }
}

fn reciprocal_value(id: &str, observation: &InvocationContext<'_>) -> StorageResult<Value> {
    if observation.has_observer() {
        intersect_model::definitions::retained_storage::reserve_serialized(id, observation)?;
    }
    Ok(Value::String(id.to_owned()))
}

// Cumulative logical retention: two string/tree slots plus their UTF-8 bytes.
// Check the complete transition before either cursor or visited-set mutation.
pub(super) fn reserve_cycle_pair(
    observation: &InvocationContext<'_>,
    left: &str,
    right: &str,
) -> StorageResult<()> {
    reserve_names(observation, 0, &[left, right])
}
fn reserve_names(
    observation: &InvocationContext<'_>,
    slots: usize,
    names: &[&str],
) -> StorageResult<()> {
    let bytes = names
        .iter()
        .try_fold(slots, |sum, name| {
            sum.checked_add(64)?.checked_add(name.len())
        })
        .ok_or("Structural preparation storage overflow")?;
    observation.retain_storage(bytes)
}
impl StructuralPreparation {
    fn begin_object(
        &mut self,
        element: ObjectView<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        reserve_names(observation, 0, &[element.id(), element.class()])?;
        self.assignment = assignment::AssignmentPreparation::new();
        self.receipt = None;
        self.reference = None;
        self.unknown = views::FieldNames::default();
        if self.synchronous
            && let Some((id, class)) = &mut self.object
        {
            id.clear();
            id.push_str(element.id());
            class.clear();
            class.push_str(element.class());
        } else {
            self.object = Some((element.id().into(), element.class().into()));
        }
        self.feature = 0;
        self.atom = 0;
        self.stage = Stage::Assignment;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // Same member engine with optional explicit effective-value selection.
    fn advance_member<D: ConstructionShapes>(
        &mut self,
        store: &(impl ModelRead + ?Sized),
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        conversions: Option<&ConversionRegistry<'_>>,
        scoped: Option<&InvocationContext<'_>>,
        shapes: &mut D,
        mut values: Option<&mut super::structural_values::StructuralValueCursor<'_, '_, '_>>,
        mut completed: Option<&mut CompletedReferences>,
    ) -> StorageResult<()> {
        let unobserved = InvocationContext::default();
        let observation = scoped.unwrap_or(&unobserved);
        let (id, class) = self.object.as_ref().ok_or("Missing structural object")?;
        let next_name = if self.feature_inputs.is_some() && self.retain_ownership {
            self.features
                .get(class)
                .and_then(|fields| fields.ordered_name(self.feature))
        } else {
            self.feature_order
                .get(class)
                .and_then(|names| names.get(self.feature))
                .map(String::as_str)
        };
        let Some(name) = next_name else {
            if !self.synchronous {
                self.object = None;
            }
            self.stage = Stage::Objects;
            return Ok(());
        };
        let field = self
            .features
            .get(class)
            .and_then(|fields| fields.get(name))
            .ok_or("Missing effective feature")?;
        let inline_validation = self.synchronous
            && field.datatype.is_none()
            && schema
                .runtime_feature_domains
                .get(class)
                .is_none_or(Vec::is_empty);
        if inline_validation && matches!(self.stage, Stage::Feature) {
            self.stage = Stage::FeatureReady;
        }
        if matches!(self.stage, Stage::Feature) {
            observation.work(1)?;
            self.conversion = Some(Box::new(
                intersect_model::definitions::schema::RuntimeAttributeConversionPreparation::from_prepared_with_sources(
                    class, field, self.features.get(class).ok_or("Missing prepared features")?.lookup(),
                    observation, &NoAttributeSourceInputs,
                ).map_err(|error| error.to_string())?,
            ));
            self.stage = Stage::Conversion;
            return Ok(());
        }
        if matches!(self.stage, Stage::Conversion) {
            observation.work(1)?;
            if self
                .conversion
                .as_mut()
                .ok_or("Missing structural attribute conversion preparation")?
                .advance_with_sources(schema, conversions, observation, &NoAttributeSourceInputs)
                .map_err(|error| error.to_string())?
            {
                self.conversion
                    .take()
                    .ok_or("Missing structural attribute conversion preparation")?
                    .finish()?;
                self.stage = Stage::FeatureReady;
            }
            return Ok(());
        }
        if matches!(self.stage, Stage::Reference) {
            observation.work(1)?;
            let (_, preparation) = self
                .reference
                .as_mut()
                .ok_or("Missing structural reference qualification")?;
            if preparation
                .advance_observed(schema, observation)
                .map_err(|error| error.to_string())?
            {
                let (target, preparation) = self
                    .reference
                    .take()
                    .ok_or("Missing structural reference qualification")?;
                if !preparation.completed_result()? {
                    return Err(format!("Reference type mismatch {class}.{name}"));
                }
                if let Some(cache) = completed {
                    cache.retain(preparation)?;
                }
                if field.containment {
                    self.owners.insert(&target, id, observation)?;
                    if !field.derived
                        && let Some(audit) = &mut self.resource_audit
                    {
                        audit.contained(&target);
                    }
                }
                if let Some(opposite) = &field.opposite {
                    let (_, back) = opposite.split_once('/').ok_or("Invalid opposite")?;
                    let value = reciprocal_value(id, observation)?;
                    if !store.matches_observed(&target, Query::Value(back, &value), observation)? {
                        return Err(format!("Missing reciprocal opposite {opposite}"));
                    }
                }
                self.atom += 1;
                self.stage = Stage::Value;
            }
            return Ok(());
        }
        if matches!(self.stage, Stage::Attribute) {
            let (value, preparation) = self
                .attribute
                .as_mut()
                .ok_or("Missing structural attribute validation")?;
            let complete = preparation
                .advance_with_sources(
                    schema,
                    value,
                    conversions,
                    observation,
                    &NoAttributeSourceInputs,
                )
                .map_err(|error| match error {
                    intersect_model::definitions::schema::PreparationError::Observation(error) => {
                        error
                    }
                    intersect_model::definitions::schema::PreparationError::Schema(error) => {
                        format!("Invalid {} value for {class}.{name}: {error}", field.target)
                    }
                })?;
            if complete {
                self.attribute
                    .take()
                    .ok_or("Missing structural attribute validation")?
                    .1
                    .finish()?;
                self.atom += 1;
                self.stage = Stage::Value;
            }
            return Ok(());
        }
        let owners = &mut self.owners;
        let pending_reference = &mut self.reference;
        let pending_attribute = &mut self.attribute;
        let prepared_fields = self
            .features
            .get(class)
            .ok_or("Missing structural feature cache")?;
        let feature_stage = matches!(self.stage, Stage::FeatureReady);
        let value_stage = matches!(self.stage, Stage::Value);
        let mut exhausted = false;
        let mut feature_done = false;
        if feature_stage && let Some(values) = values.as_deref_mut() {
            values.clear();
            if !field.derived && !field.containment {
                values.select(super::StructuralValueRequest {
                    object: id,
                    actual: class,
                    owner: prepared_fields
                        .owner(name)
                        .ok_or("Missing structural value declaring owner")?,
                    identity: prepared_fields
                        .identity(name)
                        .ok_or("Missing structural value member identity")?,
                    feature: field,
                })?;
            }
        }
        let selected_values = values.as_deref();
        let found = store.inspect(id, &mut |element| {
            let raw = element.value(name);
            let value = selected_values.map_or(raw, |values| values.effective(raw));
            if feature_stage {
                observation.work(1)?;
                if field.derived {
                    if field.containment {
                        owners.derived(element.id(), observation)?;
                    }
                    if value.is_some_and(|v| !v.is_null()) {
                        return Err("Derived values cannot be stored".into());
                    }
                    feature_done = true;
                } else if inline_validation
                    && (!field.reference
                        || value.is_none_or(|value| {
                            value.is_null() || value.collection().is_some_and(|items| items.is_empty())
                        }))
                {
                    validate_feature_shape(
                        element.class(),
                        field,
                        value,
                        self.require_lower_bounds,
                        || Ok(()),
                    )?;
                    if let Some(value) = value.filter(|value| !value.is_null()) {
                        let validate_atom = |atom: ValueView<'_>| {
                            schema
                                .validate_attribute_payload(field, &atom.to_json(), None, None)
                                .map_err(|error| {
                                    format!("Invalid {} value for {class}.{name}: {error}", field.target)
                                })
                        };
                        if let Some(items) = value.collection() {
                            for atom in items.iter() {
                                validate_atom(atom)?;
                            }
                        } else {
                            validate_atom(value)?;
                        }
                    }
                    feature_done = true;
                } else {
                    // Disabled source input is a ZST and performs no lookup.
                    // Present values always receive the original shape checks.
                    let absent_delegate = if value.is_none() && D::ENABLED {
                        shapes.absent(element, name, prepared_fields, observation)?
                    } else {
                        false
                    };
                    if !absent_delegate {
                        self.shape = Some(FeatureShape::new(
                            element.class(),
                            field,
                            value,
                            self.require_lower_bounds,
                        )?);
                    }
                }
                return Ok(());
            }
            if matches!(self.stage, Stage::Shape) {
                let shape = self
                    .shape
                    .as_mut()
                    .ok_or("Missing feature shape preparation")?;
                if shape.advance(value, || observation.work(1))? {
                    self.shape = None;
                    self.stage = Stage::Value;
                }
                return Ok(());
            }
            let selected = if let Some(list) = value.and_then(|v| v.collection()) {
                list.get(self.atom)
            } else {
                value.filter(|v| !v.is_null() && self.atom == 0)
            };
            let Some(value) = selected else {
                exhausted = true;
                return Ok(());
            };
            let mut check_value = |value: ValueView<'_>| -> StorageResult<()> {
                observation.work(1)?;
                if field.reference {
                    let identity = value.as_str();
                    let pending = match identity {
                        None => matches!(value.reference(), Some(ReferenceView::Unresolved { .. })),
                        Some(wire) => raw.is_some()
                            && wire.starts_with('?')
                            && store.is_pending_reference(element.id(), name, self.atom, wire),
                    };
                    if pending {
                        if self.references == DraftReferencePolicy::PendingNonContainment
                            && !field.containment
                        {
                            return Ok(());
                        }
                        return Err("Reference must be an identity".into());
                    }
                    let id = identity.ok_or("Reference must be an identity")?;
                    let mut preparation = None;
                    let found = store.inspect_class(id, &mut |target_class| {
                        if let Some(reused) = completed.as_deref_mut().and_then(|cache|
                            cache.take(element.class(), name, target_class))
                        {
                            preparation = Some(reused);
                            return Ok(());
                        }
                        let source = if self.synchronous {
                            prepared_fields.selected_reference(element.class(), name, field, schema, observation)?
                        } else {
                            None
                        };
                        preparation = Some(match source {
                            Some(source) => intersect_model::definitions::schema::ReferenceConformancePreparation::from_selected_source_observed(
                                element.class(), name, target_class, source.owner, source.declared, observation,
                            ),
                            None => intersect_model::definitions::schema::ReferenceConformancePreparation::new_observed(
                                element.class(), name, target_class, observation,
                            ),
                        }.map_err(|error| error.to_string())?);
                        Ok(())
                    })?;
                    if !found {
                        return Err(format!("Missing reference target {id}"));
                    }
                    let preparation = preparation.ok_or("Missing reference target class projection")?;
                    reserve_names(observation, 0, &[id])?;
                    *pending_reference = Some((id.into(), preparation));
                } else {
                    // Precharge the borrowed adapter projection before retaining
                    // one immutable JSON value for the shared cursor.
                    *pending_attribute = Some((value.to_json_observed(observation)?, Box::new(
                        intersect_model::definitions::schema::RuntimeAttributeValidationPreparation::from_prepared_with_sources(
                            element.class(), field, prepared_fields.lookup(), observation, &NoAttributeSourceInputs,
                        ).map_err(|error| error.to_string())?,
                    )));
                }
                Ok(())
            };

            check_value(value)
        })?;
        if !found {
            return Err("Missing structural object".into());
        }
        if feature_stage {
            if feature_done {
                if !field.derived {
                    self.stage = Stage::Feature;
                }
                self.feature += 1;
            } else {
                self.atom = 0;
                if self.shape.as_ref().is_some_and(|shape| !shape.complete()) {
                    self.stage = Stage::Shape;
                } else {
                    self.shape = None;
                    self.stage = Stage::Value;
                }
            }
        } else if value_stage {
            if self.reference.is_some() {
                self.stage = Stage::Reference;
            } else if self.attribute.is_some() {
                self.stage = Stage::Attribute;
            } else if exhausted {
                self.feature += 1;
                self.stage = Stage::Feature;
            } else {
                self.atom += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod cycle_storage_tests {
    use super::*;
    use intersect_model::definitions::conversion::{InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};

    struct Budget {
        used: Cell<usize>,
        limit: usize,
    }
    impl InvocationObserver for Budget {
        fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
            panic!("cycle traversal calls no provider")
        }
        fn retain_storage(&self, bytes: usize) -> StorageResult<()> {
            let next = self
                .used
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limit)
                .ok_or("cycle storage denied")?;
            self.used.set(next);
            Ok(())
        }
    }

    #[test]
    fn reciprocal_query_value_precharges_utf8_and_cumulative_retention() {
        for limit in [0, 131, 132, 133] {
            let budget = Rc::new(Budget {
                used: Cell::new(0),
                limit,
            });
            let observation = InvocationContext::new(budget.clone());
            let result = reciprocal_value("é", &observation);
            if limit < 132 {
                assert!(result.is_err());
                assert_eq!(budget.used.get(), 0);
            } else {
                assert_eq!(result.unwrap(), Value::String("é".into()));
                assert_eq!(budget.used.get(), 132);
            }
        }
        for limit in [263, 264] {
            let budget = Rc::new(Budget {
                used: Cell::new(0),
                limit,
            });
            let observation = InvocationContext::new(budget.clone());
            assert!(reciprocal_value("é", &observation).is_ok());
            assert_eq!(reciprocal_value("é", &observation).is_ok(), limit == 264);
            assert_eq!(budget.used.get(), if limit == 264 { 264 } else { 132 });
        }
        assert_eq!(
            reciprocal_value("é", &InvocationContext::default()).unwrap(),
            Value::String("é".into())
        );
    }

    #[test]
    fn cycle_scratch_preflight_preserves_private_state_and_cycle_failure_precedence() {
        let model = Model::default();
        let view = ModelView::new(&model).unwrap();
        let schema = intersect_model::definitions::compiler::inspect_ecore_schema(include_str!(
            "../../../assets/fixtures/ecore-navigation/ownership.ecore"
        ))
        .unwrap();
        // Isolate traversal over already built ownership. Start r costs 130;
        // r->é and é->r cost 131 each. Revisiting r rejects without a copy.
        for limit in [0, 129, 130, 260, 261, 391, 392] {
            let budget = Rc::new(Budget {
                used: Cell::new(0),
                limit,
            });
            let observation = InvocationContext::new(budget.clone());
            let mut pending = StructuralPreparation::ownership();
            pending.stage = Stage::CycleStart;
            pending.owners.parents = [("r".into(), "é".into()), ("é".into(), "r".into())].into();
            loop {
                let before = (
                    pending.last_cycle.clone(),
                    pending.current.clone(),
                    pending.seen.clone(),
                );
                match pending.advance(&view, &schema, None, Some(&observation)) {
                    Ok(false) => {}
                    Ok(true) => panic!("cycle accepted"),
                    Err(error) => {
                        assert_eq!(
                            (
                                pending.last_cycle.clone(),
                                pending.current.clone(),
                                pending.seen.clone()
                            ),
                            before
                        );
                        assert_eq!(
                            error,
                            if limit == 392 {
                                "Containment cycle"
                            } else {
                                "cycle storage denied"
                            }
                        );
                        let expected = [0, 130, 261, 392]
                            .into_iter()
                            .filter(|n| *n <= limit)
                            .max()
                            .unwrap();
                        assert_eq!(budget.used.get(), expected);
                        assert!(
                            pending
                                .advance(&view, &schema, None, Some(&observation))
                                .is_err()
                        );
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            }
        }
        let budget = Rc::new(Budget {
            used: Cell::new(0),
            limit: 261,
        });
        let observation = InvocationContext::new(budget.clone());
        let mut recovered = StructuralPreparation::ownership();
        recovered.stage = Stage::CycleStart;
        recovered.owners.parents.insert("r".into(), "é".into());
        while !recovered
            .advance(&view, &schema, None, Some(&observation))
            .unwrap()
        {}
        assert!(recovered.seen.is_empty());
        assert_eq!(budget.used.get(), 261);
        assert_eq!(recovered.finish().unwrap().parent("r"), Some("é"));
        assert!(model.elements.is_empty());
    }
}

#[cfg(test)]
#[path = "completed_reference_tests.rs"]
mod completed_reference_tests;
#[cfg(test)]
#[path = "prepared_reference_source_tests.rs"]
mod prepared_reference_source_tests;

#[cfg(any(test, target_arch = "wasm32"))]
#[allow(dead_code)]
struct LegacyStructuralPreparation {
    stage: Stage,
    after: Option<u64>,
    object: Option<(String, String)>,
    feature: usize,
    atom: usize,
    shape: Option<FeatureShape>,
    assignment: assignment::AssignmentPreparation,
    receipt: Option<initialization::ReceiptPreparation>,
    reference: Option<(
        String,
        intersect_model::definitions::schema::ReferenceConformancePreparation,
    )>,
    conversion:
        Option<Box<intersect_model::definitions::schema::RuntimeAttributeConversionPreparation>>,
    attribute: Option<(
        Value,
        Box<intersect_model::definitions::schema::RuntimeAttributeValidationPreparation>,
    )>,
    unknown: views::FieldNames,
    schema_validation: intersect_model::definitions::schema::InstantiationValidation,
    checked_schema: Option<intersect_model::definitions::schema::CheckedRuntimeSchemaIdentity>,
    feature_inputs: Option<intersect_model::definitions::schema::SharedFeatureInputCache>,
    ordinary_schema_started: bool,
    class: Option<String>,
    member_preparation: Option<Box<intersect_model::definitions::schema::FeatureMapPreparation>>,
    cache_after: Option<String>,
    cache_index: usize,
    features: BTreeMap<String, CachedFeatures>,
    feature_order: BTreeMap<String, Vec<String>>,
    owners: OwnershipIndex,
    resource_audit: Option<crate::core::resources::ResourceAudit>,
    audit_resources: bool,
    require_lower_bounds: bool,
    references: DraftReferencePolicy,
    retain_ownership: bool,
    last_cycle: Option<String>,
    current: Option<String>,
    seen: BTreeSet<String>,
    failed: bool,
}
#[cfg(any(test, target_arch = "wasm32"))]
const _: () = {
    assert!(
        std::mem::size_of::<StructuralPreparation>()
            == std::mem::size_of::<LegacyStructuralPreparation>()
    );
    assert!(
        std::mem::align_of::<StructuralPreparation>()
            == std::mem::align_of::<LegacyStructuralPreparation>()
    );
};
