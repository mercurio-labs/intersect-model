//! Native custom-value column controls; no Xtext or Java dependency.
use intersect_model::{
    conversion::RgbConverter,
    core::{model::Model, query::*, storage::ModelView},
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::{compile_structure_bundle_with_conversions, import_datatype_bundle},
        conversion::{ConversionContract, ConversionRegistry, ConvertedValue, ProviderId},
        resources::{DeclarationId, ResourceSet},
        schema::Schema,
    },
};
use serde_json::{Value, json};
const SOURCE: &str = r##"rcore 1; package queryPaint version "1";
metamodel "urn:query:paint" prefix p {
 datatype RGB instanceClass "example.RGB" {}
 class Paint {
  attr color: RGB [0,1];
  attr colors: RGB [0,*];
  attr computed: RGB [0,1] derived;
 }
}"##;
fn bundle() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:query-paint.rcore",
            SourceKind::Rcore,
            SOURCE,
        )],
        namespaces: vec![],
    }
}
fn declaration() -> DeclarationId {
    let input = bundle();
    ResourceSet::new(&input)
        .unwrap()
        .resolve(&input.sources[0].id, "#//RGB")
        .unwrap()
}
enum Paint {}
impl Entity for Paint {
    const CLASS: &'static str = "Paint";
    const SCHEMA_URI: &'static str = "urn:query:paint";
}
enum Rgb {}
impl ConvertedDatatype for Rgb {
    fn declaration() -> DeclarationId {
        declaration()
    }
}
enum OtherRgb {}
impl ConvertedDatatype for OtherRgb {
    fn declaration() -> DeclarationId {
        let mut id = declaration();
        id.source.sha256 = "f".repeat(64);
        id
    }
}

