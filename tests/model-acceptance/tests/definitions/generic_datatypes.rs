use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_datatype_bundle,
    datatypes::Datatype,
    generic_resolution::Resolver,
    resources::DeclarationId,
};
use serde_json::json;

const SOURCE: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">
<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EDataType" name="Nested"><eTypeParameters name="N"/></eClassifiers>
<eClassifiers x:type="e:EDataType" name="Pair" instanceClassName="java.lang.String">
<eAnnotations source="retained"/>
<eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters>
<eTypeParameters name="U"><eBounds eClassifier="#//Nested"><eTypeArguments eTypeParameter="#//Pair/T"/></eBounds></eTypeParameters>
</eClassifiers>
<eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/>
</e:EPackage>"##;

fn input(text: &str) -> (DefinitionBundle, DeclarationId) {
    let source = DefinitionSource::new("memory:generic-datatypes.ecore", SourceKind::Ecore, text);
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![2],
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        id,
    )
}

#[test]
fn imported_datatype_retains_exact_ordered_formals_and_nested_bounds() {
    let (bundle, id) = input(SOURCE);
    let datatype = import_datatype_bundle(&bundle, &id).unwrap();
    assert_eq!(datatype.declaration(), &id);
    assert_eq!(
        datatype
            .parameters()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["T", "U"]
    );
    assert_eq!(datatype.parameters()[0].declaration.path, [2, 1]);
    assert_eq!(datatype.parameters()[1].declaration.path, [2, 2]);
    let resolver = Resolver::new(&bundle).unwrap();
    for formal in datatype.parameters() {
        assert_eq!(
            resolver
                .resolve_parameter(&formal.declaration)
                .unwrap()
                .syntax,
            *formal
        );
        assert_eq!(
            &bundle.sources[0].text[formal.span.start..formal.span.end],
            formal.source_payload
        );
    }
    assert!(
        datatype
            .source_payload()
            .unwrap()
            .contains("source=\"retained\"")
    );
    assert!(
        datatype
            .source_payload()
            .unwrap()
            .contains("eTypeArguments")
    );
    assert!(
        datatype
            .codec()
            .unwrap_err()
            .contains("checked application")
    );
    assert!(
        datatype
            .bind_value(json!("no erased scalar value"))
            .is_err()
    );
    let restored: Datatype =
        serde_json::from_value(serde_json::to_value(&datatype).unwrap()).unwrap();
    assert_eq!(restored, datatype);
}

#[test]
fn generic_datatype_import_rejects_invalid_source_and_stale_identity() {
    for text in [
        SOURCE.replace("name=\"U\"", "name=\"T\""),
        SOURCE.replace("#//Base", "#//Missing"),
        SOURCE.replace("#//Pair/T", "#//Nested/N"),
        SOURCE.replace("name=\"U\"", "name=\"U\" unsupported=\"true\""),
        SOURCE.replace("<eAnnotations source=\"retained\"/>", "<unknown/>"),
    ] {
        let (bundle, id) = input(&text);
        let error = import_datatype_bundle(&bundle, &id).unwrap_err();
        assert_eq!(error.span.source, id.source);
    }
    let (bundle, mut id) = input(SOURCE);
    id.source.sha256 = "0".repeat(64);
    assert!(import_datatype_bundle(&bundle, &id).is_err());
}

#[test]
fn datatype_wire_preserves_ordinary_values_and_rejects_forged_formals() {
    let (bundle, mut id) = input(SOURCE);
    let generic = import_datatype_bundle(&bundle, &id).unwrap();
    let wire = serde_json::to_value(&generic).unwrap();
    for variant in 0..5 {
        let mut forged = wire.clone();
        match variant {
            0 => forged["parameters"][0]["declaration"]["path"] = json!([1, 0]),
            1 => forged["parameters"][1]["name"] = json!("T"),
            2 => forged["parameters"].as_array_mut().unwrap().reverse(),
            3 => {
                forged.as_object_mut().unwrap().remove("parameters");
            }
            _ => {
                let mut ordinary_id = id.clone();
                ordinary_id.path = vec![3];
                forged["binding"] = serde_json::to_value(
                    import_datatype_bundle(&bundle, &ordinary_id)
                        .unwrap()
                        .binding(),
                )
                .unwrap();
            }
        }
        assert!(serde_json::from_value::<Datatype>(forged).is_err());
    }
    id.path = vec![3];
    let ordinary = import_datatype_bundle(&bundle, &id).unwrap();
    let old_wire = serde_json::to_value(&ordinary).unwrap();
    assert!(old_wire.get("parameters").is_none());
    let restored: Datatype = serde_json::from_value(old_wire).unwrap();
    assert_eq!(ordinary, restored);
    assert_eq!(
        restored.format(&restored.parse("hello").unwrap()).unwrap(),
        "hello"
    );
}

#[test]
fn generic_datatype_bounds_resolve_exact_cross_resource_dependencies() {
    let dependency = DefinitionSource::new(
        "memory:base.ecore",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="base" nsURI="urn:base"><eClassifiers x:type="e:EClass" name="Base"/></e:EPackage>"#,
    );
    let (mut bundle, id) = input(&SOURCE.replace(
        "eClassifier=\"#//Base\"",
        "eClassifier=\"memory:base.ecore#//Base\"",
    ));
    bundle.sources[0].dependencies.push(dependency.id.clone());
    bundle.sources.push(dependency.clone());
    let datatype = import_datatype_bundle(&bundle, &id).unwrap();
    let resolved = Resolver::new(&bundle)
        .unwrap()
        .resolve_parameter(&datatype.parameters()[0].declaration)
        .unwrap();
    assert_eq!(resolved.bounds[0].bindings[0].target.source, dependency.id);
    bundle.sources.pop();
    assert!(import_datatype_bundle(&bundle, &id).is_err());
}
