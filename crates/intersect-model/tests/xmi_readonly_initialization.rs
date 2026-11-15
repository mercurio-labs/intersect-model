#![cfg(feature = "xmi")]
//! Fresh readonly restoration uses the SAME checked class initializer before
//! any serialized authored field or receipt reaches the fresh decoder row.
use crate::handle_id;
use intersect_model::{
    core::{
        api::ModelEdit,
        frozen::FrozenStore,
        interchange::{FormatContext, FormatRegistry, ReferenceClosure, Resource},
        model::Model,
        query::QueryBudget,
        resource_set::{ResourceContext, ResourceSet, UnloadPolicy},
        storage::{
            ClassInitializer, Construction, InitialValue, InitializationBinding,
            InitializationContext, InvocationContext, ModelRead, SnapshotInitializationSelector,
            StorageResult, StructuralValueContext, StructuralValueRequest, StructuralValueSelector,
        },
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::ProviderId,
        initialization::ClassInitializationContract,
        rcore::compile_structure,
        schema::Schema,
    },
    xmi::XmiFormat,
};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};
struct Initializer {
    contract: ClassInitializationContract,
    calls: Cell<usize>,
}
impl ClassInitializer for Initializer {
    fn contract(&self) -> &ClassInitializationContract {
        &self.contract
    }
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<InitialValue>> {
        budget.charge()?;
        self.calls.set(self.calls.get() + 1);
        if store.value(id, "operator")? != Some(Value::Null) {
            return Err("Composite operator must be at genuine fresh default".into());
        }
        let mut resource = None;
        store.inspect(id, &mut |row| {
            resource = Some(row.resource().to_owned());
            Ok(())
        })?;
        // Like the actual native provider, retaining an existing string would
        // make a late imported-value verification tautological. The codec must
        // invoke this while readonly id and writable operator are still null.
        let identity = match store.value(id, "identity")? {
            Some(Value::String(value)) => value,
            _ => format!("{}#{id}", resource.ok_or("Missing fresh row")?),
        };
        Ok(vec![
            InitialValue {
                feature: self.contract.members[0].clone(),
                value: json!(identity),
            },
            InitialValue {
                feature: self.contract.members[1].clone(),
                value: json!("#"),
            },
        ])
    }
}
struct Fixture {
    revalidations: Cell<usize>,
    schema: Schema,
    sources: Vec<DefinitionSource>,
    provider: Initializer,
}
impl Fixture {
    fn new() -> Self {
        let source = DefinitionSource::new(
            "memory:readonly.rcore",
            SourceKind::Rcore,
            r#"rcore 1; package readonly version "1";
metamodel "urn:readonly" prefix r {class Node {attr identity:EString [1,1] readonly; attr operator:EString [0,1]; attr note:EString [0,1]; attr unsupported:EString [0,1] readonly;}}"#,
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let schema = compile_structure(&bundle, &source.id).unwrap().schema;
        let contract = ClassInitializationContract {
            version: 1,
            schema_revision: schema.revision.clone(),
            class: "Node".into(),
            class_identity: schema.identity("Node"),
            provider: ProviderId {
                name: "test.composite-native-id".into(),
                version: "1".into(),
            },
            sources: vec![source.id.clone()],
            members: vec![
                schema
                    .effective_feature("Node", "identity")
                    .unwrap()
                    .identity,
                schema
                    .effective_feature("Node", "operator")
                    .unwrap()
                    .identity,
            ],
        };
        Self {
            revalidations: Cell::new(0),
            schema,
            sources: vec![source],
            provider: Initializer {
                contract,
                calls: Cell::new(0),
            },
        }
    }
    fn binding(&self) -> InitializationBinding<'_> {
        InitializationBinding {
            expected: &self.provider.contract,
            sources: &self.sources,
            provider: Some(&self.provider),
            conversions: None,
        }
    }
    fn set(&self) -> ResourceSet<FrozenStore> {
        let mut store = FrozenStore::for_schema(&self.schema).unwrap();
        let mut caller = QueryBudget::new(100_000);
        let context = InitializationContext {
            schema: &self.schema,
            expected: &self.provider.contract,
            sources: &self.sources,
            provider: Some(&self.provider),
            conversions: None,
        };
        for (id, resource) in [("a", "models/one.xmi"), ("b", "models/two.xmi")] {
            context
                .create(
                    &mut store,
                    Construction {
                        id,
                        resource,
                        authored: &[
                            ("operator".into(), json!("+")),
                            ("note".into(), json!("authored")),
                        ],
                    },
                    &mut caller,
                )
                .unwrap();
        }
        ResourceSet::from_store(
            self.schema.clone(),
            ResourceContext::new(&self.schema, BTreeMap::new()).unwrap(),
            store,
        )
        .unwrap()
    }
}
impl SnapshotInitializationSelector for Fixture {
    fn select<'a>(
        &'a self,
        store: &dyn ModelRead,
        schema: &Schema,
        object: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<InitializationBinding<'a>>> {
        budget.charge()?;
        if schema.revision != self.schema.revision {
            return Err("Snapshot schema mismatch".into());
        }
        store.inspect(object, &mut |row| {
            if row.initialization().is_some() {
                return Err("Imported receipt reached fresh selector".into());
            }
            Ok(())
        })?;
        Ok(Some(self.binding()))
    }
    fn revalidate<'a>(
        &'a self,
        store: &dyn ModelRead,
        schema: &Schema,
        object: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<InitializationBinding<'a>>> {
        budget.charge()?;
        self.revalidations.set(self.revalidations.get() + 1);
        if schema.revision != self.schema.revision
            || store.class(object)?.as_deref() != Some("Node")
        {
            return Err("Current snapshot binding mismatch".into());
        }
        Ok(Some(self.binding()))
    }
}
struct Raw;
impl StructuralValueSelector for Raw {
    fn select(
        &self,
        _: &dyn ModelRead,
        _: &Schema,
        _: StructuralValueRequest<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<Value>> {
        budget.charge()?;
        Ok(None)
    }
}
fn registry() -> FormatRegistry {
    let mut registry = FormatRegistry::default();
    registry.register(XmiFormat).unwrap();
    registry
}
fn semantic(mut model: Model) -> Model {
    // Global iteration across resources is not persistent root/member order.
    model.elements.sort_by(|left, right| left.id.cmp(&right.id));
    for record in model.resources.values_mut() {
        record.content_revision = 0;
    }
    model
}
fn alter(bytes: &[Resource], change: impl FnOnce(String) -> String) -> Vec<Resource> {
    vec![Resource {
        uri: bytes[0].uri.clone(),
        bytes: change(String::from_utf8(bytes[0].bytes.clone()).unwrap()).into_bytes(),
    }]
}
#[test]
fn readonly_id_and_receipt_restore_at_true_fresh_state_without_touching_retained_rows() {
    let fixture = Fixture::new();
    let mut set = fixture.set();
    let context = set.context().clone();
    let before = set.snapshot().unwrap();
    let selected = vec!["models/one.xmi".into()];
    let registry = registry();
    let control = InvocationContext::default();
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&Raw, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&fixture),
        ..Default::default()
    };
    let ticket = set
        .prepare_save_with_format_context(
            &registry,
            "xmi",
            &selected,
            ReferenceClosure::SelectedOnly,
            &formats,
        )
        .unwrap();
    let bytes = ticket.resources().to_vec();
    fixture.provider.calls.set(0);
    let b = set.reader().handle("b").unwrap();
    set.acknowledge_save_with_format_context(&registry, &ticket, &formats)
        .unwrap();
    set.unload_with_format_context(
        &registry,
        &selected,
        UnloadPolicy::RequireSaved,
        &formats,
        &control,
    )
    .unwrap();
    let prepared = set
        .prepare_reload_with_format_context(
            &registry, "xmi", &selected, &bytes, &context, &formats, &control,
        )
        .unwrap();
    assert_eq!(
        fixture.provider.calls.get(),
        1,
        "retained rows or readonly values were reinitialized"
    );
    set.commit_reload_with_format_context(prepared, &formats, |_| Ok(()), &control)
        .unwrap();
    assert_eq!(fixture.provider.calls.get(), 1);
    assert_eq!(fixture.revalidations.get(), 1);
    assert_eq!(semantic(set.snapshot().unwrap()), semantic(before));
    assert_eq!(handle_id(set.reader(), &b).unwrap(), "b");
    assert_eq!(
        set.reader().value("a", "identity").unwrap(),
        Some(json!("models/one.xmi#a"))
    );
    assert_eq!(
        set.reader().value("a", "operator").unwrap(),
        Some(json!("+"))
    );
    assert!(
        set.transaction_with_format_context(
            &formats,
            |edit| edit.set("a", "identity", json!("changed")),
            |_| Ok(())
        )
        .is_err()
    );
}
#[test]
fn repeated_class_import_initializes_each_readonly_row_at_its_own_fresh_state() {
    let fixture = Fixture::new();
    let mut set = fixture.set();
    let before = set.snapshot().unwrap();
    let registry = registry();
    let control = InvocationContext::default();
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&Raw, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&fixture),
        ..Default::default()
    };
    let bytes = registry
        .export_with_format_context("xmi", &fixture.schema, &before, None, &formats)
        .unwrap();
    assert_eq!(bytes.len(), 2);
    fixture.provider.calls.set(0);
    let decoded = registry
        .import_with_format_context("xmi", &fixture.schema, &bytes, None, &formats)
        .unwrap();
    assert_eq!(fixture.provider.calls.get(), 2);
    assert_eq!(semantic(decoded), semantic(before));
    for id in ["a", "b"] {
        assert_eq!(
            set.reader().value(id, "identity").unwrap(),
            Some(json!(if id == "a" {
                "models/one.xmi#a"
            } else {
                "models/two.xmi#b"
            }))
        );
    }
}

