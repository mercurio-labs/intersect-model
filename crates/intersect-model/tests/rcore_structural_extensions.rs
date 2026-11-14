use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::Type,
    rcore::compile_structure,
    resources::ResourceSet,
};

fn source(body: &str) -> DefinitionSource {
    DefinitionSource::new(
        "memory:extensions.rcore",
        SourceKind::Rcore,
        format!(
            "rcore 1; package extensions version \"1\"; metamodel \"urn:extensions\" prefix e {{ {body} }}"
        ),
    )
}
fn bundle(source: &DefinitionSource) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    }
}

#[test]
fn positional_links_export_readable_targets_and_relative_self_ids_resolve() {
    use intersect_model::definitions::{ecore_document, rcore::lower_structure};
    let source = DefinitionSource::new(
        "model.rcore",
        SourceKind::Rcore,
        "rcore 1; package test version \"1\"; metamodel \"urn:test\" prefix t { class Base {} class Child extends generic external \"#//@eClassifiers.0\" { ref base: external \"#//@eClassifiers.0\" [0,1]; } }",
    );
    let compiled = compile_structure(&bundle(&source), &source.id).unwrap();
    assert_eq!(
        compiled.schema.classes["Child"]
            .parents
            .first()
            .map(String::as_str),
        Some("Base")
    );
    let exported = lower_structure(source, "memory:diagram.ecore").unwrap();
    assert!(exported.generated.text.contains("eType=\"#//Base\""));
    let diagram = ecore_document::inspect(&exported.generated.text).unwrap();
    assert!(diagram.warnings.is_empty(), "{:?}", diagram.warnings);
    let child = diagram.classes.iter().find(|c| c.name == "Child").unwrap();
    assert_eq!(child.super_types, ["Base"]);
    assert_eq!(child.features[0].type_name, "Base");
}

#[test]
fn generic_classes_operations_f_bounds_and_wildcards_retain_authored_identity() {
    let source = source(
        r#"
        class Comparable<T> {}
        class Box<T extends Comparable<T>> {
            ref value: T [0,1];
            op convert<U>(input: T [0,1]) -> U [0,1];
        }
        class Holder { ref raw: Box [0,1]; ref boxed: Box<?> [0,1]; }
    "#,
    );
    let bundle = bundle(&source);
    let resources = ResourceSet::new(&bundle).unwrap();
    let resolver = Resolver::new(&bundle).unwrap();
    let t = resources.resolve(&source.id, "#//Box/T").unwrap();
    let value = resources
        .resolve(&source.id, "#//Box/value/@eGenericType.0")
        .unwrap();
    assert_eq!(
        resolver.type_expression(&value).unwrap().expression,
        Type::Parameter {
            declaration: t.clone()
        }
    );
    let bound = resources
        .resolve(&source.id, "#//Box/T/@eBounds.0")
        .unwrap();
    let Type::Classifier { arguments, .. } = resolver.type_expression(&bound).unwrap().expression
    else {
        panic!("classifier bound")
    };
    assert_eq!(arguments, vec![Type::Parameter { declaration: t }]);
    let output = resources
        .resolve(&source.id, "#//Box/convert/@eGenericType.0")
        .unwrap();
    assert_eq!(
        resolver.type_expression(&output).unwrap().expression,
        Type::Parameter {
            declaration: resources.resolve(&source.id, "#//Box/convert/U").unwrap()
        }
    );
    let wildcard = resources
        .resolve(
            &source.id,
            "#//Holder/boxed/@eGenericType.0/@eTypeArguments.0",
        )
        .unwrap();
    assert_eq!(
        resolver.type_expression(&wildcard).unwrap().expression,
        Type::Wildcard {
            upper: None,
            lower: None
        }
    );
    let inventory = resolver.resolve_all().unwrap();
    assert_eq!(inventory.parameters.len(), 3);
    // A generic declaration is resolved here; an executable schema requires an
    // explicit closed application, tested separately below.
    assert!(compile_structure(&bundle, &source.id).is_err());
}

