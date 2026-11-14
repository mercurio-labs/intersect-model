//! Concrete immutable-store authority for one structural validation call.
use super::{
    DraftConstructionStore, InvocationContext, ModelRead, StorageResult, StructuralValueSelector,
};
use crate::definitions::schema::Schema;

/// A borrowed immutable concrete store, issued only by Core. A forwarded token
/// cannot authorize a different trait object, even when schemas/revisions agree.
pub struct ImmutableStructuralAudit<'audit> {
    store: &'audit dyn ModelRead,
    schema: &'audit Schema,
}
impl<'audit> ImmutableStructuralAudit<'audit> {
    /// Mint from a concrete immutable store. Operations must use store()'s
    /// exact trait-object view, rather than re-coercing the concrete store.
    pub fn native(store: &'audit impl DraftConstructionStore, schema: &'audit Schema) -> Self {
        Self { store, schema }
    }
    pub fn matches(&self, store: &dyn ModelRead, schema: &Schema) -> bool {
        std::ptr::eq(self.store, store) && std::ptr::eq(self.schema, schema)
    }
    pub fn store(&self) -> &'audit dyn ModelRead {
        self.store
    }
    pub fn schema(&self) -> &'audit Schema {
        self.schema
    }
}

/// Lend an operation-local selector. Declining must not run the operation;
/// accepting must run it exactly once. The canonical result stays with Core,
/// so a hook cannot mask a validation failure or start a second validation.
pub fn with_immutable_structural_audit(
    selector: &dyn StructuralValueSelector,
    audit: &ImmutableStructuralAudit<'_>,
    control: &InvocationContext<'_>,
    operation: &mut dyn FnMut(&dyn StructuralValueSelector) -> StorageResult<()>,
) -> StorageResult<()> {
    let mut calls = 0usize;
    let mut result = None;
    let mut duplicate = false;
    let hooked = selector.with_immutable_audit(audit, control, &mut |scoped| {
        if calls != 0 {
            duplicate = true;
            return;
        }
        calls = 1;
        result = Some(operation(scoped));
    });
    // The first canonical failure wins over hook/protocol failures.
    if let Some(Err(error)) = result.take() {
        return Err(error);
    }
    if duplicate {
        return Err("Immutable structural audit invoked more than once".into());
    }
    let accepted = hooked?;
    match (accepted, calls) {
        (false, 0) => return operation(selector),
        (true, 1) => {}
        (false, _) => {
            return Err("Declined immutable structural audit invoked its operation".into());
        }
        (true, _) => {
            return Err("Accepted immutable structural audit did not invoke its operation".into());
        }
    }
    control.checkpoint()
}
