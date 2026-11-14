#![cfg(not(target_arch = "wasm32"))]

use intersect_model::{
    api::{BasicModel, ModelEdit},
    core::{
        interchange::{FormatRegistry, ModelFormat, ReferenceClosure, Resource},
        model::Model,
        resource_set::{ResourceContext, ResourceSet, TargetResolution},
        resources::{ResourceLoadState, ResourceRecord},
        storage::{self, ModelRead, ModelStore},
    },
    definitions::schema::Schema,
    sqlite::SqliteStore,
};
use serde_json::json;
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

struct TemporaryDatabase(PathBuf);
impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct ConcurrentEditFormat {
    external: RefCell<SqliteStore>,
    incoming: Model,
    after_external_edit: Rc<RefCell<Option<Model>>>,
}
impl ModelFormat for ConcurrentEditFormat {
    fn id(&self) -> &str {
        "concurrent-edit"
    }
    fn extensions(&self) -> &[&str] {
        &["race"]
    }
    fn media_type(&self) -> &str {
        "application/x-intersect-concurrent-edit-test"
    }
    fn decode(&self, _: &Schema, _: &[Resource]) -> Result<Model, String> {
        Err("This regression codec requires retained context".into())
    }
    fn encode(&self, _: &Schema, _: &Model) -> Result<Vec<Resource>, String> {
        Err("This regression codec only decodes".into())
    }
    fn decode_with_context(
        &self,
        _: &Schema,
        _: &[Resource],
        retained: &Model,
    ) -> Result<Model, String> {
        // The staging snapshot must have closed its read transaction before
        // invoking a codec. This independent connection publishes a real edit
        // after that baseline was captured and before replacement publication.
        let mut external = self.external.borrow_mut();
        storage::atomic(&mut *external, |store| {
            store.set_value("selected", "name", Some(json!("external edit")))
        })?;
        *self.after_external_edit.borrow_mut() = Some(storage::export(&*external)?);
        let mut candidate = retained.clone();
        candidate.elements.extend(self.incoming.elements.clone());
        candidate.resources.extend(self.incoming.resources.clone());
        Ok(candidate)
    }
}

#[test]
fn concurrent_selected_edit_during_decode_rejects_stale_replacement_without_storage_changes()
-> Result<(), String> {
    let package = intersect_model::compile_rcore(include_str!(
        "../../../../fixtures/rcore/api/organization.rcore"
    ))
    .map_err(|error| error.to_string())?;
    let schema = package.schema().clone();
    let node = schema
        .declarations
        .iter()
        .find(|(key, d)| d.name == "Node" && schema.classes.contains_key(*key))
        .map(|(key, _)| key.clone())
        .ok_or("Missing Node class")?;
    let context = ResourceContext::new(&schema, BTreeMap::new())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "intersect-resource-set-race-{}-{nonce}.sqlite",
        std::process::id()
    ));
    let reserved = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    let database = TemporaryDatabase(path);
    drop(reserved);
    let mut set =
        ResourceSet::from_store(schema, context.clone(), SqliteStore::open(&database.0)?)?;
    set.transaction(|edit| {
        edit.create("selected", &node, "selected.xmi")?;
        edit.set("selected", "name", json!("original"))?;
        edit.create("retained", &node, "retained.xmi")?;
        edit.set("retained", "name", json!("retained"))
    })?;
    set.bind_fragment("retained.xmi", "stable", "retained")?;
    let before = set.snapshot()?;
    let rows = set.reader().id_page(None, 10)?;
    let retained = set.reader().get("retained")?;
    let mut incoming = before.clone();
    incoming
        .elements
        .retain(|element| element.resource == "selected.xmi");
    incoming.resources.retain(|uri, _| uri == "selected.xmi");
    incoming.elements[0]
        .values
        .insert("name".into(), json!("replacement"));
    let after_external_edit = Rc::new(RefCell::new(None));
    let mut registry = FormatRegistry::default();
    registry.register(ConcurrentEditFormat {
        external: RefCell::new(SqliteStore::open(&database.0)?),
        incoming,
        after_external_edit: after_external_edit.clone(),
    })?;
    let error = set
        .import_with(
            &registry,
            "concurrent-edit",
            &["selected.xmi".into()],
            &[Resource {
                uri: "selected.xmi".into(),
                bytes: Vec::new(),
            }],
            &context,
            |_| Ok(()),
        )
        .err()
        .ok_or("Concurrent selected edit was overwritten")?;
    assert!(error.contains("Stale resource replacement base"), "{error}");
    let expected = after_external_edit
        .borrow()
        .clone()
        .ok_or("Codec edit hook did not execute")?;
    assert_eq!(set.snapshot()?, expected);
    assert_eq!(
        set.reader().value("selected", "name")?,
        Some(json!("external edit"))
    );
    assert_eq!(set.reader().get("retained")?, retained);
    assert_eq!(
        set.resources()?["retained.xmi"],
        before.resources["retained.xmi"]
    );
    assert_eq!(set.reader().id_page(None, 10)?, rows);
    assert!(
        matches!(set.resolve("selected.xmi", "retained.xmi#stable")?, TargetResolution::Resolved { element, .. } if element == "retained")
    );
    Ok(())
}

