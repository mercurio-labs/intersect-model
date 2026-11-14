//! Consolidated qualification suite.
#![allow(clippy::duplicate_mod)]

#[path = "definitions/attribute_conversion_cursor.rs"]
pub mod definitions_attribute_conversion_cursor;

#[path = "definitions/class_key_cursor.rs"]
pub mod definitions_class_key_cursor;

#[path = "definitions/concrete_operation_bridge.rs"]
pub mod definitions_concrete_operation_bridge;

#[path = "definitions/concrete_operation_identity.rs"]
pub mod definitions_concrete_operation_identity;

#[path = "definitions/conformance_cursor.rs"]
pub mod definitions_conformance_cursor;

#[path = "definitions/constructor_conversion.rs"]
pub mod definitions_constructor_conversion;

#[path = "definitions/conversion_relations.rs"]
pub mod definitions_conversion_relations;

#[path = "definitions/datatype_applications.rs"]
pub mod definitions_datatype_applications;

#[path = "definitions/dynamic.rs"]
pub mod definitions_dynamic;

#[path = "definitions/ecore_annotations.rs"]
pub mod definitions_ecore_annotations;

#[path = "definitions/ecore_big_integer.rs"]
pub mod definitions_ecore_big_integer;

#[path = "definitions/ecore_binary.rs"]
pub mod definitions_ecore_binary;

#[path = "definitions/ecore_datatypes.rs"]
pub mod definitions_ecore_datatypes;

#[path = "definitions/ecore_date.rs"]
pub mod definitions_ecore_date;

#[path = "definitions/ecore_decimal.rs"]
pub mod definitions_ecore_decimal;

#[path = "definitions/ecore_document.rs"]
pub mod definitions_ecore_document;

#[path = "definitions/ecore_enums.rs"]
pub mod definitions_ecore_enums;

#[path = "definitions/ecore_floats.rs"]
pub mod definitions_ecore_floats;

#[path = "definitions/ecore_generic_agreement.rs"]
pub mod definitions_ecore_generic_agreement;

#[path = "definitions/ecore_generic_argument_parity.rs"]
pub mod definitions_ecore_generic_argument_parity;

#[path = "definitions/ecore_generic_conformance.rs"]
pub mod definitions_ecore_generic_conformance;

#[path = "definitions/ecore_generic_inheritance.rs"]
pub mod definitions_ecore_generic_inheritance;

#[path = "definitions/ecore_generic_members.rs"]
pub mod definitions_ecore_generic_members;

#[path = "definitions/ecore_generic_operations.rs"]
pub mod definitions_ecore_generic_operations;

#[path = "definitions/ecore_generic_overrides.rs"]
pub mod definitions_ecore_generic_overrides;

#[path = "definitions/ecore_generic_resolution.rs"]
pub mod definitions_ecore_generic_resolution;

#[path = "definitions/ecore_generic_schema.rs"]
pub mod definitions_ecore_generic_schema;

#[path = "definitions/ecore_generic_types.rs"]
pub mod definitions_ecore_generic_types;

#[path = "definitions/ecore_generics.rs"]
pub mod definitions_ecore_generics;

#[path = "definitions/ecore_import.rs"]
pub mod definitions_ecore_import;

#[path = "definitions/ecore_inheritance.rs"]
pub mod definitions_ecore_inheritance;

#[path = "definitions/ecore_members.rs"]
pub mod definitions_ecore_members;

#[path = "definitions/ecore_mercurio_enums.rs"]
pub mod definitions_ecore_mercurio_enums;

#[path = "definitions/ecore_observation.rs"]
pub mod definitions_ecore_observation;

#[path = "definitions/ecore_operations.rs"]
pub mod definitions_ecore_operations;

#[path = "definitions/ecore_parse_limits.rs"]
pub mod definitions_ecore_parse_limits;

#[path = "definitions/ecore_resources.rs"]
pub mod definitions_ecore_resources;

