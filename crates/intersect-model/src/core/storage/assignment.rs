//! Schema-aware unsettable state over the shared storage representation.
use super::*;
use intersect_model::definitions::schema::Feature;
// Read adapters keep assignment validation on one algorithm for owned maps
// and complete shared definition inputs.
pub(super) trait FieldLookup {
    fn get(&self, name: &str) -> Option<&Feature>;
    fn next_field(&self, after: Option<&str>) -> Option<(&str, &Feature)>;
}
impl FieldLookup for BTreeMap<String, Feature> {
    fn get(&self, name: &str) -> Option<&Feature> {
        BTreeMap::get(self, name)
    }
    fn next_field(&self, after: Option<&str>) -> Option<(&str, &Feature)> {
        use std::ops::Bound::{Excluded, Unbounded};
        match after {
            Some(after) => self.range::<str, _>((Excluded(after), Unbounded)).next(),
            None => self.iter().next(),
        }
        .map(|(name, value)| (name.as_str(), value))
    }
}

pub(super) fn validate_assignment_state(
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    object: ObjectView<'_>,
    fields: &BTreeMap<String, Feature>,
) -> StorageResult<()> {
    let mut pending = AssignmentPreparation::new();
    while !pending.advance(schema, object, fields, || Ok(()))? {}
    Ok(())
}

