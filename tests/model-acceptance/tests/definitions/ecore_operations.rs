use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::compile_structure_bundle,
    operations::{ParameterType, TypeTarget},
    resources::DeclarationId,
};

fn package(body: &str) -> String {
    format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:test'>{body}</e:EPackage>"
    )
}
fn class(name: &str, parents: &str, body: &str) -> String {
    format!(
        "<eClassifiers xsi:type='e:EClass' name='{name}' eSuperTypes='{parents}'>{body}</eClassifiers>"
    )
}
fn operation(name: &str, types: &[&str], result: &str) -> String {
    let parameters: String = types
        .iter()
        .enumerate()
        .map(|(i, ty)| format!("<eParameters name='p{i}' eType='{ty}'/>"))
        .collect();
    let result = if result.is_empty() {
        String::new()
    } else {
        format!(" eType='{result}'")
    };
    format!("<eOperations name='{name}'{result}>{parameters}</eOperations>")
}
const INT: &str = "e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt";
const STRING: &str = "e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString";

fn assert_canonical_payloads(text: &str, schema: &intersect_model::definitions::schema::Schema) {
    let source = DefinitionSource::new(
        "memory:fixture.rcore",
        SourceKind::Rcore,
        intersect_model_acceptance::migrate_model_fixture(text).unwrap(),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resources = intersect_model::definitions::resources::ResourceSet::new(&bundle).unwrap();
    for class in schema.classes.values() {
        for operation in &class.operations {
            let node = resources.node(&operation.declaration).unwrap();
            assert_eq!(
                operation.source_payload,
                source.text[node.span.start..node.span.end]
            );
            for parameter in &operation.parameters {
                resources.node(&parameter.declaration).unwrap();
            }
        }
    }
}

#[test]
fn enum_overloads_and_overrides_use_declaration_identity_not_numeric_domain() {
    let enumeration = "<eClassifiers xsi:type='e:EEnum' name='EInt'><eLiterals name='First' value='7'/><eLiterals name='Alias' value='7'/></eClassifiers>";
    let text = package(
        &(enumeration.to_owned()
            + class(
                "Base",
                "",
                &(operation("select", &["#//EInt"], "#//EInt")
                    + operation("select", &[INT], INT).as_str()),
            )
            .as_str()
            + class(
                "Child",
                "#//Base",
                &operation("select", &["#//EInt"], "#//EInt"),
            )
            .as_str()),
    );
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let operations = schema.resolved_operations("Child").unwrap();
    assert_eq!(operations.len(), 2);
    let enumeration = &schema.enumerations["EInt"];
    let selected = operations
        .iter()
        .find(|entry| entry.selected.owner == "Child")
        .unwrap();
    assert_eq!(
        selected.signature.parameters,
        [ParameterType::Classifier {
            declaration: enumeration.declaration().clone()
        }]
    );
    assert_eq!(
        selected.selected.operation.result.target,
        Some(TypeTarget::Enumeration("EInt".into()))
    );
    assert_eq!(selected.overridden.len(), 1);
    assert!(operations.iter().any(|entry| entry.signature.parameters
        == [ParameterType::Builtin {
            name: "EInt".into()
        }]));
    let restored: intersect_model::definitions::schema::Schema =
        serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    assert_eq!(restored.resolved_operations("Child").unwrap(), operations);
    assert!(
        schema
            .operation_parameter_type(&TypeTarget::Class("EInt".into()))
            .is_err()
    );
    assert!(
        schema
            .operation_parameter_type(&TypeTarget::Enumeration("Child".into()))
            .is_err()
    );
}

#[test]
fn same_named_external_enum_revisions_form_distinct_operation_signatures() {
    let enum_text = package(
        "<eClassifiers xsi:type='e:EEnum' name='Mode'><eLiterals name='One'/></eClassifiers>",
    );
    let first = DefinitionSource::new("file:///types.ecore", SourceKind::Ecore, enum_text.clone());
    let second = DefinitionSource::new("file:///types.ecore", SourceKind::Ecore, enum_text + "\n");
    let mut one = DefinitionSource::new(
        "file:///one.ecore",
        SourceKind::Ecore,
        package(&class(
            "One",
            "",
            &operation("f", &["e:EEnum types.ecore#//Mode"], "types.ecore#//Mode"),
        ))
        .replace("urn:test", "urn:one"),
    );
    one.dependencies.push(first.id.clone());
    let mut two = DefinitionSource::new(
        "file:///two.ecore",
        SourceKind::Ecore,
        package(&class(
            "Two",
            "",
            &operation("f", &["e:EEnum types.ecore#//Mode"], "types.ecore#//Mode"),
        ))
        .replace("urn:test", "urn:two"),
    );
    two.dependencies.push(second.id.clone());
    let mut entry = DefinitionSource::new(
        "file:///entry.ecore",
        SourceKind::Ecore,
        package(&class("Host", "one.ecore#//One two.ecore#//Two", ""))
            .replace("urn:test", "urn:entry"),
    );
    entry.dependencies = vec![one.id.clone(), two.id.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), one, two, first.clone(), second.clone()],
        namespaces: vec![],
    };
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let first =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &first.id);
    let second = intersect_model_acceptance::migration_fixture::converted_source_fixture(
        &report, &second.id,
    );
    let entry =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &entry.id);
    let mut bundle = report.converted.unwrap();
    let schema = compile_structure_bundle(&bundle, &entry.id, "urn:entry").unwrap();
    let resolved = schema.resolved_operations("Host").unwrap();
    assert_eq!(resolved.len(), 2);
    for (selection, source) in resolved.iter().zip([first.id, second.id]) {
        assert_eq!(
            selection.signature.parameters,
            [ParameterType::Classifier {
                declaration: DeclarationId {
                    source,
                    path: vec![0]
                }
            }]
        );
        assert!(selection.overridden.is_empty());
        assert!(matches!(
            selection.selected.operation.result.target,
            Some(TypeTarget::Enumeration(_))
        ));
    }
    bundle.sources.reverse();
    assert_eq!(
        compile_structure_bundle(&bundle, &entry.id, "urn:entry").unwrap(),
        schema
    );
}