#[path = "definitions/ecore_scalars.rs"]
pub mod definitions_ecore_scalars;

#[path = "definitions/effective_feature_cursor.rs"]
pub mod definitions_effective_feature_cursor;

#[path = "definitions/eobject_construction.rs"]
pub mod definitions_eobject_construction;

#[path = "definitions/feature_map_cursor.rs"]
pub mod definitions_feature_map_cursor;

#[path = "definitions/features.rs"]
pub mod definitions_features;

#[path = "definitions/generic_datatypes.rs"]
pub mod definitions_generic_datatypes;

#[path = "definitions/observed_runtime_membership.rs"]
pub mod definitions_observed_runtime_membership;

#[path = "definitions/operation_policy_qualification.rs"]
pub mod definitions_operation_policy_qualification;

#[path = "definitions/prepared_runtime_features.rs"]
pub mod definitions_prepared_runtime_features;

#[path = "definitions/rcore_behavior_check.rs"]
pub mod definitions_rcore_behavior_check;

#[path = "definitions/rcore_behavior_collection_ir.rs"]
pub mod definitions_rcore_behavior_collection_ir;

#[path = "definitions/rcore_behavior_collection_typing.rs"]
pub mod definitions_rcore_behavior_collection_typing;

#[path = "definitions/rcore_behavior_domain_precision.rs"]
pub mod definitions_rcore_behavior_domain_precision;

#[path = "definitions/rcore_behavior_enums.rs"]
pub mod definitions_rcore_behavior_enums;

#[path = "definitions/rcore_behavior_function_effects.rs"]
pub mod definitions_rcore_behavior_function_effects;

#[path = "definitions/rcore_behavior_function_instantiations.rs"]
pub mod definitions_rcore_behavior_function_instantiations;

#[path = "definitions/rcore_behavior_function_lowering.rs"]
pub mod definitions_rcore_behavior_function_lowering;

#[path = "definitions/rcore_behavior_function_verifier_boundaries.rs"]
pub mod definitions_rcore_behavior_function_verifier_boundaries;

#[path = "definitions/rcore_behavior_generic_functions.rs"]
pub mod definitions_rcore_behavior_generic_functions;

#[path = "definitions/rcore_behavior_generic_handoff.rs"]
pub mod definitions_rcore_behavior_generic_handoff;

#[path = "definitions/rcore_behavior_ir.rs"]
pub mod definitions_rcore_behavior_ir;

#[path = "definitions/rcore_behavior_ir_v2.rs"]
pub mod definitions_rcore_behavior_ir_v2;

#[path = "definitions/rcore_behavior_ir_v2_adversarial.rs"]
pub mod definitions_rcore_behavior_ir_v2_adversarial;

#[path = "definitions/rcore_behavior_model_types.rs"]
pub mod definitions_rcore_behavior_model_types;

#[path = "definitions/rcore_behavior_modules.rs"]
pub mod definitions_rcore_behavior_modules;

#[path = "definitions/rcore_behavior_overload_bindings.rs"]
pub mod definitions_rcore_behavior_overload_bindings;

#[path = "definitions/rcore_behavior_overload_checking.rs"]
pub mod definitions_rcore_behavior_overload_checking;

#[path = "definitions/rcore_behavior_overload_domain_failures.rs"]
pub mod definitions_rcore_behavior_overload_domain_failures;

#[path = "definitions/rcore_behavior_patterns.rs"]
pub mod definitions_rcore_behavior_patterns;

#[path = "definitions/rcore_behavior_providers.rs"]
pub mod definitions_rcore_behavior_providers;

#[path = "definitions/rcore_behavior_s04_corpus.rs"]
pub mod definitions_rcore_behavior_s04_corpus;

#[path = "definitions/rcore_behavior_s04_signatures.rs"]
pub mod definitions_rcore_behavior_s04_signatures;

