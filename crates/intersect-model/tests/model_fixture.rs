//! Model-only fixtures share the checked recipe used by model unit controls.
use intersect_model::definitions::{
    bundle, declarations, ecore, ecore_migration, model_definition, preparation,
};
// This shared recipe also contains a unit-only borrowed-owner helper.
#[allow(dead_code)]
#[path = "../src/definitions/model_test_support.rs"]
mod checked;
pub(crate) use checked::{compile, prepare};
