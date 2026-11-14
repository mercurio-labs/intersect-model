//! Rcore `bind scope` functions linking a frozen model. The same verified
//! scope image also runs over a FrozenStore through `SourceModelReads`; both
//! engines must return the same candidates.
use intersect_model::{
    core::{
        frozen::{ById, Diagnostics, FrozenStore, Metamodel, ModelBuilder, Violation},
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
use std::sync::Arc;

const STRUCTURE: &str = r#"rcore 1;
package example.frozenScopes version "1";
metamodel "urn:frozen-scopes" prefix m {
  class Namespace { ref types: Type [0,*] containment; }
  class Type { attr name: String [1,1]; }
  class Property {
    ref owner: Namespace [0,1];
    ref targetType: Type [0,1];
  }
}
"#;
const VISIBLE_TYPES: &str = r#"
behavior {
  import model "urn:frozen-scopes" as m;
  bind scope m.Property.targetType = visible_types;
  fn visible_types(owner:m.Property, spelling:String)->Sequence<m.Type> =
    match owner.owner {
      Some(ns) => Sequence(filter(ns.types, |t| t.name == spelling)),
      None => []
    };
}
"#;
/// A scope for `owner` that reads `owner` itself: a reference cycle.
const SELF_OWNER: &str = r#"
  bind scope m.Property.owner = own_owner;
  fn own_owner(p:m.Property, spelling:String)->Sequence<m.Namespace> =
    match p.owner {
      Some(ns) => Sequence([ns]),
      None => []
    };
}
"#;

struct Fixture {
    bundle: DefinitionBundle,
    schema: Schema,
    scopes: Vec<VerifiedScope>,
}
fn fixture(behavior: &str) -> Result<Fixture, String> {
    let source = DefinitionSource::new(
        "memory:frozen-scopes.rcore",
        SourceKind::Rcore,
        format!("{STRUCTURE}{behavior}"),
    );
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
    let schema = compile_structure_bundle(&bundle, &entry, "urn:frozen-scopes")
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

/// Namespaces `local` {Voltage, Current} and `other` {Voltage}, one Property.
/// `owner_by_proxy` records Property.owner as an unresolved name as well, so
/// the targetType scope must resolve it on demand.
fn build(f: &Fixture, owner_by_proxy: bool) -> Result<ModelBuilder, String> {
    let mm = Arc::new(Metamodel::freeze(&f.schema).map_err(err)?);
    let class = |name| mm.class(&key(&f.schema, name)?).ok_or("class".to_owned());
    let (namespace, ty, property) = (class("Namespace")?, class("Type")?, class("Property")?);
    let feature = |c, name: &str| mm.feature(c, name).ok_or(format!("feature {name}"));
    let types = feature(namespace, "types")?;
    let name = feature(ty, "name")?;
    let owner = feature(property, "owner")?;
    let target = feature(property, "targetType")?;
    let mut b = ModelBuilder::new(mm.clone());
    let p = b.create("property", property, "memory:r").map_err(err)?;
    // targetType is recorded before owner, so linking meets it first.
    b.add_proxy(p, target, "Voltage").map_err(err)?;
    let local = b.create("local", namespace, "memory:r").map_err(err)?;
    let other = b.create("other", namespace, "memory:r").map_err(err)?;
    for (id, ns, label) in [
        ("voltage", local, "Voltage"),
        ("current", local, "Current"),
        ("foreign", other, "Voltage"),
    ] {
        let t = b.create(id, ty, "memory:r").map_err(err)?;
        b.set_str(t, name, label).map_err(err)?;
        b.add_ref(ns, types, t).map_err(err)?;
    }
    if owner_by_proxy {
        b.add_proxy(p, owner, "local").map_err(err)?;
    } else {
        b.add_ref(p, owner, local).map_err(err)?;
    }
    Ok(b)
}

/// The same scope over a FrozenStore built with the checked storage API.
fn store_candidates(f: &Fixture) -> Result<Vec<String>, String> {
    let class = |name| key(&f.schema, name);
    let mut store = FrozenStore::for_schema(&f.schema).map_err(|e| e.to_string())?;
    storage::atomic(&mut store, |s| {
        for (id, c) in [
            ("property", "Property"),
            ("local", "Namespace"),
            ("other", "Namespace"),
            ("voltage", "Type"),
            ("current", "Type"),
            ("foreign", "Type"),
        ] {
            s.create_element(&f.schema, id, &class(c)?, "memory:r", 0, 0)?;
        }
        for (id, label) in [
            ("voltage", "Voltage"),
            ("current", "Current"),
            ("foreign", "Voltage"),
        ] {
            storage::set_with_opposite(s, &f.schema, id, "name", json!(label))?;
        }
        storage::set_with_opposite(
            s,
            &f.schema,
            "local",
            "types",
            json!(["voltage", "current"]),
        )?;
        storage::set_with_opposite(s, &f.schema, "other", "types", json!(["foreign"]))?;
        storage::set_with_opposite(s, &f.schema, "property", "owner", json!("local"))?;
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
        "Voltage",
        &ExecutionLimits::default(),
        &work,
    )
    .map(|c| c.into_identities())
    .map_err(err)
}

#[test]
fn rcore_scope_links_frozen_model_like_the_store_path() -> Result<(), String> {
    let f = fixture(VISIBLE_TYPES)?;
    let b = build(&f, false)?;
    let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &f.scopes)
        .map_err(err)?
        .with_execution(Execution::Shadow);
    let model = b.link(&scopes).map_err(err)?.validate().map_err(err)?;
    let stats = scopes.stats();
    assert_eq!(
        (stats.fast, stats.deferred),
        (1, 0),
        "direct executor ran and agreed"
    );
    let p = model.lookup("property").ok_or("property")?;
    let target = p.feature("targetType").ok_or("targetType")?;
    let resolved: Vec<_> = p.refs(target).map(|o| o.identity().to_owned()).collect();
    assert_eq!(resolved, ["voltage"]);
    assert_eq!(store_candidates(&f)?, resolved, "both engines agree");
    Ok(())
}

#[test]
fn scope_reads_resolve_other_references_on_demand() -> Result<(), String> {
    let f = fixture(VISIBLE_TYPES)?;
    let b = build(&f, true)?;
    // targetType's scope reads Property.owner, still a proxy; ById resolves it.
    let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &f.scopes)
        .map_err(err)?
        .with_fallback(&ById)
        .with_execution(Execution::Shadow);
    let model = b.link(&scopes).map_err(err)?.validate().map_err(err)?;
    let p = model.lookup("property").ok_or("property")?;
    let target = p.feature("targetType").ok_or("targetType")?;
    let owner = p.feature("owner").ok_or("owner")?;
    assert_eq!(
        p.refs(target).map(|o| o.identity()).collect::<Vec<_>>(),
        ["voltage"]
    );
    assert_eq!(
        p.refs(owner).map(|o| o.identity()).collect::<Vec<_>>(),
        ["local"]
    );
    Ok(())
}

#[test]
fn self_dependent_scope_is_reported_as_a_cycle() -> Result<(), String> {
    let behavior = VISIBLE_TYPES.trim_end().trim_end_matches('}').to_owned() + SELF_OWNER;
    let f = fixture(&behavior)?;
    assert_eq!(f.scopes.len(), 2);
    let b = build(&f, true)?;
    let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &f.scopes)
        .map_err(err)?
        .with_execution(Execution::Shadow);
    let Err(Diagnostics(problems)) = b.link(&scopes) else {
        return Err("A self-dependent scope must not link".into());
    };
    assert!(
        problems
            .iter()
            .any(|p| matches!(p, Violation::ReferenceCycle { feature, .. } if feature == "owner")),
        "{problems:?}"
    );
    Ok(())
}

