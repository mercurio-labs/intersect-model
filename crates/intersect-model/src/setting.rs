//! Language-owned executable selection for source-bound computed setting contracts.
//! Explicit provider execution remains separate from serialized grammar admission.
use intersect_model::core::{
    query::{DerivedValues, QueryBudget},
    storage::{
        ComputedEdit, ComputedSettings, DraftConstructionStore, ModelRead, ModelStore, SettingEdit,
        SettingValues,
    },
};
use intersect_model::definitions::{
    bundle::DefinitionBundle,
    conversion::ConversionRegistry,
    schema::Schema,
    setting::{ComputedSettingContract, SettingOperation},
};
use serde_json::Value;

pub trait SourceSettingValues: SettingValues {
    fn contract(&self) -> &ComputedSettingContract;
}

/// Installed adapter for source-authored writable Rcore projections. Construct
/// only after recompiling the package from its original sources. This adapter
/// checks the selected contract and delegates all reads and plans to PackageDerived.
pub struct RcoreProjection<'a> {
    package: crate::definitions::model_definition::ModelDefinitionView<'a>,
    contract: &'a ComputedSettingContract,
}
impl<'a> RcoreProjection<'a> {
    pub fn new(
        package: impl Into<crate::definitions::model_definition::ModelPackageView<'a>>,
        bundle: &DefinitionBundle,
        contract: &'a ComputedSettingContract,
    ) -> Result<Self, String> {
        use intersect_model::definitions::schema::Expression;
        let retained = package.into();
        let package = retained.definition;
        contract.validate(package.schema(), bundle)?;
        if contract.provider.name != "rcore.projection" || contract.provider.version != "1" {
            return Err("Unsupported Rcore projection provider".into());
        }
        let behavior = retained
            .behavior
            .ok_or("Rcore projection requires an explicit source behavior selection")?;
        if !contract.sources.contains(behavior) || !bundle.sources.iter().any(|s| &s.id == behavior)
        {
            return Err("Rcore projection contract omits its selected behavior source".into());
        }
        let rules = package
            .semantics
            .derived
            .iter()
            .filter(|r| {
                r.feature == contract.feature && package.schema.conforms(&contract.class, &r.class)
            })
            .collect::<Vec<_>>();
        if rules.len() != 1
            || !matches!(
                rules[0].expression,
                Expression::Stored { writable: true, .. }
            )
        {
            return Err("Rcore projection requires one source-authored writable projection".into());
        }
        Ok(Self { package, contract })
    }
}
impl SourceSettingValues for RcoreProjection<'_> {
    fn contract(&self) -> &ComputedSettingContract {
        self.contract
    }
}
impl DerivedValues for RcoreProjection<'_> {
    fn schema_revision(&self) -> Option<&str> {
        Some(&self.package.schema.revision)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        class == self.contract.class
            && feature == self.contract.feature
            && crate::query::ModelDerived(self.package).supports(class, feature)
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        crate::query::ModelDerived(self.package).read(store, id, feature, budget)
    }
    fn explicit_set_state(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<bool, String> {
        crate::query::ModelDerived(self.package).explicit_set_state(store, id, feature, budget)
    }
}
impl SettingValues for RcoreProjection<'_> {
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        edit: &ComputedEdit,
        budget: &mut QueryBudget,
    ) -> Result<Vec<SettingEdit>, String> {
        crate::query::ModelDerived(self.package).plan(store, id, feature, edit, budget)
    }
}

