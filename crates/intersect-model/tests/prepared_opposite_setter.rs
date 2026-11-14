//! SAME canonical setter, with only exact immutable runtime validation reused.
use intersect_model::{
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::InvocationContext,
        ecore_migration::BundleMigrationReport,
        model_definition::ModelCompilation,
        preparation::PreparedModelDefinition,
        schema::Schema,
    },
    frozen::FrozenStore,
    storage::{self, ModelRead, ModelStore, StorageResult},
};
use serde_json::{Value, json};
const XML: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="preparedOpposites" nsURI="urn:prepared-opposites">
<eClassifiers xsi:type="e:EClass" name="Node">
<eStructuralFeatures xsi:type="e:EAttribute" name="label" unsettable="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
<eStructuralFeatures xsi:type="e:EReference" name="children" upperBound="-1" containment="true" eType="#//Node" eOpposite="#//Node/parent"/>
<eStructuralFeatures xsi:type="e:EReference" name="parent" eType="#//Node" eOpposite="#//Node/children"/>
</eClassifiers></e:EPackage>"##;
fn inputs() -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let source = DefinitionSource::new("memory:prepared-opposites.ecore", SourceKind::Ecore, XML);
    let mut rules = DefinitionSource::new(
        "memory:prepared-opposites.rules",
        SourceKind::RcoreBehavior,
        r#"rcore 1; package preparedOpposites version "1";"#,
    );
    rules.dependencies = vec![source.id.clone()];
    let original = ModelCompilation {
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![source.clone(), rules.clone()],
            namespaces: vec![],
        },
        structure: source.id.clone(),
        behavior: rules.id,
        namespace: "urn:prepared-opposites".into(),
        materialized_types: vec![],
    };
    let migration = intersect_model::migration::migrate_ecore_bundle(
        original.bundle.clone(),
        &[(source.id, "memory:prepared-opposites.rcore".into())].into(),
    )
    .map_err(|e| e.to_string())?;
    Ok((migration.remap_model_compilation(&original)?, migration))
}
fn definition<'a>(
    input: &'a ModelCompilation,
    migration: &'a BundleMigrationReport,
) -> StorageResult<PreparedModelDefinition<'a>> {
    crate::model_fixture::prepare(
        intersect_model::definitions::model_definition::StructureSelection {
            bundle: &input.bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        migration,
        intersect_model::definitions::ecore::ParseLimits::default(),
    )
}
fn graph(schema: &Schema) -> StorageResult<FrozenStore> {
    let mut store = FrozenStore::for_schema(schema).map_err(|e| e.to_string())?;
    storage::atomic(&mut store, |store| {
        for (index, id) in ["a", "b", "c"].into_iter().enumerate() {
            store.create_element(
                schema,
                id,
                "Node",
                "memory:prepared-opposites-model",
                index,
                index + 1,
            )?;
        }
        Ok(())
    })?;
    Ok(store)
}
#[test]
fn prepared_and_literal_edits_keep_reciprocals_assignment_and_failure_rollback() -> StorageResult<()>
{
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let schema = definition.schema();
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let mut literal = graph(schema)?;
    let mut prepared = graph(schema)?;
    for (owner, field, value) in [
        ("a", "children", json!(["b", "c"])),
        ("b", "label", json!("authored")),
        ("a", "children", json!(["c"])),
        ("b", "parent", json!("c")),
        ("b", "parent", Value::Null),
    ] {
        storage::set_with_opposite(&mut literal, schema, owner, field, value.clone())?;
        storage::set_with_opposite_prepared(&mut prepared, schema, &metadata, owner, field, value)?;
        assert_eq!(storage::export(&prepared)?, storage::export(&literal)?);
    }
    assert_eq!(prepared.value("c", "parent")?, Some(json!("a")));
    assert!(storage::is_explicitly_set(&prepared, schema, "b", "label")?);
    for (owner, field, value) in [
        ("a", "children", json!(["missing"])),
        ("a", "children", json!([123])),
        ("b", "missingField", Value::Null),
        ("missing", "parent", json!("a")),
    ] {
        let before = storage::export(&prepared)?;
        let old = storage::set_with_opposite(&mut literal, schema, owner, field, value.clone());
        let new = storage::set_with_opposite_prepared(
            &mut prepared,
            schema,
            &metadata,
            owner,
            field,
            value,
        );
        assert!(
            old.is_err(),
            "Known immediate rejection accepted: {owner}.{field}"
        );
        assert_eq!(new, old);
        assert_eq!(storage::export(&prepared)?, before);
        assert_eq!(storage::export(&literal)?, before);
    }
    Ok(())
}
#[test]
fn equal_copied_schema_and_foreign_prepared_owner_cannot_supply_the_proof() -> StorageResult<()> {
    let (input, migration) = inputs()?;
    let first = definition(&input, &migration)?;
    let second = definition(&input, &migration)?;
    let first_metadata = first
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let second_metadata = second
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let mut store = graph(first.schema())?;
    let before = storage::export(&store)?;
    let copy = first.schema().clone();
    for (schema, metadata) in [(&copy, &first_metadata), (first.schema(), &second_metadata)] {
        assert_eq!(
            storage::set_with_opposite_prepared(
                &mut store,
                schema,
                metadata,
                "a",
                "children",
                json!(["b"])
            ),
            Err("Prepared opposite setter requires the exact schema owner".into())
        );
        assert_eq!(storage::export(&store)?, before);
    }
    let mut stale = FrozenStore::for_schema(first.schema()).map_err(|e| e.to_string())?;
    storage::atomic(&mut stale, |store| {
        store.set_revision("different-live-revision")
    })?;
    let stale_before = storage::export(&stale)?;
    let literal =
        storage::set_with_opposite(&mut stale, first.schema(), "a", "children", json!(["b"]));
    let prepared = storage::set_with_opposite_prepared(
        &mut stale,
        first.schema(),
        &first_metadata,
        "a",
        "children",
        json!(["b"]),
    );
    assert_eq!(prepared, literal);
    assert_eq!(
        prepared,
        Err("Model belongs to a different schema revision".into())
    );
    assert_eq!(storage::export(&stale)?, stale_before);
    let mut invalid = first.schema().clone();
    invalid
        .classes
        .get_mut("Node")
        .ok_or("Missing Node")?
        .parents
        .push("MissingParent".into());
    assert!(invalid.features("Node").is_err());
    assert!(
        storage::set_with_opposite(&mut store, &invalid, "a", "children", json!(["b"])).is_err()
    );
    assert_eq!(storage::export(&store)?, before);
    Ok(())
}
