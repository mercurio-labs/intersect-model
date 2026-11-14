//! Handwritten oracle for the prepared construction-only computed setter: the
//! prepared owner's validation proof and the borrowed feature index change
//! no result, model or charged work, and cannot be transferred to another schema.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::{definition, inputs_for};
use crate::definitions::schema::{FeatureIndex, FeatureLookup};
use serde_json::json;

const XML: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="reuse" nsURI="urn:source-reuse"><eClassifiers xsi:type="e:EClass" name="A"><eStructuralFeatures xsi:type="e:EAttribute" name="label" unsettable="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EReference" name="children" containment="true" upperBound="-1" eType="#//A" eOpposite="#//A/parent"/><eStructuralFeatures xsi:type="e:EReference" name="parent" eType="#//A" eOpposite="#//A/children"/><eStructuralFeatures xsi:type="e:EAttribute" name="computed" derived="true" changeable="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="count" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/></eClassifiers><eClassifiers xsi:type="e:EClass" name="B" eSuperTypes="#//A"/></e:EPackage>"##;
const URI: &str = "memory:setting-prepared";

/// Creates a child and links it by its opposite, then writes the label.
struct Provider<'a> {
    schema: &'a Schema,
}
impl DerivedValues for Provider<'_> {
    fn schema_revision(&self) -> Option<&str> {
        Some(&self.schema.revision)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        (class == "A" || class == "B") && feature == "computed"
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        _: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Value> {
        budget.charge()?;
        Ok(store.value(id, "label")?.unwrap_or(Value::Null))
    }
}
impl SettingValues for Provider<'_> {
    fn plan(
        &self,
        _: &dyn ModelRead,
        id: &str,
        _: &str,
        edit: &ComputedEdit,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<SettingEdit>> {
        budget.charge()?;
        Ok(match edit {
            ComputedEdit::Set(Value::String(value)) if value == "unknown-feature" => {
                vec![SettingEdit::Set {
                    id: id.into(),
                    feature: "missing".into(),
                    value: json!("x"),
                }]
            }
            ComputedEdit::Set(Value::String(value)) if value == "bad-label" => {
                vec![SettingEdit::Set {
                    id: id.into(),
                    feature: "label".into(),
                    value: json!(7),
                }]
            }
            ComputedEdit::Set(Value::String(value)) => {
                let child = format!("{id}.{value}");
                vec![
                    SettingEdit::Create {
                        id: child.clone(),
                        class: "B".into(),
                        resource: URI.into(),
                        start: 0,
                        end: 1,
                    },
                    SettingEdit::Set {
                        id: child.clone(),
                        feature: "parent".into(),
                        value: json!(id),
                    },
                    SettingEdit::Set {
                        id: child,
                        feature: "count".into(),
                        value: json!(value.len()),
                    },
                    SettingEdit::Set {
                        id: id.into(),
                        feature: "label".into(),
                        value: json!(value),
                    },
                ]
            }
            ComputedEdit::Unset => vec![SettingEdit::Unset {
                id: id.into(),
                feature: "label".into(),
            }],
            _ => return Err("Prepared setting oracle requires Set or Unset".into()),
        })
    }
}

fn graph(schema: &Schema) -> StorageResult<FrozenStore> {
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    atomic(&mut store, |store| {
        bind_schema(store, schema)?;
        store.create_element(schema, "a", "A", URI, 0, 1)?;
        store.create_element(schema, "b", "B", URI, 1, 2)
    })?;
    Ok(store)
}
fn edit(index: usize) -> ComputedEdit {
    match index {
        0 => ComputedEdit::Set(json!("x")),
        1 => ComputedEdit::Set(json!("yy")),
        2 => ComputedEdit::Set(json!("unknown-feature")),
        3 => ComputedEdit::Set(json!("bad-label")),
        4 => ComputedEdit::Set(json!(5)),
        5 => ComputedEdit::Append(json!("x")),
        6 => ComputedEdit::Unset,
        _ => ComputedEdit::Set(json!("zzz")),
    }
}

#[test]
fn prepared_construction_only_matches_results_models_and_work() -> StorageResult<()> {
    let (input, migration) = inputs_for(XML)?;
    let d = definition(&input, &migration)?;
    let metadata = d
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let provider = Provider { schema: d.schema() };
    let settings = ComputedSettings {
        schema: d.schema(),
        provider: &provider,
        conversions: None,
    };
    let index = FeatureIndex::new(d.schema());
    let mut literal = graph(d.schema())?;
    let mut prepared = graph(d.schema())?;
    let mut failures = Vec::new();
    for owner in ["a", "b"] {
        for step in 0..8 {
            let mut old_budget = QueryBudget::new(100_000);
            let mut new_budget = QueryBudget::new(100_000);
            let old = settings.apply_construction_only(
                &mut literal,
                owner,
                "computed",
                edit(step),
                &mut old_budget,
            );
            let new = settings.apply_construction_only_prepared(
                &mut prepared,
                owner,
                "computed",
                edit(step),
                &mut new_budget,
                &metadata,
                &index,
            );
            assert_eq!(new, old, "{owner} step {step}");
            if let Err(error) = &old {
                failures.push(format!("{owner}{step}: {error}"));
            }
            assert_eq!(new_budget.reads, old_budget.reads);
            assert_eq!(export(&prepared)?, export(&literal)?);
        }
    }
    // Unknown feature, invalid stored value, invalid computed value, append.
    let expected = ["a", "b"].into_iter().flat_map(|owner| {
        [
            (2, "Unknown feature"),
            (3, "Invalid EString wire value"),
            (4, "Invalid EString wire value"),
            (5, "Computed append requires a many-valued feature"),
        ]
        .map(|(step, error)| format!("{owner}{step}: {error}"))
    });
    assert_eq!(failures, expected.collect::<Vec<_>>());
    assert_eq!(
        prepared.value("a", "children")?,
        Some(json!(["a.x", "a.yy", "a.zzz"]))
    );
    assert_eq!(prepared.value("b.zzz", "parent")?, Some(json!("b")));
    Ok(())
}

