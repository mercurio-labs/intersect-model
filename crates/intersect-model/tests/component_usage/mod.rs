//! Behavioral controls for the real mounted store, not a parallel model engine.
#[path = "../../examples/support/component_fixture.rs"]
mod fixture;

use fixture::{Fixture, URI, prepare};
use intersect_model::{
    components::{ComponentImage, MountedDraft, MountedModel, SharedSchema},
    frozen::{ById, ModelBuilder},
    storage::{ModelRead, Query, ReferenceView, StorageResult},
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn component_usage_keeps_native_redefinition_validation_layout() -> StorageResult<()> {
    use intersect_model::{
        definitions::{
            bundle::{DefinitionBundle, DefinitionSource, SourceKind},
            compiler::compile_structure_bundle,
            ecore::ParseLimits,
            rcore::feature_redefinitions,
        },
        frozen::Metamodel,
    };
    let source = r##"rcore 1; package redefinition version "1";
metamodel "urn:component-redefinition" prefix m {
 class Base { ref member: Base [1,1]; }
 class Derived extends Base {
   ref selected: Base [1,1] derived annotation "redefines" { reference "#//@eClassifiers.0/@eStructuralFeatures.0"; };
 }
}"##;
    let definition = DefinitionSource::new("memory:redefinition.rcore", SourceKind::Rcore, source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![definition.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &definition.id, "urn:component-redefinition")
        .map_err(|e| format!("{e:?}"))?;
    let redefinitions =
        feature_redefinitions(&definition, ParseLimits::default()).map_err(|e| format!("{e:?}"))?;
    let mm = Arc::new(
        Metamodel::freeze(&schema)
            .and_then(|mm| mm.with_redefinitions(&redefinitions))
            .map_err(|e| format!("{e:?}"))?,
    );
    let class = schema
        .declarations
        .iter()
        .find(|(_, value)| value.name == "Derived")
        .map(|(key, _)| key)
        .ok_or("Derived")?;
    let mut builder = ModelBuilder::new(mm.clone());
    builder
        .create(
            "derived",
            mm.class(class).ok_or("Derived")?,
            "memory:/redefinition",
        )
        .map_err(|e| format!("{e:?}"))?;
    let native = builder
        .link(&ById)
        .map_err(|e| format!("{e:?}"))?
        .validate()
        .map_err(|e| format!("{e:?}"))?;
    let context = SharedSchema::from_metamodel(mm)?;
    let (library, bytes) =
        ComponentImage::prepare_with_snapshot(context.clone(), "urn:redefined-library", native)?;
    let restored = ComponentImage::restore(context.clone(), &library.descriptor(), &bytes)?;
    let mut model = MountedModel::new(context, vec![restored])?;
    model.validate()?;
    let base = schema
        .declarations
        .iter()
        .find(|(_, value)| value.name == "Base")
        .map(|(key, _)| key)
        .ok_or("Base")?;
    model.create("base", base, "memory:/base")?;
    assert!(
        model.validate().is_err(),
        "an unredefined required member must still be checked"
    );
    model.set("base", "member", json!("base"))?;
    model.validate()
}

#[test]
fn component_usage_runs_authored_scope_and_defaults_over_borrowed_arenas() -> StorageResult<()> {
    use intersect_model::{
        definitions::{
            bundle::{DefinitionBundle, DefinitionSource, SourceKind},
            compiler::compile_structure_bundle,
            ecore::ParseLimits,
            rcore::feature_redefinitions,
            rcore_behavior::ir_v2::Limits,
            rcore_scope::{
                compile_default_with_source_limits, compile_scope,
                compile_validate_with_source_limits, source_default_bindings,
                source_scope_bindings, source_validate_bindings,
            },
        },
        frozen_scopes::{Execution, RcoreScopes},
        storage::{self, ModelStore},
    };
    let source = DefinitionSource::new(
        "memory:mounted-scopes.rcore",
        SourceKind::Rcore,
        r##"rcore 1;
package mountedScopes version "1";
metamodel "urn:mounted-scopes" prefix m {
  class Namespace { ref types: Type [0,*] containment; }
  class Type { attr name: String [1,1]; }
  class Property { attr label: String [0,1]; ref owner: Namespace [0,1]; ref target: Type [0,1]; }
  class Renamed extends Type {
    attr sourceName: String [1,1] annotation "redefines" { reference "#//@eClassifiers.1/@eStructuralFeatures.0"; };
  }
  class Broken extends Property { }
}
behavior {
 import model "urn:mounted-scopes" as m;
 bind scope m.Property.target = visible;
 fn visible(p:m.Property, spelling:String)->Sequence<m.Type> = match p.owner {
   Some(ns) => Sequence(filter(ns.types, |t| t.name == spelling)), None => [] };
 bind default m.Property.label = label;
 fn label(p:m.Property)->Sequence<String> = Sequence(["project default"]);
 bind validate m.Property = checked;
 fn checked(p:m.Property)->Sequence<Issue> = match p.owner {
   Some(ns) => if size(filter(ns.types, |t| t.name == "Motor")) > 0 {
     []
   } else { [issue("error", "mounted.types", "Different members")] },
   None => [issue("error", "mounted.owner", "Missing owner")]
 };
 bind validate m.Broken = invalid_split;
 // The direct executor defers an empty separator to the authoritative VM.
 fn invalid_split(p:m.Broken)->Sequence<Issue> = match p.label {
   Some(text) => if size(split(text, "")) > 0 { [] } else { [] },
   None => []
 };
}"##,
    );
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &entry, "urn:mounted-scopes")
        .map_err(|e| format!("{e:?}"))?;
    let redefinitions =
        feature_redefinitions(&source, ParseLimits::default()).map_err(|e| format!("{e:?}"))?;
    let metamodel = intersect_model::frozen::Metamodel::freeze(&schema)
        .and_then(|mm| mm.with_redefinitions(&redefinitions))
        .map_err(|e| format!("{e:?}"))?;
    let context = SharedSchema::from_metamodel(Arc::new(metamodel))?;
    let mm = context.metamodel();
    let key = |name: &str| {
        schema
            .declarations
            .iter()
            .find(|(key, value)| value.name == name && schema.classes.contains_key(*key))
            .map(|(key, _)| key.clone())
            .ok_or_else(|| format!("Missing {name}"))
    };
    let ns = mm.class(&key("Namespace")?).ok_or("Namespace")?;
    let ty = mm.class(&key("Type")?).ok_or("Type")?;
    let renamed = mm.class(&key("Renamed")?).ok_or("Renamed")?;
    let property = mm.class(&key("Property")?).ok_or("Property")?;
    let mut compiled = source_scope_bindings(&source, ParseLimits::default())
        .map_err(|e| format!("{e:?}"))?
        .iter()
        .map(|binding| {
            compile_scope(&bundle, &entry, binding.declaration, &Limits::default())
                .map_err(|e| format!("{e:?}"))
        })
        .collect::<StorageResult<Vec<_>>>()?;
    compiled.extend(
        source_default_bindings(&source, ParseLimits::default())
            .map_err(|e| format!("{e:?}"))?
            .iter()
            .map(|binding| {
                compile_default_with_source_limits(
                    &bundle,
                    &entry,
                    binding.declaration,
                    &Limits::default(),
                    ParseLimits::default(),
                )
                .map_err(|e| format!("{e:?}"))
            })
            .collect::<StorageResult<Vec<_>>>()?,
    );
    compiled.extend(
        source_validate_bindings(&source, ParseLimits::default())
            .map_err(|e| format!("{e:?}"))?
            .iter()
            .map(|binding| {
                compile_validate_with_source_limits(
                    &bundle,
                    &entry,
                    binding.declaration,
                    &Limits::default(),
                    ParseLimits::default(),
                )
                .map_err(|e| format!("{e:?}"))
            })
            .collect::<StorageResult<Vec<_>>>()?,
    );
    let scopes = RcoreScopes::new(&mm, &bundle, &compiled)
        .map_err(|e| format!("{e:?}"))?
        .with_execution(Execution::Shadow);
    let mut builder = ModelBuilder::new(mm.clone());
    let namespace = builder
        .create("namespace", ns, "memory:/library")
        .map_err(|e| format!("{e:?}"))?;
    let selected = builder
        .create("selected", renamed, "memory:/library")
        .map_err(|e| format!("{e:?}"))?;
    let outside = builder
        .create("outside-scope", ty, "memory:/library")
        .map_err(|e| format!("{e:?}"))?;
    builder
        .create("unchanged-library-property", property, "memory:/library")
        .map_err(|e| format!("{e:?}"))?;
    let name = mm.feature(ty, "name").ok_or("name")?;
    builder
        .set_str(
            selected,
            mm.feature(renamed, "sourceName").ok_or("sourceName")?,
            "Motor",
        )
        .map_err(|e| format!("{e:?}"))?;
    builder
        .set_str(outside, name, "Motor")
        .map_err(|e| format!("{e:?}"))?;
    builder
        .add_ref(namespace, mm.feature(ns, "types").ok_or("types")?, selected)
        .map_err(|e| format!("{e:?}"))?;
    let library = ComponentImage::prepare(
        context.clone(),
        "urn:scoped-library",
        builder
            .link(&ById)
            .map_err(|e| format!("{e:?}"))?
            .validate()
            .map_err(|e| format!("{e:?}"))?,
    )?;
    let mut draft = MountedDraft::new(context.clone(), vec![library.clone()])?;
    storage::atomic(draft.project_store_mut(), |store| {
        store.create_element(
            &schema,
            "project",
            &key("Property")?,
            "memory:/project",
            0,
            0,
        )?;
        store.set_resolved_value(
            "project",
            "owner",
            Some(json!(library.object_id("namespace")?)),
        )?;
        store.set_value("project", "target", Some(json!("Motor")))
    })?;
    let found = draft
        .scope_candidates(&scopes, "project", "target", "Motor")?
        .ok_or("Unexpected pending scope")?;
    assert_eq!(found, vec![library.object_id("selected")?]);
    draft.resolve_reference("project", "target", None, &found[0])?;
    draft.complete(&scopes)?;
    assert!(draft.issues(&scopes)?.is_empty());
    storage::atomic(draft.project_store_mut(), |store| {
        store.set_resolved_value("project", "owner", Some(json!(null)))
    })?;
    let issues = draft.issues(&scopes)?;
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].1.code, "mounted.owner");
    assert!(scopes.stats().vm >= 3);
    storage::atomic(draft.project_store_mut(), |store| {
        store.set_resolved_value(
            "project",
            "owner",
            Some(json!(library.object_id("namespace")?)),
        )
    })?;
    let model = draft.finish()?;
    let fallback = RcoreScopes::new(&mm, &bundle, &compiled).map_err(|e| format!("{e:?}"))?;
    let mut broken = MountedDraft::new(context, vec![library.clone()])?;
    storage::atomic(broken.project_store_mut(), |store| {
        store.create_element(&schema, "broken", &key("Broken")?, "memory:/broken", 0, 0)?;
        store.set_resolved_value("broken", "label", Some(json!("text")))
    })?;
    let error = broken
        .issues(&fallback)
        .expect_err("empty separator must remain a VM argument error");
    assert!(error.contains("separator"), "{error}");
    assert!(fallback.stats().deferred >= 1);
    assert!(fallback.stats().vm >= 1);
    assert_eq!(
        model.value("project", "label")?,
        Some(json!("project default"))
    );
    assert!(
        model
            .value(library.object_id("unchanged-library-property")?, "label")?
            .is_none_or(|value| value.is_null())
    );
    assert_eq!(model.value("project", "target")?, Some(json!(found[0])));
    Ok(())
}