/// Borrowed, explicit selection; serialized metadata never selects executable code.
pub struct SourceBoundSettings<'a> {
    pub schema: &'a Schema,
    pub bundle: &'a DefinitionBundle,
    pub expected: &'a ComputedSettingContract,
    pub provider: &'a dyn SourceSettingValues,
    pub conversions: Option<&'a ConversionRegistry<'a>>,
}
impl SourceBoundSettings<'_> {
    pub fn apply(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
    ) -> Result<(), String> {
        self.apply_with_reference_policy(
            store,
            id,
            edit,
            budget,
            intersect_model::core::storage::DraftReferencePolicy::Resolved,
        )
    }

    /// Execute inside a caller-owned linking transaction. Pending references are
    /// permitted only by the shared core policy; callers must link and strictly
    /// validate before publication. Source/provider identity checks remain exact.
    pub fn apply_with_reference_policy(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        references: intersect_model::core::storage::DraftReferencePolicy,
    ) -> Result<(), String> {
        self.apply_mode(
            store,
            id,
            edit,
            budget,
            references,
            &DefaultSourceApplication,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_with_draft_construction(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        construction: &intersect_model::storage::DraftConstructionContext<'_>,
        metadata: &intersect_model::definitions::preparation::PreparedPhysicalMetadata<'_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
    ) -> Result<(), String> {
        self.apply_mode(
            store,
            id,
            edit,
            budget,
            intersect_model::storage::DraftReferencePolicy::PendingNonContainment,
            &DraftSourceApplication {
                construction,
                metadata,
                elements_per_resource,
                source,
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_with_draft_construction_source_and_initializers(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        construction: &intersect_model::storage::DraftConstructionContext<'_>,
        metadata: &intersect_model::definitions::preparation::PreparedPhysicalMetadata<'_>,
        source_lease: &intersect_model::storage::DraftConstructionSource<'_, '_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
        initializers: &dyn intersect_model::storage::ClassInitializationSelector,
    ) -> Result<(), String> {
        self.apply_mode(
            store,
            id,
            edit,
            budget,
            intersect_model::storage::DraftReferencePolicy::PendingNonContainment,
            &InitializingDraftSourceApplication {
                base: DraftSourceApplication {
                    construction,
                    metadata,
                    elements_per_resource,
                    source,
                },
                source_lease,
                initializers,
            },
        )
    }

    fn apply_mode<S: ModelStore, A: SourceSettingApplication<S>>(
        &self,
        store: &mut S,
        id: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        references: intersect_model::storage::DraftReferencePolicy,
        application: &A,
    ) -> Result<(), String> {
        self.apply_contract_mode(store, id, edit, budget, references, application, false)
    }

    /// `contract_validated`: the caller's borrow already validated this exact
    /// contract against this exact schema and bundle (`CheckedSourceSettings::bind`);
    /// validation is a pure function of the three, so it is not repeated.
    /// The same work is still charged.
    #[allow(clippy::too_many_arguments)]
    fn apply_contract_mode<S: ModelStore, A: SourceSettingApplication<S>>(
        &self,
        store: &mut S,
        id: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        references: intersect_model::storage::DraftReferencePolicy,
        application: &A,
        contract_validated: bool,
    ) -> Result<(), String> {
        // Charge source traversal before digest verification, as initialization does.
        for source in &self.bundle.sources {
            budget.charge()?;
            for _ in 0..source.text.len().div_ceil(1024) {
                budget.charge()?;
            }
        }
        for _ in &self.expected.sources {
            budget.charge()?;
        }
        for _ in &self.expected.operations {
            budget.charge()?;
        }
        if !contract_validated {
            self.expected.validate(self.schema, self.bundle)?;
        }
        let operation = match &edit {
            ComputedEdit::Set(_) => SettingOperation::Set,
            ComputedEdit::Append(_) => SettingOperation::Append,
            ComputedEdit::Unset => SettingOperation::Unset,
        };
        if !self.expected.operations.contains(&operation) {
            return Err("Computed setting operation is not declared".into());
        }
        if store.class(id)?.as_deref() != Some(self.expected.class.as_str()) {
            return Err("Computed setting receiver differs from its contract".into());
        }
        let guard = BoundProvider {
            expected: self.expected,
            provider: self.provider,
        };
        guard.check()?;
        application.apply(
            &ComputedSettings {
                schema: self.schema,
                provider: &guard,
                conversions: self.conversions,
            },
            store,
            id,
            &self.expected.feature,
            edit,
            budget,
            references,
        )
    }
}
struct BoundProvider<'a> {
    expected: &'a ComputedSettingContract,
    provider: &'a dyn SourceSettingValues,
}
impl BoundProvider<'_> {
    fn check(&self) -> Result<(), String> {
        if self.provider.contract() != self.expected {
            return Err("Computed setting provider contract mismatch".into());
        }
        Ok(())
    }
}
impl DerivedValues for BoundProvider<'_> {
    fn schema_revision(&self) -> Option<&str> {
        self.provider.schema_revision()
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        class == self.expected.class
            && feature == self.expected.feature
            && self.provider.supports(class, feature)
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        self.check()?;
        let result = self.provider.read(store, id, feature, budget)?;
        self.check()?;
        Ok(result)
    }
    fn explicit_set_state(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<bool, String> {
        self.check()?;
        let result = self
            .provider
            .explicit_set_state(store, id, feature, budget)?;
        self.check()?;
        Ok(result)
    }
}
impl SettingValues for BoundProvider<'_> {
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        edit: &ComputedEdit,
        budget: &mut QueryBudget,
    ) -> Result<Vec<SettingEdit>, String> {
        self.check()?;
        let result = self.provider.plan(store, id, feature, edit, budget)?;
        self.check()?;
        Ok(result)
    }
}

