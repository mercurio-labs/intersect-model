use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, NamespaceBinding, SourceKind},
    rcore_behavior::modules::{ImportBinding, index_modules},
    rcore_behavior::{
        ast::{Expr, Type},
        bindings::{LocalKind, Target, bind},
    },
};

fn module(uri: &str, name: &str, version: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        uri,
        SourceKind::RcoreBehavior,
        format!("rcore 1; package {name} version \"{version}\"; {body}"),
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
fn lexical_scopes_preserve_initializer_capture_and_generic_identities() {
    let entry = module(
        "file:///entry-behavior.rcore",
        "entry",
        "1",
        "fn f(x: Int) -> Int = { let x = x + 1; let callback = |y: Int| x + y; match Some(x) { Some(z) => callback(z), none => x } }; fn id<T>(value: T) -> T = value;",
    );
    let bound = bind(&bundle(vec![entry.clone()]), &entry.id).unwrap();
    let document = &bound.index.modules[0].document;
    let bindings = &bound.bindings[0];
    let xs: Vec<_> = document
        .expressions
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(&e.value, Expr::Name(name) if name.value == "x"))
        .map(|(i, _)| &bindings.expressions[&i])
        .collect();
    assert!(matches!(
        xs[0],
        Target::Local {
            kind: LocalKind::Parameter,
            ..
        }
    ));
    assert!(xs[1..].iter().all(|x| matches!(
        x,
        Target::Local {
            kind: LocalKind::Let,
            ..
        }
    )));
    for (i, ty) in document.types.iter().enumerate() {
        if matches!(&ty.value, Type::Named {path, ..} if path[0].value == "T") {
            assert!(matches!(
                &bindings.types[&i],
                Target::Local {
                    kind: LocalKind::Generic,
                    ..
                }
            ));
        }
    }
}

#[test]
fn invalid_scope_references_are_located_and_never_deferred_as_members() {
    for (body, expected) in [
        (
            "fn f() -> Int = { let x = x; x };",
            "Unresolved value name x",
        ),
        (
            "fn f(x: Int, x: Int) -> Int = x;",
            "Duplicate local binding x",
        ),
        ("fn f<T, T>() -> Int = 0;", "Duplicate local binding T"),
        (
            "fn f() -> Int = { let x = 1; let x = 2; x };",
            "Duplicate local binding x",
        ),
        (
            "fn f() -> Int = match Some(1) { Some(x) => x, none => x };",
            "Unresolved value name x",
        ),
        (
            "fn f() -> Int = { let f = |x: Int| x; x };",
            "Unresolved value name x",
        ),
        ("fn f() -> Unknown = 0;", "Unresolved type name Unknown"),
        (
            "fn f() -> Int = unknown.member;",
            "Unresolved value name unknown",
        ),
    ] {
        let entry = module("file:///entry-behavior.rcore", "entry", "1", body);
        let errors = bind(&bundle(vec![entry.clone()]), &entry.id).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{body}: {errors:?}"
        );
        assert!(errors.iter().all(|e| e.span.source == entry.id));
    }
}

#[test]
fn mutual_recursion_and_qualified_privacy_are_order_independent() {
    let mut a = module(
        "file:///a-behavior.rcore",
        "a",
        "1",
        "import package b version \"1\" as b; pub fn first() -> Int = b.second();",
    );
    let mut b = module(
        "file:///b-behavior.rcore",
        "b",
        "1",
        "import package a version \"1\" as a; pub fn second() -> Int = a.first(); private fn secret() -> Int = 0;",
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let mut input = bundle(vec![a.clone(), b.clone()]);
    let result = bind(&input, &a.id).unwrap();
    assert!(result.bindings.iter().all(|m| {
        m.expressions
            .values()
            .any(|v| matches!(v, Target::Symbol(_)))
    }));
    input.sources.reverse();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::to_value(bind(&input, &a.id).unwrap()).unwrap()
    );
    // Parentheses must not turn a namespace into an unchecked instance receiver.
    let library = module(
        "file:///private-behavior.rcore",
        "private_lib",
        "1",
        "fn secret() -> Int = 0;",
    );
    for access in ["lib.secret()", "(lib).secret()", "lib.unknown()"] {
        let mut entry = module(
            "file:///entry-behavior.rcore",
            "entry",
            "1",
            &format!("import package private_lib version \"1\" as lib; fn f() -> Int = {access};"),
        );
        entry.dependencies.push(library.id.clone());
        assert!(bind(&bundle(vec![entry.clone(), library.clone()]), &entry.id).is_err());
    }
}