#[test]
fn component_usage_snapshot_restores_exact_identity_and_rejects_corruption() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mm = f.context.metamodel();
    let class = mm.class(&f.node).ok_or("Missing Node")?;
    let mut builder = ModelBuilder::new(mm);
    builder
        .create("cached", class, "memory:/cached.nodes")
        .map_err(|e| format!("{e:?}"))?;
    let frozen = builder
        .link(&ById)
        .map_err(|e| format!("{e:?}"))?
        .validate()
        .map_err(|e| format!("{e:?}"))?;
    let (image, bytes) = ComponentImage::prepare_with_snapshot(f.context.clone(), URI, frozen)?;
    let descriptor = image.descriptor();
    let restored = ComponentImage::restore(f.context.clone(), &descriptor, &bytes)?;
    assert_eq!(restored.identity(), image.identity());
    assert_eq!(restored.object_id("cached")?, image.object_id("cached")?);
    let model = MountedModel::new(f.context.clone(), vec![restored])?;
    model.validate()?;
    let mut corrupt = bytes.clone();
    let at = corrupt.len() / 2;
    corrupt[at] ^= 1;
    assert!(ComponentImage::restore(f.context.clone(), &descriptor, &corrupt).is_err());
    let mut value = serde_json::to_value(&descriptor).map_err(|e| format!("{e:?}"))?;
    value["identity"]["engine"] = json!("another-runtime");
    let incompatible = serde_json::from_value(value).map_err(|e| format!("{e:?}"))?;
    assert!(ComponentImage::restore(f.context.clone(), &incompatible, &bytes).is_err());
    assert!(ComponentImage::restore(f.context, &descriptor, &bytes[..bytes.len() / 2]).is_err());
    Ok(())
}

