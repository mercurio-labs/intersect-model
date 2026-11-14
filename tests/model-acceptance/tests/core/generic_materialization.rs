use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_closed_types,
    generic_types::{ClosedType, Type},
    resources::DeclarationId,
};
use serde_json::json;
const BODY: &str = r##"
<eClassifiers x:type="e:EClass" name="Value"><eStructuralFeatures x:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" defaultValueLiteral="new"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Other"/>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures><eOperations name="accept"><eParameters name="input"><eGenericType eTypeParameter="#//Box/T"/></eParameters></eOperations></eClassifiers>
<eClassifiers x:type="e:EClass" name="ValueBox"><eGenericSuperTypes eClassifier="#//Box"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes></eClassifiers>
"##;
fn setup(
    body: &str,
) -> (
    DefinitionBundle,
    ClosedType,
    ClosedType,
    ClosedType,
    ClosedType,
) {
    try_setup(body).unwrap()
}
fn try_setup(
    body: &str,
) -> Result<
    (
        DefinitionBundle,
        ClosedType,
        ClosedType,
        ClosedType,
        ClosedType,
    ),
    String,
> {
    let source = DefinitionSource::new(
        "memory:materialize",
        SourceKind::Ecore,
        if body.trim_start().starts_with("<e:EPackage") {
            body.to_owned()
        } else {
            format!(
                r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">{body}</e:EPackage>"#
            )
        },
    );
    let report = intersect_model::migration::migrate_ecore(source, "memory:materialize.rcore")
        .map_err(|e| e.to_string())?;
    let source = report
        .converted
        .ok_or_else(|| format!("{:?}", report.issues))?;
    let class = |n, arguments| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![n],
        },
        arguments,
        raw: false,
    };
    let value = ClosedType::new(class(0, vec![])).unwrap();
    let other = ClosedType::new(class(1, vec![])).unwrap();
    let boxed = ClosedType::new(class(2, vec![value.expression().clone()])).unwrap();
    let child = ClosedType::new(class(3, vec![])).unwrap();
    Ok((
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        value,
        other,
        boxed,
        child,
    ))
}

#[test]
fn materialized_source_types_construct_default_link_and_roll_back() {
    let (bundle, value, other, boxed, child) = setup(BODY);
    let schema =
        materialize_closed_types(&bundle, &[child.clone(), value.clone(), other.clone()]).unwrap();
    let source = &bundle.sources[0].id;
    let value_key = schema.class_key(source, "urn:g", "Value").unwrap();
    let other_key = schema.class_key(source, "urn:g", "Other").unwrap();
    let child_key = schema.class_key(source, "urn:g", "ValueBox").unwrap();
    let box_key = schema.instantiated_class_key(&boxed).unwrap();
    assert!(schema.conforms(child_key, box_key));
    assert_eq!(schema.feature_owner(child_key, "value").unwrap(), box_key);
    assert_eq!(
        schema.classes[box_key].feature_declarations["value"],
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &bundle.sources[0],
            "Box/value"
        )
    );
    assert_eq!(
        schema.resolved_operations(child_key).unwrap()[0]
            .selected
            .operation
            .declaration,
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &bundle.sources[0],
            "Box/accept"
        )
    );
    let mut model = Model::default();
    model
        .create(&schema, "v", value_key, "memory:model", 0, 0)
        .unwrap();
    model
        .create(&schema, "o", other_key, "memory:model", 0, 0)
        .unwrap();
    model
        .create(&schema, "b", child_key, "memory:model", 0, 0)
        .unwrap();
    assert_eq!(model.elements[0].values["name"], json!("new"));
    model
        .edit(&schema, &[("b".into(), "value".into(), json!("v"))])
        .unwrap();
    let before = model.clone();
    assert!(
        model
            .edit(&schema, &[("b".into(), "value".into(), json!("o"))])
            .is_err()
    );
    assert_eq!(model, before);
    let restored = serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    model.validate(&restored).unwrap();
    let reordered = materialize_closed_types(&bundle, &[other, value, child]).unwrap();
    assert_eq!(schema, reordered);
}
#[test]
fn absent_targets_and_existing_structural_errors_are_not_hidden_by_projection() {
    let (bundle, _, _, _, child) = setup(BODY);
    assert!(
        materialize_closed_types(&bundle, &[child])
            .unwrap_err()
            .message
            .contains("missing reference/operation target")
    );
    for (body, message) in [
        (
            BODY.replace("defaultValueLiteral=\"new\"", "upperBound=\"-2\""),
            "multiplicity",
        ),
        (
            BODY.replace(
                "name=\"value\"",
                "name=\"value\" lowerBound=\"2\" upperBound=\"1\"",
            ),
            "multiplicity",
        ),
    ] {
        let error = match try_setup(&body) {
            Ok((bundle, value, _, _, child)) => materialize_closed_types(&bundle, &[child, value])
                .unwrap_err()
                .to_string(),
            Err(error) => error,
        };
        assert!(
            error.to_lowercase().contains(message) || error.contains("bound"),
            "{error}"
        );
    }
}

