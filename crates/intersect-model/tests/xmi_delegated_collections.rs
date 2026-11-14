//! Generic source-qualified absent collections; no language provider or second codec.
#![cfg(feature = "xmi")]
use crate::handle_id;
use intersect_model::{
    core::{
        frozen::FrozenStore,
        interchange::{FormatContext, FormatRegistry, ReferenceClosure, Resource},
        model::Model,
        query::QueryBudget,
        resource_set::{ResourceContext, ResourceSet, UnloadPolicy},
        resources::SourceCorrespondence,
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceId, SourceKind},
        rcore::compile_structure,
        schema::{MemberIdentity, Schema},
    },
    storage::{
        InvocationContext, ModelRead, StructuralValueContext, StructuralValueRequest,
        StructuralValueSelector,
    },
    xmi::XmiFormat,
};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};

const SELECTED: &str = "models/selected.xmi";
const RETAINED: &str = "models/retained.xmi";

fn schema() -> Result<(Schema, SourceId), String> {
    let source = DefinitionSource::new(
        "fixture:delegated-collections.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package collections version "1";
metamodel "urn:delegated-collections" prefix c {
    class Target {}
    class Holder {
        ref backing: Target [1,*];
        ref members: Target [1,*];
        attr note: EString [0,1];
    }
}"#,
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let compiled = compile_structure(&bundle, &source.id).map_err(|error| format!("{error:?}"))?;
    Ok((compiled.schema, source.id))
}
fn model(schema: &Schema) -> Result<Model, String> {
    let mut model = Model::default();
    for (id, class, resource, start, end) in [
        ("target-a", "Target", SELECTED, 2, 8),
        ("target-b", "Target", SELECTED, 9, 15),
        ("holder", "Holder", SELECTED, 16, 29),
        ("retained", "Target", RETAINED, 30, 37),
    ] {
        model.create(schema, id, class, resource, start, end)?;
    }
    let holder = model
        .elements
        .iter_mut()
        .find(|row| row.id == "holder")
        .ok_or("missing holder")?;
    holder
        .values
        .insert("backing".into(), json!(["target-a", "target-b"]));
    holder.values.remove("members");
    for (uri, resource) in &mut model.resources {
        let source = SourceId::for_text(
            format!("fixture:{uri}"),
            "native canonical collection fixture",
        );
        resource.source = Some(SourceCorrespondence {
            uri: source.uri,
            revision: source.sha256,
        });
    }
    Ok(model)
}
#[derive(Clone, Copy)]
enum Selection {
    Backing,
    Decline,
    Empty,
}
struct Collections {
    source: SourceId,
    selection: Selection,
    calls: Cell<usize>,
}
impl Collections {
    fn new(source: &SourceId, selection: Selection) -> Self {
        Self {
            source: source.clone(),
            selection,
            calls: Cell::new(0),
        }
    }
}
impl StructuralValueSelector for Collections {
    fn select(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        request: StructuralValueRequest<'_>,
        caller: &mut QueryBudget,
    ) -> Result<Option<Value>, String> {
        caller.charge()?;
        if request.actual != "Holder" || request.feature.name != "members" {
            return Ok(None);
        }
        self.calls.set(self.calls.get() + 1);
        let member = schema.effective_feature("Holder", "members")?;
        if request.owner != member.owner
            || request.identity != &member.identity
            || request.feature != member.feature
        {
            return Err("collection request differs from actual schema".into());
        }
        if !matches!(request.identity, MemberIdentity::Source { declaration } if declaration.source == self.source)
        {
            return Err("delegated collection source mismatch".into());
        }
        // This generic fixture delegates only physical absence. Present values
        // must still pass the ORIGINAL raw shape/target validation.
        if store.value(request.object, "members")?.is_some() {
            return Ok(None);
        }
        match self.selection {
            Selection::Backing => store.value(request.object, "backing"),
            Selection::Decline => Ok(None),
            Selection::Empty => Ok(Some(json!([]))),
        }
    }
}
fn registry() -> Result<FormatRegistry, String> {
    let mut registry = FormatRegistry::default();
    registry.register(XmiFormat)?;
    Ok(registry)
}
fn admitted(schema: &Schema, source: &SourceId) -> Result<ResourceSet<FrozenStore>, String> {
    let store = FrozenStore::from_materialized(schema, &model(schema)?)?;
    let handle = store
        .handle("holder")
        .ok_or("missing original holder handle")?;
    let values = Collections::new(source, Selection::Backing);
    ResourceSet::from_store_with_structural_values(
        schema.clone(),
        ResourceContext::new(
            schema,
            BTreeMap::from([("selector-source".into(), source.sha256.clone())]),
        )?,
        store,
        None,
        &values,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
        |actual, _| {
            assert_eq!(handle_id(actual, &handle)?, "holder");
            assert_eq!(actual.value("holder", "members")?, None);
            Ok(())
        },
    )
}
fn normalized(mut model: Model) -> Model {
    model.elements.sort_by(|a, b| a.id.cmp(&b.id));
    for resource in model.resources.values_mut() {
        resource.content_revision = 0;
    }
    model
}

#[test]
fn selected_collection_snapshot_preserves_absence_source_handles_and_spans() -> Result<(), String> {
    let (schema, source) = schema()?;
    let mut set = admitted(&schema, &source)?;
    let before = set.snapshot()?;
    let context = set.context().clone();
    let old = set.reader().handle("holder").ok_or("missing holder")?;
    let retained = set.reader().handle("retained").ok_or("missing retained")?;
    let selected = vec![SELECTED.to_owned()];
    let registry = registry()?;
    assert!(
        set.prepare_save(&registry, "xmi", &selected, ReferenceClosure::SelectedOnly)
            .is_err()
    );
    let mut caller = QueryBudget::new(2_000_000);
    let control = InvocationContext::default();
    let values = Collections::new(&source, Selection::Backing);
    let authority = StructuralValueContext::new(&values, &mut caller, &control);
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
        .map_err(|error| {
            format!(
                "checked collection save (provider_calls={}): {error}",
                values.calls.get()
            )
        })?;
    assert!(values.calls.get() > 0);
    let bytes = ticket.resources().to_vec();
    let xml = std::str::from_utf8(&bytes[0].bytes).map_err(|error| error.to_string())?;
    assert!(xml.contains("absentStored=\"[&quot;members&quot;]\""));
    assert!(!xml.contains("<members ") && !xml.contains("members=\""));
    set.acknowledge_save_with_format_context(&registry, &ticket, &formats)?;
    set.unload_with_format_context(
        &registry,
        &selected,
        UnloadPolicy::RequireSaved,
        &formats,
        &control,
    )?;
    assert!(handle_id(set.reader(), &old).is_err());
    assert_eq!(handle_id(set.reader(), &retained)?, "retained");
    let prepared = set.prepare_reload_with_format_context(
        &registry, "xmi", &selected, &bytes, &context, &formats, &control,
    )?;
    set.commit_reload_with_format_context(
        prepared,
        &formats,
        |candidate| {
            assert_eq!(normalized(candidate.clone()), normalized(before.clone()));
            Ok(())
        },
        &control,
    )?;
    assert_eq!(normalized(set.snapshot()?), normalized(before));
    assert_eq!(set.reader().value("holder", "members")?, None);
    assert!(handle_id(set.reader(), &old).is_err());
    let new = set
        .reader()
        .handle("holder")
        .ok_or("missing reloaded holder")?;
    assert_eq!(handle_id(set.reader(), &new)?, "holder");
    assert_eq!(handle_id(set.reader(), &retained)?, "retained");
    assert_eq!(set.context(), &context);
    assert!(
        set.acknowledge_save_with_format_context(&registry, &ticket, &formats)
            .is_err()
    );
    Ok(())
}

#[test]
fn absence_authority_and_present_raw_collection_shapes_remain_strict() -> Result<(), String> {
    let (schema, source) = schema()?;
    let model = model(&schema)?;
    let registry = registry()?;
    assert!(registry.export("xmi", &schema, &model).is_err());
    let control = InvocationContext::default();
    for mode in [Selection::Decline, Selection::Empty] {
        let values = Collections::new(&source, mode);
        let mut caller = QueryBudget::new(100_000);
        let authority = StructuralValueContext::new(&values, &mut caller, &control);
        let formats = FormatContext {
            structural_values: Some(authority.borrowed()),
            ..Default::default()
        };
        assert!(
            registry
                .export_with_format_context("xmi", &schema, &model, None, &formats)
                .is_err()
        );
    }
    let values = Collections::new(&source, Selection::Backing);
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&values, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        ..Default::default()
    };
    let mut missing_optional = model.clone();
    missing_optional
        .elements
        .iter_mut()
        .find(|row| row.id == "holder")
        .ok_or("missing holder")?
        .values
        .remove("note");
    let error = registry
        .export_with_format_context("xmi", &schema, &missing_optional, None, &formats)
        .err()
        .ok_or("unsupported optional absence accepted")?;
    assert!(
        error.contains("requires an explicit supported structural value"),
        "{error}"
    );
    for invalid in [Value::Null, json!("target-a"), json!(7), json!(["missing"])] {
        let mut present = model.clone();
        present
            .elements
            .iter_mut()
            .find(|row| row.id == "holder")
            .ok_or("missing holder")?
            .values
            .insert("members".into(), invalid);
        assert!(
            registry
                .export_with_format_context("xmi", &schema, &present, None, &formats)
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn missing_or_foreign_selector_cannot_publish_decoded_absence() -> Result<(), String> {
    let (schema, source) = schema()?;
    let mut set = admitted(&schema, &source)?;
    let before = set.snapshot()?;
    let old = set.reader().handle("holder").ok_or("missing holder")?;
    let context = set.context().clone();
    let selected = vec![SELECTED.to_owned()];
    let registry = registry()?;
    let control = InvocationContext::default();
    let values = Collections::new(&source, Selection::Backing);
    let mut caller = QueryBudget::new(1_000_000);
    let authority = StructuralValueContext::new(&values, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        ..Default::default()
    };
    let ticket = set.prepare_save_with_format_context(
        &registry,
        "xmi",
        &selected,
        ReferenceClosure::SelectedOnly,
        &formats,
    )?;
    let bytes = ticket.resources();
    assert!(
        set.import_with(&registry, "xmi", &selected, bytes, &context, |_| Ok(()))
            .is_err()
    );
    let mut foreign = source.clone();
    foreign.sha256 = "foreign-source-digest".into();
    let wrong = Collections::new(&foreign, Selection::Backing);
    let mut wrong_caller = QueryBudget::new(100_000);
    let wrong_authority = StructuralValueContext::new(&wrong, &mut wrong_caller, &control);
    let wrong_formats = FormatContext {
        structural_values: Some(wrong_authority.borrowed()),
        ..Default::default()
    };
    let published = Cell::new(false);
    let error = set
        .import_with_format_context(
            &registry,
            "xmi",
            &selected,
            bytes,
            &context,
            &wrong_formats,
            |_| {
                published.set(true);
                Ok(())
            },
        )
        .err()
        .ok_or("foreign source selector published")?;
    assert!(
        error.contains("delegated collection source mismatch"),
        "{error}"
    );
    assert!(!published.get());
    assert_eq!(set.snapshot()?, before);
    assert_eq!(handle_id(set.reader(), &old)?, "holder");
    let original = std::str::from_utf8(&bytes[0].bytes).map_err(|error| error.to_string())?;
    for marker in [
        "[&quot;unknown&quot;]",
        "[&quot;members&quot;,&quot;members&quot;]",
    ] {
        let malformed = vec![Resource {
            uri: bytes[0].uri.clone(),
            bytes: original
                .replace("[&quot;members&quot;]", marker)
                .into_bytes(),
        }];
        assert_ne!(malformed, bytes);
        assert!(
            set.import_with_format_context(
                &registry,
                "xmi",
                &selected,
                &malformed,
                &context,
                &formats,
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(set.snapshot()?, before);
        assert_eq!(handle_id(set.reader(), &old)?, "holder");
    }
    Ok(())
}

#[test]
fn wrong_resource_source_context_and_late_veto_roll_back_same_store() -> Result<(), String> {
    let (schema, source) = schema()?;
    let mut set = admitted(&schema, &source)?;
    let before = set.snapshot()?;
    let holder = set.reader().handle("holder").ok_or("missing holder")?;
    let retained = set.reader().handle("retained").ok_or("missing retained")?;
    let context = set.context().clone();
    let registry = registry()?;
    let selected = vec![SELECTED.to_owned()];
    let values = Collections::new(&source, Selection::Backing);
    let mut caller = QueryBudget::new(1_000_000);
    let control = InvocationContext::default();
    let authority = StructuralValueContext::new(&values, &mut caller, &control);
    let formats = FormatContext {
        structural_values: Some(authority.borrowed()),
        ..Default::default()
    };
    let ticket = set.prepare_save_with_format_context(
        &registry,
        "xmi",
        &selected,
        ReferenceClosure::SelectedOnly,
        &formats,
    )?;
    let wrong_context = ResourceContext::new(
        &schema,
        BTreeMap::from([("selector-source".into(), "wrong-source".into())]),
    )?;
    let called = Cell::new(false);
    assert!(
        set.import_with_format_context(
            &registry,
            "xmi",
            &selected,
            ticket.resources(),
            &wrong_context,
            &formats,
            |_| {
                called.set(true);
                Ok(())
            }
        )
        .is_err()
    );
    assert!(!called.get());
    assert_eq!(set.snapshot()?, before);
    let error = set
        .import_with_format_context(
            &registry,
            "xmi",
            &selected,
            ticket.resources(),
            &context,
            &formats,
            |candidate| {
                assert_eq!(normalized(candidate.clone()), normalized(before.clone()));
                called.set(true);
                Err("late collection publication veto".into())
            },
        )
        .err()
        .ok_or("late veto unexpectedly published")?;
    assert!(called.get());
    assert!(
        error.contains("late collection publication veto"),
        "{error}"
    );
    assert_eq!(set.snapshot()?, before);
    assert_eq!(set.context(), &context);
    assert_eq!(handle_id(set.reader(), &holder)?, "holder");
    assert_eq!(handle_id(set.reader(), &retained)?, "retained");
    Ok(())
}
