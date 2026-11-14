//! Native schema identity controls, not generic compiler qualification.
use intersect_model::core::model::Model;
use intersect_model::definitions::{
    generic_types::{ClosedType, Type},
    schema::Schema,
};
use serde_json::json;
#[path = "common/generic_schema.rs"]
mod generic_schema;
use generic_schema::schema;

#[test]
fn generic_instances_keep_distinct_identity_through_construction_assignment_and_reload() {
    let schema = schema();
    let mut model = Model::default();
    model
        .create(&schema, "a", "box_value", "memory:model", 0, 0)
        .unwrap();
    model
        .create(&schema, "b", "box_other", "memory:model", 0, 0)
        .unwrap();
    model
        .create(&schema, "use", "Use", "memory:model", 0, 0)
        .unwrap();
    assert_ne!(
        model.elements[0].type_identity,
        model.elements[1].type_identity
    );
    assert_eq!(
        schema
            .instantiated_class_key(&schema.instantiations["box_value"])
            .unwrap(),
        "box_value"
    );
    assert!(
        schema
            .class_key(&schema.declarations["box_value"].source, "urn:g", "Box")
            .is_err()
    );
    model
        .edit(&schema, &[("use".into(), "value".into(), json!("a"))])
        .unwrap();
    let before = model.clone();
    assert!(
        model
            .edit(&schema, &[("use".into(), "value".into(), json!("b"))])
            .is_err()
    );
    assert_eq!(model, before);
    let reloaded: Model = serde_json::from_str(&serde_json::to_string(&model).unwrap()).unwrap();
    let restored: Schema = serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    reloaded.validate(&restored).unwrap();
    assert_eq!(schema, restored);
    let mut forged = reloaded;
    forged.elements[0].type_identity = forged.elements[1].type_identity.clone();
    assert!(forged.validate(&restored).is_err());
}
#[test]
fn source_revisions_argument_order_and_deserialization_cannot_collapse_closed_identity() {
    // Independently encoded canonical wire value pins the persisted identity version.
    let fixed: ClosedType = serde_json::from_str(r#"{"kind":"classifier","declaration":{"source":{"uri":"urn:identity","sha256":"0000000000000000000000000000000000000000000000000000000000000000"},"path":[1]},"arguments":[{"kind":"classifier","declaration":{"source":{"uri":"urn:identity","sha256":"0000000000000000000000000000000000000000000000000000000000000000"},"path":[2]},"arguments":[],"raw":false}],"raw":false}"#).unwrap();
    assert_eq!(
        fixed.identity(),
        "intersect:generic:1:65eb166adf9742ef802bfa1b10d215e397d33318c5223fd4cd1065292444c953"
    );

    let schema = schema();
    let original = &schema.instantiations["box_value"];
    let mut expression = original.expression().clone();
    if let Type::Classifier { arguments, .. } = &mut expression
        && let Type::Classifier { declaration, .. } = &mut arguments[0]
    {
        declaration.source.sha256 = "1".repeat(64);
    }
    let changed = ClosedType::new(expression).unwrap();
    assert_ne!(original.identity(), changed.identity());
    let mut schema_changed = schema.clone();
    schema_changed
        .instantiations
        .insert("box_value".into(), changed.into());
    let mut model = Model::default();
    model
        .create(&schema, "a", "box_value", "memory:model", 0, 0)
        .unwrap();
    assert!(model.validate(&schema_changed).is_err());
    let mut ordered = original.expression().clone();
    if let Type::Classifier { arguments, .. } = &mut ordered {
        arguments.push(schema.instantiations["box_other"].expression().clone());
    }
    let left = ClosedType::new(ordered.clone()).unwrap();
    if let Type::Classifier { arguments, .. } = &mut ordered {
        arguments.reverse();
    }
    assert_ne!(
        left.identity(),
        ClosedType::new(ordered).unwrap().identity()
    );
    let mut wire = serde_json::to_value(original).unwrap();
    wire["arguments"][0] = json!({"kind":"wildcard","upper":null,"lower":null});
    assert!(serde_json::from_value::<ClosedType>(wire).is_err());
    let mut wire = serde_json::to_value(original).unwrap();
    wire["raw"] = json!(true);
    assert!(serde_json::from_value::<ClosedType>(wire).is_err());
}
#[test]
fn inconsistent_instantiation_tables_reject_before_model_mutation() {
    let original = schema();
    for mode in 0..3 {
        let mut schema = original.clone();
        match mode {
            0 => {
                schema.declarations.get_mut("box_value").unwrap().path = vec![1];
            }
            1 => {
                schema
                    .instantiations
                    .insert("missing".into(), schema.instantiations["box_value"].clone());
            }
            _ => {
                schema.instantiations.insert(
                    "box_other".into(),
                    schema.instantiations["box_value"].clone(),
                );
            }
        }
        let mut model = Model::default();
        assert!(
            model
                .create(&schema, "a", "box_value", "memory:model", 0, 0)
                .is_err()
        );
        assert_eq!(model, Model::default());
        assert!(model.validate(&schema).is_err());
    }
    let mut legacy = original;
    legacy.instantiations.clear();
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("instantiations")
            .is_none()
    );
    assert!(
        !legacy
            .identity("box_value")
            .starts_with("intersect:generic:")
    );
}

#[test]
fn operation_overloads_discriminate_instantiated_arguments_without_losing_declarations() {
    let mut schema = schema();
    let resolved = schema.resolved_operations("Use").unwrap();
    assert_eq!(resolved.len(), 2);
    assert_ne!(resolved[0].signature, resolved[1].signature);
    for (index, entry) in resolved.iter().enumerate() {
        assert_eq!(
            entry.selected.operation.declaration.path,
            vec![3, index + 2]
        );
        assert_eq!(
            schema
                .resolve_operation("Use", &entry.signature)
                .unwrap()
                .selected
                .operation
                .declaration,
            entry.selected.operation.declaration
        );
        let wire = serde_json::to_string(&entry.signature).unwrap();
        let restored = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            schema
                .resolve_operation("Use", &restored)
                .unwrap()
                .signature,
            entry.signature
        );
    }
    let signature = resolved[0].signature.clone();
    let original = resolved[0].selected.operation.declaration.clone();
    let mut child = schema.classes["Use"].clone();
    child.name = "NativeChild".into();
    child.parents = vec!["Use".into()];
    child.features.clear();
    child.operations.truncate(1);
    // A native legacy class has explicit member IDs but no Ecore classifier
    // provenance; preserve that distinction instead of inventing source nodes.
    child.operations[0].declaration.path = vec![100, 0];
    child.operations[0].parameters[0].declaration.path = vec![100, 0, 0];
    schema.classes.insert("NativeChild".into(), child);
    let overridden = schema.resolve_operation("NativeChild", &signature).unwrap();
    assert_eq!(overridden.selected.owner, "NativeChild");
    assert_eq!(overridden.overridden, vec![original]);
    assert_eq!(schema.resolved_operations("NativeChild").unwrap().len(), 2);
    schema.classes.get_mut("Use").unwrap().operations[1].parameters[0]
        .value
        .target = Some(intersect_model::definitions::operations::TypeTarget::Class(
        "box_value".into(),
    ));
    assert!(
        schema
            .resolved_operations("Use")
            .unwrap_err()
            .contains("Duplicate local operation signature")
    );
}
