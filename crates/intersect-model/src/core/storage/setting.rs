//! Checked computed setters. Language providers own the transformation policy;
//! core applies their stored edit plan through existing atomic services.
use super::*;
use crate::core::query::{DerivedValues, QueryBudget};

/// The requested semantic operation, distinct from its stored implementation.
pub enum ComputedEdit {
    Set(Value),
    /// Append one value to a changeable many-valued computed feature.
    Append(Value),
    Unset,
}

/// A provider cannot bypass structural checks or mutate the store during planning.
pub enum SettingEdit {
    /// Create a stored object with schema defaults inside the setting transaction.
    /// The language provider supplies deterministic identity and source location.
    /// Further feature writes use ordinary Set/Unset plans; final draft validation
    /// applies to the complete plan. A separately selected source-bound service
    /// may initialize the SAME newly inserted object through its checked lease.
    Create {
        id: String,
        class: String,
        resource: String,
        start: usize,
        end: usize,
    },
    Set {
        id: String,
        feature: String,
        value: Value,
    },
    Unset {
        id: String,
        feature: String,
    },
}

/// A revision-bound, language-owned setting implementation. Read and planning must
/// use the supplied current store and charge the supplied deterministic budget.
/// Plans may only target stored features; recursive computed dispatch is explicit
/// unsupported behavior. No external side effects belong in a setting provider.
pub trait SettingValues: DerivedValues {
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        edit: &ComputedEdit,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<SettingEdit>>;
}

/// Provider selection for a checked model facade operation. The owning facade
/// supplies its schema; neither provider nor registry is retained after the call.
#[derive(Clone, Copy)]
pub struct SettingContext<'a> {
    pub provider: &'a dyn SettingValues,
    pub conversions: Option<&'a ConversionRegistry<'a>>,
}

