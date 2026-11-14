//! Consolidated qualification suite.
#![allow(clippy::duplicate_mod)]

mod stores {
    #[allow(unused_imports)]
    pub use intersect_model_test_support::stores::*;
    #[test]
    fn frozen_store_starts_empty_with_the_requested_schema() -> Result<(), String> {
        intersect_model_test_support::stores::verify_frozen_store_starts_empty_with_the_requested_schema()
    }
}

#[path = "xmi/ecore_migration.rs"]
pub mod xmi_ecore_migration;

#[path = "xmi/initialization.rs"]
pub mod xmi_initialization;
