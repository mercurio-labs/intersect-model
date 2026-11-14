use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        signatures,
        types::{CollectionKind, Scalar, Type, TypeRef, TypeTable},
    },
};

fn index(
    body: &str,
) -> Result<
    signatures::Signatures,
    Vec<intersect_model::definitions::rcore_behavior::SyntaxDiagnostic>,
> {
    let source = DefinitionSource::new(
        "memory:s04-signatures",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package signatures version \"1\"; {body}"),
    );
    let entry = source.id.clone();
    signatures::index(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &entry,
    )
}

#[test]
fn structural_annotations_preserve_collection_kinds_and_canonical_record_identity() {
    let report = index("fn f(a: Sequence<Int>, b: Set<Int>, c: OrderedSet<Int>, d: Bag<Int>, e: (Int, String), r: {x:Int,y:String}) -> () = (); fn g(r: {y:String,x:Int}) -> Unit = ();").unwrap();
    let f = &report.functions[0];
    for (ty, expected) in f.parameters.iter().zip([
        CollectionKind::Sequence,
        CollectionKind::Set,
        CollectionKind::OrderedSet,
        CollectionKind::Bag,
    ]) {
        assert!(
            matches!(report.types.get(*ty).unwrap(), Type::Collection { kind, .. } if kind == &expected)
        );
    }
    assert!(
        matches!(report.types.get(f.parameters[4]).unwrap(), Type::Tuple(items) if items.len()==2)
    );
    assert_eq!(f.parameters[5], report.functions[1].parameters[0]);
    assert_eq!(f.result, report.functions[1].result);
    assert!(matches!(
        report.types.get(f.result).unwrap(),
        Type::Scalar(Scalar::Unit)
    ));
}

#[test]
fn generic_signatures_use_function_scopes_and_lower_earlier_bounds() {
    let report = index("fn f<T, U:T>(x: Sequence<U>) -> T = f(x); fn g<T>(x:T) -> T = x;").unwrap();
    let f = &report.functions[0];
    let g = &report.functions[1];
    assert_eq!(f.generics[1].bound, Some(f.generics[0].ty));
    assert_eq!(f.result, f.generics[0].ty);
    assert_eq!(g.parameters[0], g.generics[0].ty);
    assert_ne!(f.generics[0].ty, g.generics[0].ty);
    assert!(
        matches!(report.types.get(f.generics[1].ty).unwrap(), Type::Parameter {symbol,ordinal:1} if symbol==&f.symbol)
    );
    assert!(
        matches!(report.types.get(f.parameters[0]).unwrap(), Type::Collection {element,..} if *element==f.generics[1].ty)
    );
}

#[test]
fn malformed_structural_and_generic_signatures_have_source_diagnostics() {
    for (body, expected) in [
        (
            "fn f(x: Set<Int,String>) -> Int = 0;",
            "Set requires 1 type arguments",
        ),
        (
            "fn f(x: {x:Int,x:String}) -> Int = 0;",
            "Duplicate record field x",
        ),
        ("fn f<T:T>(x:T) -> T = x;", "only earlier type parameters"),
        (
            "fn f<T:Sequence<U>, U:T>(x:T) -> T = x;",
            "only earlier type parameters",
        ),
        (
            "fn f<T>(x:T<Int>) -> T = x;",
            "Generic parameter requires 0 type arguments",
        ),
    ] {
        let errors = index(body).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|e| e.span.source.uri == "memory:s04-signatures" && e.span.start > 0)
        );
    }
}

#[test]
fn structural_type_insertion_checks_children_and_nesting() {
    let mut table = TypeTable::default();
    assert!(table.intern(Type::Tuple(vec![TypeRef(12)])).is_err());
    assert!(
        table
            .intern(Type::Record(
                [("x".into(), TypeRef(12))].into_iter().collect()
            ))
            .is_err()
    );
    assert!(
        table
            .intern(Type::Collection {
                kind: CollectionKind::Bag,
                element: TypeRef(12)
            })
            .is_err()
    );
    let mut ty = table.intern(Type::Scalar(Scalar::Int64)).unwrap();
    for _ in 1..64 {
        ty = table
            .intern(Type::Collection {
                kind: CollectionKind::Sequence,
                element: ty,
            })
            .unwrap();
    }
    assert!(table.intern(Type::Tuple(vec![ty])).is_err());
}

#[test]
fn empty_tuple_and_unit_have_one_canonical_type_identity() {
    use intersect_model::definitions::rcore_behavior::types::{Scalar, Type, TypeTable};
    let mut table = TypeTable::default();
    assert_eq!(
        table.intern(Type::Tuple(vec![])).unwrap(),
        table.intern(Type::Scalar(Scalar::Unit)).unwrap()
    );
}
