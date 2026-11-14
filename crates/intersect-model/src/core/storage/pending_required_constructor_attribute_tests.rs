//! Required constructor-null component only; no language or full lower-bound claim.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::{definition, inputs_for};
use crate::definitions::preparation::PreparedModelDefinition;

const SOURCE: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="reuse" nsURI="urn:source-reuse">
<eClassifiers xsi:type="e:EEnum" name="Kind"><eLiterals name="first"/><eLiterals name="second" value="1"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="A"><eStructuralFeatures xsi:type="e:EAttribute" name="required" lowerBound="1" defaultValueLiteral="first" eType="#//Kind"/><eStructuralFeatures xsi:type="e:EAttribute" name="optional" defaultValueLiteral="first" eType="#//Kind"/></eClassifiers>
</e:EPackage>"##;

struct Native(Vec<DraftClassContract>);
impl DraftPhysicalDefaults for Native {
    fn contracts(&self) -> &[DraftClassContract] {
        &self.0
    }
}
fn native(definition: &PreparedModelDefinition<'_>) -> StorageResult<Native> {
    let schema = definition.schema();
    Ok(Native(vec![DraftClassContract {
        version: 1,
        schema_revision: schema.revision.clone(),
        class: "A".into(),
        class_identity: schema.identity("A"),
        sources: definition
            .sources()
            .sources
            .iter()
            .map(|s| s.id.clone())
            .collect(),
        members: schema
            .effective_features("A")?
            .into_iter()
            .map(|f| DraftMemberDisposition {
                name: f.feature.name.clone(),
                identity: f.identity,
                disposition: if f.feature.name == "required" {
                    DraftDisposition::StoredRequiredNullDefault
                } else {
                    DraftDisposition::StoredNullDefault
                },
            })
            .collect(),
    }]))
}
fn graph(context: &DraftConstructionContext<'_>) -> StorageResult<FrozenStore> {
    let mut store = FrozenStore::for_schema(context.schema).map_err(|error| error.to_string())?;
    atomic(&mut store, |store| {
        context.create(
            store,
            Construction {
                id: "a",
                resource: "memory:required-constructor",
                authored: &[],
            },
            "A",
            0,
            1,
            &mut QueryBudget::new(100_000),
        )
    })?;
    Ok(store)
}
fn with_definition(
    f: impl FnOnce(&PreparedModelDefinition<'_>) -> StorageResult<()>,
) -> StorageResult<()> {
    let (input, migration) = inputs_for(SOURCE)?;
    let prepared = definition(&input, &migration)?;
    f(&prepared)
}
#[test]
fn pending_required_constructor_attribute_transient_null_then_validated_store() -> StorageResult<()>
{
    with_definition(|definition| {
        let native = native(definition)?;
        let context = DraftConstructionContext::bind(
            definition.schema(),
            definition.sources(),
            &native,
            &mut QueryBudget::new(100_000),
        )?;
        let mut store = graph(&context)?;
        let model = export(&store)?;
        let actual = model
            .elements
            .first()
            .ok_or("Missing actual pending receiver")?;
        assert_eq!(actual.values.get("required"), Some(&Value::Null));
        assert_eq!(actual.values.get("optional"), Some(&Value::Null));
        assert!(validate(&store, definition.schema()).is_err());
        let actual =
            serde_json::to_value(definition.schema().enumerations["Kind"].parse("second")?)
                .map_err(|e| e.to_string())?;
        atomic(&mut store, |store| {
            store.set_value("a", "required", Some(actual.clone()))
        })?;
        validate(&store, definition.schema())?;
        assert_eq!(store.value("a", "required")?, Some(actual));
        assert_eq!(store.value("a", "optional")?, Some(Value::Null));
        Ok(())
    })
}

#[test]
fn pending_required_constructor_attribute_missing_codec_and_wrong_source_remain_errors()
-> StorageResult<()> {
    with_definition(|definition| {
        for corruption in 0..3 {
            let mut native = native(definition)?;
            let member = native.0[0]
                .members
                .iter_mut()
                .find(|m| m.name == "required")
                .ok_or("Missing required slot")?;
            match corruption {
                0 => member.disposition = DraftDisposition::StoredNullDefault,
                1 => {
                    member.identity = definition
                        .schema()
                        .effective_feature("A", "optional")?
                        .identity
                }
                _ => {
                    let optional = native.0[0]
                        .members
                        .iter_mut()
                        .find(|m| m.name == "optional")
                        .ok_or("Missing optional slot")?;
                    optional.disposition = DraftDisposition::StoredRequiredNullDefault;
                }
            }
            assert!(
                DraftConstructionContext::bind(
                    definition.schema(),
                    definition.sources(),
                    &native,
                    &mut QueryBudget::new(100_000)
                )
                .is_err(),
                "corruption {corruption}"
            );
        }
        let native = native(definition)?;
        let context = DraftConstructionContext::bind(
            definition.schema(),
            definition.sources(),
            &native,
            &mut QueryBudget::new(100_000),
        )?;
        let store = graph(&context)?;
        let original = export(&store)?;
        let mut missing = original.clone();
        missing.elements[0].values.remove("required");
        let missing = FrozenStore::from_materialized(definition.schema(), &missing)?;
        assert!(validate(&missing, definition.schema()).is_err());
        let mut wrong = original;
        wrong.elements[0]
            .values
            .insert("required".into(), Value::Bool(true));
        let wrong = FrozenStore::from_materialized(definition.schema(), &wrong)?;
        let error =
            validate(&wrong, definition.schema()).expect_err("Wrong enum payload completed");
        assert!(!error.contains("uninitialized"), "{error}");
        assert!(!error.to_ascii_lowercase().contains("budget"), "{error}");
        Ok(())
    })
}