#[test]
fn wrong_readonly_value_object_resource_source_or_missing_authority_never_publish() {
    let fixture = Fixture::new();
    let mut set = fixture.set();
    let before = set.snapshot().unwrap();
    let context = set.context().clone();
    let selected = vec!["models/one.xmi".into()];
    let registry = registry();
    let control = InvocationContext::default();
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&Raw, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&fixture),
        ..Default::default()
    };
    let ticket = set
        .prepare_save_with_format_context(
            &registry,
            "xmi",
            &selected,
            ReferenceClosure::SelectedOnly,
            &formats,
        )
        .unwrap();
    let bytes = ticket.resources();
    let no_initializer = FormatContext {
        structural_values: Some(authority.borrowed()),
        ..Default::default()
    };
    assert!(
        set.prepare_reload_with_format_context(
            &registry,
            "xmi",
            &selected,
            bytes,
            &context,
            &no_initializer,
            &control
        )
        .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), before);
    for corrupt in [
        alter(bytes, |xml| {
            xml.replace("identity=\"models/one.xmi#a\"", "identity=\"forged\"")
        }),
        alter(bytes, |xml| {
            xml.replace("id=\"a\"", "id=\"forged-object\"").replace(
                "&quot;object&quot;:&quot;a&quot;",
                "&quot;object&quot;:&quot;forged-object&quot;",
            )
        }),
        alter(bytes, |xml| {
            xml.replace("models/one.xmi", "models/elsewhere.xmi")
        }),
    ] {
        assert_ne!(corrupt, bytes);
        assert!(
            set.import_with_format_context(
                &registry,
                "xmi",
                &selected,
                &corrupt,
                &context,
                &formats,
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(set.snapshot().unwrap(), before);
    }
    let mut stale = Fixture::new();
    stale.sources[0] = DefinitionSource::new(
        "memory:readonly.rcore",
        SourceKind::Rcore,
        format!("{}\n//changed", stale.sources[0].text),
    );
    let stale_formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&stale),
        ..Default::default()
    };
    assert!(
        set.prepare_reload_with_format_context(
            &registry,
            "xmi",
            &selected,
            bytes,
            &context,
            &stale_formats,
            &control
        )
        .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), before);
}
#[test]
fn unsupported_readonly_member_and_untrusted_receipt_cannot_authorize_restore() {
    let fixture = Fixture::new();
    let mut set = fixture.set();
    let registry = registry();
    let control = InvocationContext::default();
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&Raw, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&fixture),
        ..Default::default()
    };
    let mut model = set.snapshot().unwrap();
    model
        .elements
        .iter_mut()
        .find(|e| e.id == "a")
        .unwrap()
        .values
        .insert("unsupported".into(), json!("not-in-contract"));
    let bytes = registry
        .export_with_format_context(
            "xmi",
            &fixture.schema,
            &model,
            Some((&["models/one.xmi".into()], ReferenceClosure::SelectedOnly)),
            &formats,
        )
        .unwrap();
    let retained = Model {
        schema_revision: fixture.schema.revision.clone(),
        resources: model
            .resources
            .iter()
            .filter(|(uri, _)| uri.as_str() != "models/one.xmi")
            .map(|(uri, row)| (uri.clone(), row.clone()))
            .collect(),
        elements: model
            .elements
            .into_iter()
            .filter(|row| row.resource != "models/one.xmi")
            .collect(),
    };
    let error = registry
        .import_with_format_context("xmi", &fixture.schema, &bytes, Some(&retained), &formats)
        .unwrap_err();
    assert!(
        error.contains("not declared by current initializer"),
        "{error}"
    );
    let ticket = set
        .prepare_save_with_format_context(
            &registry,
            "xmi",
            &["models/one.xmi".into()],
            ReferenceClosure::SelectedOnly,
            &formats,
        )
        .unwrap();
    let forged = alter(ticket.resources(), |xml| {
        xml.replace("test.composite-native-id", "forged-provider")
    });
    assert_ne!(forged, ticket.resources());
    let error = registry
        .import_with_format_context("xmi", &fixture.schema, &forged, Some(&retained), &formats)
        .unwrap_err();
    assert!(error.contains("receipt mismatch"), "{error}");
}