#[test]
fn enum_return_types_do_not_create_overloads_and_missing_enum_keys_reject() {
    let enumeration =
        "<eClassifiers xsi:type='e:EEnum' name='Mode'><eLiterals name='One'/></eClassifiers>";
    let duplicate = package(
        &(enumeration.to_owned()
            + class(
                "Host",
                "",
                &(operation("f", &["#//Mode"], "#//Mode")
                    + operation("f", &["#//Mode"], INT).as_str()),
            )
            .as_str()),
    );
    assert!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&duplicate)
            .unwrap_err()
            .contains("Duplicate local operation signature")
    );
    let text = package(
        &(enumeration.to_owned()
            + class("Host", "", &operation("f", &["#//Mode"], "#//Mode")).as_str()),
    );
    let mut schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    schema.enumerations.clear();
    assert!(
        schema
            .resolved_operations("Host")
            .unwrap_err()
            .contains("Unknown operation enum")
    );
}

#[test]
fn mercurio_representative_signature_shapes_use_native_selection() {
    // Handwritten reduced shape control from the pinned effective extract;
    // these stub classes do not import or execute the SysML metamodel.
    let action = "<eOperations name='argument' eType='#//Expression' lowerBound='0' ordered='false'><eParameters name='i' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt' lowerBound='1' ordered='false'/></eOperations><eOperations name='inputParameters' eType='#//Feature' upperBound='-1' ordered='false'/>";
    let accept = "<eOperations name='isTriggerAction' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EBoolean' lowerBound='1' ordered='false'/>";
    let text = package(
        &(class("Expression", "", "")
            + class("Feature", "", "").as_str()
            + class("ActionUsage", "", action).as_str()
            + class("AcceptActionUsage", "#//ActionUsage", accept).as_str()),
    );
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let resolved = schema.resolved_operations("AcceptActionUsage").unwrap();
    assert_eq!(
        resolved
            .iter()
            .map(|r| r.signature.name.as_str())
            .collect::<Vec<_>>(),
        ["argument", "inputParameters", "isTriggerAction"]
    );
    assert_eq!(
        resolved[0].signature.parameters,
        [ParameterType::Builtin {
            name: "EInt".into()
        }]
    );
    assert_eq!(
        resolved[0].selected.operation.result.target,
        Some(TypeTarget::Class("Expression".into()))
    );
    assert_eq!(resolved[0].selected.operation.parameters[0].value.lower, 1);
    assert_eq!(resolved[0].selected.operation.result.lower, 0);
    assert_eq!(resolved[1].selected.operation.result.upper, -1);
    assert!(!resolved[1].selected.operation.result.ordered);
    assert_eq!(
        resolved[2].selected.operation.result.target,
        Some(TypeTarget::Builtin("EBoolean".into()))
    );
    assert_eq!(resolved[2].selected.operation.result.lower, 1);
    assert_eq!(resolved[0].selected.operation.declaration.path, [2, 0]);
    assert_eq!(resolved[2].selected.operation.declaration.path, [3, 0]);
}