/// Explicit executable selection for one immutable package borrow. Metadata never
/// installs a provider. Recreate this selection after replacing/replaying a package.
// Handwritten transient execution adapters. The disabled route invokes the exact
// prior service; no context is installed into the stored model or provider.
trait SourceSettingApplication<S: ModelStore> {
    #[allow(clippy::too_many_arguments)]
    fn apply(
        &self,
        settings: &ComputedSettings<'_>,
        store: &mut S,
        id: &str,
        feature: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        references: intersect_model::storage::DraftReferencePolicy,
    ) -> Result<(), String>;
}
struct DefaultSourceApplication;
impl<S: ModelStore> SourceSettingApplication<S> for DefaultSourceApplication {
    fn apply(
        &self,
        settings: &ComputedSettings<'_>,
        store: &mut S,
        id: &str,
        feature: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        references: intersect_model::storage::DraftReferencePolicy,
    ) -> Result<(), String> {
        settings.apply_with_reference_policy(store, id, feature, edit, budget, references)
    }
}
struct ConstructionOnlySourceApplication;
impl<S: ModelStore> SourceSettingApplication<S> for ConstructionOnlySourceApplication {
    fn apply(
        &self,
        settings: &ComputedSettings<'_>,
        store: &mut S,
        id: &str,
        feature: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        _: intersect_model::storage::DraftReferencePolicy,
    ) -> Result<(), String> {
        settings.apply_construction_only(store, id, feature, edit, budget)
    }
}
struct PreparedConstructionOnlySourceApplication<'call, 'metadata> {
    metadata: &'call intersect_model::definitions::preparation::PreparedPhysicalMetadata<'metadata>,
    features: &'call dyn intersect_model::definitions::schema::FeatureLookup,
}
impl<S: ModelStore> SourceSettingApplication<S>
    for PreparedConstructionOnlySourceApplication<'_, '_>
{
    fn apply(
        &self,
        settings: &ComputedSettings<'_>,
        store: &mut S,
        id: &str,
        feature: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        _: intersect_model::storage::DraftReferencePolicy,
    ) -> Result<(), String> {
        settings.apply_construction_only_prepared(
            store,
            id,
            feature,
            edit,
            budget,
            self.metadata,
            self.features,
        )
    }
}
struct DraftSourceApplication<'call, 'definition, 'metadata> {
    construction: &'call intersect_model::storage::DraftConstructionContext<'definition>,
    metadata: &'call intersect_model::definitions::preparation::PreparedPhysicalMetadata<'metadata>,
    elements_per_resource: Option<usize>,
    source: (&'call str, &'call str),
}
impl<S: DraftConstructionStore> SourceSettingApplication<S> for DraftSourceApplication<'_, '_, '_> {
    fn apply(
        &self,
        settings: &ComputedSettings<'_>,
        store: &mut S,
        id: &str,
        feature: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        _: intersect_model::storage::DraftReferencePolicy,
    ) -> Result<(), String> {
        settings.apply_with_draft_construction(
            store,
            id,
            feature,
            edit,
            budget,
            self.construction,
            self.metadata,
            self.elements_per_resource,
            self.source,
        )
    }
}

