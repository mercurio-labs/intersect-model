use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        bindings::{LocalKind, Target, bind},
        modules::index_modules,
        signatures,
    },
};
fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
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
fn local_sets_keep_every_exact_symbol_and_lexical_values_shadow_them() {
    let entry = source(
        "local",
        "fn pick(x:Int8)->Int8=x;fn pick(x:Bool)->Bool=x;fn useSet()->Int8=((pick))(1);fn shadow()->Int8={let pick=|x:Int8|x;pick(1)};",
    );
    let definitions = bundle(vec![entry.clone()]);
    let bound = bind(&definitions, &entry.id).unwrap();
    let module = &bound.index.modules[0];
    assert_eq!(
        module.symbols.iter().filter(|s| s.name == ["pick"]).count(),
        2
    );
    let bindings = &bound.bindings[0];
    let overloads: Vec<_> = bindings
        .expressions
        .values()
        .filter_map(|target| match target {
            Target::Overload(ids) => Some(ids),
            _ => None,
        })
        .collect();
    assert!(!overloads.is_empty());
    for ids in overloads {
        assert_eq!(
            ids.iter().map(|id| id.declaration).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert!(ids.iter().all(|id| id.source == entry.id));
    }
    assert!(bindings.expressions.values().any(|target| matches!(
        target,
        Target::Local {
            kind: LocalKind::Let,
            ..
        }
    )));
    assert_eq!(
        signatures::index(&definitions, &entry.id)
            .unwrap()
            .functions
            .len(),
        4
    );
}

#[test]
fn public_import_sets_ignore_private_candidates_and_singular_lookup_is_ambiguous() {
    let helper = source(
        "helper",
        "fn pick(x:String)->String=x;pub fn pick(x:Int8)->Int8=x;pub fn pick(x:Bool)->Bool=x;fn secret(x:Int8)->Int8=x;",
    );
    let mut entry = source(
        "entry",
        "import package helper version \"1\" as h;fn run()->Int8=h.pick(1);",
    );
    entry.dependencies.push(helper.id.clone());
    let definitions = bundle(vec![entry.clone(), helper.clone()]);
    let indexed = index_modules(&definitions, &entry.id).unwrap();
    let candidates = indexed
        .imported_symbols(&entry.id, "h", &["pick".into()])
        .unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|s| s.id.declaration)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(
        indexed
            .imported_symbol(&entry.id, "h", &["pick".into()])
            .unwrap_err()
            .contains("Ambiguous")
    );
    assert!(
        indexed
            .imported_symbols(&entry.id, "h", &["secret".into()])
            .unwrap_err()
            .contains("Private")
    );
    let bound = bind(&definitions, &entry.id).unwrap();
    let bindings = bound
        .bindings
        .iter()
        .find(|b| b.source == entry.id)
        .unwrap();
    let ids = bindings
        .expressions
        .values()
        .find_map(|t| match t {
            Target::Overload(ids) => Some(ids),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        ids.iter().map(|id| id.declaration).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(ids.iter().all(|id| id.source == helper.id));
    let mut reversed = definitions.clone();
    reversed.sources.reverse();
    assert_eq!(
        serde_json::to_value(bind(&reversed, &entry.id).unwrap()).unwrap(),
        serde_json::to_value(bound).unwrap()
    );
    for body in [
        "import package helper version \"1\" as h;fn run()->Int8=h.secret(1);",
        "import package helper version \"1\" as h;fn run(x:h.pick)->Int8=1;",
    ] {
        let mut invalid = source("entry", body);
        invalid.dependencies.push(helper.id.clone());
        let errors = bind(&bundle(vec![invalid.clone(), helper.clone()]), &invalid.id).unwrap_err();
        assert!(errors.iter().any(|e| e.span.source == invalid.id));
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Private") || e.message.contains("Type name"))
        );
    }
}

#[test]
fn duplicate_signatures_ignore_formal_spelling_but_preserve_bounds_and_structure() {
    for body in [
        "fn same(x:Int)->Int=x;fn same(y:Int64)->Int64=y;",
        "fn same<T>(x:T)->T=x;fn same<U>(y:U)->U=y;",
        "fn same<T:Int8,U:T>(x:U)->T=x;fn same<A:Int8,B:A>(y:B)->A=y;",
        "fn same<T>(x:(T)->T)->(T)->T=x;fn same<U>(x:(U)->U)->(U)->U=x;",
        "fn same<T>(x:{a:T,b:T})->{a:T,b:T}=x;fn same<U>(x:{b:U,a:U})->{b:U,a:U}=x;",
    ] {
        let entry = source("duplicate", body);
        let definitions = bundle(vec![entry.clone()]);
        assert_eq!(
            index_modules(&definitions, &entry.id).unwrap().modules[0]
                .symbols
                .len(),
            2
        );
        let errors = signatures::index(&definitions, &entry.id).unwrap_err();
        let duplicate = errors
            .iter()
            .find(|e| e.message.contains("Duplicate overload signature"))
            .unwrap_or_else(|| panic!("{body}: {errors:?}"));
        assert_eq!(duplicate.span.source, entry.id);
        assert_eq!(
            &entry.text[duplicate.span.start..duplicate.span.end],
            "same"
        );
        assert!(duplicate.span.start > entry.text.find("same").unwrap());
    }
    for body in [
        "fn same<T:Int8>(x:T)->T=x;fn same<U:Int16>(x:U)->U=x;",
        "fn same<T>(x:T)->T=x;fn same<U,V>(x:U)->U=x;",
        "fn same(x:Int8)->Int8=1;fn same(x:Int8)->Int16=1;",
        "fn same<T>(x:Sequence<T>)->Sequence<T> =x;fn same<U>(x:Set<U>)->Set<U> =x;",
    ] {
        let entry = source("distinct", body);
        assert_eq!(
            signatures::index(&bundle(vec![entry.clone()]), &entry.id)
                .unwrap_or_else(|e| panic!("{body}: {e:?}"))
                .functions
                .len(),
            2
        );
    }
}

#[test]
fn overload_names_do_not_relax_import_alias_collisions() {
    let helper = source("helper", "pub fn pick(x:Int8)->Int8=x;");
    let mut entry = source(
        "collision",
        "import package helper version \"1\" as pick;fn pick(x:Int8)->Int8=x;fn pick(x:Bool)->Bool=x;",
    );
    entry.dependencies.push(helper.id.clone());
    let errors = index_modules(&bundle(vec![entry.clone(), helper]), &entry.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Duplicate import/declaration name")
                && e.span.source == entry.id)
    );
}