#[path = "definitions/rcore_behavior_scalar_lift.rs"]
pub mod definitions_rcore_behavior_scalar_lift;

#[path = "definitions/rcore_behavior_scalar_lowering.rs"]
pub mod definitions_rcore_behavior_scalar_lowering;

#[path = "definitions/rcore_behavior_signatures.rs"]
pub mod definitions_rcore_behavior_signatures;

#[path = "definitions/rcore_behavior_structural.rs"]
pub mod definitions_rcore_behavior_structural;

#[path = "definitions/rcore_behavior_structural_relations.rs"]
pub mod definitions_rcore_behavior_structural_relations;

#[path = "definitions/rcore_behavior_symbolic_ecore.rs"]
pub mod definitions_rcore_behavior_symbolic_ecore;

#[path = "definitions/rcore_behavior_syntax.rs"]
pub mod definitions_rcore_behavior_syntax;

#[path = "definitions/rcore_behavior_types.rs"]
pub mod definitions_rcore_behavior_types;

#[path = "definitions/rcore_direct.rs"]
pub mod definitions_rcore_direct;

#[path = "definitions/rcore_operations.rs"]
pub mod definitions_rcore_operations;

#[path = "definitions/rcore_structure.rs"]
pub mod definitions_rcore_structure;

#[path = "definitions/rcore_syntax.rs"]
pub mod definitions_rcore_syntax;

#[path = "definitions/reference_conformance_cursor.rs"]
pub mod definitions_reference_conformance_cursor;

#[path = "definitions/reference_identity.rs"]
pub mod definitions_reference_identity;

#[path = "definitions/reference_structure_cursor.rs"]
pub mod definitions_reference_structure_cursor;

#[path = "definitions/runtime_feature_domains.rs"]
pub mod definitions_runtime_feature_domains;

#[path = "definitions/runtime_lower_bounds.rs"]
pub mod definitions_runtime_lower_bounds;

#[path = "definitions/runtime_lower_limits.rs"]
pub mod definitions_runtime_lower_limits;

#[path = "definitions/runtime_operation_limits.rs"]
pub mod definitions_runtime_operation_limits;

#[path = "definitions/runtime_operations.rs"]
pub mod definitions_runtime_operations;

#[path = "definitions/schema_observation.rs"]
pub mod definitions_schema_observation;

#[path = "definitions/schema_validation_cursor.rs"]
pub mod definitions_schema_validation_cursor;

#[path = "definitions/shared_schema_materialization.rs"]
pub mod definitions_shared_schema_materialization;

// Portable carriers share their native unit controls with the existing suite.
#[allow(dead_code)] // The executable ABI is exercised by explicit portable probe runs.
#[path = "../examples/rcore_structural_preparation_probe.rs"]
pub mod portable_rcore_structural_preparation_probe;

#[allow(dead_code)] // The executable ABI is exercised by explicit portable probe runs.
#[path = "../examples/rcore_behavior_compilation_probe.rs"]
pub mod portable_rcore_behavior_compilation_probe;

#[allow(dead_code)] // The executable ABI is exercised by explicit portable probe runs.
#[path = "../examples/rcore_module_preparation_probe.rs"]
pub mod portable_rcore_module_preparation_probe;

#[cfg(feature = "milestone-tests")]
#[allow(dead_code)] // The executable ABI is exercised by explicit portable probe runs.
#[path = "../examples/generic_resolver_preparation_probe.rs"]
pub mod portable_generic_resolver_preparation_probe;

#[allow(dead_code)] // The executable ABI is exercised by explicit portable probe runs.
#[path = "../examples/rcore_enum_queries_probe.rs"]
pub mod portable_rcore_enum_queries_probe;

#[allow(dead_code)] // The executable ABI is exercised by explicit portable probe runs.
#[path = "../examples/rcore_model_feature_queries_probe.rs"]
pub mod portable_rcore_model_feature_queries_probe;
