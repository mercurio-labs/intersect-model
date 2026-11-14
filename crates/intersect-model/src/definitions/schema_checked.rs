//! Owning immutable runtime-type validation. Wire data never carries this authority.
use super::{
    ClassKeyPreparation, PreparationError, RuntimeAlternative, SchemaRepresentation,
    ValidatedRuntimeTypes,
};
use crate::definitions::conversion::InvocationContext;
use std::sync::Arc;

/// Runtime-type metadata checked once for this exact immutable schema borrow.
/// Only the shared validator can issue it. It grants neither graph validity nor
/// model/source mutation authority, and it cannot survive mutation of its schema.
pub struct ScopeSchema<'schema> {
    schema: &'schema super::Schema,
    checked: ValidatedRuntimeTypes<'schema>,
}
impl<'schema> ScopeSchema<'schema> {
    pub(crate) fn prepare_observed(
        schema: &'schema super::Schema,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, String> {
        observation.retain_storage(crate::logical_size!(Self, 24))?;
        let checked = ValidatedRuntimeTypes::prepare_observed(schema, observation)
            .map_err(|error| error.to_string())?;
        Ok(Self { schema, checked })
    }
    pub fn matches_schema(&self, schema: &super::Schema) -> bool {
        self.checked.belongs_to(schema)
    }
    pub fn class_key_for_type_observed(
        &self,
        target: &crate::definitions::generic_types::RuntimeType,
        observation: &InvocationContext<'_>,
    ) -> Result<&'schema str, String> {
        self.checked
            .class_key_for_type_observed(target, observation)
            .map_err(|error| error.to_string())
    }
    pub fn conforms_observed(
        &self,
        actual: &str,
        expected: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        self.schema
            .conforms_observed(actual, expected, observation)
            .map_err(|error| error.to_string())
    }
}