#[test]
fn ambiguous_scope_result_is_refused() -> Result<(), String> {
    let f = fixture(VISIBLE_TYPES)?;
    let mut b = build(&f, false)?;
    // A second Voltage in the same namespace makes the result ambiguous.
    let mm = b.metamodel().clone();
    let ty = mm.class(&key(&f.schema, "Type")?).ok_or("Type")?;
    let ns = mm.class(&key(&f.schema, "Namespace")?).ok_or("Namespace")?;
    let local = mm.feature(ns, "types").ok_or("types")?;
    let name = mm.feature(ty, "name").ok_or("name")?;
    let extra = b.create("voltage2", ty, "memory:r").map_err(err)?;
    b.set_str(extra, name, "Voltage").map_err(err)?;
    let local_ns = b.lookup("local").ok_or("local")?;
    b.add_ref(local_ns, local, extra).map_err(err)?;
    let scopes = RcoreScopes::new(&mm, &f.bundle, &f.scopes)
        .map_err(err)?
        .with_execution(Execution::Shadow);
    assert!(b.link(&scopes).is_err());
    Ok(())
}

/// `hex`, `identity` and string `+` on the VM and the direct executor:
/// the type whose hex identity, with a prefix, matches the hex spelling.
const BY_HEX_IDENTITY: &str = r#"
behavior {
  import model "urn:frozen-scopes" as m;
  bind scope m.Property.targetType = by_identity;
  fn by_identity(owner:m.Property, spelling:String)->Sequence<m.Type> =
    match owner.owner {
      Some(ns) => Sequence(filter(ns.types,
        |t| "id:" + hex(identity(t)) == "id:" + hex("voltage") && t.name == spelling)),
      None => []
    };
}
"#;

