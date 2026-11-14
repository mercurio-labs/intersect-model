use intersect_model::core::{
    api::{BasicModel, VersionedModel},
    frozen::FrozenStore,
    model::Model,
    query::*,
    storage::{self, IndexPolicy, ModelRead, ModelStore, ModelView},
};
use intersect_model::definitions::{scalars::Builtin, schema::Schema};
use serde_json::{Value, json};

enum Item {}
impl Entity for Item {
    const CLASS: &'static str = "Item";
    const SCHEMA_URI: &'static str = "urn:exact-query";
}

#[test]
fn local_datatype_and_enum_names_cannot_impersonate_builtin_long_values() {
    for classifier in [
        "<eClassifiers x:type='e:EDataType' name='ELong' instanceClassName='java.lang.String'/>",
        "<eClassifiers x:type='e:EEnum' name='ELong'><eLiterals name='ONE' value='1'/></eClassifiers>",
    ] {
        let source = format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:shadow-long'>{classifier}<eClassifiers x:type='e:EClass' name='Item'><eStructuralFeatures x:type='e:EAttribute' name='value' eType='#//ELong'/></eClassifiers></e:EPackage>"
        );
        let schema =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source)
                .unwrap();
        assert_eq!(
            path_type(&schema, "Item", "value").unwrap_err(),
            "Unsupported query datatype ELong"
        );
    }
}

fn fixture() -> (Schema, Model) {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture("<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:exact-query'><eClassifiers x:type='e:EClass' name='Item'><eStructuralFeatures x:type='e:EAttribute' name='value' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELong'/><eStructuralFeatures x:type='e:EAttribute' name='optional' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELongObject'/><eStructuralFeatures x:type='e:EAttribute' name='values' upperBound='-1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELong'/><eStructuralFeatures x:type='e:EAttribute' name='small' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt'/><eStructuralFeatures x:type='e:EAttribute' name='computed' lowerBound='1' derived='true' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELong'/><eStructuralFeatures x:type='e:EAttribute' name='computedSmall' lowerBound='1' derived='true' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt'/></eClassifiers></e:EPackage>").unwrap();
    let mut model = Model::default();
    for (id, decimal) in [
        ("high", "9223372036854775807"),
        ("low", "-9223372036854775808"),
        ("wide", "9007199254740993"),
        ("two", "2"),
    ] {
        model.create(&schema, id, "Item", "r", 0, 0).unwrap();
        model
            .edit(
                &schema,
                &[
                    (
                        id.into(),
                        "value".into(),
                        Builtin::ELong.parse(decimal).unwrap(),
                    ),
                    (id.into(), "small".into(), json!(2)),
                    (
                        id.into(),
                        "values".into(),
                        json!([
                            {"kind":"int64","value":"-9223372036854775808"},
                            {"kind":"int64","value":"9223372036854775807"}
                        ]),
                    ),
                ],
            )
            .unwrap();
    }
    model
        .edit(
            &schema,
            &[(
                "high".into(),
                "optional".into(),
                json!({"kind":"int64","value":"9223372036854775807"}),
            )],
        )
        .unwrap();
    (schema, model)
}

fn plan(schema: &Schema, columns: &[&str]) -> QueryPlan {
    QueryPlan {
        schema_uri: schema.uri.clone(),
        schema_revision: schema.revision.clone(),
        from: "Item".into(),
        filter: None,
        select: columns
            .iter()
            .map(|name| Column {
                name: (*name).into(),
                expression: Expression::Field {
                    path: (*name).into(),
                },
            })
            .collect(),
        order_by: vec![],
        limit: 50,
    }
}

