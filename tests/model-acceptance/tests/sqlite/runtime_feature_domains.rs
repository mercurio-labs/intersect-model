#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::{
    model::Element,
    storage::{self, ModelStore},
};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
use intersect_model::sqlite::SqliteStore;
use serde_json::json;

// Frozen acceptance: persisted receiver-specific domains must preserve independent
// raw formal bounds, concrete argument invariance, exact identities, and atomic
// rollback after a real database close/reopen. Optional empty domains stay legal.
#[test]
fn receiver_specific_pair_domains_survive_reopen_and_rejected_transactions() {
    let source = DefinitionSource::new(
        "memory:sqlite-feature-domains",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let ty = |n, arguments, raw| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![n],
        },
        arguments,
        raw,
    };
    let a = ty(1, vec![], false);
    let b = ty(2, vec![], false);
    let other = ty(3, vec![], false);
    let expressions = vec![
        ty(0, vec![], false),
        a.clone(),
        b.clone(),
        other.clone(),
        ty(4, vec![], true),
        ty(4, vec![a.clone(), b], false),
        ty(4, vec![a.clone(), a.clone()], false),
        ty(4, vec![a.clone(), other], false),
        ty(5, vec![], true),
        ty(5, vec![a], false),
    ];
    let types: Vec<_> = expressions
        .into_iter()
        .map(|t| RuntimeType::new(t).unwrap())
        .collect();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let schema = materialize_runtime_types(&bundle, &types, None).unwrap();
    let key = |i: usize| schema.class_key_for_type(&types[i]).unwrap();
    assert_ne!(key(8), key(9));
    assert!(schema.reference_conforms(key(8), "pair", key(5)).unwrap());
    assert!(!schema.reference_conforms(key(8), "pair", key(7)).unwrap());
    assert!(schema.reference_conforms(key(9), "pair", key(6)).unwrap());
    assert!(!schema.reference_conforms(key(9), "pair", key(5)).unwrap());

    let path = std::env::temp_dir().join(format!(
        "intersect-feature-domains-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let replay_path = path.with_extension("source.json");
    let expected = {
        let mut store = SqliteStore::open(&path).unwrap();
        let checkpoint = store.begin().unwrap();
        for (id, index) in [("ab", 5), ("aa", 6), ("bad", 7), ("raw", 8), ("closed", 9)] {
            storage::insert(
                &mut store,
                &schema,
                Element::new(&schema, id, key(index), "memory:stored", 0, 0).unwrap(),
            )
            .unwrap();
        }
        store.commit(&checkpoint).unwrap();
        storage::edit(
            &mut store,
            &schema,
            &[
                ("raw".into(), "pair".into(), json!("ab")),
                ("closed".into(), "pair".into(), json!("aa")),
            ],
        )
        .unwrap();
        storage::validate(&store, &schema).unwrap();
        std::fs::write(
            &replay_path,
            serde_json::to_vec(&(bundle.clone(), types.clone(), schema.clone())).unwrap(),
        )
        .unwrap();
        storage::export(&store).unwrap()
    };
    let (retained_bundle, retained_types, retained_schema): (
        DefinitionBundle,
        Vec<RuntimeType>,
        intersect_model::definitions::schema::Schema,
    ) = serde_json::from_slice(&std::fs::read(&replay_path).unwrap()).unwrap();
    let rebuilt = materialize_runtime_types(&retained_bundle, &retained_types, None).unwrap();
    assert_eq!(rebuilt, retained_schema);
    {
        let mut store = SqliteStore::open(&path).unwrap();
        storage::validate(&store, &rebuilt).unwrap();
        assert_eq!(storage::export(&store).unwrap(), expected);
        for rejected in [
            vec![
                ("raw".into(), "pair".into(), json!("aa")),
                ("closed".into(), "pair".into(), json!("ab")),
            ],
            vec![
                ("raw".into(), "pair".into(), json!("aa")),
                ("raw".into(), "pair".into(), json!("bad")),
            ],
        ] {
            assert!(storage::edit(&mut store, &rebuilt, &rejected).is_err());
            assert_eq!(storage::export(&store).unwrap(), expected);
        }
    }
    {
        let store = SqliteStore::open(&path).unwrap();
        storage::validate(&store, &rebuilt).unwrap();
        assert_eq!(storage::export(&store).unwrap(), expected);
    }
    // No concrete Pair is required to keep an optional raw feature empty.
    let sparse = materialize_runtime_types(
        &retained_bundle,
        &[
            retained_types[0].clone(),
            retained_types[4].clone(),
            retained_types[8].clone(),
        ],
        None,
    )
    .unwrap();
    let raw_key = sparse.class_key_for_type(&retained_types[8]).unwrap();
    assert!(
        sparse
            .feature_domains(raw_key, "pair")
            .iter()
            .all(|d| d.alternatives.is_empty())
    );
    let mut optional = intersect_model::core::model::Model::default();
    optional
        .create(&sparse, "empty", raw_key, "memory:optional", 0, 0)
        .unwrap();
    optional.validate(&sparse).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(replay_path).unwrap();
}