#[test]
fn unavailable_empty_resources_reject_save_and_allow_explicit_replace_or_import_recovery()
-> Result<(), String> {
    let package = intersect_model::compile_rcore(include_str!(
        "../../../../fixtures/rcore/api/organization.rcore"
    ))
    .map_err(|error| error.to_string())?;
    let schema = package.schema().clone();
    let node = schema
        .declarations
        .iter()
        .find(|(key, declaration)| declaration.name == "Node" && schema.classes.contains_key(*key))
        .map(|(key, _)| key.clone())
        .ok_or("Missing Node class")?;
    let context = ResourceContext::new(&schema, BTreeMap::new())?;
    let selection = ["selected.xmi".into()];
    let mut incoming = BasicModel::new(schema.clone())?;
    incoming.create("recovered", &node, "selected.xmi")?;
    incoming.set("recovered", "name", json!("recovered content"))?;
    let incoming = storage::export(&incoming)?;
    let mut registry = FormatRegistry::default();
    registry.register(intersect_model::xmi::XmiFormat)?;
    let encoded = registry.export("xmi", &schema, &incoming)?;
    for state in [ResourceLoadState::Failed, ResourceLoadState::Unloaded] {
        let mut set =
            ResourceSet::from_store(schema.clone(), context.clone(), SqliteStore::in_memory()?)?;
        set.create_resource("selected.xmi")?;
        set.transaction(|edit| edit.create("retained", &node, "retained.xmi"))?;
        let retained = set.reader().get("retained")?;
        let retained_record = set.resources()?["retained.xmi"].clone();
        let retained_row = set.reader().id_page(None, 10)?[0].clone();
        let mut unavailable = ResourceRecord::new("selected.xmi")?;
        unavailable.load_state = state;
        unavailable
            .diagnostics
            .push("Explicitly unavailable source".into());
        let unavailable = Model {
            schema_revision: schema.revision.clone(),
            resources: BTreeMap::from([("selected.xmi".into(), unavailable)]),
            elements: Vec::new(),
        };
        set.replace(&selection, unavailable.clone(), &context)?;
        assert_eq!(set.resources()?["selected.xmi"].load_state, state);
        assert!(
            set.prepare_save(&registry, "xmi", &selection, ReferenceClosure::SelectedOnly)
                .is_err()
        );
        set.replace(&selection, incoming.clone(), &context)?;
        assert_eq!(
            set.resources()?["selected.xmi"].load_state,
            ResourceLoadState::Loaded
        );
        assert_eq!(
            set.reader().value("recovered", "name")?,
            Some(json!("recovered content"))
        );
        set.replace(&selection, unavailable, &context)?;
        let before_retry = set.snapshot()?;
        assert!(
            set.import_with(
                &registry,
                "xmi",
                &selection,
                &[Resource {
                    uri: "selected.xmi".into(),
                    bytes: b"<unsupported/>".to_vec()
                }],
                &context,
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(set.snapshot()?, before_retry);
        set.import_with(&registry, "xmi", &selection, &encoded, &context, |_| Ok(()))?;
        assert_eq!(
            set.resources()?["selected.xmi"].load_state,
            ResourceLoadState::Loaded
        );
        assert_eq!(
            set.reader().value("recovered", "name")?,
            Some(json!("recovered content"))
        );
        assert_eq!(set.reader().get("retained")?, retained);
        assert_eq!(set.resources()?["retained.xmi"], retained_record);
        assert_eq!(set.reader().id_page(None, 1)?, [retained_row]);
        assert!(
            set.prepare_save(&registry, "xmi", &selection, ReferenceClosure::SelectedOnly)
                .is_ok()
        );
    }
    Ok(())
}

#[test]
fn failed_recovery_publication_restores_exact_unavailable_resource_metadata() -> Result<(), String>
{
    let package = intersect_model::compile_rcore(include_str!(
        "../../../../fixtures/rcore/api/organization.rcore"
    ))
    .map_err(|error| error.to_string())?;
    let schema = package.schema().clone();
    let node = schema
        .declarations
        .iter()
        .find(|(key, declaration)| declaration.name == "Node" && schema.classes.contains_key(*key))
        .map(|(key, _)| key.clone())
        .ok_or("Missing Node class")?;
    let context = ResourceContext::new(&schema, BTreeMap::new())?;
    let selection = ["selected.xmi".into()];
    let mut incoming = BasicModel::new(schema.clone())?;
    incoming.create("recovered", &node, "selected.xmi")?;
    let incoming = storage::export(&incoming)?;
    incoming.validate(&schema)?;
    for state in [ResourceLoadState::Failed, ResourceLoadState::Unloaded] {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "intersect-resource-recovery-failure-{}-{nonce}.sqlite",
            std::process::id()
        ));
        let reserved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        let database = TemporaryDatabase(path);
        drop(reserved);
        let mut set = ResourceSet::from_store(
            schema.clone(),
            context.clone(),
            SqliteStore::open(&database.0)?,
        )?;
        set.create_resource("selected.xmi")?;
        set.transaction(|edit| edit.create("retained", &node, "retained.xmi"))?;
        let mut unavailable = ResourceRecord::new("selected.xmi")?;
        unavailable.load_state = state;
        unavailable
            .diagnostics
            .push("Preserve this failure diagnostic".into());
        unavailable.source = Some(intersect_model::core::resources::SourceCorrespondence {
            uri: "selected.nodes".into(),
            revision: "source-before-failure".into(),
        });
        set.replace(
            &selection,
            Model {
                schema_revision: schema.revision.clone(),
                resources: BTreeMap::from([("selected.xmi".into(), unavailable)]),
                elements: Vec::new(),
            },
            &context,
        )?;
        let before = set.snapshot()?;
        let rows = set.reader().id_page(None, 10)?;
        // The candidate is valid. Fail only after publication has temporarily
        // enabled writes to the unavailable resource and starts inserting rows.
        let connection =
            rusqlite::Connection::open(&database.0).map_err(|error| error.to_string())?;
        connection.execute_batch("CREATE TRIGGER reject_recovered_insert BEFORE INSERT ON intersect_elements WHEN NEW.id='recovered' BEGIN SELECT RAISE(ABORT,'injected publication failure'); END;")
            .map_err(|error| error.to_string())?;
        drop(connection);
        let error = set
            .replace(&selection, incoming.clone(), &context)
            .err()
            .ok_or("Injected insertion failure was ignored")?;
        assert!(error.contains("injected publication failure"), "{error}");
        assert_eq!(set.snapshot()?, before);
        assert_eq!(set.resources()?["selected.xmi"].load_state, state);
        assert_eq!(set.reader().id_page(None, 10)?, rows);
        assert!(set.reader().get("recovered")?.is_none());
    }
    Ok(())
}