#[test]
fn json_datatype_formatting_checks_destination_provider_and_round_trip() {
    let (schema, _, registry, value) = fixture();
    let datatype = &schema.datatypes["RGB"];
    assert_eq!(
        registry.format_datatype(datatype, &value).unwrap(),
        "#123ABC"
    );
    assert_eq!(
        datatype.format_json(&value, Some(&registry)).unwrap(),
        "#123ABC"
    );
    assert!(datatype.format_json(&value, None).is_err());
    let mut wrong = value.clone();
    wrong["provider"]["version"] = json!("wrong");
    assert!(registry.format_datatype(datatype, &wrong).is_err());
}
fn fixture() -> (Schema, Model, ConversionRegistry<'static>, Value) {
    let input = bundle();
    let id = declaration();
    let datatype = import_datatype_bundle(&input, &id).unwrap();
    let contract = ConversionContract {
        datatype: id.clone(),
        provider: ProviderId {
            name: "example.rgb".into(),
            version: "1".into(),
        },
        serializable: true,
        declarations: vec![],
        members: vec![],
    };
    let mut registry = ConversionRegistry::default();
    registry
        .register_shared(&datatype, std::rc::Rc::new(RgbConverter::new(contract)))
        .unwrap();
    let schema = compile_structure_bundle_with_conversions(
        &input,
        &input.sources[0].id,
        "urn:query:paint",
        &registry,
    )
    .unwrap();
    let color = serde_json::to_value(registry.parse(&id, "#123ABC").unwrap()).unwrap();
    let mut model = Model::default();
    let index = model
        .create(&schema, "paint", "Paint", "memory:paint", 0, 1)
        .unwrap();
    model.elements[index]
        .values
        .insert("color".into(), color.clone());
    model.elements[index]
        .values
        .insert("colors".into(), json!([color.clone()]));
    model.validate_with_conversions(&schema, &registry).unwrap();
    (schema, model, registry, color)
}
fn plan(schema: &Schema, path: &str) -> QueryPlan {
    QueryPlan {
        schema_uri: schema.uri.clone(),
        schema_revision: schema.revision.clone(),
        from: "Paint".into(),
        filter: None,
        select: vec![Column {
            name: "color".into(),
            expression: Expression::Field { path: path.into() },
        }],
        order_by: vec![],
        limit: 10,
    }
}
pub fn explicit_nominal_columns_share_typed_dynamic_prepared_and_collection_execution() {
    let (schema, mut model, registry, color) = fixture();
    let parameters = Parameters::new();
    let plan = plan(&schema, "color");
    assert!(
        plan.execute(
            &schema,
            &ModelView::new(&model).unwrap(),
            &parameters,
            &NoDerivedValues,
            QueryLimits::default()
        )
        .unwrap_err()
        .contains("Unsupported query datatype RGB")
    );
    let binding = Binding::<Paint>::with_conversions(&schema, &registry).unwrap();
    assert!(binding.field::<String>("color").is_err());
    assert!(
        binding
            .field::<Option<ConvertedColumn<OtherRgb>>>("color")
            .is_err()
    );
    let color_field = binding
        .field::<Option<ConvertedColumn<Rgb>>>("color")
        .unwrap();
    let many = binding
        .field::<Vec<ConvertedColumn<Rgb>>>("colors")
        .unwrap();
    let typed = binding.query().select((color_field, many));
    let prepared = typed
        .prepare_with_conversions(&schema, &parameters, &NoDerivedValues, &registry)
        .unwrap();
    let result = prepared
        .execute(&ModelView::new(&model).unwrap(), QueryLimits::default())
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        serde_json::to_value(result.rows[0].0.as_ref().unwrap().converted()).unwrap(),
        color
    );
    assert_eq!(
        serde_json::to_value(result.rows[0].1[0].converted()).unwrap(),
        color
    );
    assert_eq!(
        plan.execute_with_conversions(
            &schema,
            &ModelView::new(&model).unwrap(),
            &parameters,
            &NoDerivedValues,
            &registry,
            QueryLimits::default()
        )
        .unwrap()
        .rows,
        json!([[color]])
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_array().unwrap().clone())
            .collect::<Vec<_>>()
    );
    model.elements[0].values.insert("color".into(), Value::Null);
    model.elements[0].values.insert("colors".into(), json!([]));
    let result = prepared
        .execute(&ModelView::new(&model).unwrap(), QueryLimits::default())
        .unwrap();
    assert!(result.rows[0].0.is_none());
    assert!(result.rows[0].1.is_empty());
}
pub fn selected_provider_destination_and_payload_are_checked_before_results() {
    let (schema, model, registry, _) = fixture();
    let parameters = Parameters::new();
    let plan = plan(&schema, "color");
    let empty = ConversionRegistry::default();
    assert!(
        plan.prepare_with_conversions(&schema, &parameters, &NoDerivedValues, &empty)
            .is_err()
    );
    // A compiled provider selection applies even when no payload is present.
    let datatype = &schema.datatypes["RGB"];
    let mut changed = schema.conversion_contracts[0].clone();
    changed.provider.version = "2".into();
    let mut stale = ConversionRegistry::default();
    stale
        .register_shared(datatype, std::rc::Rc::new(RgbConverter::new(changed)))
        .unwrap();
    let mut absent = model.clone();
    absent.elements[0]
        .values
        .insert("color".into(), Value::Null);
    absent.elements[0].values.insert("colors".into(), json!([]));
    let mut no_rows = absent.clone();
    no_rows.elements.clear();
    no_rows.resources.clear();
    for candidate in [&absent, &no_rows] {
        for path in ["color", "colors"] {
            let query = self::plan(&schema, path);
            assert!(
                query
                    .execute_with_conversions(
                        &schema,
                        &ModelView::new(candidate).unwrap(),
                        &parameters,
                        &NoDerivedValues,
                        &stale,
                        QueryLimits::default()
                    )
                    .unwrap_err()
                    .contains("compiled schema contract")
            );
        }
    }
    for field in ["provider", "datatype", "payload"] {
        let mut bad = model.clone();
        let value = bad.elements[0].values.get_mut("color").unwrap();
        match field {
            "provider" => value["provider"]["version"] = json!("stale"),
            "datatype" => value["datatype"]["source"]["sha256"] = json!("f".repeat(64)),
            _ => value["value"]["red"] = json!(256),
        }
        assert!(
            plan.execute_with_conversions(
                &schema,
                &ModelView::new(&bad).unwrap(),
                &parameters,
                &NoDerivedValues,
                &registry,
                QueryLimits::default()
            )
            .is_err(),
            "{field}"
        );
    }
    assert!(
        plan.execute_with_conversions(
            &schema,
            &ModelView::new(&model).unwrap(),
            &parameters,
            &NoDerivedValues,
            &registry,
            QueryLimits {
                max_steps: 0,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert_eq!(
        plan.execute_with_conversions(
            &schema,
            &ModelView::new(&model).unwrap(),
            &parameters,
            &NoDerivedValues,
            &registry,
            QueryLimits::default()
        )
        .unwrap()
        .rows
        .len(),
        1
    );
}
pub fn nominal_projection_never_invents_comparison_or_ordering_semantics() {
    let (schema, model, registry, _) = fixture();
    let parameters = Parameters::new();
    let mut query = plan(&schema, "color");
    query.filter = Some(Predicate::Compare {
        left: Expression::Field {
            path: "color".into(),
        },
        right: Expression::Field {
            path: "color".into(),
        },
        comparison: Comparison::Eq,
    });
    assert!(
        query
            .validate_with_conversions(&schema, &parameters, &NoDerivedValues, &registry)
            .unwrap_err()
            .contains("projection only")
    );
    query.filter = None;
    query.order_by.push(Ordering {
        expression: Expression::Field {
            path: "color".into(),
        },
        descending: false,
    });
    assert!(
        query
            .validate_with_conversions(&schema, &parameters, &NoDerivedValues, &registry)
            .unwrap_err()
            .contains("ordering")
    );
    query.order_by.clear();
    query.filter = Some(Predicate::IsNull {
        expression: Expression::Field {
            path: "color".into(),
        },
    });
    assert!(
        query
            .execute_with_conversions(
                &schema,
                &ModelView::new(&model).unwrap(),
                &parameters,
                &NoDerivedValues,
                &registry,
                QueryLimits::default()
            )
            .unwrap()
            .rows
            .is_empty()
    );
}
pub fn derived_nominal_results_are_validated_against_the_selected_destination() {
    struct Derived(Value);
    impl DerivedValues for Derived {
        fn supports(&self, class: &str, feature: &str) -> bool {
            class == "Paint" && feature == "computed"
        }
        fn read(
            &self,
            _: &dyn intersect_model::storage::ModelRead,
            _: &str,
            _: &str,
            budget: &mut QueryBudget,
        ) -> QueryResult<Value> {
            budget.charge()?;
            Ok(self.0.clone())
        }
    }
    let (schema, model, registry, color) = fixture();
    let parameters = Parameters::new();
    let query = plan(&schema, "computed");
    let good = Derived(color.clone());
    assert_eq!(
        query
            .execute_with_conversions(
                &schema,
                &ModelView::new(&model).unwrap(),
                &parameters,
                &good,
                &registry,
                QueryLimits::default()
            )
            .unwrap()
            .rows[0][0],
        color
    );
    let mut invalid = color;
    invalid["value"]["blue"] = json!(-1);
    assert!(
        query
            .execute_with_conversions(
                &schema,
                &ModelView::new(&model).unwrap(),
                &parameters,
                &Derived(invalid),
                &registry,
                QueryLimits::default()
            )
            .is_err()
    );
    let wrong = ConvertedValue::Converted {
        datatype: OtherRgb::declaration(),
        provider: ProviderId {
            name: "example.rgb".into(),
            version: "1".into(),
        },
        value: json!({"red":1,"green":2,"blue":3}),
    };
    assert!(ConvertedColumn::<Rgb>::decode(serde_json::to_value(wrong).unwrap()).is_err());
}

#[cfg(test)]
mod tests {
    #[test]
    fn group_1() {
        super::explicit_nominal_columns_share_typed_dynamic_prepared_and_collection_execution();
    }
    #[test]
    fn group_2() {
        super::selected_provider_destination_and_payload_are_checked_before_results();
    }
    #[test]
    fn group_3() {
        super::nominal_projection_never_invents_comparison_or_ordering_semantics();
    }
    #[test]
    fn group_4() {
        super::derived_nominal_results_are_validated_against_the_selected_destination();
    }
}