enum AssignmentStage {
    Presence,
    Markers,
    Values,
    Done,
}
/// Shared assignment validation, advancing one schema feature or explicit marker.
/// The object and schema must remain immutable until completion.
pub(super) struct AssignmentPreparation {
    stage: AssignmentStage,
    last: Option<String>,
}
impl AssignmentPreparation {
    pub(super) fn new() -> Self {
        Self {
            stage: AssignmentStage::Presence,
            last: None,
        }
    }
    pub(super) fn advance(
        &mut self,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        object: ObjectView<'_>,
        fields: &(impl FieldLookup + ?Sized),
        mut charge: impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<bool> {
        let next = || fields.next_field(self.last.as_deref());
        match self.stage {
            AssignmentStage::Presence => {
                if let Some((name, field)) = next() {
                    charge()?;
                    if field.unsettable && !field.derived {
                        if object.assignment_state().is_none() {
                            return Err(
                                "Unsettable object requires explicit assignment state".into()
                            );
                        }
                        self.last = None;
                        self.stage = AssignmentStage::Markers;
                    } else {
                        self.last = Some(name.to_owned());
                    }
                } else {
                    if object.assignment_state().is_some() {
                        return Err(
                            "Explicit assignment state requires unsettable schema support".into(),
                        );
                    }
                    self.stage = AssignmentStage::Done;
                }
            }
            AssignmentStage::Markers => {
                let state = object
                    .assignment_state()
                    .ok_or("Unsettable object requires explicit assignment state")?;
                if let Some(name) = state.next_set_feature(self.last.as_deref()) {
                    charge()?;
                    if !fields.get(name).is_some_and(|f| f.unsettable && !f.derived) {
                        return Err(format!(
                            "Invalid assignment-state feature {}.{name}",
                            object.class()
                        ));
                    }
                    self.last = Some(name.into());
                } else {
                    self.last = None;
                    self.stage = AssignmentStage::Values;
                }
            }
            AssignmentStage::Values => {
                if let Some((name, field)) = next() {
                    charge()?;
                    if field.unsettable && !field.derived {
                        let state = object
                            .assignment_state()
                            .ok_or("Unsettable object requires explicit assignment state")?;
                        let value = object
                            .value(name)
                            .ok_or("Unsettable feature requires a stored effective value")?;
                        if schema.is_container_feature(field)?
                            && state.explicitly_set(name) == value.is_null()
                        {
                            return Err(format!(
                                "Container assignment state differs from container presence: {}.{name}",
                                object.class()
                            ));
                        }
                        if !state.explicitly_set(name) && !value.equals_json(&field.default) {
                            return Err(format!(
                                "Unset feature differs from its default: {}.{name}",
                                object.class()
                            ));
                        }
                    }
                    self.last = Some(name.to_owned());
                } else {
                    self.stage = AssignmentStage::Done;
                }
            }
            AssignmentStage::Done => {}
        }
        Ok(matches!(self.stage, AssignmentStage::Done))
    }
}

pub(super) fn update_assignment(
    store: &mut (impl ModelStore + ?Sized),
    id: &str,
    name: &str,
    assigned: bool,
) -> StorageResult<()> {
    let mut next = None;
    store.inspect(id, &mut |object| {
        next = Some(
            object
                .assignment_state()
                .ok_or("Unsettable object requires explicit assignment state")?
                .with_assignment(name, assigned)?,
        );
        Ok(())
    })?;
    store.set_assignment_state(id, Some(next.ok_or("Missing element")?))
}

/// Query explicit assignment history for an unsettable feature. This is not the
/// ordinary-feature value/default comparison or the raw field-presence API.
pub fn is_explicitly_set(
    store: &(impl ModelRead + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
) -> StorageResult<bool> {
    query_set_state(store, schema, id, name, true)
}

/// Query a stored feature's semantic set state: assignment history for unsettable
/// features, otherwise comparison with its canonical effective schema default.
/// Raw ObjectView::is_set remains a field-presence query. Computed features require
/// their owning semantic provider instead of a fabricated default comparison.
pub fn is_set(
    store: &(impl ModelRead + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
) -> StorageResult<bool> {
    query_set_state(store, schema, id, name, false)
}
fn query_set_state(
    store: &(impl ModelRead + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    explicit_only: bool,
) -> StorageResult<bool> {
    if !super::revision_matches(store, &schema.revision)? {
        return Err("Model belongs to a different schema revision".into());
    }
    let mut result = None;
    store.inspect(id, &mut |object| {
        if object.type_identity() != schema.identity(object.class()) {
            return Err("Model type identity does not match schema".into());
        }
        let fields = schema.features(object.class())?;
        let feature = fields.get(name).ok_or("Unknown feature")?;
        if explicit_only && !feature.unsettable {
            return Err("Feature is not unsettable".into());
        }
        if feature.derived {
            return Err("Computed feature set state requires a semantic provider".into());
        }
        validate_assignment_state(schema, object, &fields)?;
        result = Some(if feature.unsettable {
            object
                .assignment_state()
                .ok_or("Missing assignment state")?
                .explicitly_set(name)
        } else {
            !object
                .value(name)
                .ok_or("Stored feature requires an effective value")?
                .equals_json(&feature.default)
        });
        Ok(())
    })?;
    result.ok_or_else(|| "Missing element".into())
}

/// Restore a stored feature's schema default and clear any unsettable marker
/// atomically. Like set_with_opposite, this allows drafts; full model validation
/// remains a separate operation, including required multiplicities.
pub fn unset(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
) -> StorageResult<()> {
    unset_context(store, schema, id, name, None)
}
/// Reset through the active language-owned conversion registry. Missing or stale
/// required providers reject before mutation, including null/empty defaults.
pub fn unset_with_conversions(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    conversions: &ConversionRegistry<'_>,
) -> StorageResult<()> {
    unset_context(store, schema, id, name, Some(conversions))
}
fn unset_context(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    atomic(store, |store| {
        bind_schema(store, schema)?;
        let class = store.class(id)?.ok_or("Missing element")?;
        let fields = schema.features(&class)?;
        let field = fields.get(name).ok_or("Unknown feature")?;
        if !field.reference {
            schema.require_runtime_attribute_conversion(&class, field, conversions)?;
        }
        set_inner(store, schema, id, name, field.default.clone(), conversions)?;
        if field.unsettable {
            update_assignment(store, id, name, false)?;
        }
        Ok(())
    })
}

/// Query current derived value state using a revision-bound language provider.
/// Stored features retain the normal assignment-state contract. A derived
/// unsettable feature needs a setting-state service, not value/default inference.
/// Providers read the current store and share the caller's explicit work budget.
pub fn is_set_with_derived(
    store: &dyn ModelRead,
    schema: &Schema,
    id: &str,
    name: &str,
    provider: &dyn crate::core::query::DerivedValues,
    budget: &mut crate::core::query::QueryBudget,
) -> StorageResult<bool> {
    computed_set_state(store, schema, id, name, provider, budget, None)
}

/// Validate computed attributes through the exact selected conversion registry,
/// including null/empty results whose elements alone cannot reveal a dependency.
pub fn is_set_with_derived_and_conversions(
    store: &dyn ModelRead,
    schema: &Schema,
    id: &str,
    name: &str,
    provider: &dyn crate::core::query::DerivedValues,
    budget: &mut crate::core::query::QueryBudget,
    conversions: &ConversionRegistry<'_>,
) -> StorageResult<bool> {
    computed_set_state(store, schema, id, name, provider, budget, Some(conversions))
}

fn computed_set_state(
    store: &dyn ModelRead,
    schema: &Schema,
    id: &str,
    name: &str,
    provider: &dyn crate::core::query::DerivedValues,
    budget: &mut crate::core::query::QueryBudget,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<bool> {
    budget.charge()?;
    if !super::revision_matches(store, &schema.revision)? {
        return Err("Model belongs to a different schema revision".into());
    }
    let mut class = None;
    store.inspect(id, &mut |object| {
        if object.type_identity() != schema.identity(object.class()) {
            return Err("Model type identity does not match schema".into());
        }
        class = Some(object.class().to_owned());
        Ok(())
    })?;
    let class = class.ok_or("Missing element")?;
    let fields = schema.features(&class)?;
    let field = fields.get(name).ok_or("Unknown feature")?;
    if !field.derived {
        return is_set(store, schema, id, name);
    }
    if provider.schema_revision() != Some(schema.revision.as_str()) {
        return Err(
            "Computed state provider is missing or belongs to a different schema revision".into(),
        );
    }
    if !provider.supports(&class, name) {
        return Err(format!("No semantic provider for {class}.{name}"));
    }
    schema.require_runtime_attribute_conversion(&class, field, conversions)?;
    let value = provider.read(store, id, name, budget)?;
    validate_computed_value(store, schema, &class, field, &value, budget, conversions)?;
    if field.unsettable {
        let assigned = provider.explicit_set_state(store, id, name, budget)?;
        if !assigned && value != field.default {
            return Err("Unset computed feature differs from its default".into());
        }
        Ok(assigned)
    } else {
        Ok(value != field.default)
    }
}

pub(super) fn validate_computed_value(
    store: &dyn ModelRead,
    schema: &Schema,
    class: &str,
    field: &Feature,
    value: &Value,
    budget: &mut crate::core::query::QueryBudget,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    validate_feature_shape(class, field, Some(ValueView::json(value)), true, || {
        budget.charge()
    })?;
    validate_computed_atoms(store, schema, class, field, value, budget, conversions)
}

pub(super) fn validate_computed_append(
    store: &dyn ModelRead,
    schema: &Schema,
    class: &str,
    field: &Feature,
    value: &Value,
    budget: &mut crate::core::query::QueryBudget,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    let singleton = Value::Array(vec![value.clone()]);
    validate_feature_shape(
        class,
        field,
        Some(ValueView::json(&singleton)),
        false,
        || budget.charge(),
    )?;
    validate_computed_atoms(store, schema, class, field, &singleton, budget, conversions)
}

fn validate_computed_atoms(
    store: &dyn ModelRead,
    schema: &Schema,
    class: &str,
    field: &Feature,
    value: &Value,
    budget: &mut crate::core::query::QueryBudget,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    for atom in values(value, field.upper)? {
        budget.charge()?;
        if field.reference {
            let target = atom.as_str().ok_or("Reference must be an identity")?;
            let mut found = false;
            store.inspect(target, &mut |object| {
                if object.type_identity() != schema.identity(object.class()) {
                    return Err("Computed reference target has a foreign type identity".into());
                }
                if !schema.reference_conforms(class, &field.name, object.class())? {
                    return Err("Computed reference target has an incompatible class".into());
                }
                found = true;
                Ok(())
            })?;
            if !found {
                return Err(format!("Missing computed reference target {target}"));
            }
        } else {
            schema.validate_runtime_attribute(class, field, atom, conversions)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod preparation_tests {
    use super::*;
    use crate::core::model::{AssignmentState, Element};
    use intersect_model::definitions::compiler;

    #[test]
    fn assignment_passes_yield_each_marker_and_feature_and_preserve_error_order() {
        let members = (0..64).map(|i| format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{i:02}" unsettable="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#)).collect::<String>();
        let schema = compiler::inspect_ecore_schema(&format!(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:assignment"><eClassifiers x:type="e:EClass" name="Node">{members}</eClassifiers></e:EPackage>"#)).unwrap();
        let fields = schema.features("Node").unwrap();
        let mut object = Element::new(&schema, "n", "Node", "r", 0, 0).unwrap();
        object.assignment_state = Some(AssignmentState::new(fields.keys().cloned()).unwrap());
        // First unsettable feature, 64 markers, then 64 effective-value checks.
        let total = 129;
        for limit in 0..=total {
            let mut pending = AssignmentPreparation::new();
            let mut work = 0;
            let result = loop {
                let before = work;
                let step =
                    pending.advance(&schema, ObjectView::from_element(&object), &fields, || {
                        if work == limit {
                            return Err("cancelled".into());
                        }
                        work += 1;
                        Ok(())
                    });
                assert!(work - before <= 1);
                match step {
                    Ok(false) => {}
                    other => break other,
                }
            };
            assert_eq!(work, limit);
            if limit == total {
                assert_eq!(result, Ok(true));
            } else {
                assert_eq!(result, Err("cancelled".into()));
            }
        }
        object
            .values
            .insert("f00".into(), Value::String("not the default".into()));
        object.assignment_state = Some(AssignmentState::new(["zz-invalid"]).unwrap());
        assert_eq!(
            validate_assignment_state(&schema, ObjectView::from_element(&object), &fields),
            Err("Invalid assignment-state feature Node.zz-invalid".into())
        );
        object.assignment_state = Some(AssignmentState::new(Vec::<String>::new()).unwrap());
        assert_eq!(
            validate_assignment_state(&schema, ObjectView::from_element(&object), &fields),
            Err("Unset feature differs from its default: Node.f00".into())
        );
    }
}
