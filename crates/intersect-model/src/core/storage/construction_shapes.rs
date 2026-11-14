//! Borrowed construction-only shape input to the existing structural engine.
//! This carries no completed graph facts and is never retained on its holders.
use super::*;
use crate::core::query::QueryBudget;

pub(super) trait ConstructionShapes {
    const ENABLED: bool;
    fn absent(
        &mut self,
        element: ObjectView<'_>,
        name: &str,
        fields: &impl initialization::MemberLookup,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<bool>;
}
pub(super) struct NoConstructionShapes;
impl ConstructionShapes for NoConstructionShapes {
    const ENABLED: bool = false;
    fn absent(
        &mut self,
        _: ObjectView<'_>,
        _: &str,
        _: &impl initialization::MemberLookup,
        _: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        Ok(false)
    }
}
pub(super) struct BudgetConstructionShapes<'a, 'context> {
    pub context: &'a DraftConstructionContext<'context>,
    pub budget: &'a mut QueryBudget,
}
fn absent(
    context: &DraftConstructionContext<'_>,
    element: ObjectView<'_>,
    name: &str,
    fields: &impl initialization::MemberLookup,
    charge: &mut impl FnMut() -> StorageResult<()>,
) -> StorageResult<bool> {
    // Exactly the former NI lookup charge/order, now shared by both carriers.
    charge()?;
    let identity = fields
        .member_identity(name)
        .ok_or("Missing structural member identity")?;
    context.permits_absent_physical_slot(element, name, identity, charge)
}
impl ConstructionShapes for BudgetConstructionShapes<'_, '_> {
    const ENABLED: bool = true;
    fn absent(
        &mut self,
        element: ObjectView<'_>,
        name: &str,
        fields: &impl initialization::MemberLookup,
        _: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        absent(self.context, element, name, fields, &mut || {
            self.budget.charge()
        })
    }
}
// New ephemeral source carrier; existing default carriers keep their fields.
pub(super) struct LeasedConstructionShapes<'a, 'context, 'source, 'metadata> {
    pub context: &'a DraftConstructionContext<'context>,
    pub source: &'a DraftConstructionSource<'source, 'metadata>,
    pub budget: &'a mut QueryBudget,
}
impl ConstructionShapes for LeasedConstructionShapes<'_, '_, '_, '_> {
    const ENABLED: bool = true;
    fn absent(
        &mut self,
        element: ObjectView<'_>,
        name: &str,
        fields: &impl initialization::MemberLookup,
        _: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        self.budget.charge()?;
        let identity = fields
            .member_identity(name)
            .ok_or("Missing structural member identity")?;
        self.source
            .permits_absence(self.context, element, name, identity, &mut || {
                self.budget.charge()
            })
    }
}