#[test]
fn prepared_construction_only_rejects_copied_invalid_and_foreign_schemas() -> StorageResult<()> {
    let (input, migration) = inputs_for(XML)?;
    let d = definition(&input, &migration)?;
    let other = definition(&input, &migration)?;
    let metadata = d
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let foreign = other
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let copy = d.schema().clone();
    let mut invalid = d.schema().clone();
    invalid
        .classes
        .get_mut("B")
        .ok_or("Missing B")?
        .parents
        .push("MissingParent".into());
    assert!(invalid.features("B").is_err());
    let mut store = graph(d.schema())?;
    let before = export(&store)?;
    let apply = |store: &mut FrozenStore,
                 schema: &Schema,
                 metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
                 index: &dyn FeatureLookup| {
        let provider = Provider { schema };
        ComputedSettings {
            schema,
            provider: &provider,
            conversions: None,
        }
        .apply_construction_only_prepared(
            store,
            "a",
            "computed",
            ComputedEdit::Set(json!("x")),
            &mut QueryBudget::new(100_000),
            metadata,
            index,
        )
    };
    let owner_error = Err("Prepared computed setting requires the exact schema owner".into());
    // An equal copy, an invalid copy or another owner's proof never validates.
    assert_eq!(apply(&mut store, &copy, &metadata, &copy), owner_error);
    assert_eq!(
        apply(&mut store, &invalid, &metadata, &invalid),
        owner_error
    );
    assert_eq!(
        apply(
            &mut store,
            d.schema(),
            &foreign,
            &FeatureIndex::new(d.schema())
        ),
        owner_error
    );
    // A feature index over another schema is refused before any work.
    assert_eq!(
        apply(&mut store, d.schema(), &metadata, &FeatureIndex::new(&copy)),
        Err("Computed setting feature index belongs to another schema".into())
    );
    assert_eq!(export(&store)?, before);
    // The literal route still validates the invalid copy and rejects it.
    let provider = Provider { schema: &invalid };
    assert!(
        ComputedSettings {
            schema: &invalid,
            provider: &provider,
            conversions: None,
        }
        .apply_construction_only(
            &mut store,
            "b",
            "computed",
            ComputedEdit::Set(json!("x")),
            &mut QueryBudget::new(100_000),
        )
        .is_err()
    );
    assert_eq!(export(&store)?, before);
    // The live store revision is still bound per edit.
    let mut stale = FrozenStore::for_schema(d.schema()).map_err(|error| error.to_string())?;
    atomic(&mut stale, |store| {
        store.set_revision("different-live-revision")
    })?;
    let stale_before = export(&stale)?;
    assert_eq!(
        apply(
            &mut stale,
            d.schema(),
            &metadata,
            &FeatureIndex::new(d.schema())
        ),
        Err("Model belongs to a different schema revision".into())
    );
    assert_eq!(export(&stale)?, stale_before);
    // Through the proof, the same edit succeeds on the owner's schema.
    assert_eq!(
        apply(
            &mut store,
            d.schema(),
            &metadata,
            &FeatureIndex::new(d.schema())
        ),
        Ok(())
    );
    Ok(())
}

#[test]
fn feature_index_maps_and_prepared_attribute_checks_equal_the_schema() -> StorageResult<()> {
    let (input, migration) = inputs_for(XML)?;
    let d = definition(&input, &migration)?;
    let schema = d.schema();
    let index = FeatureIndex::new(schema);
    for class in ["A", "B"] {
        let map = index.feature_map(class)?;
        assert_eq!(map.fields(), &schema.features(class)?);
        assert!(std::rc::Rc::ptr_eq(&map, &index.feature_map(class)?));
        for field in map.fields().values().filter(|f| !f.reference) {
            assert_eq!(
                schema.require_runtime_attribute_conversion_prepared(class, field, None, &*map),
                schema.require_runtime_attribute_conversion(class, field, None)
            );
            for value in [json!("s"), json!(5), json!(true), Value::Null] {
                assert_eq!(
                    schema.validate_runtime_attribute_prepared(class, field, &value, None, &*map),
                    schema.validate_runtime_attribute(class, field, &value, None)
                );
            }
        }
    }
    assert_eq!(
        index.feature_map("Missing").map(|_| ()),
        schema.features("Missing").map(|_| ())
    );
    // Another receiver's selection is not accepted for this receiver.
    let a = index.feature_map("A")?;
    let label = a.get("label").ok_or("Missing label")?;
    assert_eq!(
        schema.validate_runtime_attribute_prepared("B", label, &json!("s"), None, &*a),
        Err("Attribute does not belong to the receiver's effective features".into())
    );
    Ok(())
}