const OPPOSITES: &str =
    include_str!("../../../../fixtures/ecore-generics/materialize-opposites.ecore");
#[test]
fn specialized_opposites_preserve_reciprocity_and_argument_identity() {
    let (bundle, value, other, boxed, child) = setup(OPPOSITES);
    let mut other_expression = boxed.expression().clone();
    if let Type::Classifier { arguments, .. } = &mut other_expression {
        arguments[0] = other.expression().clone();
    }
    let other_box = ClosedType::new(other_expression).unwrap();
    let schema = materialize_closed_types(
        &bundle,
        &[
            value,
            other,
            boxed.clone(),
            child.clone(),
            other_box.clone(),
        ],
    )
    .unwrap();
    let mut model = Model::default();
    for (id, ty) in [
        ("p", &boxed),
        ("c", &child),
        ("q", &boxed),
        ("o", &other_box),
    ] {
        model
            .create(
                &schema,
                id,
                schema.class_key_for_type(ty).unwrap(),
                "memory:model",
                0,
                0,
            )
            .unwrap();
    }
    model
        .edit(&schema, &[("p".into(), "children".into(), json!(["c"]))])
        .unwrap();
    assert_eq!(model.elements[1].values["parent"], json!("p"));
    model
        .edit(&schema, &[("c".into(), "parent".into(), json!("q"))])
        .unwrap();
    assert_eq!(model.elements[0].values["children"], json!([]));
    assert_eq!(model.elements[2].values["children"], json!(["c"]));
    let before = model.clone();
    assert!(
        model
            .edit(&schema, &[("c".into(), "parent".into(), json!("o"))])
            .is_err()
    );
    assert_eq!(model, before);
    model.validate(&schema).unwrap();
}
#[test]
fn specialized_opposites_reject_bad_source_declarations_and_nonreciprocal_links() {
    for (text, diagnostic) in [
        (
            OPPOSITES.replace("eOpposite=\"#//Box/parent\"", "eOpposite=\"#//Value/name\""),
            "EReference",
        ),
        (
            OPPOSITES.replace("href=\"#//Box/children\"", "href=\"#//Box/value\""),
            "Inconsistent opposite",
        ),
        (
            OPPOSITES.replace(
                "name=\"parent\"",
                "name=\"parent\" eOpposite=\"#//Box/children\"",
            ),
            "Duplicate reference encoding",
        ),
    ] {
        let error = match try_setup(&text) {
            Ok((bundle, value, _, boxed, child)) => {
                materialize_closed_types(&bundle, &[value, boxed, child])
                    .unwrap_err()
                    .to_string()
            }
            Err(error) => error,
        };
        assert!(error.contains(diagnostic), "{error}");
    }
}

#[test]
fn scalar_bound_specialization_uses_shared_model_defaults_and_rollback() {
    let text = include_str!("../../../../fixtures/ecore-generics/materialize-scalar-bounds.ecore");
    let compile = |text: &str| {
        let source = DefinitionSource::new("memory:scalar-bound", SourceKind::Ecore, text);
        let (source, _) =
            intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
        let usage = DeclarationId {
            source: source.id.clone(),
            path: vec![1, 0, 0],
        };
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let resolver =
            intersect_model::definitions::generic_resolution::Resolver::new(&bundle).unwrap();
        let ty = ClosedType::new(resolver.type_expression(&usage).unwrap().expression).unwrap();
        (
            materialize_closed_types(&bundle, std::slice::from_ref(&ty)),
            ty,
        )
    };
    let (result, ty) = compile(text);
    let schema = result.unwrap();
    let key = schema.instantiated_class_key(&ty).unwrap();
    let mut model = Model::default();
    model
        .create(&schema, "h", key, "memory:model", 0, 0)
        .unwrap();
    assert_eq!(model.elements[0].values["value"], json!("hello"));
    model
        .edit(&schema, &[("h".into(), "value".into(), json!("updated"))])
        .unwrap();
    let before = model.clone();
    assert!(
        model
            .edit(&schema, &[("h".into(), "value".into(), json!(12))])
            .is_err()
    );
    assert_eq!(model, before);
    let restored = serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    model.validate(&restored).unwrap();
    let invalid = text.replace("#//EJavaObject", "#//EDate");
    assert!(
        compile(&invalid)
            .0
            .unwrap_err()
            .message
            .contains("violates generic bounds")
    );
}