/// Explicit providers borrowed for one operation, never installed in model state.
pub struct ComputedSettings<'a> {
    pub schema: &'a Schema,
    pub provider: &'a dyn SettingValues,
    pub conversions: Option<&'a ConversionRegistry<'a>>,
}
impl ComputedSettings<'_> {
    /// Validate the request, apply the plan atomically, then validate the computed
    /// result from the edited store. Failed plans, edits, results and budgets roll
    /// back values, opposites and assignment history. Provider budgets are consumed
    /// even on failure. Required stored multiplicities may remain incomplete drafts.
    pub fn apply(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.apply_with_reference_policy(
            store,
            id,
            name,
            edit,
            budget,
            DraftReferencePolicy::Resolved,
        )
    }

    /// Apply during a caller-owned linking transaction. Provisional validation
    /// permits only existing canonical pending non-containment references;
    /// incoming values, planned writes and computed readback remain checked.
    /// The caller must link and strictly validate before publishing the model.
    pub fn apply_with_reference_policy(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        references: DraftReferencePolicy,
    ) -> StorageResult<()> {
        self.apply_services(
            store,
            id,
            name,
            edit,
            budget,
            references,
            &NoSettingDraftServices,
            self.schema,
        )
    }

    /// Construction-only application for a parse whose graph is linked and
    /// validated afterwards: created objects use the store's plain
    /// constructor and every planned write is still the checked setter, but
    /// no whole-graph audit runs per edit (pending references are expected).
    pub fn apply_construction_only(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.apply_services(
            store,
            id,
            name,
            edit,
            budget,
            DraftReferencePolicy::PendingNonContainment,
            &ConstructionOnlySettingServices,
            self.schema,
        )
    }

    /// The same construction-only application with a prepared owner's
    /// completed runtime validation of this EXACT schema (pointer identity,
    /// as `set_with_opposite_prepared`) instead of rerunning the immutable
    /// validator per edit, and feature maps answered by `features`, a lookup
    /// over the same schema borrow. A copied/equal schema is rejected. Store
    /// revision and configuration are still bound per edit, and every
    /// planned write is still the checked setter.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_construction_only_prepared(
        &self,
        store: &mut impl ModelStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        features: &dyn FeatureLookup,
    ) -> StorageResult<()> {
        self.apply_services(
            store,
            id,
            name,
            edit,
            budget,
            DraftReferencePolicy::PendingNonContainment,
            &PreparedConstructionOnlySettingServices { metadata },
            features,
        )
    }

    /// Explicit provisional service for native store construction.
    /// Ordinary validation/default creation remain on the disabled route.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_with_draft_construction(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        construction: &DraftConstructionContext<'_>,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
    ) -> StorageResult<()> {
        budget.charge()?;
        if !store.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        budget.charge()?;
        let mut resource = None;
        store.inspect(id, &mut |object| {
            resource = Some(object.resource().to_owned());
            Ok(())
        })?;
        if resource.as_deref() != Some(source.0) {
            return Err("Computed draft setting owner belongs to another source".into());
        }
        self.apply_services(
            store,
            id,
            name,
            edit,
            budget,
            DraftReferencePolicy::PendingNonContainment,
            &CheckedSettingDraftServices {
                construction,
                metadata,
                elements_per_resource,
                resource: source.0,
                source_text: source.1,
            },
            self.schema,
        )
    }

    /// Explicit invocation-borrowed metadata reuse for the same provisional
    /// setting service. Every intermediate construction and final whole-graph
    /// audit remains fresh. The supplied source lease retains only immutable
    /// contract/member metadata under its existing logical storage policy;
    /// its admission and all work use the caller's supplied QueryBudget.
    /// No graph, readback, semantic result or new work allowance is retained.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_with_draft_construction_source(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        construction: &DraftConstructionContext<'_>,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        source_lease: &DraftConstructionSource<'_, '_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
    ) -> StorageResult<()> {
        self.apply_draft_source_context(
            store,
            id,
            name,
            edit,
            budget,
            construction,
            metadata,
            source_lease,
            elements_per_resource,
            source,
            None,
        )
    }

    /// Opt-in creation initialization for the same checked source-bound setting
    /// transaction. Existing operations without a selector keep their old path.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_with_draft_construction_source_and_initializers(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        construction: &DraftConstructionContext<'_>,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        source_lease: &DraftConstructionSource<'_, '_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
        initializers: &dyn ClassInitializationSelector,
    ) -> StorageResult<()> {
        self.apply_draft_source_context(
            store,
            id,
            name,
            edit,
            budget,
            construction,
            metadata,
            source_lease,
            elements_per_resource,
            source,
            Some(initializers),
        )
    }

    #[allow(clippy::too_many_arguments)] // One unchanged source guard and atomic worker for both routes.
    fn apply_draft_source_context(
        &self,
        store: &mut impl DraftConstructionStore,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        construction: &DraftConstructionContext<'_>,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        source_lease: &DraftConstructionSource<'_, '_>,
        elements_per_resource: Option<usize>,
        source: (&str, &str),
        initializers: Option<&dyn ClassInitializationSelector>,
    ) -> StorageResult<()> {
        // The lease's existing live-callback tickets cover their callbacks;
        // this ticket also poisons failures/unwinds in planning or readback.
        struct Ticket<'a, 'c, 'm> {
            source: &'a DraftConstructionSource<'c, 'm>,
            complete: bool,
        }
        impl Drop for Ticket<'_, '_, '_> {
            fn drop(&mut self) {
                if !self.complete {
                    self.source.invalidate();
                }
            }
        }
        let mut ticket = Ticket {
            source: source_lease,
            complete: false,
        };
        let result = (|| {
            budget.charge()?;
            if !store.is_transaction_active() {
                return Err("Storage write requires a transaction".into());
            }
            budget.charge()?;
            let mut resource = None;
            store.inspect(id, &mut |object| {
                resource = Some(object.resource().to_owned());
                Ok(())
            })?;
            if resource.as_deref() != Some(source.0) {
                return Err("Computed draft setting owner belongs to another source".into());
            }
            let services = LeasedSettingDraftServices {
                checked: CheckedSettingDraftServices {
                    construction,
                    metadata,
                    elements_per_resource,
                    resource: source.0,
                    source_text: source.1,
                },
                source: source_lease,
            };
            match initializers {
                None => self.apply_services(
                    store,
                    id,
                    name,
                    edit,
                    budget,
                    DraftReferencePolicy::PendingNonContainment,
                    &services,
                    self.schema,
                ),
                Some(initializers) => self.apply_services(
                    store,
                    id,
                    name,
                    edit,
                    budget,
                    DraftReferencePolicy::PendingNonContainment,
                    &InitializingSettingDraftServices {
                        leased: services,
                        initializers,
                    },
                    self.schema,
                ),
            }
        })();
        ticket.complete = result.is_ok();
        result
    }

    #[allow(clippy::too_many_arguments)] // Same atomic edit worker with transient construction services.
    fn apply_services<S: ModelStore, A: SettingDraftServices<S>>(
        &self,
        store: &mut S,
        id: &str,
        name: &str,
        edit: ComputedEdit,
        budget: &mut QueryBudget,
        references: DraftReferencePolicy,
        services: &A,
        features: &dyn FeatureLookup,
    ) -> StorageResult<()> {
        budget.charge()?;
        if !std::ptr::eq(features.schema(), self.schema) {
            return Err("Computed setting feature index belongs to another schema".into());
        }
        if !super::revision_matches(store, &self.schema.revision)? {
            return Err("Model belongs to a different schema revision".into());
        }
        let mut class = None;
        store.inspect(id, &mut |object| {
            if object.type_identity() != self.schema.identity(object.class()) {
                return Err("Model type identity does not match schema".into());
            }
            class = Some(object.class().to_owned());
            Ok(())
        })?;
        let class = class.ok_or("Missing element")?;
        let fields = features.feature_map(&class)?;
        let field = fields.get(name).ok_or("Unknown feature")?;
        if !field.derived {
            return Err("Computed setting service requires a derived feature".into());
        }
        if !field.changeable {
            return Err("Cannot write a non-changeable computed feature".into());
        }
        if self.provider.schema_revision() != Some(self.schema.revision.as_str())
            || !self.provider.supports(&class, name)
        {
            return Err("Missing or stale computed setting provider".into());
        }
        // `fields` is this schema's member selection for `class` (checked above).
        self.schema.require_runtime_attribute_conversion_prepared(
            &class,
            field,
            self.conversions,
            &*fields,
        )?;
        if let ComputedEdit::Set(value) = &edit {
            assignment::validate_computed_value(
                store,
                self.schema,
                &class,
                field,
                value,
                budget,
                self.conversions,
            )?;
        }
        if let ComputedEdit::Append(value) = &edit {
            if field.upper == 0 || field.upper == 1 {
                return Err("Computed append requires a many-valued feature".into());
            }
            // Validate the incoming atom, not the final computed collection's
            // lower bound. The unchanged readback check validates the full result.
            assignment::validate_computed_append(
                store,
                self.schema,
                &class,
                field,
                value,
                budget,
                self.conversions,
            )?;
        }
        if field.unsettable {
            self.provider.explicit_set_state(store, id, name, budget)?;
        }
        atomic(store, |store| {
            services.bind(store, self.schema)?;
            services.check(store, self.schema, budget)?;
            let plan = self.provider.plan(store, id, name, &edit, budget)?;
            for operation in plan {
                budget.charge()?;
                match operation {
                    SettingEdit::Create {
                        id,
                        class,
                        resource,
                        start,
                        end,
                    } => {
                        services.create(
                            store,
                            self.schema,
                            &id,
                            &class,
                            &resource,
                            start,
                            end,
                            budget,
                        )?;
                    }
                    SettingEdit::Set { id, feature, value } => {
                        set_inner_indexed(store, features, &id, &feature, value, self.conversions)?;
                    }
                    SettingEdit::Unset { id, feature } => match self.conversions {
                        Some(registry) => {
                            unset_with_conversions(store, self.schema, &id, &feature, registry)?
                        }
                        None => unset(store, self.schema, &id, &feature)?,
                    },
                }
            }
            services.validate(store, self.schema, self.conversions, references, budget)?;
            let value = self.provider.read(store, id, name, budget)?;
            assignment::validate_computed_value(
                store,
                self.schema,
                &class,
                field,
                &value,
                budget,
                self.conversions,
            )?;
            if field.unsettable {
                let assigned = self.provider.explicit_set_state(store, id, name, budget)?;
                let expected_assigned = !matches!(edit, ComputedEdit::Unset);
                if assigned != expected_assigned {
                    return Err(
                        "Computed setting did not establish the requested assignment state".into(),
                    );
                }
                if !assigned && value != field.default {
                    return Err("Unset computed feature differs from its default".into());
                }
            }
            services.check(store, self.schema, budget)?;
            Ok(())
        })
    }
}