#[test]
fn component_usage_draft_reconciles_project_references_without_authoring_library()
-> StorageResult<()> {
    use intersect_model::storage::ModelStore;
    let f = Fixture::new()?;
    let target = f.library.object_id("shared")?.to_owned();
    let mut draft = MountedDraft::new(f.context.clone(), vec![f.library.clone()])?;
    intersect_model::storage::atomic(draft.project_store_mut(), |store| {
        store.create_element(
            f.context.schema(),
            "project",
            &f.node,
            "memory:/project.nodes",
            0,
            0,
        )?;
        store.set_resolved_value("project", "out", Some(json!([target])))
    })?;
    draft.reconcile_opposites()?;
    let model = draft.finish()?;
    assert_eq!(model.value(&target, "incoming")?, Some(json!(["project"])));
    let other = f.project("other")?;
    assert!(
        other
            .value(&target, "incoming")?
            .is_none_or(|value| value == json!([]) || value.is_null())
    );
    assert!(model.shares_component_with(&other, URI));
    Ok(())
}

#[test]
fn component_usage_shares_arena_descriptors_and_borrowed_values() -> StorageResult<()> {
    let f = Fixture::new()?;
    let a = f.project("a")?;
    let b = f.project("b")?;
    let id = f.library.object_id("shared")?;
    assert!(a.shares_component_with(&b, URI));
    assert!(Arc::ptr_eq(a.shared_schema(), b.shared_schema()));
    assert_eq!(a.project_object_count()?, 1);
    assert_eq!(b.project_object_count()?, 1);
    let mut pointers = Vec::new();
    let type_id = f
        .context
        .descriptors()?
        .type_id(&f.node)
        .ok_or("Missing type")?;
    let feature = f
        .context
        .descriptors()?
        .type_descriptor(&type_id)?
        .feature("name")
        .ok_or("Missing feature")?
        .id();
    for model in [&a, &b] {
        model.inspect(id, &mut |object| {
            assert_eq!(object.type_id(), Some(type_id.clone()));
            let value = object.value_by_feature(&feature)?.ok_or("Missing name")?;
            pointers.push(value.as_str().ok_or("Name is not text")?.as_ptr() as usize);
            Ok(())
        })?;
    }
    assert_eq!(pointers[0], pointers[1]);
    a.inspect("a", &mut |object| {
        assert_eq!(object.type_id(), Some(type_id.clone()));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn component_usage_qualifies_local_and_internal_references_without_collisions() -> StorageResult<()>
{
    let f = Fixture::new()?;
    let another = prepare(
        f.context.clone(),
        &f.node,
        "urn:example:another",
        "Other library",
    )?;
    let mut model = MountedModel::new(f.context.clone(), vec![f.library.clone(), another.clone()])?;
    model.create("shared", &f.node, "memory:/library.nodes")?;
    let first = f.library.object_id("shared")?;
    let second = another.object_id("shared")?;
    let internal = f.library.object_id("internal")?;
    assert_ne!(first, second);
    assert_ne!(first, "shared");
    model.set("shared", "target", json!(second))?;
    assert_eq!(model.value("shared", "target")?, Some(json!(second)));
    model.set("shared", "out", json!([first, second]))?;
    assert_eq!(model.value(first, "out")?, Some(json!([internal])));
    assert_eq!(
        model
            .component_target(first)
            .ok_or("Missing owner")?
            .element_id,
        "shared"
    );
    let mut handles = Vec::new();
    model.inspect("shared", &mut |object| {
        for value in object
            .value("out")
            .and_then(|v| v.collection())
            .ok_or("Missing references")?
            .iter()
        {
            match value.reference() {
                Some(ReferenceView::Resolved(h)) => handles.push(h),
                _ => return Err("Reference not resolved".into()),
            }
        }
        Ok(())
    })?;
    assert_ne!(handles[0], handles[1]);
    model.inspect_handle(&handles[1], &mut |object| {
        assert_eq!(
            object.value("name").and_then(|v| v.as_str()),
            Some("Other library")
        );
        Ok(())
    })?;
    model.validate()
}

#[test]
fn component_usage_inverse_overlay_is_isolated_and_transactional() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mut a = f.project("a")?;
    let b = f.project("b")?;
    let id = f.library.object_id("shared")?;
    a.set("a", "out", json!([id]))?;
    assert_eq!(a.value(id, "incoming")?, Some(json!(["a"])));
    assert_ne!(b.value(id, "incoming")?, Some(json!(["a"])));
    let result: StorageResult<()> = a.transaction(|edit| {
        edit.set("a", "out", json!([]))?;
        edit.set("a", "name", json!("temporary"))?;
        Err("abort".into())
    });
    assert!(result.is_err());
    assert_eq!(a.value("a", "out")?, Some(json!([id])));
    assert_eq!(a.value(id, "incoming")?, Some(json!(["a"])));
    assert_ne!(a.value("a", "name")?, Some(json!("temporary")));
    a.set("a", "out", json!([]))?;
    assert_ne!(a.value(id, "incoming")?, Some(json!(["a"])));
    a.validate()?;
    b.validate()
}

#[test]
fn component_usage_preserves_authored_edges_and_scalar_inverse_displacement() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mut a = f.project("a")?;
    a.create("next", &f.node, "memory:/project.nodes")?;
    let shared = f.library.object_id("shared")?;
    let internal = f.library.object_id("internal")?;
    a.set("a", "out", json!([internal]))?;
    assert_eq!(a.value(internal, "incoming")?, Some(json!([shared, "a"])));
    a.set("a", "out", json!([]))?;
    assert_eq!(a.value(internal, "incoming")?, Some(json!([shared])));
    a.set("a", "partner", json!(shared))?;
    a.set("next", "partner", json!(shared))?;
    assert_eq!(a.value("a", "partner")?, Some(json!(null)));
    assert_eq!(a.value(shared, "partneredBy")?, Some(json!("next")));
    a.validate()
}

#[test]
fn component_usage_rejects_library_writes_and_rolls_back_failed_batches() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mut a = f.project("a")?;
    let id = f.library.object_id("shared")?;
    let before = a.get(id)?;
    assert!(a.set(id, "name", json!("forbidden")).is_err());
    assert!(a.set(id, "out", json!([])).is_err());
    assert!(a.create(id, &f.node, "memory:/project.nodes").is_err());
    assert!(a.create("bad-resource", &f.node, id).is_err());
    assert_eq!(a.get(id)?, before);
    a.create("wrong", &f.other, "memory:/project.nodes")?;
    assert!(a.set("a", "target", json!("wrong")).is_err());
    assert!(a.set("a", "target", json!("absent")).is_err());
    assert!(
        a.set_many(&[("a", "out", json!([id])), ("a", "out", json!(["wrong"]))])
            .is_err()
    );
    assert_ne!(a.value(id, "incoming")?, Some(json!(["a"])));
    assert_ne!(a.value("a", "out")?, Some(json!([id])));
    assert!(a.set("a", "out", json!(["absent"])).is_err());
    a.validate()
}

