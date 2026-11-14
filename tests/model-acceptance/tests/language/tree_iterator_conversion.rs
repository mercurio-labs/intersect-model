use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::standard_datatype,
    conversion::{ConversionContract, ConversionProvider, ConversionRegistry, ProviderId},
    resources::DeclarationId,
};
use intersect_model::tree_iterator_conversion::{TreeIteratorConverter, TreeSnapshot};
use serde_json::json;
fn setup(paths: Vec<Vec<usize>>) -> (DefinitionBundle, ConversionContract) {
    let source = DefinitionSource::new(
        "memory:tree",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='tree' nsURI='urn:tree'><eClassifiers xsi:type='e:EClass' name='A'><eStructuralFeatures xsi:type='e:EAttribute' name='name' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers><eClassifiers xsi:type='e:EClass' name='B'/></e:EPackage>",
    );
    let roots = paths
        .into_iter()
        .map(|path| DeclarationId {
            source: source.id.clone(),
            path,
        })
        .collect();
    let contract = ConversionContract {
        members: Vec::new(),
        declarations: roots,
        datatype: standard_datatype("ETreeIterator")
            .unwrap()
            .declaration()
            .clone(),
        provider: ProviderId {
            name: "ecore.definition-tree-iterator".into(),
            version: "1".into(),
        },
        serializable: false,
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        contract,
    )
}
fn drain(adapter: &TreeIteratorConverter, cursor: &mut TreeSnapshot) -> Vec<Vec<usize>> {
    let mut result = vec![];
    while let Some(id) = adapter.next(cursor).unwrap() {
        result.push(id.path);
    }
    result
}
#[test]
fn preorder_pruning_exhaustion_and_snapshot_replay_preserve_cursor_lifecycle() {
    let (bundle, contract) = setup(vec![vec![]]);
    let adapter = TreeIteratorConverter::new(contract, &bundle).unwrap();
    let mut cursor = adapter.start();
    let initial = cursor.clone();
    assert!(adapter.has_next(&cursor).unwrap());
    assert_eq!(cursor, initial);
    assert!(adapter.prune(&mut cursor).is_err());
    assert_eq!(cursor, initial);
    assert_eq!(
        drain(&adapter, &mut cursor),
        [vec![], vec![0], vec![0, 0], vec![1]]
    );
    let exhausted = cursor.clone();
    assert!(!adapter.has_next(&cursor).unwrap());
    assert_eq!(adapter.next(&mut cursor).unwrap(), None);
    assert_eq!(cursor, exhausted);
    let mut cursor = adapter.start();
    assert_eq!(
        adapter.next(&mut cursor).unwrap().unwrap().path,
        Vec::<usize>::new()
    );
    assert_eq!(adapter.next(&mut cursor).unwrap().unwrap().path, [0]);
    adapter.prune(&mut cursor).unwrap();
    let pruned = cursor.clone();
    adapter.prune(&mut cursor).unwrap();
    assert_eq!(cursor, pruned);
    let restored = adapter
        .snapshot(&serde_json::to_value(&cursor).unwrap())
        .unwrap();
    assert_eq!(restored, cursor);
    assert_eq!(drain(&adapter, &mut cursor), [vec![1]]);
    let mut fresh = adapter.start();
    adapter.next(&mut fresh).unwrap();
    adapter.prune(&mut fresh).unwrap();
    assert!(drain(&adapter, &mut fresh).is_empty());
    assert!(adapter.remove(&mut fresh).is_err());
    let (bundle, contract) = setup(vec![vec![], vec![0]]);
    let adapter = TreeIteratorConverter::new(contract, &bundle).unwrap();
    assert_eq!(
        drain(&adapter, &mut adapter.start()),
        [vec![], vec![0], vec![0, 0], vec![1], vec![0], vec![0, 0]]
    );
    let (bundle, contract) = setup(vec![]);
    let adapter = TreeIteratorConverter::new(contract, &bundle).unwrap();
    let mut empty = adapter.start();
    assert!(drain(&adapter, &mut empty).is_empty());
    assert!(adapter.prune(&mut empty).is_err());
}
#[test]
fn forged_cursor_and_unknown_stale_or_duplicate_roots_reject_without_mutation() {
    let (bundle, contract) = setup(vec![vec![]]);
    let adapter = TreeIteratorConverter::new(contract.clone(), &bundle).unwrap();
    let valid = serde_json::to_value(adapter.start()).unwrap();
    for (field, bad) in [
        ("position", json!(5)),
        ("position", json!(u64::MAX)),
        ("pruned", json!([0])),
        ("pruned", json!([1])),
        ("roots", json!([])),
    ] {
        let mut value = valid.clone();
        value[field] = bad;
        assert!(adapter.snapshot(&value).is_err());
    }
    for pruned in [vec![1, 1], vec![2, 1], vec![3]] {
        let invalid = TreeSnapshot {
            roots: contract.declarations.clone(),
            position: 2,
            pruned,
        };
        assert!(adapter.has_next(&invalid).is_err());
    }
    let mut wrong = adapter.start();
    wrong.position = 99;
    let before = wrong.clone();
    assert!(adapter.next(&mut wrong).is_err());
    assert!(adapter.prune(&mut wrong).is_err());
    assert_eq!(wrong, before);
    for value in [
        json!(null),
        json!({}),
        json!({"roots":contract.declarations,"position":0,"pruned":[],"hostIterator":1}),
    ] {
        assert!(adapter.validate(&value).is_err());
    }
    for variant in 0..4 {
        let mut bad = contract.clone();
        match variant {
            0 => bad.declarations[0].source.sha256 = "stale".into(),
            1 => bad.declarations[0].path = vec![99],
            2 => bad.declarations.push(bad.declarations[0].clone()),
            _ => bad.serializable = true,
        };
        assert!(TreeIteratorConverter::new(bad, &bundle).is_err());
    }
}
#[test]
fn raw_generic_metadata_and_nonserializable_registry_remain_explicit() {
    let datatype = standard_datatype("ETreeIterator").unwrap();
    assert!(!datatype.serializable());
    assert_eq!(
        datatype.instance_class_name(),
        Some("org.eclipse.emf.common.util.TreeIterator")
    );
    let (bundle, contract) = setup(vec![vec![1]]);
    let adapter = TreeIteratorConverter::new(contract, &bundle).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &adapter).unwrap();
    let value = registry
        .bind(
            datatype.declaration(),
            serde_json::to_value(adapter.start()).unwrap(),
        )
        .unwrap();
    registry.validate(&value).unwrap();
    assert!(registry.format(&value).is_err());
    assert!(registry.parse(datatype.declaration(), "iterator").is_err());
    assert!(adapter.parse("iterator").is_err());
    assert!(adapter.format(&json!({})).is_err());
    assert_eq!(adapter.default_value().unwrap(), None);
    assert!(
        serde_json::to_string(&datatype)
            .unwrap()
            .contains("eTypeParameters")
    );
}