#[test]
fn library_model_providers_and_inherited_static_features_keep_ecore_identity() {
    let model = DefinitionSource::new(
        "file:///library.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/library/library.ecore"),
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let text = format!(
        "{}\nfn field() -> String = lib.BookCopy.name;",
        include_str!("../../../../fixtures/library/library-v2-behavior.rcore")
    );
    let mut entry = DefinitionSource::new(
        "file:///library-behavior.rcore",
        SourceKind::RcoreBehavior,
        text,
    );
    entry.dependencies.push(model.id.clone());
    let result = bind(&bundle(vec![entry.clone(), model.clone()]), &entry.id).unwrap();
    let bindings = &result.bindings[0];
    assert_eq!(bindings.providers.len(), 3);
    assert!(!bindings.instance_members.is_empty());
    assert!(bindings.expressions.values().any(
        |target| matches!(target, Target::Feature(id) if id.source == model.id && id.path == [0,0])
    ));
    for reference in ["lib.Absent", "lib.BookCopy.absent"] {
        let mut bad = module(
            "file:///bad-behavior.rcore",
            "bad",
            "1",
            &format!(
                "import model \"urn:intersect:example:library:1\" as lib; fn f() -> Int = {reference};"
            ),
        );
        bad.dependencies.push(model.id.clone());
        assert!(bind(&bundle(vec![bad.clone(), model.clone()]), &bad.id).is_err());
    }
}

#[test]
fn long_flat_bodies_bind_without_recursive_ast_walks() {
    let entry = module(
        "file:///long-behavior.rcore",
        "long",
        "1",
        &format!("fn f(x: Int) -> Int = {}x;", "x + ".repeat(10_000)),
    );
    let bound = bind(&bundle(vec![entry.clone()]), &entry.id).unwrap();
    assert_eq!(bound.bindings[0].expressions.len(), 10_001);
}

#[test]
fn pattern_constructor_names_resolve_before_arm_bindings_are_introduced() {
    let entry = module(
        "file:///patterns-behavior.rcore",
        "patterns",
        "1",
        "fn f(value: Int) -> Int = match value { Some(Some, Some(x)) => x, _ => 0 };",
    );
    let bound = bind(&bundle(vec![entry.clone()]), &entry.id).unwrap();
    assert_eq!(bound.bindings[0].constructors.len(), 2);
    assert!(
        bound.bindings[0]
            .constructors
            .values()
            .all(|v| matches!(v, Target::Builtin {name, version: 2} if name == "Some"))
    );
    let duplicate = module(
        "file:///duplicate-behavior.rcore",
        "patterns",
        "1",
        "fn f(value: Int) -> Int = match value { Some(x, x) => x, _ => 0 };",
    );
    assert!(
        bind(&bundle(vec![duplicate.clone()]), &duplicate.id)
            .unwrap_err()
            .iter()
            .any(|d| d.message.contains("Duplicate local binding x"))
    );
}

#[test]
fn cycles_exports_and_order_preserve_exact_symbol_identities() {
    let mut a = module(
        "file:///a-behavior.rcore",
        "sample.a",
        "1",
        "import package sample.b version \"1\" as b; pub fn visible() -> Int = 1; private fn hidden() -> Int = 2;",
    );
    let mut b = module(
        "file:///b-behavior.rcore",
        "sample.b",
        "1",
        "import package sample.a version \"1\" as a; pub fn caller() -> Int = a.visible();",
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let mut input = bundle(vec![a.clone(), b.clone()]);
    let index = index_modules(&input, &a.id).unwrap();
    let symbol = index
        .imported_symbol(&b.id, "a", &["visible".into()])
        .unwrap();
    assert_eq!(symbol.id.source, a.id);
    assert_eq!(symbol.id.declaration, 1);
    assert!(
        index
            .imported_symbol(&b.id, "a", &["hidden".into()])
            .unwrap_err()
            .contains("Private")
    );
    assert!(
        index
            .imported_symbol(&b.id, "a", &["b".into()])
            .unwrap_err()
            .contains("Unknown exported")
    );
    assert!(
        index
            .imported_symbol(&b.id, "missing", &["visible".into()])
            .is_err()
    );
    input.sources.reverse();
    assert_eq!(
        serde_json::to_value(&index).unwrap(),
        serde_json::to_value(index_modules(&input, &a.id).unwrap()).unwrap()
    );
}

#[test]
fn declared_path_version_and_exact_dependency_all_participate_in_selection() {
    let old = module(
        "file:///old-behavior.rcore",
        "tools",
        "1",
        "pub fn value() -> Int = 1;",
    );
    let new = module(
        "file:///new-behavior.rcore",
        "tools",
        "2",
        "pub fn value() -> Int = 2;",
    );
    let twin = module(
        "file:///twin-behavior.rcore",
        "tools",
        "1",
        "pub fn value() -> Int = 3;",
    );
    let mut entry = module(
        "file:///entry-behavior.rcore",
        "entry",
        "1",
        "import package tools version \"1\" as tools;",
    );
    let input =
        |entry: DefinitionSource| bundle(vec![entry, old.clone(), new.clone(), twin.clone()]);
    assert!(
        index_modules(&input(entry.clone()), &entry.id).unwrap_err()[0]
            .message
            .contains("No exact dependency")
    );
    entry.dependencies = vec![new.id.clone()];
    assert!(index_modules(&input(entry.clone()), &entry.id).is_err());
    entry.dependencies.push(old.id.clone());
    let index = index_modules(&input(entry.clone()), &entry.id).unwrap();
    assert_eq!(
        index
            .imported_symbol(&entry.id, "tools", &["value".into()])
            .unwrap()
            .id
            .source,
        old.id
    );
    entry.dependencies.push(twin.id.clone());
    let errors = index_modules(&input(entry.clone()), &entry.id).unwrap_err();
    assert!(errors[0].message.contains("Ambiguous"));
    assert_eq!(
        &entry.text[errors[0].span.start..errors[0].span.end],
        "tools"
    );
}

#[test]
fn duplicates_are_located_and_quoted_path_segments_do_not_collapse() {
    let same = module(
        "file:///same-behavior.rcore",
        "same",
        "1",
        "pub fn same() -> Int = 1; fn same() -> Int = 2;",
    );
    let definitions = bundle(vec![same.clone()]);
    assert_eq!(
        index_modules(&definitions, &same.id).unwrap().modules[0]
            .symbols
            .len(),
        2
    );
    let errors =
        intersect_model::definitions::rcore_behavior::signatures::index(&definitions, &same.id)
            .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Duplicate overload signature"))
    );
    assert!(errors.iter().all(|e| e.span.source == same.id));
    for body in [
        "pub fn same() -> Int = 1; derive same(self) -> Int = 2;",
        "import package tools version \"1\" as same; import package tools version \"1\" as same;",
        "import package tools version \"1\" as same; fn same() -> Int = 1;",
    ] {
        let tools = module("file:///tools-behavior.rcore", "tools", "1", "");
        let mut entry = module("file:///entry-behavior.rcore", "entry", "1", body);
        entry.dependencies.push(tools.id.clone());
        let errors = index_modules(&bundle(vec![entry.clone(), tools]), &entry.id).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("Duplicate")));
        assert!(errors.iter().all(|e| e.span.source == entry.id));
    }
    let entry = module(
        "file:///entry-behavior.rcore",
        "entry",
        "1",
        "pub fn `a.b`() -> Int = 1; pub derive a.b(self) -> Int = 2;",
    );
    let index = index_modules(&bundle(vec![entry.clone()]), &entry.id).unwrap();
    assert_eq!(index.modules[0].symbols[0].name, ["a.b"]);
    assert_eq!(index.modules[0].symbols[1].name, ["a", "b"]);
}