#[test]
fn component_usage_refuses_cross_component_containment_atomically() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mut a = f.project("a")?;
    let id = f.library.object_id("shared")?;
    let before = a.get("a")?;
    assert!(a.set("a", "children", json!([id])).is_err());
    assert_eq!(a.get("a")?, before);
    assert_ne!(a.value(id, "parent")?, Some(json!("a")));
    a.validate()
}

#[test]
fn component_usage_pages_are_bounded_distinct_and_complete() -> StorageResult<()> {
    let f = Fixture::new()?;
    let other = prepare(
        f.context.clone(),
        &f.node,
        "urn:example:other",
        "Other library",
    )?;
    let mut a = MountedModel::new(f.context.clone(), vec![f.library.clone(), other])?;
    a.create("a", &f.node, "memory:/project.nodes")?;
    a.create("b", &f.node, "memory:/project.nodes")?;
    let expected = a.query(Query::Class(&f.node))?;
    assert_eq!(expected.len(), 6);
    let mut actual = Vec::new();
    let mut cursor = None;
    loop {
        let batch = a.query_candidates(Query::Class(&f.node), cursor, 2)?;
        assert!(batch.rows.len() <= 2);
        for (row, id) in &batch.rows {
            assert!(cursor.is_none_or(|previous| *row > previous));
            if a.matches(id, Query::Class(&f.node))? {
                actual.push(id.clone());
            }
        }
        if batch.exhausted {
            break;
        }
        assert_ne!(batch.after, cursor);
        cursor = batch.after;
    }
    assert_eq!(actual, expected);
    assert_eq!(
        actual
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        6
    );
    assert!(a.query_candidates(Query::Class(&f.node), None, 0).is_err());
    assert!(a.id_page(None, 0)?.is_empty());
    assert_eq!(a.page(None, 2)?.len(), 2);
    Ok(())
}

