use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        signatures,
        types::{Scalar, Type},
    },
};

fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}"),
        SourceKind::RcoreBehavior,
        format!("rcore 1; package {name} version \"1\"; {body}"),
    )
}
fn bundle(sources: Vec<DefinitionSource>) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}

#[test]
fn recursive_module_interfaces_share_canonical_types_independently_of_source_order() {
    let mut a = source(
        "a",
        "import package b version \"1\" as b; pub fn f(x: Int?) -> Result<Int64, String> = b.g(x); fn same(x: Option<Int64>) -> Int = 0;",
    );
    let mut b = source(
        "b",
        "import package a version \"1\" as a; pub fn g(x: Option<Int>) -> Result<Int, String> = a.f(x);",
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let forward = signatures::index(&bundle(vec![a.clone(), b.clone()]), &a.id).unwrap();
    let backward = signatures::index(&bundle(vec![b, a.clone()]), &a.id).unwrap();
    assert_eq!(
        serde_json::to_value(&forward).unwrap(),
        serde_json::to_value(&backward).unwrap()
    );
    assert_eq!(forward.functions.len(), 3);
    let f = &forward.functions[0];
    let same = &forward.functions[1];
    let g = &forward.functions[2];
    assert_eq!(f.parameters, same.parameters);
    assert_eq!(f.parameters, g.parameters);
    assert_eq!(f.result, g.result);
    assert!(matches!(
        forward.types.get(same.result).unwrap(),
        Type::Scalar(Scalar::Int64)
    ));
    assert!(matches!(
        forward.types.get(f.result).unwrap(),
        Type::Result { .. }
    ));
}

#[test]
fn every_annotation_is_checked_including_local_lambda_and_explicit_call_types() {
    let valid = source(
        "valid",
        "fn apply(f: (Int) -> Bool, x: Int) -> Bool = f(x); fn f(x: Int) -> Int = { let y: Int = x; let cb = |z: Int| z; cb(y) };",
    );
    let report = signatures::index(&bundle(vec![valid.clone()]), &valid.id).unwrap();
    assert_eq!(
        report.modules[0].annotations.len(),
        report.bound.index.modules[0].document.types.len()
    );
    assert!(matches!(
        report.types.get(report.functions[0].parameters[0]).unwrap(),
        Type::Function { .. }
    ));
    for (body, expected) in [
        (
            "fn f(x: Int) -> Int = { let y: Option = x; y };",
            "Option requires 1",
        ),
        (
            "fn f(x: Int) -> Int = { let y = |z: Bool<Int>| z; x };",
            "Bool requires 0",
        ),
        (
            "fn f(x: Int) -> Int = f::<Result<Int>>(x);",
            "Result requires 2",
        ),
        ("fn f(x: Int<Int>) -> Int = 0;", "Int requires 0"),
        (
            "fn f(x: Result<Int, Bool, String>) -> Int = 0;",
            "Result requires 2",
        ),
    ] {
        let input = source("invalid", body);
        let errors = signatures::index(&bundle(vec![input.clone()]), &input.id).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{errors:?}"
        );
        for error in errors {
            assert_eq!(error.span.source, input.id);
            assert!(error.span.start < error.span.end);
        }
    }
}

#[test]
fn structural_and_generic_signatures_do_not_claim_body_checking() {
    for body in [
        "fn f(x: Sequence<Int>) -> Int = 0;",
        "fn f(x: (Int, Bool)) -> Int = 0;",
        "fn f(x: {value: Int}) -> Int = 0;",
        "fn f<T>(x: T) -> T = x;",
        "fn f<T>() -> Int = 0;",
    ] {
        let input = source("future", body);
        assert!(signatures::index(&bundle(vec![input.clone()]), &input.id).is_ok());
    }
    let input = source("body", "fn wrong() -> Int = true;");
    // The public entrypoint promises interface checking only. A later full checker
    // must reject this body; never use this report as package activation evidence.
    assert!(signatures::index(&bundle(vec![input.clone()]), &input.id).is_ok());
}

#[test]
fn model_annotations_retain_exact_classifier_revisions_and_reject_wrong_kinds() {
    let xml = r#"<m:EPackage xmlns:m="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="model" nsURI="urn:model" nsPrefix="m"><eClassifiers x:type="m:EClass" name="Node"/><eClassifiers x:type="m:EEnum" name="Color"/></m:EPackage>"#;
    let model = DefinitionSource::new("memory:model.ecore", SourceKind::Ecore, xml);
    let mut input = source(
        "main",
        "import model \"urn:model\" as m; fn f(x: m.Node) -> m.Node = x;",
    );
    input.dependencies.push(model.id.clone());
    let report = signatures::index(&bundle(vec![model.clone(), input.clone()]), &input.id).unwrap();
    let Type::Model(id) = report.types.get(report.functions[0].result).unwrap() else {
        panic!("model type")
    };
    assert_eq!(id.source, model.id);
    assert_eq!(id.path, vec![0]);
    let changed =
        DefinitionSource::new("memory:model.ecore", SourceKind::Ecore, format!("{xml}\n"));
    let mut next = input.clone();
    next.dependencies = vec![changed.id.clone()];
    let changed_report =
        signatures::index(&bundle(vec![changed.clone(), next.clone()]), &next.id).unwrap();
    assert_ne!(
        report.types.get(report.functions[0].result).unwrap(),
        changed_report
            .types
            .get(changed_report.functions[0].result)
            .unwrap()
    );
    for (ty, expected) in [
        ("m.Color<Int>", "requires 0"),
        ("m.Node<Int>", "requires 0"),
    ] {
        let mut bad = source(
            "bad",
            &format!("import model \"urn:model\" as m; fn f(x: {ty}) -> Int = 0;"),
        );
        bad.dependencies.push(model.id.clone());
        let errors =
            signatures::index(&bundle(vec![model.clone(), bad.clone()]), &bad.id).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{errors:?}"
        );
    }
}