#[test]
fn same_uri_classifier_revisions_never_collapse_operation_signatures() {
    let first = DefinitionSource::new(
        "file:///types.ecore",
        SourceKind::Ecore,
        package(&class("Item", "", "")),
    );
    let second = DefinitionSource::new(
        "file:///types.ecore",
        SourceKind::Ecore,
        first.text.clone() + "\n",
    );
    let mut one = DefinitionSource::new(
        "file:///one.ecore",
        SourceKind::Ecore,
        package(&class(
            "One",
            "",
            &operation("f", &["types.ecore#//Item"], ""),
        ))
        .replace("urn:test", "urn:one"),
    );
    one.dependencies.push(first.id.clone());
    let mut two = DefinitionSource::new(
        "file:///two.ecore",
        SourceKind::Ecore,
        package(&class(
            "Two",
            "",
            &operation("f", &["types.ecore#//Item"], ""),
        ))
        .replace("urn:test", "urn:two"),
    );
    two.dependencies.push(second.id.clone());
    let mut entry = DefinitionSource::new(
        "file:///entry.ecore",
        SourceKind::Ecore,
        package(&class("Host", "one.ecore#//One two.ecore#//Two", ""))
            .replace("urn:test", "urn:entry"),
    );
    entry.dependencies = vec![one.id.clone(), two.id.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), one, two, first.clone(), second.clone()],
        namespaces: vec![],
    };
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let first =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &first.id);
    let second = intersect_model_acceptance::migration_fixture::converted_source_fixture(
        &report, &second.id,
    );
    let entry =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &entry.id);
    let bundle = report.converted.unwrap();
    let schema = compile_structure_bundle(&bundle, &entry.id, "urn:entry").unwrap();
    let resolved = schema.resolved_operations("Host").unwrap();
    assert_eq!(resolved.len(), 2);
    for (selection, source) in resolved.iter().zip([first.id, second.id]) {
        assert_eq!(
            selection.signature.parameters,
            [ParameterType::Classifier {
                declaration: DeclarationId {
                    source,
                    path: vec![0]
                }
            }]
        );
        assert!(selection.overridden.is_empty());
    }
}