#[test]
fn component_usage_forks_share_dependencies_but_not_state_or_handles() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mut a = f.project("a")?;
    let id = f.library.object_id("shared")?;
    a.set("a", "out", json!([id]))?;
    let handle = a.handle(id).ok_or("Missing handle")?;
    let local_handle = a.handle("a").ok_or("Missing handle")?;
    let mut b = a.fork()?;
    assert!(a.shares_component_with(&b, URI));
    b.set("a", "out", json!([]))?;
    b.set("a", "name", json!("fork"))?;
    assert_ne!(a.value("a", "name")?, b.value("a", "name")?);
    assert_eq!(a.value(id, "incoming")?, Some(json!(["a"])));
    assert_ne!(b.value(id, "incoming")?, Some(json!(["a"])));
    assert!(b.inspect_handle(&handle, &mut |_| Ok(())).is_err());
    assert!(b.inspect_handle(&local_handle, &mut |_| Ok(())).is_err());
    a.validate()?;
    b.validate()
}

#[test]
fn component_usage_refuses_conflicting_mounts_and_incompatible_schemas() -> StorageResult<()> {
    let f = Fixture::new()?;
    assert!(
        MountedModel::new(
            f.context.clone(),
            vec![f.library.clone(), f.library.clone()]
        )
        .is_err()
    );
    let v2 = prepare(f.context.clone(), &f.node, URI, "Version two")?;
    assert_ne!(f.library.identity(), v2.identity());
    assert!(MountedModel::new(f.context.clone(), vec![f.library.clone(), v2]).is_err());
    let other = intersect_model::compile_rcore(
        "rcore 1; package other version \"1\"; metamodel \"urn:other\" prefix o { class Item {} }",
    )
    .map_err(|e| format!("{e:?}"))?;
    let other = SharedSchema::new(other.schema())?;
    assert!(MountedModel::new(other.clone(), vec![f.library.clone()]).is_err());
    let mm = f.context.metamodel();
    let model = ModelBuilder::new(mm)
        .link(&ById)
        .map_err(|e| format!("{e:?}"))?
        .validate()
        .map_err(|e| format!("{e:?}"))?;
    assert!(intersect_model::components::ComponentImage::prepare(other, URI, model).is_err());
    Ok(())
}