#[test]
fn materialization_preserves_declared_operation_selection() {
    let text =
        include_str!("../../../../fixtures/ecore-generics/materialize-operation-selection.ecore");
    let compile = |text: &str| {
        let (bundle, a, b, _, derived) = setup(text);
        let result = materialize_closed_types(&bundle, &[a, b, derived]);
        (bundle, result)
    };
    // Declared f(A), f(B) remain distinct even though both project to f(B).
    // Declared f(A), f(A) remain one override despite f(B), f(A) projections.
    for (text, count) in [
        (text.to_owned(), 2),
        (text.replace("eType=\"#//B\"", "eType=\"#//A\""), 1),
    ] {
        let (bundle, result) = compile(&text);
        let schema = result.unwrap();
        let key = schema
            .class_key(&bundle.sources[0].id, "urn:g", "Derived")
            .unwrap();
        let groups = schema.resolved_operations(key).unwrap();
        assert_eq!(groups.len(), count);
        if count == 2 {
            assert_ne!(groups[0].signature, groups[1].signature);
            assert_eq!(
                schema
                    .operation_signature(groups[0].selected.operation)
                    .unwrap(),
                schema
                    .operation_signature(groups[1].selected.operation)
                    .unwrap()
            );
            assert!(groups.iter().all(|group| group.overridden.is_empty()));
        } else {
            assert_eq!(
                groups[0].selected.operation.declaration.clone(),
                intersect_model_acceptance::migration_fixture::named_declaration_fixture(
                    &bundle.sources[0],
                    "Derived/f"
                )
            );
            assert_eq!(
                groups[0].overridden[0].clone(),
                intersect_model_acceptance::migration_fixture::named_declaration_fixture(
                    &bundle.sources[0],
                    "Base/f"
                )
            );
        }
        for group in &groups {
            assert_eq!(
                serde_json::to_value(schema.resolve_operation(key, &group.signature).unwrap())
                    .unwrap(),
                serde_json::to_value(group).unwrap()
            );
        }
        let restored: intersect_model::definitions::schema::Schema =
            serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
        assert_eq!(restored.resolved_operations(key).unwrap(), groups);
        let mut model = Model::default();
        model
            .create(&schema, "d", key, "memory:model", 0, 0)
            .unwrap();
        model.validate(&restored).unwrap();
    }
    // f(A) remains f(A) in both declarations after substituting A.
    let matching = text
        .replace("eClassifier=\"#//B\"", "eClassifier=\"#//A\"")
        .replace("eType=\"#//B\"", "eType=\"#//A\"");
    let (bundle, result) = compile(&matching);
    let schema = result.unwrap();
    let key = schema
        .class_key(&bundle.sources[0].id, "urn:g", "Derived")
        .unwrap();
    let selection = schema.resolved_operations(key).unwrap();
    assert_eq!(selection.len(), 1);
    assert_eq!(
        selection[0].selected.operation.declaration.clone(),
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &bundle.sources[0],
            "Derived/f"
        )
    );
    assert_eq!(
        selection[0].overridden[0].clone(),
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &bundle.sources[0],
            "Base/f"
        )
    );
    let restored = serde_json::from_str::<intersect_model::definitions::schema::Schema>(
        &serde_json::to_string(&schema).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.resolved_operations(key).unwrap(), selection);
    let mut model = Model::default();
    model
        .create(&schema, "d", key, "memory:model", 0, 0)
        .unwrap();
    model.validate(&restored).unwrap();
}

