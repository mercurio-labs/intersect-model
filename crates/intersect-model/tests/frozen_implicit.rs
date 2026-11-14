//! Rcore `bind implicit` and `new` on the frozen engine: a constructing
//! function adds objects to a containment reference after linking, before
//! defaults and validation.
use intersect_model::{
    core::{
        frozen::{ById, FrozenStore, Metamodel},
        storage::{self, ModelStore},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        ecore::ParseLimits,
        rcore_behavior::ir_v2::Limits,
        rcore_scope::{
            BindingKind, compile_binding_kinds_with_source_limits,
            compile_default_with_source_limits, source_default_bindings, source_implicit_bindings,
        },
        schema::Schema,
    },
    frozen_scopes::RcoreScopes,
};
use std::sync::Arc;

const SOURCE: &str = r##"rcore 1;
package example.frozenImplicit version "1";
metamodel "urn:frozen-implicit" prefix m {
  class Node { ref members: Member [0,*] containment; }
  class Special extends Node { }
  class Member {
    attr name: String [0,1];
    ref owner: Node [0,1];
    ref parts: Member [0,*] containment;
  }
}
behavior {
  import model "urn:frozen-implicit" as m;
  bind implicit m.Special.members = result_member;
  // `new` in a helper: any function an implicit binding reaches may construct.
  fn result_member(n:m.Special)->Sequence<m.Member> = Sequence([member(n, "result")]);
  fn member(n:m.Node, label:String)->m.Member =
    new m.Member { name: label, owner: Some(n), parts: [new m.Member { name: label + ".part" }] };
}
"##;

const DEFAULT_WITH_NEW: &str = r##"rcore 1;
package example.frozenImplicitRefused version "1";
metamodel "urn:frozen-implicit-refused" prefix m {
  class Node { ref members: Member [0,*] containment; ref first: Member [0,1]; }
  class Member { attr name: String [0,1]; }
}
behavior {
  import model "urn:frozen-implicit-refused" as m;
  bind default m.Node.first = made;
  fn made(n:m.Node)->Sequence<m.Member> = Sequence([new m.Member { name: "x" }]);
}
"##;

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}

fn bundle(uri: &str, text: &str) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(uri, SourceKind::Rcore, text)],
        namespaces: vec![],
    }
}

fn class_key(schema: &Schema, name: &str) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("Missing class {name}"))
}

/// Implicit objects follow the explicit contents, only for the binding's
/// receiver class, with deterministic identities, nested contents, references
/// and the implicit mark; they validate like the rest of the model.
#[test]
fn implicit_bindings_add_constructed_objects() -> Result<(), String> {
    let bundle = bundle("memory:frozen-implicit.rcore", SOURCE);
    let entry = bundle.sources[0].id.clone();
    let schema = compile_structure_bundle(&bundle, &entry, "urn:frozen-implicit").map_err(err)?;
    let declarations: Vec<_> = source_implicit_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .iter()
        .map(|b| (b.declaration, BindingKind::Implicit))
        .collect();
    assert_eq!(declarations.len(), 1);
    let implicits = compile_binding_kinds_with_source_limits(
        &bundle,
        &entry,
        &declarations,
        &Limits::default(),
        ParseLimits::default(),
    )
    .map_err(err)?;
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(err)?);
    let scopes = RcoreScopes::new(&mm, &bundle, &implicits).map_err(err)?;

    let node = class_key(&schema, "Node")?;
    let special = class_key(&schema, "Special")?;
    let member = class_key(&schema, "Member")?;
    let mut store = FrozenStore::new(mm.clone());
    storage::atomic(&mut store, |s| {
        storage::bind_schema(s, &schema)?;
        s.create_element(&schema, "plain", &node, "memory:r", 0, 4)?;
        s.create_element(&schema, "special", &special, "memory:r", 5, 9)?;
        s.create_element(&schema, "written", &member, "memory:r", 6, 8)?;
        s.set_value("special", "members", Some(serde_json::json!(["written"])))
    })?;
    let model = store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .complete_implicit(&scopes)
        .map_err(err)?
        .validate()
        .map_err(err)?
        .to_model()
        .map_err(err)?;
    let element = |id: &str| {
        model
            .elements
            .iter()
            .find(|e| e.id == id)
            .ok_or(format!("exported {id}"))
    };
    // Only the receiver class gets the implicit member.
    assert_eq!(
        element("plain")?.values.get("members"),
        Some(&serde_json::json!([]))
    );
    let result = "special/implicit/members/0";
    assert_eq!(
        element("special")?.values.get("members"),
        Some(&serde_json::json!(["written", result]))
    );
    let made = element(result)?;
    assert!(made.implicit);
    assert!(!element("written")?.implicit);
    assert_eq!(made.values.get("name"), Some(&serde_json::json!("result")));
    assert_eq!(
        made.values.get("owner"),
        Some(&serde_json::json!("special"))
    );
    let part = format!("{result}/parts/0");
    assert_eq!(made.values.get("parts"), Some(&serde_json::json!([part])));
    let part = element(&part)?;
    assert!(part.implicit);
    assert_eq!(
        part.values.get("name"),
        Some(&serde_json::json!("result.part"))
    );
    Ok(())
}

/// A default may not construct: `new` belongs to implicit bindings.
#[test]
fn defaults_refuse_constructing_functions() -> Result<(), String> {
    let bundle = bundle("memory:frozen-implicit-refused.rcore", DEFAULT_WITH_NEW);
    let entry = bundle.sources[0].id.clone();
    let declaration = source_default_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .first()
        .map(|b| b.declaration)
        .ok_or("default binding")?;
    let error = compile_default_with_source_limits(
        &bundle,
        &entry,
        declaration,
        &Limits::default(),
        ParseLimits::default(),
    )
    .err()
    .ok_or("a default that constructs must not compile")?;
    assert!(
        error
            .iter()
            .any(|d| d.message.contains("`new` is allowed only")),
        "{error:?}"
    );
    Ok(())
}
