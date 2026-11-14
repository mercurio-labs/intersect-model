//! Shared borrowed stored-model observations for source scopes and getters.
use crate::{
    core::storage::{
        ContainmentNavigation, ModelRead, PendingContainmentIndex, ReferenceView, ValueView,
    },
    definitions::{
        conversion::InvocationContext,
        generic_types::RuntimeType,
        rcore_behavior::ir_v2::OwnershipOp,
        resources::DeclarationId,
        schema::{EffectiveFeaturePreparation, Feature, MemberIdentity, Schema, ScopeSchema},
    },
    scope_runtime::{ScopePage, ScopeRead, ScopeValue},
};
use std::cell::RefCell;

pub struct SourceModelReads<'a> {
    store: &'a dyn ModelRead,
    schema: &'a Schema,
    // One immutable source-function invocation only; dropped before linker writes.
    ownership: RefCell<Option<PendingContainmentIndex<'a>>>,
}

impl<'a> SourceModelReads<'a> {
    pub fn new(store: &'a dyn ModelRead, schema: &'a Schema) -> Self {
        Self {
            store,
            schema,
            ownership: RefCell::new(None),
        }
    }
}

fn singular(
    value: ValueView<'_>,
    observation: &InvocationContext<'_>,
) -> Result<ScopeValue, String> {
    match value.reference() {
        Some(ReferenceView::Unresolved { .. }) => Ok(ScopeValue::Pending),
        Some(ReferenceView::Unavailable { .. }) => Ok(ScopeValue::Unavailable),
        _ => value.to_json_observed(observation).map(ScopeValue::Value),
    }
}

#[doc(hidden)]
pub fn observed_member<'schema>(
    schema: &'schema Schema,
    actual: &str,
    declaration: &DeclarationId,
    observation: &InvocationContext<'_>,
) -> Result<&'schema Feature, String> {
    let mut pending = EffectiveFeaturePreparation::new_observed(actual, observation)
        .map_err(|error| error.to_string())?;
    while !pending
        .advance_observed(schema, observation)
        .map_err(|error| error.to_string())?
    {}
    for selected in pending.finish()? {
        observation.work(1)?;
        let member = selected.resolve(schema)?;
        if matches!(&member.identity, MemberIdentity::Source { declaration: selected } if selected == declaration)
        {
            return Ok(member.feature);
        }
    }
    Err("Unknown exact source feature for Rcore scope receiver".into())
}

impl ScopeRead for SourceModelReads<'_> {
    fn ownership(
        &self,
        object: &str,
        operation: OwnershipOp,
        selected: &RuntimeType,
        cursor: usize,
        schema: &ScopeSchema<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        self.check(observation)?;
        let mut index = self
            .ownership
            .try_borrow_mut()
            .map_err(|_| "Rcore scope containment navigation is already borrowed")?;
        if index.is_none() {
            *index = Some(PendingContainmentIndex::prepare(
                self.store,
                self.schema,
                observation,
            )?);
        }
        let operation = match operation {
            OwnershipOp::AsType => {
                return Err("Checked type views use the shared source authority".into());
            }
            OwnershipOp::Container => ContainmentNavigation::Container,
            OwnershipOp::Ancestors => ContainmentNavigation::Ancestors,
            OwnershipOp::Contents => ContainmentNavigation::Contents,
            OwnershipOp::Roots => ContainmentNavigation::Roots,
        };
        match index
            .as_ref()
            .ok_or("Missing scope containment navigation")?
            .page_with_schema(object, operation, selected, cursor, schema, observation)?
        {
            Some(id) => Ok(ScopePage::Value(serde_json::Value::String(id))),
            None => Ok(ScopePage::End),
        }
    }
    fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        observation.work(1)?;
        observation.checkpoint()?;
        if self.store.revision()? != self.schema.revision {
            return Err("Rcore scope model has a different schema revision".into());
        }
        Ok(())
    }
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String> {
        self.check(observation)?;
        observation.work(1)?;
        let actual = self
            .store
            .class(object)?
            .ok_or("Missing Rcore scope object")?;
        let found = self.store.inspect(object, &mut |view| {
            observation.work(1)?;
            if view.id() != object
                || view.class() != actual
                || view.type_identity() != self.schema.identity(&actual)
            {
                return Err("Rcore scope object has a foreign source/type identity".into());
            }
            Ok(())
        })?;
        if !found {
            return Err("Rcore scope object disappeared".into());
        }
        self.check(observation)?;
        Ok(actual)
    }
    fn singular(
        &self,
        object: &str,
        declaration: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopeValue, String> {
        let actual = self.class(object, observation)?;
        let member = observed_member(self.schema, &actual, declaration, observation)?;
        if member.upper != 1 || member.derived || member.volatile {
            return Err("Rcore scope requires an admitted singular stored feature".into());
        }
        let mut result = ScopeValue::Absent;
        let present = self.store.inspect_stored_value_observed(
            object,
            &member.name,
            false,
            observation,
            &mut |value| {
                result = singular(value, observation)?;
                Ok(())
            },
        )?;
        self.check(observation)?;
        if present {
            Ok(result)
        } else {
            Ok(ScopeValue::Absent)
        }
    }
    fn page(
        &self,
        object: &str,
        declaration: &DeclarationId,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        let actual = self.class(object, observation)?;
        let member = observed_member(self.schema, &actual, declaration, observation)?;
        if member.upper == 1 || member.derived || member.volatile {
            return Err("Rcore scope requires an admitted stored collection".into());
        }
        let mut result = ScopePage::End;
        let present = self.store.inspect_stored_value_observed(
            object,
            &member.name,
            true,
            observation,
            &mut |value| {
                let values = value
                    .collection()
                    .ok_or("Scope collection has a malformed value")?;
                if let Some(value) = values.get(cursor) {
                    result = match singular(value, observation)? {
                        ScopeValue::Value(value) => ScopePage::Value(value),
                        ScopeValue::Pending => ScopePage::Pending,
                        ScopeValue::Unavailable => ScopePage::Unavailable,
                        ScopeValue::Absent => {
                            return Err("Missing scope collection occurrence".into());
                        }
                    };
                }
                Ok(())
            },
        )?;
        self.check(observation)?;
        if !present {
            return Err("Missing Rcore scope collection".into());
        }
        Ok(result)
    }
}
