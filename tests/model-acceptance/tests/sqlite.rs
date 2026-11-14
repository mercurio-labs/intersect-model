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

#[path = "sqlite/collections.rs"]
pub mod sqlite_collections;

#[path = "sqlite/deletion.rs"]
pub mod sqlite_deletion;

#[path = "sqlite/facade_conversions.rs"]
pub mod sqlite_facade_conversions;

#[path = "sqlite/incoming_cursor.rs"]
pub mod sqlite_incoming_cursor;

#[path = "sqlite/initialization.rs"]
pub mod sqlite_initialization;

#[path = "sqlite/navigation_coherence.rs"]
pub mod sqlite_navigation_coherence;

#[path = "sqlite/query_candidates.rs"]
pub mod sqlite_query_candidates;

#[path = "sqlite/resource_set.rs"]
pub mod sqlite_resource_set;

#[path = "sqlite/runtime_feature_domains.rs"]
pub mod sqlite_runtime_feature_domains;