#[test]
fn exact_long_projection_and_order_survive_indexed_unindexed_and_snapshot_readers() {
    let (schema, model) = fixture();
    let before = serde_json::to_vec(&model).unwrap();
    let indexed = FrozenStore::from_materialized(&schema, &model).unwrap();
    let mut unindexed = FrozenStore::with_index_policy(
        indexed.metamodel().clone(),
        IndexPolicy::selected([] as [&str; 0]),
    );
    storage::atomic(&mut unindexed, |store| {
        store.set_revision(&model.schema_revision)?;
        for element in &model.elements {
            store.put(element.clone())?;
        }
        Ok(())
    })
    .unwrap();
    let versioned = VersionedModel::new(
        BasicModel::from_store(
            schema.clone(),
            FrozenStore::from_materialized(&schema, &model).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let snapshot = versioned.snapshot().unwrap();
    let view = ModelView::new(&model).unwrap();
    let mut query = plan(&schema, &["$id", "value", "optional", "values"]);
    query.order_by = vec![Ordering {
        expression: Expression::Field {
            path: "value".into(),
        },
        descending: false,
    }];
    let expected = [
        ("low", "-9223372036854775808"), ("two", "2"),
        ("wide", "9007199254740993"), ("high", "9223372036854775807"),
    ].into_iter().map(|(id, value)| vec![json!(id), json!({"kind":"int64","value":value}),
        if id == "high" { json!({"kind":"int64","value":"9223372036854775807"}) } else { Value::Null },
        json!([{"kind":"int64","value":"-9223372036854775808"},{"kind":"int64","value":"9223372036854775807"}])
    ]).collect::<Vec<_>>();
    for store in [&view as &dyn ModelRead, &indexed, &unindexed, &snapshot] {
        let output = query
            .execute(
                &schema,
                store,
                &Parameters::new(),
                &NoDerivedValues,
                QueryLimits::default(),
            )
            .unwrap();
        assert_eq!(output.rows, expected);
        for (field, literal, count) in [
            ("value", json!(2), 1),
            (
                "value",
                json!({"kind":"int64","value":"9007199254740993"}),
                1,
            ),
            ("small", json!({"kind":"int64","value":"2"}), 4),
        ] {
            let mut filter = plan(&schema, &["$id"]);
            filter.filter = Some(Predicate::Compare {
                left: Expression::Field { path: field.into() },
                comparison: Comparison::Eq,
                right: Expression::Literal { value: literal },
            });
            assert_eq!(
                filter
                    .execute(
                        &schema,
                        store,
                        &Parameters::new(),
                        &NoDerivedValues,
                        QueryLimits::default()
                    )
                    .unwrap()
                    .rows
                    .len(),
                count
            );
        }
    }
    assert_eq!(serde_json::to_vec(&model).unwrap(), before);
}

#[test]
fn typed_long_literals_parameters_and_projection_keep_full_precision_on_wire_replay() {
    let (schema, model) = fixture();
    let store = FrozenStore::from_materialized(&schema, &model).unwrap();
    let binding = Binding::<Item>::new(&schema).unwrap();
    let field = binding.field::<i64>("value").unwrap();
    let query = binding.query().filter(field.eq(9007199254740993)).select((
        binding.field::<String>("$id").unwrap(),
        field,
        binding.field::<Option<i64>>("optional").unwrap(),
        binding.field::<Vec<i64>>("values").unwrap(),
    ));
    let output = query
        .execute(
            &schema,
            &store,
            &Parameters::new(),
            &NoDerivedValues,
            QueryLimits::default(),
        )
        .unwrap();
    assert_eq!(
        output.rows,
        vec![(
            "wide".into(),
            9007199254740993,
            None,
            vec![i64::MIN, i64::MAX]
        )]
    );
    let wire = serde_json::to_value(query.plan().unwrap()).unwrap();
    assert_eq!(
        wire["filter"]["right"]["value"],
        json!({"kind":"int64","value":"9007199254740993"})
    );
    let replay: QueryPlan = serde_json::from_value(wire).unwrap();
    assert_eq!(
        replay
            .execute(
                &schema,
                &store,
                &Parameters::new(),
                &NoDerivedValues,
                QueryLimits::default()
            )
            .unwrap()
            .rows[0][1],
        json!({"kind":"int64","value":"9007199254740993"})
    );
    let parameter = Parameter::<i64>::new("maximum");
    let mut parameters = Parameters::new();
    parameter.bind(&mut parameters, i64::MAX);
    assert_eq!(
        parameters["maximum"],
        json!({"kind":"int64","value":"9223372036854775807"})
    );
    let query = binding
        .query()
        .filter(
            binding
                .field::<i64>("value")
                .unwrap()
                .eq_parameter(&parameter),
        )
        .select(binding.field::<i64>("value").unwrap());
    assert_eq!(
        query
            .execute(
                &schema,
                &store,
                &parameters,
                &NoDerivedValues,
                QueryLimits::default()
            )
            .unwrap()
            .rows,
        [i64::MAX]
    );
}

#[test]
fn malformed_long_literals_and_provider_values_fail_without_mutating_the_model() {
    struct Provider(Value);
    impl DerivedValues for Provider {
        fn supports(&self, class: &str, feature: &str) -> bool {
            class == "Item" && matches!(feature, "computed" | "computedSmall")
        }
        fn read(
            &self,
            _: &dyn ModelRead,
            _: &str,
            _: &str,
            budget: &mut QueryBudget,
        ) -> Result<Value, String> {
            budget.charge()?;
            Ok(self.0.clone())
        }
    }
    let (schema, model) = fixture();
    let store = FrozenStore::from_materialized(&schema, &model).unwrap();
    let before = serde_json::to_vec(&model).unwrap();
    for bad in [
        json!({"kind":"int64","value":"01"}),
        json!({"kind":"int64","value":"9223372036854775808"}),
        json!({"kind":"int64","value":"2","extra":true}),
        json!({"kind":"bigint","value":"2"}),
    ] {
        let mut query = plan(&schema, &["value"]);
        query.filter = Some(Predicate::Compare {
            left: Expression::Field {
                path: "value".into(),
            },
            comparison: Comparison::Eq,
            right: Expression::Literal { value: bad.clone() },
        });
        assert!(
            query
                .execute(
                    &schema,
                    &store,
                    &Parameters::new(),
                    &NoDerivedValues,
                    QueryLimits::default()
                )
                .is_err()
        );
        assert!(
            plan(&schema, &["computed"])
                .execute(
                    &schema,
                    &store,
                    &Parameters::new(),
                    &Provider(bad),
                    QueryLimits::default()
                )
                .is_err()
        );
    }
    assert!(
        plan(&schema, &["computed"])
            .execute(
                &schema,
                &store,
                &Parameters::new(),
                &Provider(json!(2)),
                QueryLimits::default()
            )
            .is_err()
    );
    assert_eq!(
        plan(&schema, &["computed"])
            .execute(
                &schema,
                &store,
                &Parameters::new(),
                &Provider(json!({"kind":"int64","value":"2"})),
                QueryLimits::default()
            )
            .unwrap()
            .rows,
        vec![vec![json!({"kind":"int64","value":"2"})]; 4]
    );
    for bad in [json!({"kind":"int64","value":"2"}), json!(i64::MAX)] {
        assert!(
            plan(&schema, &["computedSmall"])
                .execute(
                    &schema,
                    &store,
                    &Parameters::new(),
                    &Provider(bad),
                    QueryLimits::default()
                )
                .is_err()
        );
    }
    assert_eq!(
        plan(&schema, &["computedSmall"])
            .execute(
                &schema,
                &store,
                &Parameters::new(),
                &Provider(json!(2)),
                QueryLimits::default()
            )
            .unwrap()
            .rows,
        vec![vec![json!(2)]; 4]
    );
    assert_eq!(serde_json::to_vec(&model).unwrap(), before);
}