#[test]
fn materialization_rejects_declared_sibling_ambiguity_before_projection() {
    let fixture =
        include_str!("../../../../fixtures/ecore-generics/materialize-operation-selection.ecore");
    let inherited = fixture.replace(
        r##"<eOperations name="f"><eParameters name="p" eType="#//B"/></eOperations>"##,
        "",
    );
    for parents in ["#//Derived #//Right", "#//Right #//Derived"] {
        let text = inherited.replace("</e:EPackage>", &format!(r##"<eClassifiers x:type="e:EClass" name="Right"><eOperations name="f"><eParameters name="p" eType="#//A"/></eOperations></eClassifiers><eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="{parents}"/></e:EPackage>"##));
        let (bundle, a, b, _, _) = setup(&text);
        let diamond = ClosedType::new(Type::Classifier {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![5],
            },
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        let error = materialize_closed_types(&bundle, &[a, b, diamond]).unwrap_err();
        assert_eq!(
            error.kind,
            intersect_model::definitions::ecore::ErrorKind::InvalidDefinition
        );
        assert!(
            error
                .message
                .contains("Ambiguous inherited generic operation"),
            "{}",
            error.message
        );
        assert_eq!(error.span.source, bundle.sources[0].id);
    }
}

#[test]
fn declared_selection_contracts_reject_stale_or_incomplete_schema_data() {
    let fixture =
        include_str!("../../../../fixtures/ecore-generics/materialize-operation-selection.ecore")
            .replace("eType=\"#//B\"", "eType=\"#//A\"");
    let (bundle, a, b, _, derived) = setup(&fixture);
    let schema = materialize_closed_types(&bundle, &[a, b, derived]).unwrap();
    let key = schema
        .class_key(&bundle.sources[0].id, "urn:g", "Derived")
        .unwrap()
        .to_owned();
    let original = serde_json::to_value(&schema).unwrap();
    let group = &original["declared_operation_selections"][&key]["groups"][0];
    let mut mutations = Vec::new();
    let mut bad = original.clone();
    bad["declared_operation_selections"]
        .as_object_mut()
        .unwrap()
        .remove(&key);
    mutations.push(bad);
    let mut bad = original.clone();
    bad["declared_operation_selections"][&key]["groups"] = json!([]);
    mutations.push(bad);
    let mut bad = original.clone();
    bad["declared_operation_selections"][&key]["groups"] = json!([group, group]);
    mutations.push(bad);
    let mut bad = original.clone();
    bad["declared_operation_selections"][&key]["groups"][0]["operations"][0]["owner"] =
        json!("missing");
    mutations.push(bad);
    let mut bad = original.clone();
    bad["declared_operation_selections"][&key]["groups"][0]["selected"] =
        group["operations"][1]["operation"]["declaration"].clone();
    mutations.push(bad);
    let mut bad = original.clone();
    bad["declared_operation_selections"][&key]["groups"][0]["operations"][0]["raw_parameters"][0]
        ["raw_types"][0]["source"]["sha256"] = json!("0".repeat(64));
    mutations.push(bad);
    let mut bad = original.clone();
    bad["declared_operation_selections"][&key]["receiver"]["declaration"]["path"] = json!([0]);
    mutations.push(bad);
    let mut bad = original.clone();
    bad["classes"][&key]["operations"][0]["parameters"][0]["name"] = json!("changed");
    mutations.push(bad);
    let mut bad = original.clone();
    bad["declared_operation_selections"][&key]["groups"][0]["signature"]["name"] = json!("changed");
    mutations.push(bad);
    for (index, bad) in mutations.into_iter().enumerate() {
        let restored: intersect_model::definitions::schema::Schema =
            serde_json::from_value(bad).unwrap();
        assert!(
            restored.resolved_operations(&key).is_err(),
            "mutation {index} accepted"
        );
    }
}

fn method_context(text: &str) -> (DefinitionBundle, Vec<ClosedType>, DeclarationId) {
    let source = DefinitionSource::new("memory:method-templates", SourceKind::Ecore, text);
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let operation = declarations[&vec![1, 1]].clone();
    let types = [0, 2, 3]
        .into_iter()
        .map(|n| {
            ClosedType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![n],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        })
        .collect();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        types,
        operation,
    )
}

#[test]
fn method_templates_materialize_select_and_check_explicit_arguments() {
    use intersect_model::definitions::{generic_resolution::Resolver, operations::TypeTarget};
    let (bundle, types, operation) = method_context(include_str!(
        "../../../../fixtures/ecore-generics/method-templates.ecore"
    ));
    let schema = materialize_closed_types(&bundle, &types).unwrap();
    let receiver = &types[1];
    let key = schema.class_key_for_type(receiver).unwrap();
    let selected = schema.resolved_operations(key).unwrap();
    assert_eq!(selected.len(), 1);
    let projected = selected[0].selected.operation;
    assert_eq!(projected.declaration, operation);
    assert!(projected.generic);
    assert!(!projected.result.ordered);
    assert_eq!(projected.parameters[1].value.upper, -1);
    assert!(!projected.parameters[1].value.unique);
    assert!(
        schema
            .operation_signature(projected)
            .unwrap_err()
            .contains("explicit method arguments")
    );
    let template = schema.operation_template(key, &operation).unwrap();
    assert_eq!(
        template.formals[0].declaration.declaration,
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &bundle.sources[0],
            "Base/convert/T"
        )
    );
    assert_eq!(
        template.formals[0].bounds,
        vec![types[0].expression().clone()]
    );
    assert_eq!(
        projected.parameters[0].value.target,
        Some(TypeTarget::Expression(types[0].expression().clone()))
    );
    assert!(matches!(
        projected.result.target,
        Some(TypeTarget::Expression(Type::Parameter { .. }))
    ));
    assert!(template.source_payload.contains("retained-template"));
    assert!(
        template
            .signatures
            .iter()
            .any(|s| s.feature == "eExceptions"
                && matches!(s.specialized[0], Type::Parameter { .. }))
    );
    let resolver = Resolver::new(&bundle).unwrap();
    let applied = schema
        .instantiate_generic_operation(&resolver, receiver, &operation, &types[..1])
        .unwrap();
    assert_eq!(
        applied,
        resolver
            .instantiate_operation(receiver, &operation, &types[..1])
            .unwrap()
    );
    assert!(
        applied
            .operation
            .signatures
            .iter()
            .flat_map(|s| &s.specialized)
            .all(|ty| ty == types[0].expression())
    );
    for arguments in [
        vec![],
        vec![types[2].clone()],
        vec![types[0].clone(), types[0].clone()],
    ] {
        assert!(
            schema
                .instantiate_generic_operation(&resolver, receiver, &operation, &arguments)
                .is_err()
        );
    }
    let mut stale = operation.clone();
    stale.source.sha256 = "0".repeat(64);
    assert!(
        schema
            .instantiate_generic_operation(&resolver, receiver, &stale, &types[..1])
            .is_err()
    );
    assert!(
        schema
            .instantiate_generic_operation(&resolver, &types[2], &operation, &types[..1])
            .is_err()
    );
    let restored: intersect_model::definitions::schema::Schema =
        serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    assert_eq!(
        restored.operation_template(key, &operation).unwrap(),
        template
    );
    assert_eq!(
        restored
            .instantiate_generic_operation(&resolver, receiver, &operation, &types[..1])
            .unwrap(),
        applied
    );
    let mut model = Model::default();
    model
        .create(&schema, "child", key, "memory:model", 0, 0)
        .unwrap();
    model.validate(&restored).unwrap();
}

#[test]
fn method_templates_reject_stale_structure_and_recheck_retained_bounds() {
    use intersect_model::definitions::generic_resolution::Resolver;
    let fixture = include_str!("../../../../fixtures/ecore-generics/method-templates.ecore");
    let (bundle, types, operation) = method_context(fixture);
    let schema = materialize_closed_types(&bundle, &types).unwrap();
    let key = schema.class_key_for_type(&types[1]).unwrap().to_owned();
    let resolver = Resolver::new(&bundle).unwrap();
    let original = serde_json::to_value(&schema).unwrap();
    for change in ["duplicate", "foreign", "projection"] {
        let mut bad = original.clone();
        let template = &mut bad["declared_operation_selections"][&key]["groups"][0]["operations"]
            [0]["template"];
        match change {
            "duplicate" => {
                let formal = template["formals"][0].clone();
                template["formals"].as_array_mut().unwrap().push(formal);
            }
            "foreign" => {
                template["formals"][0]["declaration"]["declaration"]["path"] = json!([1, 0])
            }
            _ => template["signatures"][0]["specialized"] = json!([types[2].expression()]),
        }
        let bad: intersect_model::definitions::schema::Schema =
            serde_json::from_value(bad).unwrap();
        assert!(
            bad.operation_template(&key, &operation).is_err(),
            "{change}"
        );
    }
    let mut bad = original;
    bad["declared_operation_selections"][&key]["groups"][0]["operations"][0]["template"]["formals"]
        [0]["bounds"] = json!([types[2].expression()]);
    let bad: intersect_model::definitions::schema::Schema = serde_json::from_value(bad).unwrap();
    // Structurally valid serialized bounds are not source verification evidence.
    assert!(bad.operation_template(&key, &operation).is_ok());
    assert!(
        bad.instantiate_generic_operation(&resolver, &types[1], &operation, &types[..1])
            .unwrap_err()
            .message
            .contains("differs from checked source")
    );
    for text in [
        fixture.replace("name=\"mapped\"", "name=\"input\""),
        fixture.replace("upperBound=\"-1\"", "lowerBound=\"2\" upperBound=\"1\""),
        fixture.replace(
            "name=\"mapped\"",
            "name=\"mapped\" eExceptions=\"#//Value\"",
        ),
    ] {
        let report = intersect_model::migration::migrate_ecore(
            DefinitionSource::new("memory:invalid", SourceKind::Ecore, &text),
            "memory:invalid.rcore",
        )
        .unwrap();
        if report.converted.is_none() {
            assert!(report.issues.iter().any(|issue| issue.blocks_conversion));
        } else {
            let (bundle, types, _) = method_context(&text);
            assert!(materialize_closed_types(&bundle, &types).is_err());
        }
    }
}

#[test]
fn structural_services_resolve_diamond_owners_and_closed_opposite_domains() {
    use intersect_model::core::{
        api::{BasicModel, ModelEdit, TransactionalModel},
        query::QueryBudget,
        storage::{
            self, ContainmentDestination, DeleteRequest, DeleteSelection, IncomingReferences,
            ModelRead, Query, ReparentRequest,
        },
    };
    let text = OPPOSITES.replace(
        "</e:EPackage>",
        r##"
<eClassifiers x:type="e:EClass" name="Left" eSuperTypes="#//ValueBox"/>
<eClassifiers x:type="e:EClass" name="Right" eSuperTypes="#//ValueBox"/>
<eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="#//Left #//Right"/>
</e:EPackage>"##,
    );
    let (bundle, value, other, boxed, child) = setup(&text);
    let diamond = ClosedType::new(Type::Classifier {
        declaration: DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![6],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let mut other_expression = boxed.expression().clone();
    if let Type::Classifier { arguments, .. } = &mut other_expression {
        arguments[0] = other.expression().clone();
    }
    let other_box = ClosedType::new(other_expression).unwrap();
    let schema = materialize_closed_types(
        &bundle,
        &[
            value,
            other,
            boxed.clone(),
            child.clone(),
            diamond.clone(),
            other_box.clone(),
        ],
    )
    .unwrap();
    let owner = schema.class_key_for_type(&boxed).unwrap();
    let diamond_key = schema.class_key_for_type(&diamond).unwrap();
    let inherited = schema.effective_feature(diamond_key, "children").unwrap();
    assert_eq!(inherited.owner, owner);
    assert_eq!(
        inherited.identity,
        schema
            .effective_feature(owner, "children")
            .unwrap()
            .identity
    );
    let mut model = BasicModel::new(schema.clone()).unwrap();
    for (id, ty) in [
        ("a", &diamond),
        ("b", &diamond),
        ("c", &child),
        ("nested", &boxed),
        ("foreign", &other_box),
    ] {
        model
            .create(id, schema.class_key_for_type(ty).unwrap(), "generic.model")
            .unwrap();
    }
    model.set("a", "children", json!(["c"])).unwrap();
    model.set("c", "children", json!(["nested"])).unwrap();
    let move_to = |owner| ReparentRequest {
        object: "c",
        destination: Some(ContainmentDestination {
            owner,
            feature: "children",
            position: None,
        }),
    };
    let before = storage::export(&model).unwrap();
    assert!(
        model
            .reparent(move_to("foreign"), None, &mut QueryBudget::new(100_000))
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), before);
    assert!(
        model
            .reparent(move_to("nested"), None, &mut QueryBudget::new(100_000))
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), before);
    model
        .reparent(move_to("b"), None, &mut QueryBudget::new(100_000))
        .unwrap();
    assert_eq!(model.value("a", "children").unwrap(), Some(json!([])));
    assert_eq!(model.value("c", "parent").unwrap(), Some(json!("b")));
    assert_eq!(
        model.query(Query::Value("parent", &json!("b"))).unwrap(),
        vec!["c"]
    );
    let moved = storage::export(&model).unwrap();
    let mut managed = TransactionalModel::new(model);
    let deletion = DeleteRequest {
        roots: &["c"],
        selection: DeleteSelection::Subtrees,
        incoming: IncomingReferences::Reject,
    };
    assert!(
        managed
            .transaction(|edit| {
                edit.delete_objects(deletion, None, &mut QueryBudget::new(100_000))?;
                Err::<(), String>("veto".into())
            })
            .is_err()
    );
    assert_eq!(storage::export(&managed).unwrap(), moved);
    assert_eq!(
        managed
            .transaction(|edit| edit.delete_objects(deletion, None, &mut QueryBudget::new(100_000)))
            .unwrap(),
        vec!["c", "nested"]
    );
    assert_eq!(managed.value("b", "children").unwrap(), Some(json!([])));
    managed.validate().unwrap();
}

#[test]
fn raw_generic_containment_enforces_erased_bounds_during_structural_edits() {
    use intersect_model::core::{
        api::{BasicModel, ModelEdit},
        query::QueryBudget,
        storage::{
            self, ContainmentDestination, DeleteRequest, DeleteSelection, IncomingReferences,
            ModelRead, ReparentRequest,
        },
    };
    let text = BODY
        .replace(
            "<eTypeParameters name=\"T\"/>",
            r##"<eTypeParameters name="T"><eBounds eClassifier="#//Value"/></eTypeParameters>"##,
        )
        .replace(
            "name=\"value\"",
            "name=\"value\" containment=\"true\" unsettable=\"true\"",
        );
    let (bundle, value, other, boxed, child) = setup(&text);
    let mut raw_expression = boxed.expression().clone();
    if let Type::Classifier { arguments, raw, .. } = &mut raw_expression {
        arguments.clear();
        *raw = true;
    }
    let raw =
        intersect_model::definitions::generic_types::RuntimeType::new(raw_expression).unwrap();
    let value =
        intersect_model::definitions::generic_types::RuntimeType::new(value.expression().clone())
            .unwrap();
    let other =
        intersect_model::definitions::generic_types::RuntimeType::new(other.expression().clone())
            .unwrap();
    let child =
        intersect_model::definitions::generic_types::RuntimeType::new(child.expression().clone())
            .unwrap();
    let schema = intersect_model::definitions::compiler::materialize_runtime_types(
        &bundle,
        &[value.clone(), other.clone(), child.clone(), raw.clone()],
        None,
    )
    .unwrap();
    let mut model = BasicModel::new(schema.clone()).unwrap();
    for (id, ty) in [
        ("raw", &raw),
        ("closed", &child),
        ("v", &value),
        ("other", &other),
    ] {
        model
            .create(id, schema.class_key_for_type(ty).unwrap(), "raw.model")
            .unwrap();
    }
    model.set("closed", "value", json!("v")).unwrap();
    let request = |object| ReparentRequest {
        object,
        destination: Some(ContainmentDestination {
            owner: "raw",
            feature: "value",
            position: None,
        }),
    };
    let before = storage::export(&model).unwrap();
    assert!(
        model
            .reparent(request("other"), None, &mut QueryBudget::new(100_000))
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), before);
    model
        .reparent(request("v"), None, &mut QueryBudget::new(100_000))
        .unwrap();
    assert_eq!(model.value("closed", "value").unwrap(), Some(json!(null)));
    assert!(!model.is_set("closed", "value").unwrap());
    assert_eq!(model.value("raw", "value").unwrap(), Some(json!("v")));
    assert!(model.is_set("raw", "value").unwrap());
    assert_eq!(
        model
            .delete_objects(
                DeleteRequest {
                    roots: &["raw"],
                    selection: DeleteSelection::Subtrees,
                    incoming: IncomingReferences::Reject
                },
                None,
                &mut QueryBudget::new(100_000)
            )
            .unwrap(),
        vec!["raw", "v"]
    );
    model.validate().unwrap();
}