struct InitializingDraftSourceApplication<'a, 'd, 'm, 'c, 's> {
    base: DraftSourceApplication<'a, 'd, 'm>,
    source_lease: &'a intersect_model::storage::DraftConstructionSource<'c, 's>,
    initializers: &'a dyn intersect_model::storage::ClassInitializationSelector,
}
impl<S: DraftConstructionStore> SourceSettingApplication<S>
    for InitializingDraftSourceApplication<'_, '_, '_, '_, '_>
{
    fn apply(
        &self,
        settings: &ComputedSettings<'_>,
        store: &mut S,
        id: &str,
        feature: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        _: intersect_model::storage::DraftReferencePolicy,
    ) -> Result<(), String> {
        settings.apply_with_draft_construction_source_and_initializers(
            store,
            id,
            feature,
            edit,
            budget,
            self.base.construction,
            self.base.metadata,
            self.source_lease,
            self.base.elements_per_resource,
            self.base.source,
            self.initializers,
        )
    }
}

/// Checked source-provider selection over one schema and immutable contract set.
/// Construction still requires its separately checked context and supported store.
pub struct CheckedSourceSettings<'a> {
    schema: &'a Schema,
    contracts: &'a [ComputedSettingContract],
    bundle: &'a DefinitionBundle,
    providers: std::collections::BTreeMap<(String, String), &'a dyn SourceSettingValues>,
}
impl<'a> CheckedSourceSettings<'a> {
    pub fn bind(
        schema: &'a Schema,
        bundle: &'a DefinitionBundle,
        contracts: &'a [ComputedSettingContract],
        providers: &[&'a dyn SourceSettingValues],
    ) -> Result<Self, String> {
        let mut selected = std::collections::BTreeMap::new();
        // Validated at the first contract, as before, and then once only.
        let mut validated = None;
        for provider in providers {
            let contract = provider.contract();
            if !contracts.contains(contract) {
                return Err("Text setting provider has no exact grammar contract".into());
            }
            let validated = match validated {
                Some(validated) => validated,
                None => *validated.insert(bundle.validated()?),
            };
            contract.validate_in(schema, &validated)?;
            if selected
                .insert(
                    (contract.class.clone(), contract.feature.clone()),
                    *provider,
                )
                .is_some()
            {
                return Err("Duplicate text setting provider".into());
            }
        }
        if selected.len() != contracts.len() {
            return Err("Missing text setting provider".into());
        }
        Ok(Self {
            schema,
            contracts,
            bundle,
            providers: selected,
        })
    }
    #[doc(hidden)]
    pub fn belongs_to(&self, schema: &Schema, bundle: &DefinitionBundle) -> bool {
        std::ptr::eq(self.schema, schema) && std::ptr::eq(self.bundle, bundle)
    }
    #[doc(hidden)]
    pub fn supports(&self, class: &str, feature: &str) -> bool {
        self.providers
            .contains_key(&(class.to_owned(), feature.to_owned()))
    }
    /// Exact source/member/operation selection, not an independent setter.
    #[doc(hidden)]
    pub fn admits(
        &self,
        class: &str,
        feature: &str,
        member: &intersect_model::definitions::schema::MemberIdentity,
        append: bool,
    ) -> Result<bool, String> {
        if !self.supports(class, feature) {
            return Ok(false);
        }
        let contract = self
            .contracts
            .iter()
            .find(|c| c.class == class && c.feature == feature)
            .ok_or("Missing text setting contract")?;
        if &self.schema.effective_feature(class, feature)?.identity != member {
            return Err("Computed setting source member mismatch".into());
        }
        let operation = if append {
            SettingOperation::Append
        } else {
            SettingOperation::Set
        };
        if !contract.operations.contains(&operation) {
            return Err("Computed setting operation is not declared".into());
        }
        Ok(true)
    }
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        target: (&str, &str),
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        conversions: Option<&ConversionRegistry<'_>>,
        construction: &intersect_model::storage::DraftConstructionContext<'_>,
        metadata: &intersect_model::definitions::preparation::PreparedPhysicalMetadata<'_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
    ) -> Result<(), String> {
        self.selected(target, conversions)?
            .apply_with_draft_construction(
                store,
                id,
                edit,
                budget,
                construction,
                metadata,
                elements_per_resource,
                source,
            )
    }
    /// Construction-only application into any store, for a parse that is
    /// linked and validated afterwards: computed-created objects use the
    /// store's plain constructor, planned writes stay checked, and references
    /// may stay pending (no per-edit graph audit).
    #[doc(hidden)]
    pub fn apply_plain(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        target: (&str, &str),
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        self.selected(target, conversions)?.apply_mode(
            store,
            id,
            edit,
            budget,
            intersect_model::storage::DraftReferencePolicy::PendingNonContainment,
            &ConstructionOnlySourceApplication,
        )
    }
    /// `apply_plain` for a parse holding this definition's prepared physical
    /// metadata and a feature lookup over the same schema: the immutable
    /// schema validation the metadata completed is not rerun per edit, and
    /// feature maps come from the lookup. The selected contract, which `bind`
    /// validated against the same borrowed schema and sources, is not
    /// revalidated (its work is still charged). Every other check is unchanged.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn apply_plain_prepared(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        target: (&str, &str),
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        conversions: Option<&ConversionRegistry<'_>>,
        metadata: &intersect_model::definitions::preparation::PreparedPhysicalMetadata<'_>,
        features: &dyn intersect_model::definitions::schema::FeatureLookup,
    ) -> Result<(), String> {
        // `bind` validated every grammar contract (one per class/feature),
        // and `selected` returns one of them with the same schema and bundle.
        self.selected(target, conversions)?.apply_contract_mode(
            store,
            id,
            edit,
            budget,
            intersect_model::storage::DraftReferencePolicy::PendingNonContainment,
            &PreparedConstructionOnlySourceApplication { metadata, features },
            true,
        )
    }
    /// Opt-in source-owned constructor selection for computed-created objects.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn apply_with_source_initializers(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        target: (&str, &str),
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        conversions: Option<&ConversionRegistry<'_>>,
        construction: &intersect_model::storage::DraftConstructionContext<'_>,
        metadata: &intersect_model::definitions::preparation::PreparedPhysicalMetadata<'_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
        source_lease: &intersect_model::storage::DraftConstructionSource<'_, '_>,
        initializers: &dyn intersect_model::storage::ClassInitializationSelector,
    ) -> Result<(), String> {
        self.selected(target, conversions)?
            .apply_with_draft_construction_source_and_initializers(
                store,
                id,
                edit,
                budget,
                construction,
                metadata,
                source_lease,
                elements_per_resource,
                source,
                initializers,
            )
    }
    /// Read through the same checked provider selection used by model writes.
    pub fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        class: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        let selected = self.selected((class, feature), None)?;
        if store.class(id)?.as_deref() != Some(class) {
            return Err("Text setting receiver differs from its contract".into());
        }
        BoundProvider {
            expected: selected.expected,
            provider: selected.provider,
        }
        .read(store, id, feature, budget)
    }
    /// Apply an ordinary checked edit with explicit pending-reference policy.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_checked(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        target: (&str, &str),
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        self.selected(target, conversions)?
            .apply_with_reference_policy(
                store,
                id,
                edit,
                budget,
                intersect_model::storage::DraftReferencePolicy::PendingNonContainment,
            )
    }
    fn selected<'s>(
        &'s self,
        target: (&str, &str),
        conversions: Option<&'s ConversionRegistry<'s>>,
    ) -> Result<SourceBoundSettings<'s>, String> {
        let (class, feature) = target;
        let provider = self
            .providers
            .get(&(class.to_owned(), feature.to_owned()))
            .ok_or("Missing text setting provider")?;
        let expected = self
            .contracts
            .iter()
            .find(|c| c.class == class && c.feature == feature)
            .ok_or("Missing text setting contract")?;
        Ok(SourceBoundSettings {
            schema: self.schema,
            bundle: self.bundle,
            expected,
            provider: *provider,
            conversions,
        })
    }
}
