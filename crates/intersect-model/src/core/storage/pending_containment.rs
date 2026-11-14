//! Narrow canonical stored-containment navigation during pending linking.
//! This is not a structural audit, semantic ownership provider or publication
//! capability. Noncontainment references and incomplete lower bounds are outside
//! its contract. Keep the borrowed view immutable and discard before any write.
use super::{
    FeatureShape, ModelRead, OwnershipIndex, ReferenceView, Schema, StorageResult, ValueView,
};
use crate::definitions::{
    conversion::InvocationContext,
    generic_types::RuntimeType,
    schema::{EffectiveFeaturePreparation, ReferenceConformancePreparation, ScopeSchema},
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainmentNavigation {
    Container,
    Ancestors,
    Contents,
    /// Objects without a container, in identity order.
    Roots,
}

/// A borrowed index of ONLY source-declared, nonderived stored containment.
/// It retains identities and edges, never model values or a copied graph.
/// Derived aliases are recorded separately and never introduce physical edges.
/// The lifetime prevents ordinary mutation of the borrowed view; arbitrary
/// ModelRead adapters must honor their existing stable-view contract as well.
pub struct PendingContainmentIndex<'a> {
    store: &'a dyn ModelRead,
    schema: &'a Schema,
    ownership: OwnershipIndex,
    objects: BTreeSet<String>,
}
impl<'a> PendingContainmentIndex<'a> {
    pub fn prepare(
        store: &'a dyn ModelRead,
        schema: &'a Schema,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Self> {
        let mut result = Self {
            store,
            schema,
            ownership: OwnershipIndex::new(true),
            objects: BTreeSet::new(),
        };
        result.check(observation)?;
        let mut after = None;
        loop {
            observation.work(1)?;
            let page = store.id_page_observed(after, 1, observation)?;
            if page.is_empty() {
                break;
            }
            if page.len() != 1 {
                return Err("Pending containment identity page exceeds its bound".into());
            }
            let (row, id) = &page[0];
            if after.is_some_and(|prior| *row <= prior) {
                return Err("Pending containment identity cursor did not advance".into());
            }
            after = Some(*row);
            if result.objects.contains(id) {
                return Err("Duplicate pending containment object identity".into());
            }
            observation.retain_storage(64usize.saturating_add(id.len()))?;
            result.objects.insert(id.clone());
            result.object(id, observation)?;
            let found = store.inspect(id, &mut |object| {
                let mut preparation =
                    EffectiveFeaturePreparation::new_observed(object.class(), observation)
                        .map_err(|e| e.to_string())?;
                while !preparation
                    .advance_observed(schema, observation)
                    .map_err(|e| e.to_string())?
                {}
                for selected in preparation.finish()? {
                    observation.work(1)?;
                    let member = selected.resolve(schema)?;
                    let field = member.feature;
                    if !field.containment {
                        continue;
                    }
                    if !field.reference {
                        return Err("Containment feature is not a reference".into());
                    }
                    let value = object
                        .value(&field.name)
                        .or_else(|| Some(ValueView::json(&field.default)));
                    if field.derived {
                        result.ownership.derived(id, observation)?;
                        if value.is_some_and(|value| {
                            !value.is_null()
                                && value.collection().is_none_or(|list| !list.is_empty())
                        }) {
                            return Err("Derived containment has raw stored values".into());
                        }
                        continue;
                    }
                    if field.volatile {
                        return Err(
                            "Volatile containment requires an explicit ownership provider".into(),
                        );
                    }
                    let healthy = |observation: &InvocationContext<'_>| result.check(observation);
                    let mut shape = FeatureShape::new_observed(
                        object.class(),
                        field,
                        value,
                        false,
                        observation,
                        &healthy,
                    )?;
                    while !shape.advance_observed(value, observation, &healthy)? {}
                    let count = value.and_then(ValueView::collection).map_or_else(
                        || usize::from(value.is_some_and(|value| !value.is_null())),
                        |list| list.len(),
                    );
                    for index in 0..count {
                        observation.work(1)?;
                        let atom = value
                            .and_then(|value| {
                                value.collection().map_or_else(
                                    || (index == 0).then_some(value),
                                    |list| list.get(index),
                                )
                            })
                            .ok_or("Missing pending containment occurrence")?;
                        match atom.reference() {
                            Some(ReferenceView::Unresolved { .. }) => {
                                return Err("Pending containment has an unresolved target".into());
                            }
                            Some(ReferenceView::Unavailable { .. }) => {
                                return Err("Pending containment target is unavailable".into());
                            }
                            _ => {}
                        }
                        let target = atom
                            .as_str()
                            .ok_or("Containment target must be an identity")?;
                        let target_class = result.object(target, observation)?;
                        let mut conformance = ReferenceConformancePreparation::new_observed(
                            object.class(),
                            &field.name,
                            &target_class,
                            observation,
                        )
                        .map_err(|e| e.to_string())?;
                        while !conformance
                            .advance_observed(schema, observation)
                            .map_err(|e| e.to_string())?
                        {}
                        if !conformance.finish()? {
                            return Err("Pending containment target has the wrong type".into());
                        }
                        result.ownership.insert(target, id, observation)?;
                    }
                }
                Ok(())
            })?;
            if !found {
                return Err("Pending containment object disappeared".into());
            }
            result.check(observation)?;
        }
        result.ownership.check_cycles_observed(observation)?;
        result.check(observation)?;
        Ok(result)
    }
    fn check(&self, observation: &InvocationContext<'_>) -> StorageResult<()> {
        observation.work(1)?;
        observation.checkpoint()?;
        self.store.inspect_revision(&mut |revision| {
            if revision != self.schema.revision {
                return Err("Pending containment schema revision changed".into());
            }
            Ok(())
        })
    }
    fn object(&self, id: &str, observation: &InvocationContext<'_>) -> StorageResult<String> {
        self.check(observation)?;
        observation.work(1)?;
        let mut class = None;
        let found = self.store.inspect(id, &mut |object| {
            observation.work(1)?;
            if object.id() != id
                || !self.schema.classes.contains_key(object.class())
                || !self.schema.declarations.contains_key(object.class())
                || object.type_identity() != self.schema.identity(object.class())
            {
                return Err("Pending containment object has a foreign source/type identity".into());
            }
            observation.retain_storage(object.class().len())?;
            class = Some(object.class().to_owned());
            Ok(())
        })?;
        if !found {
            return Err("Pending containment object is absent or unavailable".into());
        }
        class.ok_or_else(|| "Pending containment adapter did not inspect its object".into())
    }
    /// Whether computed containment aliases exist. They were NOT used as edges;
    /// callers must not describe this index as complete language ownership.
    pub fn has_derived_aliases(&self) -> bool {
        self.ownership.has_derived_owners()
    }
    /// Return one selected-type-filtered canonical physical result. Ancestors
    /// are nearest-first; Contents uses effective feature then occurrence order.
    pub fn page(
        &self,
        object: &str,
        operation: ContainmentNavigation,
        target: &RuntimeType,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<String>> {
        let schema = ScopeSchema::prepare_observed(self.schema, observation)?;
        self.page_with_schema(object, operation, target, cursor, &schema, observation)
    }
    /// Reuse one completed runtime-schema check for this immutable scope call.
    /// The proof must borrow this exact schema; equal wire data is insufficient.
    #[allow(clippy::too_many_arguments)]
    pub fn page_with_schema(
        &self,
        object: &str,
        operation: ContainmentNavigation,
        target: &RuntimeType,
        cursor: usize,
        schema: &ScopeSchema<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<String>> {
        self.check(observation)?;
        observation.work(1)?;
        if !schema.matches_schema(self.schema) {
            return Err("Pending containment runtime-schema owner changed".into());
        }
        if !self.objects.contains(object) {
            return Err("Unknown pending containment source".into());
        }
        self.object(object, observation)?;
        let expected = schema.class_key_for_type_observed(target, observation)?;
        if operation == ContainmentNavigation::Roots {
            return self.ownership.page_roots_observed(
                &self.objects,
                expected,
                cursor,
                schema,
                observation,
                |id, observation| self.object(id, observation),
            );
        }
        self.ownership.page_observed(
            object,
            operation,
            expected,
            cursor,
            schema,
            observation,
            |id, observation| self.object(id, observation),
        )
    }
}

#[cfg(test)]
#[path = "pending_containment_tests.rs"]
mod tests;
