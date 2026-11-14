use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        model_types::ModelTypes,
        types::{CollectionKind, Type, TypeTable},
    },
    resources::DeclarationId,
};
use std::collections::BTreeMap;

fn fixture() -> (DefinitionBundle, Vec<DeclarationId>) {
    let source = DefinitionSource::new(
        "memory:structural-relations",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:structural"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Left" eSuperTypes="#//Base"/><eClassifiers x:type="e:EClass" name="Right" eSuperTypes="#//Base"/></e:EPackage>"##,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let ids = (0..3)
        .map(|i| DeclarationId {
            source: source.id.clone(),
            path: vec![i],
        })
        .collect();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        ids,
    )
}

#[test]
fn collection_matrix_preserves_kind_covariance_and_exact_feature_cardinality() {
    let (bundle, ids) = fixture();
    let mut model = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let base = table.intern(Type::Model(ids[0].clone())).unwrap();
    let left = table.intern(Type::Model(ids[1].clone())).unwrap();
    let kinds = [
        (CollectionKind::Sequence, true, false),
        (CollectionKind::Set, false, true),
        (CollectionKind::OrderedSet, true, true),
        (CollectionKind::Bag, false, false),
    ];
    for (kind, ordered, unique) in &kinds {
        let actual = table
            .intern(Type::Collection {
                kind: kind.clone(),
                element: left,
            })
            .unwrap();
        let many = table
            .intern(Type::Many {
                element: left,
                lower: 2,
                upper: 4,
                ordered: *ordered,
                unique: *unique,
            })
            .unwrap();
        let unconstrained = table
            .intern(Type::Many {
                element: base,
                lower: 0,
                upper: -1,
                ordered: *ordered,
                unique: *unique,
            })
            .unwrap();
        assert!(model.assignable(&table, actual, unconstrained).unwrap());
        assert!(!model.assignable(&table, actual, many).unwrap());
        for (expected_kind, _, _) in &kinds {
            let expected = table
                .intern(Type::Collection {
                    kind: expected_kind.clone(),
                    element: base,
                })
                .unwrap();
            assert_eq!(
                model.assignable(&table, actual, expected).unwrap(),
                kind == expected_kind
            );
            assert_eq!(
                model.assignable(&table, many, expected).unwrap(),
                kind == expected_kind
            );
            assert!(!model.assignable(&table, expected, actual).unwrap());
        }
    }
}

#[test]
fn structural_join_preserves_positions_exact_fields_and_nested_feature_elements() {
    let (bundle, ids) = fixture();
    let mut model = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let base = table.intern(Type::Model(ids[0].clone())).unwrap();
    let left = table.intern(Type::Model(ids[1].clone())).unwrap();
    let right = table.intern(Type::Model(ids[2].clone())).unwrap();
    let many = table
        .intern(Type::Many {
            element: left,
            lower: 2,
            upper: 4,
            ordered: true,
            unique: false,
        })
        .unwrap();
    let sequence = table
        .intern(Type::Collection {
            kind: CollectionKind::Sequence,
            element: right,
        })
        .unwrap();
    let expected_sequence = table
        .intern(Type::Collection {
            kind: CollectionKind::Sequence,
            element: base,
        })
        .unwrap();
    let a = table.intern(Type::Tuple(vec![many, right])).unwrap();
    let b = table.intern(Type::Tuple(vec![sequence, left])).unwrap();
    let expected = table
        .intern(Type::Tuple(vec![expected_sequence, base]))
        .unwrap();
    let expected_tuple = expected;
    assert_eq!(model.join(&mut table, a, b).unwrap(), expected);
    assert_eq!(model.join(&mut table, b, a).unwrap(), expected);
    let record = |x, y| Type::Record(BTreeMap::from([("first".into(), x), ("second".into(), y)]));
    let a = table.intern(record(a, left)).unwrap();
    let b = table.intern(record(b, right)).unwrap();
    let expected = table.intern(record(expected, base)).unwrap();
    assert_eq!(model.join(&mut table, a, b).unwrap(), expected);
    let missing = table
        .intern(Type::Record(BTreeMap::from([("first".into(), base)])))
        .unwrap();
    assert!(!model.assignable(&table, a, missing).unwrap());
    assert!(!model.assignable(&table, missing, a).unwrap());
    assert!(model.join(&mut table, a, missing).is_err());
    let short = table.intern(Type::Tuple(vec![base])).unwrap();
    assert!(!model.assignable(&table, short, expected_tuple).unwrap());
    assert!(model.join(&mut table, short, expected_tuple).is_err());
}

#[test]
fn generic_parameters_require_exact_symbol_revision_and_ordinal() {
    use intersect_model::definitions::rcore_behavior::modules::SymbolId;
    let (bundle, ids) = fixture();
    let mut model = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let symbol = SymbolId {
        source: ids[0].source.clone(),
        declaration: 0,
    };
    let a = table
        .intern(Type::Parameter {
            symbol: symbol.clone(),
            ordinal: 0,
        })
        .unwrap();
    assert!(model.assignable(&table, a, a).unwrap());
    for (mut other, ordinal) in [(symbol.clone(), 1), (symbol.clone(), 0)] {
        if ordinal == 0 {
            other.source.sha256 = "0".repeat(64);
        }
        let b = table
            .intern(Type::Parameter {
                symbol: other,
                ordinal,
            })
            .unwrap();
        assert!(!model.assignable(&table, a, b).unwrap());
        assert!(model.join(&mut table, a, b).is_err());
    }
}