#[test]
fn cross_resource_types_require_exact_retained_dependencies() {
    let dependency = DefinitionSource::new(
        "memory:types.rcore",
        SourceKind::Rcore,
        "rcore 1; package types version \"1\"; metamodel \"urn:types\" prefix t { class Target {} }",
    );
    let mut source =
        source("class Holder { ref target: external \"memory:types.rcore#//Target\" [0,1]; }");
    source.dependencies.push(dependency.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone(), dependency.clone()],
        namespaces: vec![],
    };
    let compiled = compile_structure(&bundle, &source.id).unwrap();
    assert!(
        compiled
            .schema
            .declarations
            .values()
            .any(|id| id.source == dependency.id)
    );
    let mut missing = bundle.clone();
    missing.sources.pop();
    assert!(compile_structure(&missing, &source.id).is_err());
    let mut changed = bundle;
    changed.sources[1].text.push(' ');
    assert!(compile_structure(&changed, &source.id).is_err());
}

#[test]
fn nested_packages_datatypes_annotations_and_void_signatures_are_native_declarations() {
    let source = source(
        r##"
        annotation "documentation" { detail "text" = "café 😀"; reference "#//Holder"; }
        datatype Token instanceClass "java.lang.String" {}
        class Holder {
            annotation "class-doc" { detail "body" = "preserved, not executed"; }
            attr token: Token [0,1];
            attr byte: EByte [0,1] default "42";
            ref item: nested::Item [0,1];
            op clear() -> void;
        }
        package nested namespace "urn:nested" prefix n { class Item {} }
    "##,
    );
    let bundle = bundle(&source);
    let resources = ResourceSet::new(&bundle).unwrap();
    assert!(resources.resolve(&source.id, "#//nested/Item").is_ok());
    let datatype = resources.resolve(&source.id, "#//Token").unwrap();
    let node = resources.node(&datatype).unwrap();
    assert!(
        node.attributes
            .iter()
            .any(|a| a.name.local == "instanceClassName" && a.value == "java.lang.String")
    );
    let compiled = compile_structure(&bundle, &source.id).unwrap();
    assert_eq!(compiled.operations.len(), 1);
    assert!(compiled.classifiers.iter().any(|c| c.name == "Item"));
    assert!(
        compiled
            .schema
            .generic_signatures
            .as_ref()
            .unwrap()
            .retained_payloads
            .len()
            >= 2
    );
}

