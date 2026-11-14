#![cfg(not(target_arch = "wasm32"))]
//! The same converted transaction consumer over the frozen arena and SQLite.
use intersect_model::conversion::RgbConverter;
use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel},
    frozen::FrozenStore,
    model::Model,
    storage::{self, CollectionEdit, ModelStore},
};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_datatype_bundle,
    conversion::{ConversionContract, ConversionRegistry, ProviderId},
    resources::ResourceSet,
    schema::Schema,
};
use intersect_model::sqlite::SqliteStore;
use serde_json::json;

fn exercise<S: ModelStore>(open: impl FnOnce(&Schema) -> S) -> Model {
    let text = include_str!("../../../../fixtures/ecore-unsettable/provider.ecore");
    let source = DefinitionSource::new("memory:ecore", SourceKind::Ecore, text);
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let id = ResourceSet::new(&bundle)
        .unwrap()
        .resolve(&source.id, "#//RGB")
        .unwrap();
    let datatype = import_datatype_bundle(&bundle, &id).unwrap();
    let provider = |version: &str| {
        RgbConverter::new(ConversionContract {
            datatype: id.clone(),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: version.into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
    };
    let current = provider("1");
    let stale = provider("2");
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &current).unwrap();
    let mut stale_registry = ConversionRegistry::default();
    stale_registry.register(&datatype, &stale).unwrap();
    let schema = intersect_model::definitions::rcore::compile_structure_with_conversions(
        &bundle, &source.id, None, &registry,
    )
    .unwrap()
    .schema;
    let value = serde_json::to_value(registry.parse(&id, "#010203").unwrap()).unwrap();
    let store = open(&schema);
    let basic = BasicModel::from_store_with_conversions(schema, store, &registry).unwrap();
    let mut model = TransactionalModel::new(basic);
    model
        .transaction_with_conversions(&registry, |edit| edit.create("a", "Paint", "model"))
        .unwrap();
    let defaults = storage::export(&model).unwrap();
    assert!(model.transaction(|_| Ok(())).is_err());
    for missing in [&ConversionRegistry::default(), &stale_registry] {
        let validated = std::cell::Cell::new(false);
        assert!(
            model
                .transaction_with_conversions_and_validation(
                    missing,
                    |_| Ok(()),
                    |_| {
                        validated.set(true);
                        Ok(())
                    }
                )
                .is_err()
        );
        assert!(!validated.get());
        assert_eq!(storage::export(&model).unwrap(), defaults);
    }
    model
        .transaction_with_conversions(&registry, |edit| {
            edit.set("a", "color", value.clone())?;
            let before = storage::export(edit)?;
            assert!(
                edit.set_many(&[
                    (
                        "a",
                        "color",
                        edit.schema().features("Paint")?["color"].default.clone()
                    ),
                    ("a", "optional", json!("invalid"))
                ])
                .is_err()
            );
            assert_eq!(storage::export(edit)?, before);
            edit.set("a", "optional", value.clone())?;
            assert!(edit.edit_collection("a", "colors", CollectionEdit::Add(value.clone()))?);
            assert!(!edit.edit_collection("a", "colors", CollectionEdit::Add(value.clone()))?);
            assert!(edit.is_set("a", "color")?);
            edit.validate()
        })
        .unwrap();
    let committed = storage::export(&model).unwrap();
    model.validate_with_conversions(&registry).unwrap();
    assert!(model.validate().is_err());
    assert!(
        model
            .transaction_with_conversions(&registry, |edit| {
                edit.unset("a", "color")?;
                edit.edit_collection("a", "colors", CollectionEdit::Clear)?;
                Err::<(), String>("callback rejected".into())
            })
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), committed);
    assert!(
        model
            .transaction_with_conversions_and_validation(
                &registry,
                |edit| edit.unset("a", "optional"),
                |_| Err("domain rejected".into())
            )
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), committed);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = model.transaction_with_conversions(&registry, |edit| -> Result<(), String> {
            edit.unset("a", "color")?;
            panic!("test callback unwind");
        });
    }));
    assert!(panic.is_err());
    assert_eq!(storage::export(&model).unwrap(), committed);
    model
        .transaction_with_conversions(&registry, |edit| {
            for field in ["color", "optional", "colors"] {
                edit.unset("a", field)?;
                assert!(!edit.is_set("a", field)?);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        semantic_model(storage::export(&model).unwrap()),
        semantic_model(defaults.clone()),
    );
    let basic = model.into_basic();
    basic.validate_with_conversions(&registry).unwrap();
    storage::export(&basic).unwrap()
}

#[test]
fn conversion_sessions_share_validation_commit_and_rollback_across_stores() {
    let frozen = exercise(|schema| FrozenStore::for_schema(schema).unwrap());
    let sqlite = exercise(|_| SqliteStore::in_memory().unwrap());
    assert_eq!(semantic_model(sqlite), semantic_model(frozen));
}

// Resource content revisions are adapter-local deterministic tokens, not a
// portable edit counter. Preserve every other field in semantic comparisons.
fn semantic_model(
    mut model: intersect_model::core::model::Model,
) -> intersect_model::core::model::Model {
    for record in model.resources.values_mut() {
        record.content_revision = 0;
    }
    model
}
