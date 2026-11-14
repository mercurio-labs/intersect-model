//! Handwritten test-suite root (not generated): merges the intersect-model integration tests into one
//! test binary so the suite builds and links once. Each module is a former test
//! binary of the same name; its tests are named `<module>::<test>`.
//! Modules are `pub` so items a former binary exported stay live, and shared
//! helper files are loaded once per former binary, as before the merge.
#![allow(clippy::duplicate_mod)]

mod declaration_inventory;
mod model_definition;
mod model_fixture;

#[path = "checked_normalization_consumers.rs"]
pub mod checked_normalization_consumers;

#[path = "diagnostic_provenance.rs"]
pub mod diagnostic_provenance;

#[path = "draft_null_defaults.rs"]
pub mod draft_null_defaults;

#[path = "draft_physical_defaults.rs"]
pub mod draft_physical_defaults;

#[path = "frozen_defaults.rs"]
pub mod frozen_defaults;

#[path = "frozen_engine.rs"]
pub mod frozen_engine;

#[path = "frozen_implicit.rs"]
pub mod frozen_implicit;

#[path = "frozen_prepared_load.rs"]
pub mod frozen_prepared_load;

#[path = "frozen_scopes.rs"]
pub mod frozen_scopes;

#[path = "frozen_snapshot.rs"]
pub mod frozen_snapshot;

#[path = "frozen_validate.rs"]
pub mod frozen_validate;

#[path = "generic_source_host.rs"]
pub mod generic_source_host;

#[path = "input_string_transport_qualification.rs"]
pub mod input_string_transport_qualification;

#[path = "legacy_agreement_host_consumers.rs"]
pub mod legacy_agreement_host_consumers;

#[path = "memory_resource_count.rs"]
pub mod memory_resource_count;

#[path = "model_rule_service.rs"]
pub mod model_rule_service;

#[path = "prepared_opposite_setter.rs"]
pub mod prepared_opposite_setter;

mod package_load;
mod project_usages;

#[path = "provisional_native_initializers.rs"]
pub mod provisional_native_initializers;

#[path = "query_budget_batch.rs"]
pub mod query_budget_batch;

#[path = "query_conversions.rs"]
pub mod query_conversions;

#[path = "rcore_api.rs"]
pub mod rcore_api;

#[path = "rcore_classified_reads.rs"]
pub mod rcore_classified_reads;

#[path = "rcore_feature_naming.rs"]
pub mod rcore_feature_naming;

#[path = "rcore_getter_composition.rs"]
pub mod rcore_getter_composition;

#[path = "rcore_initializers.rs"]
pub mod rcore_initializers;

#[path = "rcore_migration.rs"]
pub mod rcore_migration;

#[path = "rcore_model_rules.rs"]
pub mod rcore_model_rules;

#[path = "rcore_prelude.rs"]
pub mod rcore_prelude;

#[path = "rcore_primitives.rs"]
pub mod rcore_primitives;

#[path = "rcore_provider_initializers.rs"]
pub mod rcore_provider_initializers;

#[path = "rcore_read_functions.rs"]
pub mod rcore_read_functions;

#[path = "rcore_reference_getters.rs"]
pub mod rcore_reference_getters;

#[path = "rcore_retained_json.rs"]
pub mod rcore_retained_json;

#[path = "rcore_source_getters.rs"]
pub mod rcore_source_getters;

#[path = "rcore_string_concat.rs"]
pub mod rcore_string_concat;

#[path = "rcore_structural_extensions.rs"]
pub mod rcore_structural_extensions;

#[path = "rcore_type_views.rs"]
pub mod rcore_type_views;

#[path = "resource_conversion_context.rs"]
pub mod resource_conversion_context;

#[path = "resource_structural_adoption.rs"]
pub mod resource_structural_adoption;

#[path = "resource_structural_values.rs"]
pub mod resource_structural_values;

#[path = "retained_numeric_payload.rs"]
pub mod retained_numeric_payload;

#[path = "retained_provider_validation.rs"]
pub mod retained_provider_validation;

#[path = "semantic_inverse_nominal.rs"]
pub mod semantic_inverse_nominal;

#[path = "semantic_read_policy.rs"]
pub mod semantic_read_policy;

#[path = "source_identity.rs"]
pub mod source_identity;

#[path = "source_inventory_host.rs"]
pub mod source_inventory_host;

#[path = "structural_substitution_consumers.rs"]
pub mod structural_substitution_consumers;

#[path = "xmi_conversions.rs"]
pub mod xmi_conversions;

#[path = "xmi_delegated_collections.rs"]
pub mod xmi_delegated_collections;

#[path = "xmi_double.rs"]
pub mod xmi_double;

#[path = "xmi_readonly_initialization.rs"]
pub mod xmi_readonly_initialization;

#[path = "xmi_frozen.rs"]
pub mod xmi_frozen;

pub(crate) fn handle_id(
    store: &intersect_model::frozen::FrozenStore,
    handle: &intersect_model::storage::ObjectHandle,
) -> Result<String, String> {
    let mut id = String::new();
    store.inspect_handle(handle, &mut |object| {
        object.id().clone_into(&mut id);
        Ok(())
    })?;
    Ok(id)
}