#[test]
fn component_usage_attribute_strings_are_not_rewritten_as_references() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mut a = f.project("a")?;
    let id = f.library.object_id("shared")?;
    a.set("a", "name", json!(id))?;
    a.inspect("a", &mut |object| {
        let value = object.value("name").ok_or("Missing attribute")?;
        assert_eq!(value.as_str(), Some(id));
        assert!(value.reference().is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn component_usage_cannot_displace_authored_scalar_relationships() -> StorageResult<()> {
    let f = Fixture::new()?;
    let mm = f.context.metamodel();
    let class = mm.class(&f.node).ok_or("Missing Node")?;
    let partner = mm.feature(class, "partner").ok_or("Missing partner")?;
    let mut builder = ModelBuilder::new(mm);
    let a = builder
        .create("a", class, "memory:/occupied")
        .map_err(|e| format!("{e:?}"))?;
    let b = builder
        .create("b", class, "memory:/occupied")
        .map_err(|e| format!("{e:?}"))?;
    builder
        .add_ref(a, partner, b)
        .map_err(|e| format!("{e:?}"))?;
    let prepared = builder
        .link(&ById)
        .map_err(|e| format!("{e:?}"))?
        .validate()
        .map_err(|e| format!("{e:?}"))?;
    let image = intersect_model::components::ComponentImage::prepare(
        f.context.clone(),
        "urn:occupied",
        prepared,
    )?;
    let target = image.object_id("b")?.to_owned();
    let mut project = MountedModel::new(f.context.clone(), vec![image])?;
    project.create("p", &f.node, "memory:/project")?;
    let before = project.get(&target)?;
    assert!(project.set("p", "partner", json!(target)).is_err());
    assert_eq!(project.get(&target)?, before);
    project.validate()
}

#[test]
fn component_usage_rollback_invalidates_tentative_handles_and_restores_on_unwind()
-> StorageResult<()> {
    let f = Fixture::new()?;
    let mut project = f.project("p")?;
    let mut tentative = None;
    let result: StorageResult<()> = project.transaction(|edit| {
        edit.create("temporary", &f.node, "memory:/project")?;
        tentative = edit.handle("temporary");
        Err("abort creation".into())
    });
    assert!(result.is_err());
    let handle = tentative.ok_or("Missing tentative handle")?;
    assert!(project.inspect_handle(&handle, &mut |_| Ok(())).is_err());
    project.create("temporary", &f.node, "memory:/project")?;
    assert_ne!(project.handle("temporary"), Some(handle.clone()));
    assert!(project.inspect_handle(&handle, &mut |_| Ok(())).is_err());
    let target = f.library.object_id("shared")?;
    let before = project.get("p")?;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: StorageResult<()> = project.transaction(|edit| {
            edit.set("p", "out", json!([target]))?;
            panic!("intentional test unwind");
        });
    }));
    assert!(panic.is_err());
    assert_eq!(project.get("p")?, before);
    assert_ne!(project.value(target, "incoming")?, Some(json!(["p"])));
    project.validate()
}