#[test]
fn repeated_explicit_assignment_and_metadata_setters_preserve_exact_resource_state()
-> Result<(), String> {
    use intersect_model::definitions::dynamic::{ScalarType, SchemaBuilder};
    let mut builder = SchemaBuilder::new("noop", "urn:noop");
    builder
        .class("Item")?
        .attribute("value", ScalarType::String)
        .unsettable(true)
        .default("seed")?;
    let schema = builder.build()?.as_schema().clone();
    let mut basic = BasicModel::from_store(schema, SqliteStore::in_memory()?)?;
    basic.create("item", "Item", "model.xmi")?;
    // The first explicit assignment is real history even though its value
    // equals the default. Later identical assignments change neither history
    // nor effective content, so they must preserve the resource revision.
    basic.set("item", "value", json!("seed"))?;
    let original = storage::export(&basic)?;
    let state = original.elements[0]
        .assignment_state
        .clone()
        .ok_or("Missing assignment evidence")?;
    assert!(state.explicitly_set("value"));
    basic.set("item", "value", json!("seed"))?;
    assert_eq!(storage::export(&basic)?, original);
    let mut store = basic.into_store();
    storage::atomic(&mut store, |store| {
        store.set_assignment_state("item", Some(state))?;
        store.set_initialization("item", None)?;
        let unchanged = store.get("item")?.ok_or("Missing item")?;
        store.put(unchanged)
    })?;
    assert_eq!(storage::export(&store)?, original);
    Ok(())
}