#[test]
fn prepared_decode_proof_requires_current_authority_and_rolls_back_late_revalidation_failure() {
    let fixture = Fixture::new();
    let mut set = fixture.set();
    let context = set.context().clone();
    let selected = vec!["models/one.xmi".into()];
    let registry = registry();
    let control = InvocationContext::default();
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&Raw, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&fixture),
        ..Default::default()
    };
    let before = set.snapshot().unwrap();
    // Serializable receipts and a valid raw model cannot mint fresh decode proof.
    let error = set
        .replace_with_format_context(
            &["models/one.xmi".into(), "models/two.xmi".into()],
            before.clone(),
            &context,
            &formats,
            |_| Ok(()),
        )
        .unwrap_err();
    assert!(
        error.contains("opaque checked fresh decode proof"),
        "{error}"
    );
    assert_eq!(set.snapshot().unwrap(), before);
    let ticket = set
        .prepare_save_with_format_context(
            &registry,
            "xmi",
            &selected,
            ReferenceClosure::SelectedOnly,
            &formats,
        )
        .unwrap();
    let bytes = ticket.resources().to_vec();
    set.unload_with_format_context(
        &registry,
        &selected,
        UnloadPolicy::DiscardChanges,
        &formats,
        &control,
    )
    .unwrap();
    let unloaded = set.snapshot().unwrap();
    let retained = set.reader().handle("b").unwrap();
    let prepared = set
        .prepare_reload_with_format_context(
            &registry, "xmi", &selected, &bytes, &context, &formats, &control,
        )
        .unwrap();
    let no_initializer = FormatContext {
        structural_values: Some(authority.borrowed()),
        ..Default::default()
    };
    let error = set
        .commit_reload_with_format_context(prepared, &no_initializer, |_| Ok(()), &control)
        .unwrap_err();
    assert!(error.contains("current initializer selection"), "{error}");
    assert_eq!(set.snapshot().unwrap(), unloaded);
    let prepared = set
        .prepare_reload_with_format_context(
            &registry, "xmi", &selected, &bytes, &context, &formats, &control,
        )
        .unwrap();
    let mut stale = Fixture::new();
    stale.sources[0] = DefinitionSource::new(
        "memory:readonly.rcore",
        SourceKind::Rcore,
        format!("{}\n//changed", stale.sources[0].text),
    );
    let wrong = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&stale),
        ..Default::default()
    };
    assert!(
        set.commit_reload_with_format_context(prepared, &wrong, |_| Ok(()), &control)
            .is_err()
    );
    assert_eq!(
        stale.revalidations.get(),
        1,
        "failure must reach current binding after candidate insertion"
    );
    assert_eq!(set.snapshot().unwrap(), unloaded);
    assert_eq!(handle_id(set.reader(), &retained).unwrap(), "b");
    let prepared = set
        .prepare_reload_with_format_context(
            &registry, "xmi", &selected, &bytes, &context, &formats, &control,
        )
        .unwrap();
    let mut drift = Fixture::new();
    drift.provider.contract.provider.version = "different".into();
    let wrong = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&drift),
        ..Default::default()
    };
    assert!(
        set.commit_reload_with_format_context(prepared, &wrong, |_| Ok(()), &control)
            .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), unloaded);
}
#[test]
fn compatibility_codec_that_discards_opaque_proof_cannot_publish_readonly_snapshot() {
    use intersect_model::core::interchange::ModelFormat;
    struct Plain;
    impl ModelFormat for Plain {
        fn id(&self) -> &str {
            "plain-xmi"
        }
        fn extensions(&self) -> &[&str] {
            &["pxmi"]
        }
        fn media_type(&self) -> &str {
            "application/xml"
        }
        fn decode(&self, schema: &Schema, resources: &[Resource]) -> Result<Model, String> {
            XmiFormat.decode(schema, resources)
        }
        fn encode(&self, schema: &Schema, model: &Model) -> Result<Vec<Resource>, String> {
            XmiFormat.encode(schema, model)
        }
        fn decode_with_format_context(
            &self,
            schema: &Schema,
            resources: &[Resource],
            retained: Option<&Model>,
            context: &FormatContext<'_>,
        ) -> Result<Model, String> {
            // It obtains correctly checked data, but deliberately returns only
            // Model. The default new trait method cannot fabricate proof.
            XmiFormat.decode_with_format_context(schema, resources, retained, context)
        }
    }
    let fixture = Fixture::new();
    let mut set = fixture.set();
    let context = set.context().clone();
    let before = set.snapshot().unwrap();
    let mut registry = registry();
    registry.register(Plain).unwrap();
    let selected = vec!["models/one.xmi".into()];
    let control = InvocationContext::default();
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&Raw, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        snapshot_initializers: Some(&fixture),
        ..Default::default()
    };
    let ticket = set
        .prepare_save_with_format_context(
            &registry,
            "xmi",
            &selected,
            ReferenceClosure::SelectedOnly,
            &formats,
        )
        .unwrap();
    let error = set
        .import_with_format_context(
            &registry,
            "plain-xmi",
            &selected,
            ticket.resources(),
            &context,
            &formats,
            |_| Ok(()),
        )
        .unwrap_err();
    assert!(
        error.contains("opaque checked fresh decode proof"),
        "{error}"
    );
    assert_eq!(set.snapshot().unwrap(), before);
}