#[test]
fn native_generic_application_constructs_and_checks_model_references() {
    use intersect_model::core::model::Model;
    use intersect_model::definitions::{
        compiler::materialize_closed_types, generic_types::ClosedType,
    };
    let source = source(
        "class Value {} class Other {} class Box<T> { ref value: T [0,1]; } class ValueBox extends Box<Value> {}",
    );
    let bundle = bundle(&source);
    let resources = ResourceSet::new(&bundle).unwrap();
    let closed = |name: &str| {
        ClosedType::new(Type::Classifier {
            declaration: resources
                .resolve(&source.id, &format!("#//{name}"))
                .unwrap(),
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let value = closed("Value");
    let other = closed("Other");
    let child = closed("ValueBox");
    let schema =
        materialize_closed_types(&bundle, &[value.clone(), other.clone(), child.clone()]).unwrap();
    let value_key = schema
        .class_key(&source.id, "urn:extensions", "Value")
        .unwrap()
        .to_owned();
    let other_key = schema
        .class_key(&source.id, "urn:extensions", "Other")
        .unwrap()
        .to_owned();
    let box_key = schema
        .class_key(&source.id, "urn:extensions", "ValueBox")
        .unwrap()
        .to_owned();
    let mut model = Model::default();
    model
        .create(&schema, "value", &value_key, "memory:objects", 0, 0)
        .unwrap();
    model
        .create(&schema, "other", &other_key, "memory:objects", 0, 0)
        .unwrap();
    model
        .create(&schema, "box", &box_key, "memory:objects", 0, 0)
        .unwrap();
    model
        .edit(
            &schema,
            &[("box".into(), "value".into(), serde_json::json!("value"))],
        )
        .unwrap();
    let before = model.clone();
    assert!(
        model
            .edit(
                &schema,
                &[("box".into(), "value".into(), serde_json::json!("other"))]
            )
            .is_err()
    );
    assert_eq!(model, before);
}

#[test]
fn malformed_extended_syntax_and_missing_generic_types_report_original_spans() {
    for body in [
        "class A<T,T> {}",
        "class A<T extends Missing> {}",
        "class A { ref x: Missing<String> [0,1]; }",
        "class A { op broken() -> void throws Missing; }",
        "datatype D instanceClass \"a\" instanceClass \"b\" {}",
    ] {
        let source = source(body);
        let errors = compile_structure(&bundle(&source), &source.id).unwrap_err();
        assert!(errors.iter().all(|e| e.span.source == source.id), "{body}");
    }
}

#[test]
fn generic_ecore_migration_retains_definition_graph_and_reports_runtime_selection() {
    use intersect_model::migration::migrate_ecore;
    let original = DefinitionSource::new(
        "memory:generic.ecore",
        SourceKind::Ecore,
        include_str!("../../../fixtures/ecore-generics/signatures.ecore"),
    );
    let report = migrate_ecore(original.clone(), "memory:generic.rcore").unwrap();
    let converted = report
        .converted
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", report.issues));
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.code == "runtime_requirement" && !i.blocks_conversion)
    );
    let inventory = Resolver::new(&bundle(converted))
        .unwrap()
        .resolve_all()
        .unwrap();
    assert_eq!(inventory.parameters.len(), 3);
    assert_eq!(report.original, original);
    assert!(
        report
            .correspondence
            .iter()
            .all(|m| m.original.source == original.id && m.converted.source == converted.id)
    );
}

#[test]
fn whole_bundle_migration_rewrites_cycles_positional_links_and_exact_dependencies() {
    use intersect_model::migration::migrate_ecore_bundle;
    use std::collections::BTreeMap;
    let mut a = DefinitionSource::new(
        "https://example.test/a.ecore",
        SourceKind::Ecore,
        r##"
      <e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="a" nsURI="urn:a">
        <eClassifiers x:type="e:EEnum" name="Mode"><eLiterals name="One"/></eClassifiers>
        <eClassifiers x:type="e:EClass" name="A"><eStructuralFeatures x:type="e:EReference" name="b" eType="b.ecore#//B" eOpposite="b.ecore#//B/a"/></eClassifiers>
      </e:EPackage>"##,
    );
    let mut b = DefinitionSource::new(
        "https://example.test/b.ecore",
        SourceKind::Ecore,
        r##"
      <e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="b" nsURI="urn:b">
        <eClassifiers x:type="e:EClass" name="B"><eStructuralFeatures x:type="e:EReference" name="a" eType="a.ecore#//@eClassifiers.1" eOpposite="a.ecore#//A/b"/></eClassifiers>
      </e:EPackage>"##,
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let original = DefinitionBundle {
        version: 1,
        sources: vec![a.clone(), b.clone()],
        namespaces: vec![],
    };
    let targets = BTreeMap::from([
        (a.id.clone(), "https://example.test/a.rcore".into()),
        (b.id.clone(), "https://example.test/b.rcore".into()),
    ]);
    let report = migrate_ecore_bundle(original.clone(), &targets).unwrap();
    let converted = report
        .converted
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", report.reports));
    assert_eq!(report.original, original);
    assert_eq!(
        converted.sources[0].dependencies,
        [converted.sources[1].id.clone()]
    );
    assert_eq!(
        converted.sources[1].dependencies,
        [converted.sources[0].id.clone()]
    );
    assert!(!converted.sources.iter().any(|s| s.text.contains(".ecore")));
    let schema = compile_structure(converted, &converted.sources[0].id)
        .unwrap()
        .schema;
    assert_eq!(schema.classes.len(), 2);
    let resources = ResourceSet::new(converted).unwrap();
    for m in report.reports.iter().flat_map(|r| &r.correspondence) {
        resources.node(&m.converted).unwrap();
    }
}

#[test]
fn multiple_package_roots_require_selection_and_preserve_cross_root_targets() {
    use intersect_model::{
        definitions::rcore::compile_structure_selected, migration::migrate_ecore_bundle,
    };
    use std::collections::BTreeMap;
    let original = DefinitionSource::new(
        "memory:roots.ecore",
        SourceKind::Ecore,
        r##"
      <x:XMI xmlns:x="http://www.omg.org/XMI" xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:t="http://www.w3.org/2001/XMLSchema-instance">
        <e:EPackage name="a" nsURI="urn:a"><eClassifiers t:type="e:EClass" name="A"/></e:EPackage>
        <e:EPackage name="b" nsURI="urn:b"><eClassifiers t:type="e:EClass" name="B"><eStructuralFeatures t:type="e:EReference" name="a" eType="#/0/A"/></eClassifiers></e:EPackage>
      </x:XMI>"##,
    );
    let migrated = migrate_ecore_bundle(
        bundle(&original),
        &BTreeMap::from([(original.id.clone(), "memory:roots.rcore".into())]),
    )
    .unwrap();
    let converted = migrated
        .converted
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", migrated.reports));
    let id = &converted.sources[0].id;
    assert!(
        compile_structure(converted, id).unwrap_err()[0]
            .message
            .contains("selection")
    );
    let compiled = compile_structure_selected(converted, id, Some("urn:b")).unwrap();
    assert_eq!(compiled.schema.declarations["B"].path, [1, 0]);
    assert_eq!(compiled.namespace, "urn:b");
    assert_eq!(compiled.schema.classes.len(), 2);
}

#[test]
fn bundle_migration_retains_existing_rcore_and_text_sources_with_exact_edges() {
    use intersect_model::{definitions::bundle::NamespaceBinding, migration::migrate_ecore_bundle};
    use std::collections::BTreeMap;
    let existing = source("class Value {}");
    let mut original = DefinitionSource::new(
        "memory:holder.ecore",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:t="http://www.w3.org/2001/XMLSchema-instance" name="holder" nsURI="urn:holder"><eClassifiers t:type="e:EClass" name="Holder"><eStructuralFeatures t:type="e:EReference" name="value" eType="memory:extensions.rcore#//Value"/></eClassifiers></e:EPackage>"#,
    );
    original.dependencies.push(existing.id.clone());
    let mut text = DefinitionSource::new(
        "memory:holder.xtext",
        SourceKind::Xtext,
        "retained, not compiled here",
    );
    text.dependencies.push(original.id.clone());
    let inputs = DefinitionBundle {
        version: 1,
        sources: vec![original.clone(), existing.clone(), text.clone()],
        namespaces: vec![NamespaceBinding {
            namespace: "urn:holder".into(),
            source: original.id.clone(),
        }],
    };
    let report = migrate_ecore_bundle(
        inputs.clone(),
        &BTreeMap::from([(original.id.clone(), "memory:holder.rcore".into())]),
    )
    .unwrap();
    assert_eq!(report.original, inputs);
    let converted = report
        .converted
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", report.reports));
    assert_eq!(converted.source(&existing.id).unwrap(), &existing);
    let retained = converted.source(&text.id).unwrap();
    assert_eq!(retained.text, text.text);
    let migrated = &converted.sources[0];
    assert_eq!(retained.dependencies, std::slice::from_ref(&migrated.id));
    assert_eq!(migrated.dependencies, std::slice::from_ref(&existing.id));
    assert_eq!(converted.namespaces[0].source, migrated.id);
    let resources = ResourceSet::new(converted).unwrap();
    let reference = resources.resolve(&migrated.id, "#//Holder/value").unwrap();
    let target = resources
        .node(&reference)
        .unwrap()
        .attributes
        .iter()
        .find(|a| a.name.local == "eType")
        .unwrap();
    assert_eq!(
        resources.resolve(&migrated.id, &target.value).unwrap(),
        resources.resolve(&existing.id, "#//Value").unwrap()
    );
    assert_eq!(
        compile_structure(converted, &migrated.id)
            .unwrap()
            .schema
            .classes
            .len(),
        2
    );
}

#[test]
fn public_generic_model_selection_survives_reload_and_rejects_ambiguous_names() {
    use intersect_model::definitions::generic_types::RuntimeType;
    use intersect_model::{api::ModelEdit, compile_rcore_bundle_selected, rcore::RcorePackage};
    let source = source("class Value {} class Other {} class Box<T> { ref value: T [0,1]; }");
    let bundle = bundle(&source);
    let resources = ResourceSet::new(&bundle).unwrap();
    let ty = |name: &str, arguments| {
        RuntimeType::new(Type::Classifier {
            declaration: resources
                .resolve(&source.id, &format!("#//{name}"))
                .unwrap(),
            arguments,
            raw: false,
        })
        .unwrap()
    };
    let value = ty("Value", vec![]);
    let other = ty("Other", vec![]);
    let boxed_value = ty("Box", vec![value.expression().clone()]);
    let boxed_other = ty("Box", vec![other.expression().clone()]);
    let package = compile_rcore_bundle_selected(
        bundle,
        &source.id,
        "urn:extensions",
        &[
            value.clone(),
            other,
            boxed_value.clone(),
            boxed_other.clone(),
        ],
    )
    .unwrap();
    let mut model = package.new_model().unwrap();
    model.create_typed("v", &value, "memory:objects").unwrap();
    model
        .create_typed("box", &boxed_value, "memory:objects")
        .unwrap();
    assert!(model.create("ambiguous", "Box", "memory:objects").is_err());
    model.set("box", "value", serde_json::json!("v")).unwrap();
    let saved = package.save(&model).unwrap();
    let (restored, mut model) = RcorePackage::reload(&saved).unwrap();
    assert_eq!(restored.save(&model).unwrap(), saved);
    model
        .create_typed("other-box", &boxed_other, "memory:objects")
        .unwrap();
    assert!(
        model
            .set("other-box", "value", serde_json::json!("v"))
            .is_err()
    );
    let mut forged: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    forged.as_object_mut().unwrap().remove("types");
    assert!(RcorePackage::reload(&serde_json::to_vec(&forged).unwrap()).is_err());
}

#[test]
fn source_bound_xml_projection_preserves_provenance_and_exact_structural_grants() {
    use intersect_model::definitions::{bundle::NamespaceBinding, ecore_document, rcore};
    let grammar = DefinitionSource::new(
        "memory:provenance.xtext",
        SourceKind::Xtext,
        "retained provenance",
    );
    let mut model = DefinitionSource::new(
        "memory:self-context.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package preview version "1"; metamodel "urn:preview:self" prefix p { class Fields { attr name: String [0,1]; } class Model extends external "urn:preview:self#//Fields" {} }"#,
    );
    model.dependencies.push(grammar.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), grammar.clone()],
        namespaces: vec![NamespaceBinding {
            namespace: "urn:preview:self".into(),
            source: model.id.clone(),
        }],
    };
    let projected =
        rcore::lower_structure_bundle(&bundle, &model.id, "urn:preview:self", "memory:projection")
            .unwrap();
    assert_eq!(projected.original, bundle);
    assert_eq!(projected.lowerings[0].original, model);
    let xml = projected.bundle.source(&projected.entry).unwrap();
    assert!(xml.dependencies.is_empty());
    assert_ne!(xml.id, model.id);
    assert_eq!(projected.bundle.namespaces[0].source, xml.id);
    let inspected =
        ecore_document::inspect_in_bundle(&projected.bundle, &projected.entry, "urn:preview:self")
            .unwrap();
    assert!(inspected.warnings.is_empty());
    assert_eq!(
        inspected
            .classes
            .iter()
            .find(|c| c.name == "Model")
            .unwrap()
            .super_types,
        ["Fields"]
    );
    let mut invalid = bundle.clone();
    invalid.namespaces.clear();
    assert!(
        rcore::lower_structure_bundle(&invalid, &model.id, "urn:preview:self", "memory:projection")
            .is_err()
    );
    let mut invalid = bundle.clone();
    invalid.sources[0].text.push(' ');
    assert!(
        rcore::lower_structure_bundle(&invalid, &model.id, "urn:preview:self", "memory:projection")
            .is_err()
    );
}
