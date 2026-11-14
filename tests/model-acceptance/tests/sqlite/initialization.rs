#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::{
    query::QueryBudget,
    storage::{
        self, ClassInitializer, Construction, InitialValue, InitializationContext, ModelRead,
        ModelStore,
    },
};
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::ProviderId,
    initialization::ClassInitializationContract,
};
use intersect_model::sqlite::SqliteStore;
use serde_json::json;
struct Policy(ClassInitializationContract);
impl ClassInitializer for Policy {
    fn contract(&self) -> &ClassInitializationContract {
        &self.0
    }
    fn plan(
        &self,
        _: &dyn ModelRead,
        _: &str,
        budget: &mut QueryBudget,
    ) -> Result<Vec<InitialValue>, String> {
        budget.charge()?;
        Ok(vec![InitialValue {
            feature: self.0.members[0].clone(),
            value: json!("constructed"),
        }])
    }
}
#[test]
fn initialization_requires_v5_and_survives_reopen_and_nested_rollback() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-settings/settings.ecore"),
    )
    .unwrap();
    let source =
        DefinitionSource::new("test:constructor", SourceKind::RcoreBehavior, "constructed");
    let provider = Policy(ClassInitializationContract {
        version: 1,
        schema_revision: schema.revision.clone(),
        class: "Item".into(),
        class_identity: schema.identity("Item"),
        provider: ProviderId {
            name: "test".into(),
            version: "1".into(),
        },
        sources: vec![source.id.clone()],
        members: vec![schema.effective_feature("Item", "title").unwrap().identity],
    });
    let context = InitializationContext {
        schema: &schema,
        expected: &provider.0,
        sources: std::slice::from_ref(&source),
        provider: Some(&provider),
        conversions: None,
    };
    let path = std::env::temp_dir().join(format!(
        "intersect-initializer-v5-{}.db",
        std::process::id()
    ));
    assert!(!path.exists());
    let mut store = SqliteStore::open(&path).unwrap();
    let receipt = context
        .create(
            &mut store,
            Construction {
                id: "a",
                resource: "model",
                authored: &[],
            },
            &mut QueryBudget::new(1000),
        )
        .unwrap();
    let original = storage::export(&store).unwrap();
    assert!(
        storage::atomic(&mut store, |store| {
            context.create(
                store,
                Construction {
                    id: "b",
                    resource: "model",
                    authored: &[],
                },
                &mut QueryBudget::new(1000),
            )?;
            store.set_initialization("a", None)?;
            Err::<(), String>("rollback".into())
        })
        .is_err()
    );
    assert_eq!(storage::export(&store).unwrap(), original);
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.get("a").unwrap().unwrap().initialization,
        Some(receipt)
    );
    assert!(store.get("b").unwrap().is_none());
    storage::validate(&store, &schema).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=3").unwrap();
    drop(connection);
    assert!(
        SqliteStore::open(&path)
            .err()
            .unwrap()
            .contains("expected format 5")
    );
    std::fs::remove_file(path).unwrap();
}
