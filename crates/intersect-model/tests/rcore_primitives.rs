//! `roots::<T>(x)` and `split(text, separator)` in Rcore scope functions, on
//! the store path (`SourceModelReads`), the resumable VM over the frozen
//! engine and the direct executor (shadow mode: both must agree).
use intersect_model::{
    core::{
        frozen::{FrozenStore, Metamodel, ModelBuilder},
        query::QueryBudget,
        storage::{self, ModelStore},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        ecore::ParseLimits,
        rcore_behavior::ir_v2::Limits,
        rcore_scope::{VerifiedScope, compile_scope, source_scope_bindings},
        schema::Schema,
    },
    frozen_scopes::{Execution, RcoreScopes},
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    scope_runtime,
    semantic_bindings::BorrowedBindingWork,
    source_model_reads::SourceModelReads,
};
use serde_json::json;
use std::{cell::RefCell, sync::Arc};

const SOURCE: &str = r#"rcore 1;
package example.primitives version "1";
metamodel "urn:primitives" prefix m {
  class Namespace { attr name: String [1,1]; ref types: Type [0,*] containment; }
  class Type { attr name: String [1,1]; }
  class Property { ref targetType: Type [0,1]; }
}
behavior {
  import model "urn:primitives" as m;
  bind scope m.Property.targetType = qualified;
  // "Namespace::Type": the first segment names a root namespace, the last a type in it.
  fn qualified(owner:m.Property, spelling:String)->Sequence<m.Type> {
    let parts = split(spelling, "::");
    match first(parts) {
      Some(head) => match last(parts) {
        Some(tail) => Sequence(flatMap(
          filter(roots::<m.Namespace>(owner), |ns| ns.name == head),
          |ns| filter(ns.types, |t| t.name == tail))),
        None => []
      },
      None => []
    }
  }
}
"#;

struct Fixture {
    bundle: DefinitionBundle,
    schema: Schema,
    scopes: Vec<VerifiedScope>,
}
fn fixture() -> Result<Fixture, String> {
    fixture_from(SOURCE)
}
fn fixture_from(text: &str) -> Result<Fixture, String> {
    let source = DefinitionSource::new("memory:primitives.rcore", SourceKind::Rcore, text);
    let entry = source.id.clone();
    let bindings =
        source_scope_bindings(&source, ParseLimits::default()).map_err(|e| format!("{e:?}"))?;
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let scopes = bindings
        .iter()
        .map(|b| compile_scope(&bundle, &entry, b.declaration, &Limits::default()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{e:?}"))?;
    let schema = compile_structure_bundle(&bundle, &entry, "urn:primitives")
        .map_err(|e| format!("{e:?}"))?;
    Ok(Fixture {
        bundle,
        schema,
        scopes,
    })
}
fn key(schema: &Schema, name: &str) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("Missing class {name}"))
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// (identity, name, [(type identity, type name)])
type NamespaceRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);
/// Root namespaces `beta` {Voltage}, `alpha` {Voltage, Current} (created out
/// of identity order) and a root property.
const NAMESPACES: [NamespaceRow; 2] = [
    ("nsB", "Beta", &[("beta_v", "Voltage")]),
    (
        "nsA",
        "Alpha",
        &[("alpha_v", "Voltage"), ("alpha_c", "Current")],
    ),
];

fn store_candidates(f: &Fixture, spelling: &str) -> Result<Vec<String>, String> {
    let mut store = FrozenStore::for_schema(&f.schema).map_err(|e| e.to_string())?;
    storage::atomic(&mut store, |s| {
        s.create_element(
            &f.schema,
            "property",
            &key(&f.schema, "Property")?,
            "memory:r",
            0,
            0,
        )?;
        for (ns, name, types) in NAMESPACES {
            s.create_element(
                &f.schema,
                ns,
                &key(&f.schema, "Namespace")?,
                "memory:r",
                0,
                0,
            )?;
            storage::set_with_opposite(s, &f.schema, ns, "name", json!(name))?;
            for (id, label) in types {
                s.create_element(&f.schema, id, &key(&f.schema, "Type")?, "memory:r", 0, 0)?;
                storage::set_with_opposite(s, &f.schema, id, "name", json!(label))?;
            }
            let ids: Vec<_> = types.iter().map(|(id, _)| *id).collect();
            storage::set_with_opposite(s, &f.schema, ns, "types", json!(ids))?;
        }
        Ok(())
    })?;
    let mut caller = QueryBudget::new(1_000_000);
    let work = BorrowedBindingWork::bind(&mut caller);
    let scope = f.scopes.first().ok_or("Missing scope")?;
    scope_runtime::evaluate(
        scope,
        &ExecutionContext::new(&f.bundle),
        &f.schema,
        &SourceModelReads::new(&store, &f.schema),
        "property",
        spelling,
        &ExecutionLimits::default(),
        &work,
    )
    .map(|c| c.into_identities())
    .map_err(err)
}