#[test]
fn explicit_operation_kinds_and_member_inventory_validation() {
    let raw = operation("f", &[INT, STRING], INT) + operation("g", &[], "").as_str();
    let typed = raw
        .replace("<eOperations ", "<eOperations xsi:type='e:EOperation' ")
        .replace("<eParameters ", "<eParameters xsi:type='e:EParameter' ");
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &package(&class("A", "", &typed)),
    )
    .unwrap();
    assert_eq!(schema.resolved_operations("A").unwrap().len(), 2);
    assert_canonical_payloads(&package(&class("A", "", &typed)), &schema);
    let error = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &package(&class("A", "", &typed.replace("e:EParameter", "e:EClass"))),
    )
    .unwrap_err();
    assert!(error.contains("Incorrect operation member type"));
    let mut changed = schema.clone();
    changed.classes.get_mut("A").unwrap().operations.reverse();
    assert!(
        changed
            .resolved_operations("A")
            .unwrap_err()
            .contains("order")
    );
    let mut changed = schema.clone();
    changed.classes.get_mut("A").unwrap().operations[0]
        .parameters
        .reverse();
    assert!(
        changed
            .resolved_operations("A")
            .unwrap_err()
            .contains("order")
    );
    let mut changed = schema.clone();
    let duplicate = changed.classes["A"].operations[0].clone();
    changed
        .classes
        .get_mut("A")
        .unwrap()
        .operations
        .push(duplicate);
    assert!(
        changed
            .effective_operations("A")
            .unwrap_err()
            .contains("Repeated")
    );
    let mut changed = schema;
    let operation_id = changed.classes["A"].operations[0].declaration.clone();
    changed
        .classes
        .get_mut("A")
        .unwrap()
        .feature_declarations
        .insert("forged".into(), operation_id);
    assert!(
        changed
            .effective_operations("A")
            .unwrap_err()
            .contains("share declaration")
    );
}

#[test]
fn deep_graphs_empty_classes_cycles_and_unknown_declarations_are_explicit() {
    let mut schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&package(
            &class("Root", "", &operation("f", &[], "")),
        ))
        .unwrap();
    let mut template = schema.classes["Root"].clone();
    template.operations.clear();
    for i in 0..10_000 {
        let mut class = template.clone();
        class.name = format!("C{i}");
        class.parents = vec![if i == 0 {
            "Root".into()
        } else {
            format!("C{}", i - 1)
        }];
        schema.classes.insert(class.name.clone(), class);
    }
    assert_eq!(schema.effective_operations("C9999").unwrap().len(), 1);
    assert_eq!(
        schema.resolved_operations("C9999").unwrap()[0]
            .selected
            .owner,
        "Root"
    );
    let mut missing = schema.classes["Root"].operations[0].declaration.clone();
    missing.path.push(999);
    assert!(
        schema
            .operation_by_id("C9999", &missing)
            .unwrap_err()
            .contains("Unknown operation")
    );
    schema.classes.get_mut("Root").unwrap().parents = vec!["C9999".into()];
    assert!(
        schema
            .resolved_operations("C9999")
            .unwrap_err()
            .contains("Cyclic")
    );
    let empty = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &package(&class("Empty", "", "")),
    )
    .unwrap();
    assert!(empty.resolved_operations("Empty").unwrap().is_empty());
}

#[test]
fn overloads_keep_declaration_and_parameter_order_with_exact_payloads() {
    let operations = [
        operation("z", &[], ""),
        operation("f", &[INT, STRING], STRING),
        operation("f", &[STRING, INT], INT),
        operation("f", &[INT], INT),
    ];
    let text = package(&class("A", "", &operations.concat()));
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let all = schema.effective_operations("A").unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].operation.result.target, None);
    for (index, member) in all.iter().enumerate() {
        assert_eq!(member.position, index);
        assert_eq!(member.operation.declaration.path, [0, index]);
    }
    assert_canonical_payloads(&text, &schema);
    let selected = schema.resolved_operations("A").unwrap();
    assert_eq!(selected.len(), 4);
    assert_eq!(
        selected[1].signature.parameters,
        [
            ParameterType::Builtin {
                name: "EInt".into()
            },
            ParameterType::Builtin {
                name: "EString".into()
            }
        ]
    );
    assert_eq!(all[1].operation.parameters[1].declaration.path, [0, 1, 1]);
    let lookup = schema
        .resolve_operation("A", &selected[2].signature)
        .unwrap();
    assert_eq!(
        lookup.selected.operation.declaration,
        all[2].operation.declaration
    );
    assert!(lookup.overridden.is_empty());
    assert_eq!(
        schema
            .operation_by_id("A", &all[2].operation.declaration)
            .unwrap(),
        all[2]
    );
    let restored = serde_json::from_str::<intersect_model::definitions::schema::Schema>(
        &serde_json::to_string(&schema).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.resolved_operations("A").unwrap(), selected);
}

