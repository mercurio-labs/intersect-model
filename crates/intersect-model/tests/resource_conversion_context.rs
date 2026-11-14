#![cfg(feature = "xmi")]
//! Provider-aware ResourceSet admission and lifecycle on one canonical store.
use crate::handle_id;
use intersect_model::{
    conversion::RgbConverter,
    core::{
        api::{BasicModel, ModelEdit, TransactionalModel},
        frozen::FrozenStore,
        interchange::{FormatContext, FormatRegistry, ReferenceClosure},
        model::Model,
        resource_set::{
            ResourceChangeKind, ResourceContext, ResourceSet, TargetResolution, UnloadPolicy,
        },
        storage::{InvocationContext, ModelRead},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::import_datatype_bundle,
        conversion::{ConversionContract, ConversionRegistry, InvocationObserver, ProviderId},
        datatypes::Datatype,
        rcore::compile_structure_with_conversions,
        resources::ResourceSet as DefinitionResources,
        schema::Schema,
    },
    xmi::XmiFormat,
};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};

struct Fixture {
    schema: Schema,
    datatype: Datatype,
    conversions: ConversionRegistry<'static>,
}
impl Fixture {
    fn new() -> Self {
        let source = DefinitionSource::new(
            "memory:resources.rcore",
            SourceKind::Rcore,
            r##"rcore 1; package colors version "1";
metamodel "urn:colors" prefix c {
    datatype RGB instanceClass "example.RGB" {}
    class Item {
        attr color: RGB [0,1] default "#010203";
        attr palette: RGB [0,*];
        attr note: EString [0,1];
        ref peer: Item [0,1];
    }
}"##,
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let id = DefinitionResources::new(&bundle)
            .unwrap()
            .resolve(&source.id, "#//RGB")
            .unwrap();
        let datatype = import_datatype_bundle(&bundle, &id).unwrap();
        let mut conversions = ConversionRegistry::default();
        conversions
            .register_shared(&datatype, Rc::new(Self::provider(&datatype, "1")))
            .unwrap();
        let schema = compile_structure_with_conversions(&bundle, &source.id, None, &conversions)
            .unwrap()
            .schema;
        Self {
            schema,
            datatype,
            conversions,
        }
    }
    fn provider(datatype: &Datatype, version: &str) -> RgbConverter {
        RgbConverter::new(ConversionContract {
            datatype: datatype.declaration().clone(),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: version.into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
    }
    fn stale(&self) -> ConversionRegistry<'static> {
        let mut registry = ConversionRegistry::default();
        registry
            .register_shared(&self.datatype, Rc::new(Self::provider(&self.datatype, "2")))
            .unwrap();
        registry
    }
    fn value(&self, text: &str) -> Value {
        serde_json::to_value(
            self.conversions
                .parse(self.datatype.declaration(), text)
                .unwrap(),
        )
        .unwrap()
    }
    fn context(&self) -> ResourceContext {
        ResourceContext::new(
            &self.schema,
            BTreeMap::from([("colors".into(), "exact-1".into())]),
        )
        .unwrap()
    }
    fn store(&self) -> FrozenStore {
        let basic =
            BasicModel::new_with_conversions(self.schema.clone(), &self.conversions).unwrap();
        let mut model = TransactionalModel::new(basic);
        model
            .transaction_with_conversions(&self.conversions, |edit| {
                edit.create("a", "Item", "models/a.xmi")?;
                edit.create("b", "Item", "models/b.xmi")?;
                edit.set("a", "color", self.value("#112233"))?;
                edit.set("b", "peer", json!("a"))
            })
            .unwrap();
        model.into_basic().into_store()
    }
    fn set(&self) -> ResourceSet<FrozenStore> {
        ResourceSet::from_store_with_validation(
            self.schema.clone(),
            self.context(),
            self.store(),
            Some(&self.conversions),
            |_| Ok(()),
        )
        .unwrap()
    }
}
fn formats() -> FormatRegistry {
    let mut registry = FormatRegistry::default();
    registry.register(XmiFormat).unwrap();
    registry
}
fn semantic(mut model: Model) -> Model {
    // Cross-resource store iteration order is not persistence semantics. Keep
    // each resource's explicit root order and every object's authored metadata.
    model.elements.sort_by(|left, right| left.id.cmp(&right.id));
    for record in model.resources.values_mut() {
        record.content_revision = 0;
    }
    model
}

#[test]
fn admission_preserves_store_handles_and_checks_providers_before_language() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let handle = store.handle("a").unwrap();
    let called = Cell::new(false);
    let set = ResourceSet::from_store_with_validation(
        fixture.schema.clone(),
        fixture.context(),
        store,
        Some(&fixture.conversions),
        |actual| {
            assert_eq!(handle_id(actual, &handle)?, "a");
            called.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert!(called.get());
    assert_eq!(handle_id(set.reader(), &handle).unwrap(), "a");
    assert!(set.validate().is_err());
    set.validate_with_conversions(&fixture.conversions).unwrap();
    let stale = fixture.stale();
    for providers in [None, Some(&ConversionRegistry::default()), Some(&stale)] {
        called.set(false);
        assert!(
            ResourceSet::from_store_with_validation(
                fixture.schema.clone(),
                fixture.context(),
                fixture.store(),
                providers,
                |_| {
                    called.set(true);
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!called.get());
    }
    let mut incompatible = fixture.schema.clone();
    incompatible.revision.push_str("-other");
    called.set(false);
    assert!(
        ResourceSet::from_store_with_validation(
            fixture.schema.clone(),
            ResourceContext::new(&incompatible, BTreeMap::new()).unwrap(),
            fixture.store(),
            Some(&fixture.conversions),
            |_| {
                called.set(true);
                Ok(())
            }
        )
        .is_err()
    );
    assert!(!called.get());
    assert!(
        ResourceSet::from_store_with_validation(
            fixture.schema.clone(),
            fixture.context(),
            fixture.store(),
            Some(&fixture.conversions),
            |_| Err("language rejected admission".into())
        )
        .is_err()
    );
}

#[test]
fn converted_edits_and_replacement_publish_atomically() {
    let fixture = Fixture::new();
    let mut set = fixture.set();
    let a_handle = set.reader().handle("a").unwrap();
    let b_handle = set.reader().handle("b").unwrap();
    let original = set.snapshot().unwrap();
    let stale = fixture.stale();
    for providers in [&ConversionRegistry::default(), &stale] {
        let checked = Cell::new(false);
        assert!(
            set.transaction_with_conversions_and_validation(
                providers,
                |edit| edit.set("a", "note", json!("rejected")),
                |_| {
                    checked.set(true);
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!checked.get());
        assert_eq!(set.snapshot().unwrap(), original);
        assert!(set.drain_changes().is_empty());
    }
    assert!(
        set.transaction(|edit| edit.set("a", "note", json!("missing providers")))
            .is_err()
    );
    assert!(
        set.transaction_with_conversions_and_validation(
            &fixture.conversions,
            |edit| edit.set("a", "note", json!("rejected")),
            |_| Err("language rejected edit".into())
        )
        .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), original);
    assert!(set.drain_changes().is_empty());
    set.transaction_with_conversions(&fixture.conversions, |edit| {
        edit.set("a", "color", fixture.value("#AABBCC"))
    })
    .unwrap();
    assert_eq!(handle_id(set.reader(), &a_handle).unwrap(), "a");
    assert_eq!(set.drain_changes()[0].kind, ResourceChangeKind::Edited);
    let committed = set.snapshot().unwrap();
    let mut incoming = committed.clone();
    incoming
        .elements
        .retain(|element| element.resource == "models/a.xmi");
    incoming.resources.retain(|uri, _| uri == "models/a.xmi");
    incoming.elements[0]
        .values
        .insert("color".into(), fixture.value("#445566"));
    let selected = vec!["models/a.xmi".into()];
    assert!(
        set.replace_with_conversions(
            &selected,
            incoming.clone(),
            &fixture.context(),
            &stale,
            |_| Ok(())
        )
        .is_err()
    );
    assert!(
        set.replace_with_conversions(
            &selected,
            incoming.clone(),
            &fixture.context(),
            &fixture.conversions,
            |_| Err("language rejected replacement".into())
        )
        .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), committed);
    assert!(set.drain_changes().is_empty());
    set.replace_with_conversions(
        &selected,
        incoming,
        &fixture.context(),
        &fixture.conversions,
        |_| Ok(()),
    )
    .unwrap();
    assert!(handle_id(set.reader(), &a_handle).is_err());
    assert_eq!(handle_id(set.reader(), &b_handle).unwrap(), "b");
    assert_eq!(
        set.reader().get("a").unwrap().unwrap().values["color"],
        fixture.value("#445566")
    );
    set.validate_with_conversions(&fixture.conversions).unwrap();
    assert_eq!(set.drain_changes()[0].kind, ResourceChangeKind::Replaced);
}

struct Cancel(Rc<Cell<bool>>);
impl InvocationObserver for Cancel {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        self.checkpoint()
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.0.get() {
            Err("cancelled".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn provider_aware_save_unload_reload_preserves_external_ids_and_rolls_back() {
    let fixture = Fixture::new();
    let formats = formats();
    let context = fixture.context();
    let conversions = FormatContext {
        conversions: Some(&fixture.conversions),
        ..Default::default()
    };
    let stale = fixture.stale();
    let wrong = FormatContext {
        conversions: Some(&stale),
        ..Default::default()
    };
    let mut set = fixture.set();
    set.bind_fragment("models/a.xmi", "item", "a").unwrap();
    let a_handle = set.reader().handle("a").unwrap();
    let b_handle = set.reader().handle("b").unwrap();
    let original = set.snapshot().unwrap();
    let all = vec!["models/a.xmi".into(), "models/b.xmi".into()];
    let selected = vec!["models/a.xmi".into()];
    assert!(
        set.prepare_save(&formats, "xmi", &all, ReferenceClosure::SelectedOnly)
            .is_err()
    );
    assert!(
        set.prepare_save_with_format_context(
            &formats,
            "xmi",
            &all,
            ReferenceClosure::SelectedOnly,
            &wrong
        )
        .is_err()
    );
    let ticket = set
        .prepare_save_with_format_context(
            &formats,
            "xmi",
            &all,
            ReferenceClosure::SelectedOnly,
            &conversions,
        )
        .unwrap();
    let bytes: Vec<_> = ticket
        .resources()
        .iter()
        .filter(|resource| resource.uri == "models/a.xmi")
        .cloned()
        .collect();
    assert!(set.acknowledge_save(&formats, &ticket).is_err());
    assert!(
        set.acknowledge_save_with_format_context(&formats, &ticket, &wrong)
            .is_err()
    );
    set.acknowledge_save_with_format_context(&formats, &ticket, &conversions)
        .unwrap();
    assert!(
        !set.is_dirty_with_format_context(&formats, "models/a.xmi", &conversions)
            .unwrap()
    );
    assert!(
        set.unload_with_format_context(
            &formats,
            &selected,
            UnloadPolicy::RequireSaved,
            &wrong,
            &InvocationContext::default()
        )
        .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), original);
    assert!(set.drain_changes().is_empty());
    set.unload_with_format_context(
        &formats,
        &selected,
        UnloadPolicy::RequireSaved,
        &conversions,
        &InvocationContext::default(),
    )
    .unwrap();
    assert!(handle_id(set.reader(), &a_handle).is_err());
    assert_eq!(handle_id(set.reader(), &b_handle).unwrap(), "b");
    assert_eq!(
        set.reader().get("b").unwrap().unwrap().values["peer"],
        json!("a")
    );
    assert!(matches!(
        set.resolve("models/b.xmi", "a.xmi#item").unwrap(),
        TargetResolution::UnavailableResource { .. }
    ));
    assert!(set.validate_with_conversions(&fixture.conversions).is_err());
    let suspended = set.snapshot().unwrap();
    assert_eq!(set.drain_changes()[0].kind, ResourceChangeKind::Unloaded);
    assert!(
        set.prepare_reload_with_format_context(
            &formats,
            "xmi",
            &selected,
            &bytes,
            &context,
            &wrong,
            &InvocationContext::default()
        )
        .is_err()
    );
    let prepare = |set: &mut ResourceSet<FrozenStore>| {
        set.prepare_reload_with_format_context(
            &formats,
            "xmi",
            &selected,
            &bytes,
            &context,
            &conversions,
            &InvocationContext::default(),
        )
        .unwrap()
    };
    let checked = Cell::new(false);
    let prepared = prepare(&mut set);
    assert!(
        set.commit_reload_with_conversions(
            prepared,
            Some(&stale),
            |_| {
                checked.set(true);
                Ok(())
            },
            &InvocationContext::default()
        )
        .is_err()
    );
    assert!(!checked.get());
    let prepared = prepare(&mut set);
    assert!(
        set.commit_reload_with_conversions(
            prepared,
            Some(&fixture.conversions),
            |_| Err("language rejected reload".into()),
            &InvocationContext::default()
        )
        .is_err()
    );
    let cancelled = Rc::new(Cell::new(false));
    let observation = InvocationContext::new(Rc::new(Cancel(cancelled.clone())));
    let prepared = prepare(&mut set);
    assert!(
        set.commit_reload_with_conversions(
            prepared,
            Some(&fixture.conversions),
            |_| {
                cancelled.set(true);
                Ok(())
            },
            &observation
        )
        .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), suspended);
    assert!(set.drain_changes().is_empty());
    let stale_prepared = prepare(&mut set);
    let prepared = prepare(&mut set);
    set.commit_reload_with_conversions(
        prepared,
        Some(&fixture.conversions),
        |_| Ok(()),
        &InvocationContext::default(),
    )
    .unwrap();
    assert!(handle_id(set.reader(), &a_handle).is_err());
    assert_eq!(handle_id(set.reader(), &b_handle).unwrap(), "b");
    assert!(
        matches!(set.resolve("models/b.xmi", "a.xmi#item").unwrap(), TargetResolution::Resolved { element, .. } if element == "a")
    );
    assert_eq!(semantic(set.snapshot().unwrap()), semantic(original));
    set.validate_with_conversions(&fixture.conversions).unwrap();
    assert_eq!(set.drain_changes()[0].kind, ResourceChangeKind::Reloaded);
    let reloaded = set.snapshot().unwrap();
    assert!(
        set.commit_reload_with_conversions(
            stale_prepared,
            Some(&fixture.conversions),
            |_| Ok(()),
            &InvocationContext::default()
        )
        .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), reloaded);
    assert!(set.drain_changes().is_empty());
}