#[test]
fn unloaded_resources_survive_disk_reopen_and_failed_loads_roll_back() -> Result<(), String> {
    use intersect_model::core::{resource_set::UnloadPolicy, storage::InvocationContext};
    let package = intersect_model::compile_rcore(
        r#"
        rcore 1; package residency version "1";
        metamodel "urn:residency" prefix residency {
            class Node { attr name: String [0,1]; ref target: Node [0,1]; }
        }
    "#,
    )
    .map_err(|error| error.to_string())?;
    let schema = package.schema().clone();
    let node = schema.classes.keys().next().ok_or("Missing Node")?.clone();
    let context = ResourceContext::new(&schema, BTreeMap::new())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let database = TemporaryDatabase(std::env::temp_dir().join(format!(
        "intersect-resource-residency-{}-{nonce}.sqlite",
        std::process::id()
    )));
    let mut set = ResourceSet::from_store(
        schema.clone(),
        context.clone(),
        SqliteStore::open(&database.0)?,
    )?;
    set.transaction(|edit| {
        edit.create("a", &node, "catalog.xmi")?;
        edit.set("a", "name", json!("Catalog target"))?;
        edit.create("b", &node, "loans.xmi")?;
        edit.set("b", "target", json!("a"))
    })?;
    let mut registry = FormatRegistry::default();
    registry.register(intersect_model::xmi::XmiFormat)?;
    let selected = vec!["catalog.xmi".into()];
    let ticket = set.prepare_save(&registry, "xmi", &selected, ReferenceClosure::SelectedOnly)?;
    let bytes = ticket.resources().to_vec();
    let before = set.snapshot()?;
    set.drain_changes();
    let connection = rusqlite::Connection::open(&database.0).map_err(|error| error.to_string())?;
    connection.execute_batch("CREATE TRIGGER reject_unload BEFORE DELETE ON intersect_elements WHEN OLD.id='a' BEGIN SELECT RAISE(ABORT,'injected unload failure'); END;").map_err(|error| error.to_string())?;
    assert!(
        set.unload(&registry, &selected, UnloadPolicy::DiscardChanges)
            .is_err()
    );
    assert_eq!(set.snapshot()?, before);
    assert!(set.drain_changes().is_empty());
    connection
        .execute_batch("DROP TRIGGER reject_unload;")
        .map_err(|error| error.to_string())?;
    set.unload(&registry, &selected, UnloadPolicy::DiscardChanges)?;
    let suspended = set.snapshot()?;
    assert_eq!(
        set.reader()
            .get("b")?
            .ok_or("Missing retained reference")?
            .values["target"],
        json!("a")
    );
    drop(set.into_store());
    drop(connection);
    let mut reopened =
        ResourceSet::from_resident_store(schema, context.clone(), SqliteStore::open(&database.0)?)?;
    assert_eq!(reopened.snapshot()?, suspended);
    assert!(reopened.validate().is_err());
    let connection = rusqlite::Connection::open(&database.0).map_err(|error| error.to_string())?;
    connection.execute_batch("CREATE TRIGGER reject_reload BEFORE INSERT ON intersect_elements WHEN NEW.id='a' BEGIN SELECT RAISE(ABORT,'injected reload failure'); END;").map_err(|error| error.to_string())?;
    let prepared = reopened.prepare_reload(
        &registry,
        "xmi",
        &selected,
        &bytes,
        &context,
        &InvocationContext::default(),
    )?;
    assert!(
        reopened
            .commit_reload(prepared, |_| Ok(()), &InvocationContext::default())
            .is_err()
    );
    assert_eq!(reopened.snapshot()?, suspended);
    assert!(reopened.drain_changes().is_empty());
    connection
        .execute_batch("DROP TRIGGER reject_reload;")
        .map_err(|error| error.to_string())?;
    let prepared = reopened.prepare_reload(
        &registry,
        "xmi",
        &selected,
        &bytes,
        &context,
        &InvocationContext::default(),
    )?;
    reopened.commit_reload(prepared, |_| Ok(()), &InvocationContext::default())?;
    reopened.validate()?;
    assert_eq!(
        reopened
            .reader()
            .get("a")?
            .ok_or("Missing restored target")?
            .values["name"],
        json!("Catalog target")
    );
    assert_eq!(
        reopened.reader().get("b")?,
        before
            .elements
            .iter()
            .find(|element| element.id == "b")
            .cloned()
    );
    Ok(())
}