#[test]
fn overrides_preserve_bases_diamonds_and_explicit_descendant_resolution() {
    let base = operation("f", &[INT], INT);
    let child = base.replace("p0", "renamed");
    for parents in ["#//Left #//Right #//Root", "#//Right #//Left #//Root"] {
        let body = class("Root", "", &base)
            + class("Left", "#//Root", &child).as_str()
            + class("Right", "#//Root", &child).as_str()
            + class("Leaf", parents, &child).as_str();
        let schema =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
                &package(&body),
            )
            .unwrap();
        let all = schema.effective_operations("Leaf").unwrap();
        assert_eq!(all.len(), 4);
        let selected = schema.resolved_operations("Leaf").unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].selected.owner, "Leaf");
        assert_eq!(selected[0].selected.operation.declaration.path, [3, 0]);
        assert_eq!(selected[0].overridden.len(), 3);
        assert_eq!(all[0].operation.declaration.path, [0, 0]);
        assert_eq!(
            schema.resolved_operations("Left").unwrap()[0].overridden[0],
            all[0].operation.declaration
        );
        let ambiguous = body.replace(&class("Leaf", parents, &child), &class("Leaf", parents, ""));
        let error = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
            &package(&ambiguous),
        )
        .unwrap_err();
        assert!(error.contains("Ambiguous inherited operation"), "{error}");
        assert!(error.contains("path: [1, 0]"), "{error}");
        assert!(error.contains("path: [2, 0]"), "{error}");
    }
    let body = class("Root", "", &base)
        + class("Left", "#//Root", "").as_str()
        + class("Right", "#//Root", "").as_str()
        + class("Leaf", "#//Left #//Right", "").as_str();
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &package(&body),
    )
    .unwrap();
    assert_eq!(schema.effective_operations("Leaf").unwrap().len(), 1);
    assert_eq!(
        schema.resolved_operations("Leaf").unwrap()[0]
            .selected
            .owner,
        "Root"
    );
}

#[test]
fn return_names_and_bounds_do_not_manufacture_overloads() {
    let first = operation("f", &[INT], INT);
    for other in [
        first.clone(),
        first.replace("p0", "other"),
        operation("f", &[INT], STRING),
        first.replace("name='p0'", "name='p0' lowerBound='1' upperBound='-1'"),
    ] {
        let error = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
            &package(&class("A", "", &(first.clone() + other.as_str()))),
        )
        .unwrap_err();
        assert!(
            error.contains("Duplicate local operation signature"),
            "{error}"
        );
    }
    // Selection is independent of result compatibility. Later binding must check it.
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&package(
            &(class("A", "", &first)
                + class("B", "#//A", &operation("f", &[INT], STRING)).as_str()),
        ))
        .unwrap();
    assert_eq!(
        schema.resolved_operations("B").unwrap()[0]
            .selected
            .operation
            .result
            .target,
        Some(TypeTarget::Builtin("EString".into()))
    );
}