#[test]
fn text_primitives_agree_on_both_executors() -> Result<(), String> {
    let f = fixture(BY_HEX_IDENTITY)?;
    let b = build(&f, false)?;
    let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &f.scopes)
        .map_err(err)?
        .with_execution(Execution::Shadow);
    let model = b.link(&scopes).map_err(err)?.validate().map_err(err)?;
    let stats = scopes.stats();
    assert_eq!(
        (stats.fast, stats.deferred),
        (1, 0),
        "both executors agreed"
    );
    let p = model.lookup("property").ok_or("property")?;
    let target = p.feature("targetType").ok_or("targetType")?;
    assert_eq!(
        p.refs(target).map(|o| o.identity()).collect::<Vec<_>>(),
        ["voltage"]
    );
    Ok(())
}

/// `distinct` and a helper called repeatedly with the same arguments (which
/// the direct executor answers from its per-run memo) on both executors.
const DISTINCT_REPEATED: &str = r#"
behavior {
  import model "urn:frozen-scopes" as m;
  bind scope m.Property.targetType = repeated;
  fn repeated(owner:m.Property, spelling:String)->Sequence<m.Type> =
    match owner.owner {
      Some(ns) => Sequence(named(ns, spelling).concat(named(ns, spelling)).distinct()),
      None => []
    };
  fn named(ns:m.Namespace, spelling:String)->Sequence<m.Type> =
    Sequence(filter(ns.types, |t| t.name == spelling));
}
"#;

#[test]
fn distinct_and_repeated_calls_agree_on_both_executors() -> Result<(), String> {
    let f = fixture(DISTINCT_REPEATED)?;
    let b = build(&f, false)?;
    let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &f.scopes)
        .map_err(err)?
        .with_execution(Execution::Shadow);
    let model = b.link(&scopes).map_err(err)?.validate().map_err(err)?;
    let stats = scopes.stats();
    assert_eq!(
        (stats.fast, stats.deferred),
        (1, 0),
        "both executors agreed"
    );
    let p = model.lookup("property").ok_or("property")?;
    let target = p.feature("targetType").ok_or("targetType")?;
    assert_eq!(
        p.refs(target).map(|o| o.identity()).collect::<Vec<_>>(),
        ["voltage"]
    );
    Ok(())
}

