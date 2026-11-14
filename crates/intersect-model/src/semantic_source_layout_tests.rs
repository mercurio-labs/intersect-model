//! Layout-only mirrors of exact frozen v20 declarations, without methods or state.
use super::*;
use crate::definitions::preparation::PreparedModelDefinition;
use std::mem::{align_of, size_of};

struct Policy;
impl SemanticReadPolicy for Policy {
    fn validate(&self, _: &PreparedModelDefinition<'_>) -> QueryResult<()> {
        Ok(())
    }
    fn plan(&self, _: ReadMember<'_>) -> ReadPlan {
        ReadPlan::Unsupported
    }
    fn dependencies(&self, _: ReadMember<'_>) -> &[&str] {
        &[]
    }
    fn read(
        &self,
        _: ReadMember<'_>,
        _: &dyn ReadDependencies,
        _: &mut QueryBudget,
    ) -> QueryResult<Value> {
        Err("Layout witness has no semantic implementation".into())
    }
}

#[allow(dead_code)]
struct FrozenSession<'a, P: SemanticReadPolicy> {
    definition: &'a PreparedModelDefinition<'a>,
    store: &'a dyn storage::ModelRead,
    policy: &'a P,
    // Added deliberately by ab2cdfe3 (prepared semantic reads): immutable
    // canonical member metadata. The frozen mirror tracks that declaration.
    metadata: Option<&'a crate::definitions::preparation::PreparedPhysicalMetadata<'a>>,
}
#[allow(dead_code)]
struct FrozenDependencies<'a, 's, P: SemanticReadPolicy> {
    session: &'s SemanticReadSession<'a, P>,
    actual: &'s str,
    id: &'s str,
    allowed: &'s [&'s str],
}
#[allow(dead_code)]
struct FrozenRelatedDependencies<'a, 's, P: SemanticReadPolicy> {
    same: FrozenDependencies<'a, 's, P>,
    caller: ReadMember<'s>,
}
#[allow(dead_code)]
struct FrozenPhysicalDependencies<'a, 's, P: SemanticReadPolicy> {
    same: FrozenDependencies<'a, 's, P>,
    caller: ReadMember<'s>,
}

#[test]
fn cohesive_default_read_holders_equal_frozen_declarations_and_scoped_layouts_are_separate() {
    assert_eq!(
        (
            size_of::<FrozenSession<'_, Policy>>(),
            align_of::<FrozenSession<'_, Policy>>()
        ),
        (
            size_of::<SemanticReadSession<'_, Policy>>(),
            align_of::<SemanticReadSession<'_, Policy>>()
        )
    );
    assert_eq!(
        (
            size_of::<FrozenDependencies<'_, '_, Policy>>(),
            align_of::<FrozenDependencies<'_, '_, Policy>>()
        ),
        (
            size_of::<Dependencies<'_, '_, Policy>>(),
            align_of::<Dependencies<'_, '_, Policy>>()
        )
    );
    assert_eq!(
        (
            size_of::<FrozenRelatedDependencies<'_, '_, Policy>>(),
            align_of::<FrozenRelatedDependencies<'_, '_, Policy>>()
        ),
        (
            size_of::<RelatedDependencies<'_, '_, Policy>>(),
            align_of::<RelatedDependencies<'_, '_, Policy>>()
        )
    );
    assert_eq!(
        (
            size_of::<FrozenPhysicalDependencies<'_, '_, Policy>>(),
            align_of::<FrozenPhysicalDependencies<'_, '_, Policy>>()
        ),
        (
            size_of::<PhysicalDependencies<'_, '_, Policy>>(),
            align_of::<PhysicalDependencies<'_, '_, Policy>>()
        )
    );
    assert_eq!(size_of::<NoReadSources>(), 0);
    println!(
        "COHESIVE_READ_LAYOUTS {}",
        serde_json::json!({
            "public_session":size_of::<SemanticReadSession<'_, Policy>>(),
            "old_default_dependencies":size_of::<FrozenDependencies<'_, '_, Policy>>(),
            "default_dependencies":size_of::<Dependencies<'_, '_, Policy>>(),
            "default_related":size_of::<RelatedDependencies<'_, '_, Policy>>(),
            "default_physical":size_of::<PhysicalDependencies<'_, '_, Policy>>(),
            "scope":"layout-only frozen declarations; no semantic or allocation claim"
        })
    );
}