/// Identity retained by continuations to prevent substitution or address reuse.
#[derive(Clone)]
pub struct CheckedRuntimeSchemaIdentity(Arc<()>);
/// Complete immutable schema with freshly checked runtime type/feature metadata.
/// This does not install providers, validate model values or certify source replay.
/// No Deserialize, Default, mutation API or public unchecked constructor is supplied.
pub struct CheckedRuntimeSchema<A: RuntimeAlternative = String> {
    schema: SchemaRepresentation<A>,
    identity: CheckedRuntimeSchemaIdentity,
}
impl<A: RuntimeAlternative> CheckedRuntimeSchema<A> {
    /// Validate owned wire data before issuing reusable runtime-type authority.
    /// The shared validator remains synchronous here; staged compiler consumers
    /// obtain the same owning result only after their complete validation finishes.
    pub fn prepare_observed(
        schema: SchemaRepresentation<A>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        ValidatedRuntimeTypes::prepare_observed(&schema, observation)?;
        Self::from_checked_compiler_output(schema, observation)
    }
    pub(crate) fn from_checked_compiler_output(
        schema: SchemaRepresentation<A>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        // Portable allowance for the boxed/owning wrapper and Arc identity header.
        observation
            .retain_storage(128 + 32)
            .map_err(PreparationError::Observation)?;
        Ok(Self {
            schema,
            identity: CheckedRuntimeSchemaIdentity(Arc::new(())),
        })
    }
    pub fn schema(&self) -> &SchemaRepresentation<A> {
        &self.schema
    }
    /// Explicit compatibility export loses validation authority before mutation.
    pub fn into_schema(self) -> SchemaRepresentation<A> {
        self.schema
    }
    pub fn identity_observed(
        &self,
        observation: &InvocationContext<'_>,
    ) -> Result<CheckedRuntimeSchemaIdentity, PreparationError> {
        observation
            .retain_storage(32)
            .map_err(PreparationError::Observation)?;
        Ok(self.identity.clone())
    }
    pub fn matches_identity(&self, identity: &CheckedRuntimeSchemaIdentity) -> bool {
        Arc::ptr_eq(&self.identity.0, &identity.0)
    }
    pub fn reference_declaration_preparation_observed(
        &self,
        declaration: &crate::definitions::resources::DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ClassKeyPreparation, PreparationError> {
        ClassKeyPreparation::checked_reference_declaration_observed(self, declaration, observation)
    }
    pub fn class_key_preparation_observed(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        observation: &InvocationContext<'_>,
    ) -> Result<ClassKeyPreparation, PreparationError> {
        ClassKeyPreparation::checked_observed(self, ty, observation)
    }
    /// Select a runtime class by its closed type expression through this
    /// schema's checked authority. Unlike `Schema::class_key_for_type`, this
    /// does not re-run instantiation validation on every lookup: the schema was
    /// validated once when the authority was issued and cannot change since.
    pub fn class_key_for_type(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
    ) -> Result<&str, String> {
        self.class_key_for_type_observed(ty, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn class_key_for_type_observed(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        observation: &InvocationContext<'_>,
    ) -> Result<&str, PreparationError> {
        let mut pending = ClassKeyPreparation::checked_observed(self, ty, observation)?;
        loop {
            observation.work(1).map_err(PreparationError::Observation)?;
            if pending.advance_checked_observed(self, observation)? {
                break;
            }
        }
        let key = pending.finish()?;
        self.schema
            .classes
            .get_key_value(&key)
            .map(|(key, _)| key.as_str())
            .ok_or_else(|| "Missing prepared runtime class key".into())
    }
}
// Debug output (bounded snapshot charging, diagnostics) shows only the data;
// the identity is an opaque allocation and grants nothing when printed.
impl<A: RuntimeAlternative + std::fmt::Debug> std::fmt::Debug for CheckedRuntimeSchema<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CheckedRuntimeSchema")
            .field(&self.schema)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::{
            OwnedRuntimeSourceArrayPreparation, materialize_runtime_source_array_inputs_observed,
        },
        conversion::{InvocationObserver, ProviderId},
        generic_types::{RuntimeType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    fn fixture() -> (
        DefinitionBundle,
        crate::definitions::bundle::SourceId,
        Vec<RuntimeType>,
        crate::definitions::schema::RuntimeSourceArraySchemaInputs,
    ) {
        let source = DefinitionSource::new(
            "memory:checked-schema.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:checked-schema'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/></e:EPackage>",
        );
        let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
        let types = (0..2)
            .map(|i| {
                RuntimeType::new(Type::Classifier {
                    declaration: ids[&vec![i]].clone(),
                    arguments: vec![],
                    raw: i == 0,
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let schema = materialize_runtime_source_array_inputs_observed(
            &bundle,
            &source.id,
            "urn:checked-schema",
            &types,
            None,
            &InvocationContext::default(),
        )
        .unwrap();
        (bundle, source.id, types, schema)
    }
    #[derive(Default)]
    struct Control {
        cancelled: Cell<bool>,
    }
    impl InvocationObserver for Control {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn checkpoint(&self) -> Result<(), String> {
            self.work(0)
        }
        fn work(&self, _: usize) -> Result<(), String> {
            if self.cancelled.get() {
                Err("checked schema cancelled".into())
            } else {
                Ok(())
            }
        }
        fn retain_storage(&self, _: usize) -> Result<(), String> {
            self.work(0)
        }
    }
    #[test]
    fn checked_selection_is_exact_and_rejects_foreign_equal_world_and_mutable_export() {
        let (_, _, types, schema) = fixture();
        let operation = InvocationContext::default();
        let checked = CheckedRuntimeSchema::prepare_observed(schema.clone(), &operation).unwrap();
        let foreign = CheckedRuntimeSchema::prepare_observed(schema.clone(), &operation).unwrap();
        for ty in &types {
            let mut pending = checked
                .class_key_preparation_observed(ty, &operation)
                .unwrap();
            while !pending
                .advance_checked_observed(&checked, &operation)
                .unwrap()
            {}
            assert_eq!(
                pending.finish().unwrap(),
                schema.class_key_for_type(ty).unwrap()
            );
        }
        let mut pending = checked
            .class_key_preparation_observed(&types[1], &operation)
            .unwrap();
        assert!(
            pending
                .advance_checked_observed(&foreign, &operation)
                .is_err()
        );
        assert!(
            pending
                .advance_checked_observed(&checked, &operation)
                .is_err()
        );
        assert!(pending.finish().is_err());
        let mut pending = checked
            .class_key_preparation_observed(&types[1], &operation)
            .unwrap();
        assert!(pending.advance_observed(&schema, &operation).is_err());
        assert!(pending.finish().is_err());
        let mut raw = checked.into_schema();
        let duplicated = raw.instantiations.values().next().unwrap().clone();
        raw.instantiations
            .insert("forged-duplicate".into(), duplicated);
        assert!(CheckedRuntimeSchema::prepare_observed(raw, &operation).is_err());
    }
    #[test]
    fn checked_selection_cancellation_is_sticky_and_fresh_selection_recovers() {
        let (_, _, types, schema) = fixture();
        let checked =
            CheckedRuntimeSchema::prepare_observed(schema, &InvocationContext::default()).unwrap();
        let control = Rc::new(Control::default());
        let operation = InvocationContext::new(control.clone());
        let mut pending = checked
            .class_key_preparation_observed(&types[1], &operation)
            .unwrap();
        control.cancelled.set(true);
        assert!(
            pending
                .advance_checked_observed(&checked, &operation)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        control.cancelled.set(false);
        assert!(
            pending
                .advance_checked_observed(&checked, &operation)
                .is_err()
        );
        assert!(pending.finish().is_err());
        let mut fresh = checked
            .class_key_preparation_observed(&types[1], &operation)
            .unwrap();
        while !fresh
            .advance_checked_observed(&checked, &operation)
            .unwrap()
        {}
        assert!(fresh.finish().is_ok());
    }
    #[test]
    fn retained_compiler_checked_finish_requires_completion_preserves_inputs_and_observation_errors()
     {
        let (bundle, entry, types, schema) = fixture();
        let operation = InvocationContext::default();
        let pending = OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
            bundle.clone(),
            entry.clone(),
            "urn:checked-schema".into(),
            &types,
            None,
            &operation,
        )
        .unwrap_or_else(|_| panic!("begin failed"));
        let incomplete = pending.finish_checked(&operation);
        assert_eq!(incomplete.bundle, bundle);
        assert!(incomplete.result.is_err());
        for cancel_finish in [false, true] {
            let mut pending =
                OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
                    bundle.clone(),
                    entry.clone(),
                    "urn:checked-schema".into(),
                    &types,
                    None,
                    &operation,
                )
                .unwrap_or_else(|_| panic!("begin failed"));
            while !pending.advance(None, &operation).unwrap() {}
            let control = Rc::new(Control {
                cancelled: Cell::new(cancel_finish),
            });
            let completed = pending.finish_checked(&InvocationContext::new(control));
            assert_eq!(completed.bundle, bundle);
            assert_eq!(completed.entry_source, entry);
            assert_eq!(completed.namespace, "urn:checked-schema");
            if cancel_finish {
                assert!(matches!(
                    completed.result,
                    Err(crate::definitions::ecore::ObservedImportError::Observation(
                        _
                    ))
                ));
            } else {
                assert_eq!(
                    serde_json::to_value(
                        completed
                            .result
                            .unwrap_or_else(|_| panic!("finish failed"))
                            .schema()
                    )
                    .unwrap(),
                    serde_json::to_value(&schema).unwrap()
                );
            }
        }
    }
}