trait SettingDraftServices<S: ModelStore> {
    /// Bind the live store to `schema`. By default the schema's immutable
    /// runtime validator runs here; only a service carrying a prepared
    /// owner's completed validation of the SAME schema may skip it.
    fn bind(&self, store: &mut S, schema: &Schema) -> StorageResult<()> {
        bind_schema(store, schema)
    }
    fn check(&self, store: &S, schema: &Schema, budget: &mut QueryBudget) -> StorageResult<()>;
    #[allow(clippy::too_many_arguments)]
    fn create(
        &self,
        store: &mut S,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<()>;
    fn validate(
        &self,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        references: DraftReferencePolicy,
        budget: &mut QueryBudget,
    ) -> StorageResult<()>;
}
struct NoSettingDraftServices;
impl<S: ModelStore> SettingDraftServices<S> for NoSettingDraftServices {
    fn check(&self, _: &S, _: &Schema, _: &mut QueryBudget) -> StorageResult<()> {
        Ok(())
    }
    fn create(
        &self,
        store: &mut S,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        _: &mut QueryBudget,
    ) -> StorageResult<()> {
        store.create_element(schema, id, class, resource, start, end)
    }
    fn validate(
        &self,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        references: DraftReferencePolicy,
        _: &mut QueryBudget,
    ) -> StorageResult<()> {
        let mut validation = StructuralPreparation::draft(references);
        while !validation.advance(store, schema, conversions, None)? {}
        validation.finish()?;
        Ok(())
    }
}
/// Plain creation and no per-edit graph audit; see `apply_construction_only`.
struct ConstructionOnlySettingServices;
impl<S: ModelStore> SettingDraftServices<S> for ConstructionOnlySettingServices {
    fn check(&self, _: &S, _: &Schema, _: &mut QueryBudget) -> StorageResult<()> {
        Ok(())
    }
    fn create(
        &self,
        store: &mut S,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        _: &mut QueryBudget,
    ) -> StorageResult<()> {
        store.create_element(schema, id, class, resource, start, end)
    }
    fn validate(
        &self,
        _: &S,
        _: &Schema,
        _: Option<&ConversionRegistry<'_>>,
        _: DraftReferencePolicy,
        _: &mut QueryBudget,
    ) -> StorageResult<()> {
        Ok(())
    }
}
/// `ConstructionOnlySettingServices` whose bind relies on a prepared owner's
/// completed validation of the exact schema; see `apply_construction_only_prepared`.
struct PreparedConstructionOnlySettingServices<'a, 'metadata> {
    metadata: &'a crate::definitions::preparation::PreparedPhysicalMetadata<'metadata>,
}
impl<S: ModelStore> SettingDraftServices<S> for PreparedConstructionOnlySettingServices<'_, '_> {
    fn bind(&self, store: &mut S, schema: &Schema) -> StorageResult<()> {
        if !self.metadata.matches_schema(schema) {
            return Err("Prepared computed setting requires the exact schema owner".into());
        }
        // PreparedPhysicalMetadata completed InstantiationValidation's SAME
        // stages over this schema, which its borrow keeps unchanged.
        bind_runtime_validated_schema(store, schema)
    }
    fn check(&self, store: &S, schema: &Schema, budget: &mut QueryBudget) -> StorageResult<()> {
        ConstructionOnlySettingServices.check(store, schema, budget)
    }
    fn create(
        &self,
        store: &mut S,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        ConstructionOnlySettingServices
            .create(store, schema, id, class, resource, start, end, budget)
    }
    fn validate(
        &self,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        references: DraftReferencePolicy,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        ConstructionOnlySettingServices.validate(store, schema, conversions, references, budget)
    }
}
struct CheckedSettingDraftServices<'a, 'metadata> {
    construction: &'a DraftConstructionContext<'a>,
    metadata: &'a crate::definitions::preparation::PreparedPhysicalMetadata<'metadata>,
    elements_per_resource: Option<usize>,
    resource: &'a str,
    source_text: &'a str,
}
impl CheckedSettingDraftServices<'_, '_> {
    fn check_with_source(
        &self,
        store: &impl DraftConstructionStore,
        schema: &Schema,
        budget: &mut QueryBudget,
        source: Option<&DraftConstructionSource<'_, '_>>,
    ) -> StorageResult<()> {
        budget.charge()?;
        if !std::ptr::eq(schema, self.metadata.definition().schema()) {
            return Err("Computed draft setting metadata belongs to another definition".into());
        }
        match source {
            None => self
                .construction
                .check_pending_access(store, schema, &mut || budget.charge()),
            Some(source) => {
                source.setting_access(self.construction, store, self.metadata, schema, &mut || {
                    budget.charge()
                })
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn create_with_source(
        &self,
        store: &mut impl DraftConstructionStore,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
        source: Option<&DraftConstructionSource<'_, '_>>,
    ) -> StorageResult<()> {
        self.create_with_source_initializers(
            store, schema, id, class, resource, start, end, budget, source, None,
        )
    }
    #[allow(clippy::too_many_arguments)] // Same checked create, with an explicit borrowed selector only on opt-in.
    fn create_with_source_initializers(
        &self,
        store: &mut impl DraftConstructionStore,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
        source: Option<&DraftConstructionSource<'_, '_>>,
        initializers: Option<&dyn ClassInitializationSelector>,
    ) -> StorageResult<()> {
        self.check_with_source(store, schema, budget, source)?;
        budget.charge()?;
        if resource != self.resource
            || start > end
            || end > self.source_text.len()
            || !self.source_text.is_char_boundary(start)
            || !self.source_text.is_char_boundary(end)
        {
            return Err("Computed draft construction is outside its exact source range".into());
        }
        if let Some(limit) = self.elements_per_resource {
            budget.charge()?;
            if store.query(Query::Resource(resource))?.len() >= limit {
                return Err("Element budget exceeded".into());
            }
        }
        if let Some(initializers) = initializers {
            let source =
                source.ok_or("Computed native initializer requires an actual source lease")?;
            budget.charge()?;
            let initializer = initializers.select(class, budget)?;
            // A selector is executable language authority. Recheck the SAME
            // owner/source after its callback before inserting the fresh object.
            self.check_with_source(store, schema, budget, Some(source))?;
            if let Some(initializer) = initializer {
                source.create_pending_with_initializer(
                    store,
                    Construction {
                        id,
                        resource,
                        authored: &[],
                    },
                    class,
                    start,
                    end,
                    &initializer,
                    budget,
                )?;
                return self.check_with_source(store, schema, budget, Some(source));
            }
        }
        match source {
            None => self.construction.create_pending(
                store,
                Construction {
                    id,
                    resource,
                    authored: &[],
                },
                class,
                start,
                end,
                self.metadata,
                budget,
            )?,
            Some(source) => source.create_pending(
                store,
                Construction {
                    id,
                    resource,
                    authored: &[],
                },
                class,
                start,
                end,
                budget,
            )?,
        };
        self.check_with_source(store, schema, budget, source)
    }
    fn validate_with_source(
        &self,
        store: &impl DraftConstructionStore,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        references: DraftReferencePolicy,
        budget: &mut QueryBudget,
        source: Option<&DraftConstructionSource<'_, '_>>,
    ) -> StorageResult<()> {
        self.check_with_source(store, schema, budget, source)?;
        let mut validation = StructuralPreparation::draft(references);
        loop {
            let complete = match source {
                None => validation.advance_construction_with_conversions(
                    store,
                    schema,
                    conversions,
                    self.construction,
                    budget,
                )?,
                Some(source) => validation.advance_construction_with_sources_and_conversions(
                    store,
                    schema,
                    conversions,
                    self.construction,
                    source,
                    budget,
                )?,
            };
            if complete {
                break;
            }
        }
        validation.finish()?;
        // This constructor-shaped pending audit is not final graph authority.
        self.check_with_source(store, schema, budget, source)
    }
}

impl<S: DraftConstructionStore> SettingDraftServices<S> for CheckedSettingDraftServices<'_, '_> {
    fn check(&self, store: &S, schema: &Schema, budget: &mut QueryBudget) -> StorageResult<()> {
        self.check_with_source(store, schema, budget, None)
    }
    fn create(
        &self,
        store: &mut S,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.create_with_source(store, schema, id, class, resource, start, end, budget, None)
    }
    fn validate(
        &self,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        references: DraftReferencePolicy,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.validate_with_source(store, schema, conversions, references, budget, None)
    }
}
struct LeasedSettingDraftServices<'a, 'c, 'm, 's> {
    checked: CheckedSettingDraftServices<'a, 'm>,
    source: &'a DraftConstructionSource<'c, 's>,
}
impl<S: DraftConstructionStore> SettingDraftServices<S>
    for LeasedSettingDraftServices<'_, '_, '_, '_>
{
    fn check(&self, store: &S, schema: &Schema, budget: &mut QueryBudget) -> StorageResult<()> {
        self.checked
            .check_with_source(store, schema, budget, Some(self.source))
    }
    fn create(
        &self,
        store: &mut S,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.checked.create_with_source(
            store,
            schema,
            id,
            class,
            resource,
            start,
            end,
            budget,
            Some(self.source),
        )
    }
    fn validate(
        &self,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        references: DraftReferencePolicy,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.checked.validate_with_source(
            store,
            schema,
            conversions,
            references,
            budget,
            Some(self.source),
        )
    }
}

// Additive service: old leased/default carriers keep their fields and exact
// disabled route. Only this borrowed wrapper can select native initialization.
struct InitializingSettingDraftServices<'a, 'c, 'm, 's> {
    leased: LeasedSettingDraftServices<'a, 'c, 'm, 's>,
    initializers: &'a dyn ClassInitializationSelector,
}
impl<S: DraftConstructionStore> SettingDraftServices<S>
    for InitializingSettingDraftServices<'_, '_, '_, '_>
{
    fn check(&self, store: &S, schema: &Schema, budget: &mut QueryBudget) -> StorageResult<()> {
        self.leased.check(store, schema, budget)
    }
    fn create(
        &self,
        store: &mut S,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.leased.checked.create_with_source_initializers(
            store,
            schema,
            id,
            class,
            resource,
            start,
            end,
            budget,
            Some(self.leased.source),
            Some(self.initializers),
        )
    }
    fn validate(
        &self,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        references: DraftReferencePolicy,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.leased
            .validate(store, schema, conversions, references, budget)
    }
}

#[cfg(test)]
#[path = "setting_source_tests.rs"]
mod source_tests;

#[cfg(test)]
#[path = "setting_prepared_tests.rs"]
mod prepared_tests;