/// One batch for all bindings of a source (shared lowering and verification
/// checks) gives exactly the images that compiling each binding alone gives.
#[test]
fn batch_compilation_matches_single_compilation() -> Result<(), String> {
    use intersect_model::definitions::rcore_scope::compile_bindings_with_source_limits;
    let behavior = VISIBLE_TYPES.trim_end().trim_end_matches('}').to_owned()
        + "\n  bind scope m.Property.owner = no_owner;\n  fn no_owner(p:m.Property, spelling:String)->Sequence<m.Namespace> = [];\n}\n";
    let f = fixture(&behavior)?;
    assert_eq!(f.scopes.len(), 2);
    let source = &f.bundle.sources[0];
    let declarations: Vec<_> = source_scope_bindings(source, ParseLimits::default())
        .map_err(|e| format!("{e:?}"))?
        .iter()
        .map(|b| (b.declaration, false))
        .collect();
    let batch = compile_bindings_with_source_limits(
        &f.bundle,
        &source.id,
        &declarations,
        &Limits::default(),
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let image = |s: &VerifiedScope| format!("{:?}\n{:?}", s.binding(), s.executable());
    assert_eq!(
        batch.iter().map(image).collect::<Vec<_>>(),
        f.scopes.iter().map(image).collect::<Vec<_>>()
    );
    Ok(())
}

/// Compiles sharing one pair of schema stores, in sequence and from another
/// thread, give exactly the images of compiling with no shared stores.
#[test]
fn shared_schema_stores_match_unshared_compilation() -> Result<(), String> {
    use intersect_model::definitions::rcore_scope::{
        BindingKind, SchemaStores, compile_binding_kinds_with_schema_stores,
    };
    let f = fixture(VISIBLE_TYPES)?;
    let source = f.bundle.sources[0].clone();
    let declarations: Vec<_> = source_scope_bindings(&source, ParseLimits::default())
        .map_err(|e| format!("{e:?}"))?
        .iter()
        .map(|b| (b.declaration, BindingKind::Scope))
        .collect();
    let image = |s: &VerifiedScope| {
        format!(
            "{:?}
{:?}",
            s.binding(),
            s.executable()
        )
    };
    let expected = f.scopes.iter().map(image).collect::<Vec<_>>();
    let stores = SchemaStores::default();
    let compile = |stores: &SchemaStores| {
        compile_binding_kinds_with_schema_stores(
            &f.bundle,
            &source.id,
            &declarations,
            &Limits::default(),
            ParseLimits::default(),
            stores,
            &|_| {},
        )
        .map(|scopes| scopes.iter().map(image).collect::<Vec<_>>())
        .map_err(|e| format!("{e:?}"))
    };
    assert_eq!(compile(&stores)?, expected);
    assert!(!stores.lowering.is_empty() && !stores.verification.is_empty());
    let warmed = (stores.lowering.len(), stores.verification.len());
    assert_eq!(compile(&stores)?, expected);
    let threaded = std::thread::scope(|scope| {
        scope
            .spawn(|| compile(&stores))
            .join()
            .map_err(|_| "compile thread panicked".to_owned())
    })??;
    assert_eq!(threaded, expected);
    // Warm compiles import every preparation instead of adding new ones.
    assert_eq!((stores.lowering.len(), stores.verification.len()), warmed);
    Ok(())
}

/// Saved bindings load as exactly the compiled images, and only for the same
/// compiler identity, intact text and the exact bundle.
#[test]
fn saved_bindings_load_only_for_the_same_compiler_and_bundle() -> Result<(), String> {
    use intersect_model::definitions::rcore_scope::{load_bindings, save_bindings};
    let f = fixture(VISIBLE_TYPES)?;
    let saved = save_bindings(&f.scopes, "compiler-a")?;
    let loaded = load_bindings(&saved, &f.bundle, "compiler-a")?;
    let image = |s: &VerifiedScope| format!("{:?}\n{:?}", s.binding(), s.executable());
    assert_eq!(
        loaded.iter().map(image).collect::<Vec<_>>(),
        f.scopes.iter().map(image).collect::<Vec<_>>()
    );
    // The loaded images link like the compiled ones.
    let b = build(&f, false)?;
    let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &loaded).map_err(err)?;
    let model = b.link(&scopes).map_err(err)?.validate().map_err(err)?;
    let p = model.lookup("property").ok_or("property")?;
    let target = p.feature("targetType").ok_or("targetType")?;
    assert_eq!(
        p.refs(target).map(|o| o.identity()).collect::<Vec<_>>(),
        ["voltage"]
    );

    let refused = |result: Result<Vec<VerifiedScope>, String>, expected: &str| match result {
        Ok(_) => Err(format!("loaded despite: {expected}")),
        Err(message) if message.contains(expected) => Ok(()),
        Err(message) => Err(format!("expected '{expected}', got '{message}'")),
    };
    refused(
        load_bindings(&saved, &f.bundle, "compiler-b"),
        "another compiler",
    )?;
    // An edited verification fact: a query function claimed pure.
    let tampered = saved.replacen("\"query\"", "\"pure\"", 1);
    assert_ne!(tampered, saved);
    refused(load_bindings(&tampered, &f.bundle, "compiler-a"), "corrupt")?;
    let mut changed = f.bundle.clone();
    let source = &mut changed.sources[0];
    *source = DefinitionSource::new(
        &source.id.uri,
        SourceKind::Rcore,
        format!("{}\n", source.text),
    );
    refused(
        load_bindings(&saved, &changed, "compiler-a"),
        "other sources",
    )?;
    Ok(())
}

