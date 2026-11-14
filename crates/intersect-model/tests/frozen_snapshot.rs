//! Handwritten: a prepared model (a "library") saved as a binary snapshot
//! (decision D10), loaded again, and extended with more objects on top
//! through `FrozenStore::from_model`. Implicit elements and defaults are
//! completed only for the objects added on top; the result is the same
//! whether the library was loaded from its snapshot or kept in memory.
use intersect_model::{
    core::{
        frozen::{ById, FrozenStore, Metamodel, Model, SnapshotError, SnapshotTrust},
        model as materialized,
        storage::{self, ModelStore},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        ecore::ParseLimits,
        rcore_behavior::ir_v2::Limits,
        rcore_scope::{
            BindingKind, compile_binding_kinds_with_source_limits, source_implicit_bindings,
        },
        schema::Schema,
    },
    frozen_scopes::RcoreScopes,
};
use std::sync::Arc;

const SOURCE: &str = r##"rcore 1;
package example.frozenSnapshot version "1";
metamodel "urn:frozen-snapshot" prefix m {
  class Node { ref members: Member [0,*] containment; ref uses: Node [0,*]; }
  class Special extends Node { }
  class Member {
    attr name: String [0,1];
    ref owner: Node [0,1];
    ref parts: Member [0,*] containment;
  }
}
behavior {
  import model "urn:frozen-snapshot" as m;
  bind implicit m.Special.members = result_member;
  fn result_member(n:m.Special)->Sequence<m.Member> = Sequence([member(n, "result")]);
  fn member(n:m.Node, label:String)->m.Member =
    new m.Member { name: label, owner: Some(n), parts: [new m.Member { name: label + ".part" }] };
}
"##;
const KEY: [u8; 32] = [3; 32];

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}

struct Fixture {
    schema: Schema,
    mm: Arc<Metamodel>,
    bundle: DefinitionBundle,
    implicits: Vec<intersect_model::definitions::rcore_scope::VerifiedScope>,
}

fn fixture() -> Result<Fixture, String> {
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:frozen-snapshot.rcore",
            SourceKind::Rcore,
            SOURCE,
        )],
        namespaces: vec![],
    };
    let entry = bundle.sources[0].id.clone();
    let schema = compile_structure_bundle(&bundle, &entry, "urn:frozen-snapshot").map_err(err)?;
    let declarations: Vec<_> = source_implicit_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .iter()
        .map(|b| (b.declaration, BindingKind::Implicit))
        .collect();
    let implicits = compile_binding_kinds_with_source_limits(
        &bundle,
        &entry,
        &declarations,
        &Limits::default(),
        ParseLimits::default(),
    )
    .map_err(err)?;
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(err)?);
    Ok(Fixture {
        schema,
        mm,
        bundle,
        implicits,
    })
}

fn class_key(schema: &Schema, name: &str) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("Missing class {name}"))
}

/// Add `special` objects (with an implicit member each) and plain nodes
/// that use the earlier ones, then link, complete and validate.
fn load(
    fixture: &Fixture,
    mut store: FrozenStore,
    resource: &str,
    names: &[&str],
    uses: &[&str],
) -> Result<Model, String> {
    let scopes = RcoreScopes::new(&fixture.mm, &fixture.bundle, &fixture.implicits).map_err(err)?;
    let (schema, special, node) = (
        &fixture.schema,
        class_key(&fixture.schema, "Special")?,
        class_key(&fixture.schema, "Node")?,
    );
    storage::atomic(&mut store, |s| {
        storage::bind_schema(s, schema)?;
        for (index, name) in names.iter().enumerate() {
            s.create_element(schema, name, &special, resource, index, index + 1)?;
        }
        let user = format!("{resource}#user");
        s.create_element(schema, &user, &node, resource, 0, 0)?;
        s.set_value(&user, "uses", Some(serde_json::json!(uses)))
    })?;
    store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .complete_implicit(&scopes)
        .map_err(err)?
        .validate()
        .map_err(err)
}

fn ids(model: &materialized::Model) -> Vec<&str> {
    model.elements.iter().map(|e| e.id.as_str()).collect()
}

#[test]
fn a_library_snapshot_loads_and_extends_like_the_library() -> Result<(), String> {
    let fixture = fixture()?;
    let library = load(
        &fixture,
        FrozenStore::new(fixture.mm.clone()),
        "memory:lib",
        &["a", "b"],
        &[],
    )?;
    let bytes = library.to_snapshot(&KEY).map_err(err)?;
    let loaded = Model::from_snapshot(fixture.mm.clone(), &bytes, &KEY, SnapshotTrust::Revalidate)
        .map_err(err)?;
    let exported = library.to_model().map_err(err)?;
    assert_eq!(loaded.to_model().map_err(err)?, exported);
    assert!(exported.elements.iter().any(|e| e.implicit));

    // On top of the snapshot and on top of the in-memory library: the same.
    let on_snapshot = load(
        &fixture,
        FrozenStore::from_model(loaded),
        "memory:user",
        &["c"],
        &["a", "memory:lib#user"],
    )?
    .to_model()
    .map_err(err)?;
    let on_library = load(
        &fixture,
        FrozenStore::from_model(library),
        "memory:user",
        &["c"],
        &["a", "memory:lib#user"],
    )?
    .to_model()
    .map_err(err)?;
    assert_eq!(on_snapshot, on_library);
    // The library's implicit members were not completed a second time; the
    // new object got its own.
    let all = ids(&on_snapshot);
    for owner in ["a", "b", "c"] {
        let implicit = format!("{owner}/implicit/members/0");
        assert_eq!(
            all.iter().filter(|id| **id == implicit).count(),
            1,
            "{implicit}"
        );
    }
    assert_eq!(on_snapshot.elements.len(), exported.elements.len() + 4);
    let user = on_snapshot
        .elements
        .iter()
        .find(|e| e.id == "memory:user#user")
        .ok_or("user element")?;
    assert_eq!(
        user.values.get("uses"),
        Some(&serde_json::json!(["a", "memory:lib#user"]))
    );
    Ok(())
}

#[test]
fn a_snapshot_of_another_key_is_refused() -> Result<(), String> {
    let fixture = fixture()?;
    let library = load(
        &fixture,
        FrozenStore::new(fixture.mm.clone()),
        "memory:lib",
        &["a"],
        &[],
    )?;
    let bytes = library.to_snapshot(&KEY).map_err(err)?;
    let refused = Model::from_snapshot(
        fixture.mm.clone(),
        &bytes,
        &[4; 32],
        SnapshotTrust::HostCache,
    );
    assert_eq!(refused.err(), Some(SnapshotError::Key));
    Ok(())
}