#[test]
fn resource_types_are_exact_and_source_permutations_do_not_change_selection() {
    let first = DefinitionSource::new(
        "file:///one.ecore",
        SourceKind::Ecore,
        package(&class("Item", "", "")),
    );
    let second = DefinitionSource::new(
        "file:///two.ecore",
        SourceKind::Ecore,
        package(&class("Item", "", "")).replace("urn:test", "urn:two"),
    );
    let ops = operation("f", &["one.ecore#//Item"], "")
        + operation("f", &["two.ecore#//Item"], "").as_str();
    let mut entry = DefinitionSource::new(
        "file:///entry.ecore",
        SourceKind::Ecore,
        package(&class("Host", "", &ops)).replace("urn:test", "urn:entry"),
    );
    entry.dependencies = vec![first.id.clone(), second.id.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), first.clone(), second.clone()],
        namespaces: vec![],
    };
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let first =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &first.id);
    let second = intersect_model_acceptance::migration_fixture::converted_source_fixture(
        &report, &second.id,
    );
    let entry =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &entry.id);
    let mut bundle = report.converted.unwrap();
    let schema = compile_structure_bundle(&bundle, &entry.id, "urn:entry").unwrap();
    bundle.sources.reverse();
    assert_eq!(
        schema,
        compile_structure_bundle(&bundle, &entry.id, "urn:entry").unwrap()
    );
    let selections = schema.resolved_operations("Host").unwrap();
    for (selection, source) in selections.iter().zip([&first.id, &second.id]) {
        assert_eq!(
            selection.signature.parameters,
            [ParameterType::Classifier {
                declaration: DeclarationId {
                    source: source.clone(),
                    path: vec![0]
                }
            }]
        );
    }
    let alias = entry.text.replace(
        "one.ecore.rcore#//@eClassifiers.0",
        "one.ecore.rcore#//Item",
    );
    let mut changed = DefinitionSource::new(&entry.id.uri, SourceKind::Rcore, alias);
    changed.dependencies = entry.dependencies;
    bundle.sources.retain(|s| s.id != entry.id);
    bundle.sources.push(changed.clone());
    let changed_schema = compile_structure_bundle(&bundle, &changed.id, "urn:entry").unwrap();
    let changed_selection = changed_schema.resolved_operations("Host").unwrap();
    assert_eq!(selections[0].signature, changed_selection[0].signature);
    assert_ne!(
        selections[0].selected.operation.declaration,
        changed_selection[0].selected.operation.declaration
    );
}

#[test]
fn builtin_and_same_named_class_are_distinct_and_invalid_inputs_reject() {
    let body = class("EInt", "", "")
        + class(
            "Host",
            "",
            &(operation("f", &[INT], "") + operation("f", &["#//EInt"], "").as_str()),
        )
        .as_str();
    assert_eq!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&package(
            &body
        ))
        .unwrap()
        .resolved_operations("Host")
        .unwrap()
        .len(),
        2
    );
    for (operation, message) in [
        (operation("f", &["#//Absent"], ""), "Absent"),
        (
            "<eOperations name='f'><eParameters name='p'/></eOperations>".into(),
            "requires a type",
        ),
        (
            "<eOperations name='f' lowerBound='2' upperBound='1'/>".into(),
            "multiplicity",
        ),
        (
            "<eOperations name='f'><eTypeParameters name='T'/></eOperations>".into(),
            "E08",
        ),
    ] {
        let error = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
            &package(&class("A", "", &operation)),
        )
        .unwrap_err();
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn retained_metadata_and_href_parameters_do_not_become_delegate_execution() {
    let op = "<eOperations name='f' lowerBound='0' upperBound='-1' unique='false'><eType href='#//A'/><eAnnotations source='delegate'><details key='body' value='not executed'/></eAnnotations><eParameters name='p' ordered='false'><eType href='http://www.eclipse.org/emf/2002/Ecore#//EInt'/></eParameters><eExceptions href='#//A'/></eOperations>";
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &package(&class("A", "", op)),
    )
    .unwrap();
    let selected = schema.resolved_operations("A").unwrap();
    let operation = selected[0].selected.operation;
    assert_canonical_payloads(&package(&class("A", "", op)), &schema);
    assert_eq!(operation.parameters[0].name, "p");
    assert!(!operation.parameters[0].value.ordered);
    assert_eq!(operation.result.upper, -1);
    assert!(!operation.result.unique);
    assert_eq!(operation.result.target, Some(TypeTarget::Class("A".into())));
    let mut invalid = schema.clone();
    invalid.classes.get_mut("A").unwrap().operations[0].generic = true;
    assert!(
        invalid
            .resolved_operations("A")
            .unwrap_err()
            .contains("explicit method arguments")
    );
    let mut invalid = schema.clone();
    invalid.classes.get_mut("A").unwrap().operations[0]
        .declaration
        .path = vec![99, 0];
    assert!(
        invalid
            .effective_operations("A")
            .unwrap_err()
            .contains("wrong owner")
    );
    assert!(schema.effective_operations("Missing").is_err());
}