/// A scope that recurses 250 calls deep before it looks the name up.
const DEEP_RECURSION: &str = r#"
behavior {
  import model "urn:frozen-scopes" as m;
  bind scope m.Property.targetType = deep;
  fn deep(owner:m.Property, spelling:String)->Sequence<m.Type> = down(owner, spelling, 0);
  fn down(owner:m.Property, spelling:String, i:Int64)->Sequence<m.Type> =
    if i >= 250 {
      match owner.owner {
        Some(ns) => Sequence(filter(ns.types, |t| t.name == spelling)),
        None => []
      }
    } else { down(owner, spelling, i + 1) };
}
"#;

/// The direct executor's call depth is `ExecutionLimits::max_frames`
/// (default 256), and a run that deep fits a 1 MiB stack, the size of a
/// Windows main thread; a lower limit defers the same run to the VM.
#[test]
fn direct_runs_to_the_frame_limit_on_a_one_mebibyte_stack() -> Result<(), String> {
    let deep = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| -> Result<(usize, usize, Vec<String>), String> {
            let f = fixture(DEEP_RECURSION)?;
            let b = build(&f, false)?;
            let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &f.scopes)
                .map_err(err)?
                .with_execution(Execution::Fast);
            let model = b.link(&scopes).map_err(err)?.validate().map_err(err)?;
            let p = model.lookup("property").ok_or("property")?;
            let target = p.feature("targetType").ok_or("targetType")?;
            let found = p.refs(target).map(|o| o.identity().to_owned()).collect();
            let stats = scopes.stats();
            Ok((stats.fast, stats.deferred, found))
        })
        .map_err(err)?
        .join()
        .map_err(|_| "the deep run overflowed or panicked".to_owned())??;
    assert_eq!(deep, (1, 0, vec!["voltage".to_owned()]));

    let f = fixture(DEEP_RECURSION)?;
    let b = build(&f, false)?;
    let mut limits = RcoreScopes::default_limits();
    limits.max_frames = 64;
    let scopes = RcoreScopes::new(b.metamodel(), &f.bundle, &f.scopes)
        .map_err(err)?
        .with_limits(limits)
        .with_execution(Execution::Fast);
    let _ = b.link(&scopes);
    assert_eq!(scopes.stats().deferred_by_reason, [("call depth", 1)]);
    Ok(())
}