/// Candidates from the frozen engine, with the VM and the direct executor
/// both run (shadow mode).
fn frozen_candidates(f: &Fixture, spelling: &str) -> Result<Vec<String>, String> {
    let mm = Arc::new(Metamodel::freeze(&f.schema).map_err(err)?);
    let class =
        |name| -> Result<_, String> { mm.class(&key(&f.schema, name)?).ok_or("class".into()) };
    let (ns_class, type_class, property) =
        (class("Namespace")?, class("Type")?, class("Property")?);
    let feature = |c, n: &str| mm.feature(c, n).ok_or(format!("feature {n}"));
    let (ns_name, types) = (feature(ns_class, "name")?, feature(ns_class, "types")?);
    let type_name = feature(type_class, "name")?;
    let target = feature(property, "targetType")?;
    let mut b = ModelBuilder::new(mm.clone());
    let p = b.create("property", property, "memory:r").map_err(err)?;
    b.add_proxy(p, target, spelling).map_err(err)?;
    for (ns, name, members) in NAMESPACES {
        let n = b.create(ns, ns_class, "memory:r").map_err(err)?;
        b.set_str(n, ns_name, name).map_err(err)?;
        for (id, label) in members {
            let t = b.create(id, type_class, "memory:r").map_err(err)?;
            b.set_str(t, type_name, label).map_err(err)?;
            b.add_ref(n, types, t).map_err(err)?;
        }
    }
    let scopes = RcoreScopes::new(&mm, &f.bundle, &f.scopes)
        .map_err(err)?
        .with_execution(Execution::Shadow);
    let seen = RefCell::new(None);
    let record = |view: &intersect_model::core::frozen::LinkView<'_>,
                  owner,
                  feature,
                  spelling: &str|
     -> Result<_, intersect_model::core::frozen::FrozenError> {
        let found = scopes.candidates(view, owner, feature, spelling)?;
        *seen.borrow_mut() = Some(
            found
                .iter()
                .filter_map(|o| view.identity(*o))
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        );
        Ok(found.first().copied())
    };
    let _ = b.link(&record);
    let stats = scopes.stats();
    if stats.fast != 1 || stats.deferred != 0 {
        return Err(format!("Direct executor did not run the scope: {stats:?}"));
    }
    seen.into_inner()
        .ok_or_else(|| "Scope was not invoked".into())
}

#[test]
fn split_and_roots_agree_on_store_vm_and_direct_executor() -> Result<(), String> {
    let f = fixture()?;
    for (spelling, expected) in [
        ("Alpha::Voltage", vec!["alpha_v"]),
        ("Beta::Voltage", vec!["beta_v"]),
        ("Alpha::Current", vec!["alpha_c"]),
        ("Gamma::Voltage", vec![]),
        ("Alpha::Missing", vec![]),
        // One segment: head and tail are the same name.
        ("Alpha", vec![]),
    ] {
        let store = store_candidates(&f, spelling)?;
        assert_eq!(store, expected, "store path for {spelling}");
        assert_eq!(
            frozen_candidates(&f, spelling)?,
            store,
            "frozen engine for {spelling}"
        );
    }
    Ok(())
}

#[test]
fn roots_are_in_identity_order_on_both_engines() -> Result<(), String> {
    // Every root namespace's Voltage, in root identity order: nsA before nsB,
    // although nsB was created first. The root property is filtered out by type.
    let f = fixture_from(&SOURCE.replace(
        "filter(roots::<m.Namespace>(owner), |ns| ns.name == head)",
        "roots::<m.Namespace>(owner)",
    ))?;
    let store = store_candidates(&f, "x::Voltage")?;
    assert_eq!(store, ["alpha_v", "beta_v"]);
    assert_eq!(frozen_candidates(&f, "x::Voltage")?, store);
    Ok(())
}

#[test]
fn empty_split_separator_is_an_error_on_every_path() -> Result<(), String> {
    let f = fixture_from(&SOURCE.replace("split(spelling, \"::\")", "split(spelling, \"\")"))?;
    assert!(store_candidates(&f, "Alpha::Voltage").is_err());
    // The direct executor defers this case; the VM reports the error.
    assert!(frozen_candidates(&f, "Alpha::Voltage").is_err());
    Ok(())
}
