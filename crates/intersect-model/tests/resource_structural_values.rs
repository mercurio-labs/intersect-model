//! Explicit delegated structural values use the original shape/target workers.
use crate::handle_id;
use intersect_model::{
    core::{
        frozen::FrozenStore,
        model::Model,
        query::QueryBudget,
        resource_set::{ResourceContext, ResourceSet},
        storage::{
            self, InvocationContext, ModelRead, ModelStore, StorageResult, StructuralValueRequest,
            StructuralValueSelector,
        },
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore::compile_structure,
        schema::{MemberIdentity, Schema},
    },
};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};

fn schema() -> Schema {
    let source = DefinitionSource::new(
        "memory:delegates.rcore",
        SourceKind::Rcore,
        r#"
rcore 1; package delegates version "1";
metamodel "urn:delegates" prefix d {
    abstract class Base {}
    class Target extends Base {}
    class Other extends Base { attr note: EString [0,1]; }
    class Holder {
        ref backing: Base [0,1];
        ref member: Target [1,1];
    }
}"#,
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    compile_structure(&bundle, &source.id).unwrap().schema
}
fn model(schema: &Schema, shadow: Option<Value>) -> Model {
    let mut model = Model::default();
    model
        .create(schema, "target", "Target", "models/a.xmi", 0, 0)
        .unwrap();
    model
        .create(schema, "other", "Other", "models/a.xmi", 10, 15)
        .unwrap();
    model
        .create(schema, "holder", "Holder", "models/a.xmi", 20, 31)
        .unwrap();
    let holder = model
        .elements
        .iter_mut()
        .find(|e| e.id == "holder")
        .unwrap();
    // Authored physical absence is the fixture's delegated storage contract.
    holder.values.remove("member");
    holder.values.insert("backing".into(), json!("target"));
    if let Some(shadow) = shadow {
        holder.values.insert("member".into(), shadow);
    }
    model
}

fn store(schema: &Schema, shadow: Option<Value>) -> FrozenStore {
    FrozenStore::from_materialized(schema, &model(schema, shadow)).unwrap()
}
#[derive(Clone, Copy)]
enum ResultKind {
    Backing,
    Null,
    WrongTarget,
    None,
}
struct Values(ResultKind);
impl StructuralValueSelector for Values {
    fn select(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        request: StructuralValueRequest<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<Value>> {
        budget.charge()?;
        if request.actual != "Holder"
            || request.owner != "Holder"
            || request.feature.name != "member"
        {
            return Ok(None);
        }
        if !matches!(request.identity, MemberIdentity::Source { .. })
            || request.feature != &schema.features("Holder")?["member"]
        {
            return Err("Unexpected source member".into());
        }
        Ok(match self.0 {
            ResultKind::Backing => store.value(request.object, "backing")?,
            ResultKind::Null => Some(Value::Null),
            ResultKind::WrongTarget => Some(json!("other")),
            ResultKind::None => None,
        })
    }
}
fn audit(
    store: &dyn ModelRead,
    schema: &Schema,
    values: &Values,
    budget: &mut QueryBudget,
) -> StorageResult<()> {
    storage::validate_with_structural_values(
        store,
        schema,
        None,
        values,
        budget,
        &InvocationContext::default(),
    )
}
#[test]
fn admission_keeps_same_store_handles_and_raw_absence() {
    let schema = schema();
    let store = store(&schema, None);
    let handle = store.handle("holder").unwrap();
    let context = ResourceContext::new(&schema, BTreeMap::new()).unwrap();
    let called = Cell::new(false);
    let mut budget = QueryBudget::new(100_000);
    let set = ResourceSet::from_store_with_structural_values(
        schema,
        context,
        store,
        None,
        &Values(ResultKind::Backing),
        &mut budget,
        &InvocationContext::default(),
        |same, caller| {
            assert_eq!(handle_id(same, &handle)?, "holder");
            assert!(same.value("holder", "member")?.is_none());
            caller.charge()?;
            called.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert!(called.get());
    assert!(budget.reads > 1);
    assert_eq!(handle_id(set.reader(), &handle).unwrap(), "holder");
    assert!(set.reader().value("holder", "member").unwrap().is_none());
    assert!(set.validate().is_err());
    set.validate_with_structural_values(
        None,
        &Values(ResultKind::Backing),
        &mut budget,
        &InvocationContext::default(),
    )
    .unwrap();
}
#[test]
fn original_requiredness_and_reference_conformance_reject_provider_results() {
    let schema = schema();
    let store = store(&schema, None);
    for result in [ResultKind::Null, ResultKind::None] {
        let error = audit(
            &store,
            &schema,
            &Values(result),
            &mut QueryBudget::new(100_000),
        )
        .unwrap_err();
        assert!(error.contains("Multiplicity violation"), "{error}");
    }
    let error = audit(
        &store,
        &schema,
        &Values(ResultKind::WrongTarget),
        &mut QueryBudget::new(100_000),
    )
    .unwrap_err();
    assert!(
        error.contains("Reference type mismatch Holder.member"),
        "{error}"
    );
}
#[test]
fn delegated_values_reject_raw_shadows_including_null() {
    let schema = schema();
    for shadow in [Value::Null, json!("target")] {
        let store = store(&schema, Some(shadow));
        let error = audit(
            &store,
            &schema,
            &Values(ResultKind::Backing),
            &mut QueryBudget::new(100_000),
        )
        .unwrap_err();
        assert!(error.contains("shadow a raw stored member"), "{error}");
    }
}
#[test]
fn failed_delegated_validation_rolls_back_same_physical_checkpoint() {
    let schema = schema();
    let mut store = store(&schema, None);
    let handle = store.handle("holder").unwrap();
    let before = storage::export(&store).unwrap();
    let mut budget = QueryBudget::new(100_000);
    audit(&store, &schema, &Values(ResultKind::Backing), &mut budget).unwrap();
    let error = storage::atomic(&mut store, |same| {
        same.set_value("holder", "backing", Some(json!("other")))?;
        audit(same, &schema, &Values(ResultKind::Backing), &mut budget)
    })
    .unwrap_err();
    assert!(
        error.contains("Reference type mismatch Holder.member"),
        "{error}"
    );
    assert_eq!(storage::export(&store).unwrap(), before);
    assert_eq!(handle_id(&store, &handle).unwrap(), "holder");
    assert!(budget.reads > 1);
    audit(&store, &schema, &Values(ResultKind::Backing), &mut budget).unwrap();
}

#[cfg(feature = "xmi")]
mod lifecycle {
    use super::*;
    use intersect_model::{
        core::{
            api::ModelEdit,
            interchange::{FormatContext, FormatRegistry, ReferenceClosure, Resource},
            resource_set::UnloadPolicy,
            storage::StructuralValueContext,
        },
        xmi::XmiFormat,
    };

    fn admitted(schema: &Schema, store: FrozenStore) -> ResourceSet<FrozenStore> {
        ResourceSet::from_store_with_structural_values(
            schema.clone(),
            ResourceContext::new(
                schema,
                BTreeMap::from([("language".into(), "exact1".into())]),
            )
            .unwrap(),
            store,
            None,
            &Values(ResultKind::Backing),
            &mut QueryBudget::new(100_000),
            &InvocationContext::default(),
            |_, _| Ok(()),
        )
        .unwrap()
    }
    fn registry() -> FormatRegistry {
        let mut r = FormatRegistry::default();
        r.register(XmiFormat).unwrap();
        r
    }
    fn semantic(mut model: Model) -> Model {
        for record in model.resources.values_mut() {
            record.content_revision = 0;
        }
        model
    }
    fn change_xml(bytes: &[Resource], change: impl FnOnce(String) -> String) -> Vec<Resource> {
        vec![Resource {
            uri: bytes[0].uri.clone(),
            bytes: change(String::from_utf8(bytes[0].bytes.clone()).unwrap()).into_bytes(),
        }]
    }
    #[test]
    fn selected_xmi_save_ack_unload_reload_preserves_absence_identity_and_spans() {
        let schema = schema();
        let mut set = admitted(&schema, store(&schema, None));
        let old_handle = set.reader().handle("holder").unwrap();
        let before = set.snapshot().unwrap();
        let resource_context = set.context().clone();
        let registry = registry();
        let selected = vec!["models/a.xmi".into()];
        let mut budget = QueryBudget::new(1_000_000);
        let control = InvocationContext::default();
        let values = Values(ResultKind::Backing);
        let authority = StructuralValueContext::new(&values, &mut budget, &control);
        let formats = FormatContext {
            structural_values: Some(authority.borrowed()),
            ..Default::default()
        };
        assert!(
            set.prepare_save(&registry, "xmi", &selected, ReferenceClosure::SelectedOnly)
                .is_err()
        );
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
        let xml = String::from_utf8(bytes[0].bytes.clone()).unwrap();
        assert!(xml.contains("absentStored="));
        assert!(!xml.contains("<member "));
        set.acknowledge_save_with_format_context(&registry, &ticket, &formats)
            .unwrap();
        assert!(
            !set.is_dirty_with_format_context(&registry, &selected[0], &formats)
                .unwrap()
        );
        set.unload_with_format_context(
            &registry,
            &selected,
            UnloadPolicy::RequireSaved,
            &formats,
            &control,
        )
        .unwrap();
        assert!(handle_id(set.reader(), &old_handle).is_err());
        let unloaded = set.snapshot().unwrap();
        // Prepared content carries no installed provider authority. Commit must
        // receive the same explicit services again and repeat full validation.
        let prepared = set
            .prepare_reload_with_format_context(
                &registry,
                "xmi",
                &selected,
                &bytes,
                &resource_context,
                &formats,
                &control,
            )
            .unwrap();
        assert!(set.commit_reload(prepared, |_| Ok(()), &control).is_err());
        assert_eq!(set.snapshot().unwrap(), unloaded);
        let prepared = set
            .prepare_reload_with_format_context(
                &registry,
                "xmi",
                &selected,
                &bytes,
                &resource_context,
                &formats,
                &control,
            )
            .unwrap();
        set.commit_reload_with_format_context(
            prepared,
            &formats,
            |candidate| {
                assert_eq!(
                    candidate
                        .elements
                        .iter()
                        .find(|e| e.id == "holder")
                        .unwrap()
                        .start,
                    20
                );
                Ok(())
            },
            &control,
        )
        .unwrap();
        assert_eq!(semantic(set.snapshot().unwrap()), semantic(before));
        assert!(handle_id(set.reader(), &old_handle).is_err());
        assert!(set.reader().value("holder", "member").unwrap().is_none());
        assert!(
            set.acknowledge_save_with_format_context(&registry, &ticket, &formats)
                .is_err()
        );
        authority.validate(set.reader(), &schema, None).unwrap();
        assert!(authority.with_budget(|caller| Ok(caller.reads)).unwrap() > 1);
    }
    #[test]
    fn transaction_replacement_and_wrong_context_fail_without_publishing() {
        let schema = schema();
        let mut set = admitted(&schema, store(&schema, None));
        let before = set.snapshot().unwrap();
        let old_handle = set.reader().handle("holder").unwrap();
        let context = set.context().clone();
        let selected = vec!["models/a.xmi".into()];
        let mut budget = QueryBudget::new(1_000_000);
        let control = InvocationContext::default();
        let values = Values(ResultKind::Backing);
        let authority = StructuralValueContext::new(&values, &mut budget, &control);
        let formats = FormatContext {
            structural_values: Some(authority.borrowed()),
            ..Default::default()
        };
        let error = set
            .transaction_with_format_context(
                &formats,
                |edit| edit.set("holder", "backing", json!("other")),
                |_| Ok(()),
            )
            .unwrap_err();
        assert!(
            error.contains("Reference type mismatch Holder.member"),
            "{error}"
        );
        assert_eq!(set.snapshot().unwrap(), before);
        assert_eq!(handle_id(set.reader(), &old_handle).unwrap(), "holder");
        let mut incoming = before.clone();
        incoming
            .elements
            .iter_mut()
            .find(|e| e.id == "holder")
            .unwrap()
            .values
            .insert("backing".into(), json!("other"));
        assert!(
            set.replace_with_format_context(&selected, incoming, &context, &formats, |_| Ok(()))
                .is_err()
        );
        assert_eq!(set.snapshot().unwrap(), before);
        let stale = ResourceContext::new(
            &schema,
            BTreeMap::from([("language".into(), "other".into())]),
        )
        .unwrap();
        assert!(
            set.replace_with_format_context(
                &selected,
                before.clone(),
                &stale,
                &formats,
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(set.snapshot().unwrap(), before);
        assert!(
            set.unload(&registry(), &selected, UnloadPolicy::DiscardChanges)
                .is_err()
        );
        assert_eq!(set.snapshot().unwrap(), before);
    }
    #[test]
    fn codec_absence_requires_exact_selector_and_rejects_malformed_or_conflicting_markers() {
        let schema = schema();
        let mut set = admitted(&schema, store(&schema, None));
        let registry = registry();
        let selected = vec!["models/a.xmi".into()];
        let mut budget = QueryBudget::new(1_000_000);
        let control = InvocationContext::default();
        let values = Values(ResultKind::Backing);
        let authority = StructuralValueContext::new(&values, &mut budget, &control);
        let formats = FormatContext {
            structural_values: Some(authority.borrowed()),
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
        assert!(
            registry
                .import("xmi", &schema, bytes)
                .unwrap_err()
                .contains("requires explicit structural-value providers")
        );
        for result in [ResultKind::None, ResultKind::WrongTarget] {
            let wrong = Values(result);
            let mut caller = QueryBudget::new(100_000);
            let wrong_authority = StructuralValueContext::new(&wrong, &mut caller, &control);
            let wrong_formats = FormatContext {
                structural_values: Some(wrong_authority.borrowed()),
                ..Default::default()
            };
            assert!(
                registry
                    .import_with_format_context("xmi", &schema, bytes, None, &wrong_formats)
                    .is_err()
            );
        }
        for replacement in [
            "[&quot;unknown&quot;]",
            "[&quot;member&quot;,&quot;member&quot;]",
        ] {
            let malformed = change_xml(bytes, |xml| {
                xml.replace("[&quot;member&quot;]", replacement)
            });
            assert_ne!(malformed, bytes);
            assert!(
                registry
                    .import_with_format_context("xmi", &schema, &malformed, None, &formats)
                    .is_err()
            );
        }
        let conflict = change_xml(bytes, |xml| {
            xml.replace("<m:Holder ", "<m:Holder member=\"anything\" ")
        });
        assert_ne!(conflict, bytes);
        let error = registry
            .import_with_format_context("xmi", &schema, &conflict, None, &formats)
            .unwrap_err();
        assert!(
            error.contains("conflicts with absent stored provenance"),
            "{error}"
        );
        let before = set.snapshot().unwrap();
        assert!(
            set.import_with_format_context(
                &registry,
                "xmi",
                &selected,
                &conflict,
                &set.context().clone(),
                &formats,
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(set.snapshot().unwrap(), before);
    }
    #[test]
    fn codec_rejects_unsupported_optional_absence_and_never_creates_second_budget() {
        let schema = schema();
        let mut model = storage::export(&store(&schema, None)).unwrap();
        model
            .elements
            .iter_mut()
            .find(|e| e.id == "other")
            .unwrap()
            .values
            .remove("note");
        let mut set = admitted(
            &schema,
            FrozenStore::from_materialized(&schema, &model).unwrap(),
        );
        let mut budget = QueryBudget::new(100_000);
        let control = InvocationContext::default();
        let values = Values(ResultKind::Backing);
        let authority = StructuralValueContext::new(&values, &mut budget, &control);
        // Generic admission keeps None-to-raw semantics for this optional field.
        authority.validate(set.reader(), &schema, None).unwrap();
        let formats = FormatContext {
            structural_values: Some(authority.borrowed()),
            ..Default::default()
        };
        let error = set
            .prepare_save_with_format_context(
                &registry(),
                "xmi",
                &["models/a.xmi".into()],
                ReferenceClosure::SelectedOnly,
                &formats,
            )
            .unwrap_err();
        assert!(
            error.contains("requires an explicit supported structural value"),
            "{error}"
        );
        let error = authority
            .with_budget(|_| authority.validate(set.reader(), &schema, None))
            .unwrap_err();
        assert!(error.contains("caller already borrowed"), "{error}");
    }
}

#[path = "resource_structural_values/immutable_audit.rs"]
mod immutable_audit;