#[test]
fn model_namespaces_use_exact_declared_dependencies_and_explicit_bindings() {
    let text = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='test' nsURI='urn:test'/>";
    let a = DefinitionSource::new("file:///a.ecore", SourceKind::Ecore, text);
    let b = DefinitionSource::new("file:///b.ecore", SourceKind::Ecore, text);
    let mut entry = module(
        "file:///entry-behavior.rcore",
        "entry",
        "1",
        "import model \"urn:test\" as lib;",
    );
    let input = |entry: DefinitionSource| bundle(vec![entry, a.clone(), b.clone()]);
    assert!(index_modules(&input(entry.clone()), &entry.id).is_err());
    entry.dependencies = vec![a.id.clone(), b.id.clone()];
    assert!(
        index_modules(&input(entry.clone()), &entry.id).unwrap_err()[0]
            .message
            .contains("Ambiguous")
    );
    let mut selected = input(entry.clone());
    selected.namespaces.push(NamespaceBinding {
        namespace: "urn:test".into(),
        source: b.id.clone(),
    });
    let index = index_modules(&selected, &entry.id).unwrap();
    let ImportBinding::Model(id) = &index.module(&entry.id).unwrap().imports["lib"] else {
        panic!("model")
    };
    assert_eq!(id.source, b.id);
    assert!(id.path.is_empty());
    selected.sources[0].dependencies = vec![a.id.clone()];
    assert!(index_modules(&selected, &entry.id).is_err());
}

#[test]
fn invalid_sources_and_versions_never_produce_interfaces() {
    let wrong = DefinitionSource::new("file:///wrong.xtext", SourceKind::Xtext, "grammar wrong");
    assert!(
        index_modules(&bundle(vec![wrong.clone()]), &wrong.id).unwrap_err()[0]
            .message
            .contains("Rcore source")
    );
    let entry = module("file:///entry-behavior.rcore", "entry", "", "");
    assert!(
        index_modules(&bundle(vec![entry.clone()]), &entry.id).unwrap_err()[0]
            .message
            .contains("nonempty")
    );
    let mut entry = module("file:///entry-behavior.rcore", "entry", "1", "");
    let id = entry.id.clone();
    entry.text.push(' ');
    assert!(
        index_modules(&bundle(vec![entry]), &id).unwrap_err()[0]
            .message
            .contains("digest")
    );
    let malformed = module(
        "file:///malformed-behavior.rcore",
        "entry",
        "1",
        "fn broken;",
    );
    assert!(index_modules(&bundle(vec![malformed.clone()]), &malformed.id).is_err());
    assert!(index_modules(&bundle(vec![]), &id).is_err());
}
