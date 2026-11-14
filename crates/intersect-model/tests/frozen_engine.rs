//! Differential checks between the frozen linker and checked model editing.
use intersect_model::core::frozen::*;
use intersect_model::core::{
    api::{BasicModel, ModelEdit},
    model as materialized, storage,
};
use intersect_model::definitions::schema::Schema;
use serde_json::{Value, json};
use std::sync::Arc;

const SOURCE: &str = r#"rcore 1;
package frozen version "1";
metamodel "urn:intersect:frozen" prefix f {
  abstract class Named { attr name: String [1,1] default ""; }
  class Node extends Named {
    ref links: Node [0,*];
    ref children: Named [0,*] containment;
    ref out: Node [0,*] opposite Node.incoming;
    ref incoming: Node [0,*] opposite Node.out;
    ref buddy: Node [0,1];
  }
  class Leaf extends Named { attr note: String [0,1]; }
}"#;
const R0: &str = "memory:/frozen/r0";
const R1: &str = "memory:/frozen/r1";

fn fixture_schema() -> Schema {
    match intersect_model::compile_rcore_at("memory:/frozen.rcore", SOURCE) {
        Ok(package) => package.schema().clone(),
        Err(error) => panic!("fixture compiles: {error}"),
    }
}
fn key(schema: &Schema, name: &str) -> String {
    // Runtime class keys are opaque; declarations carry the authored name.
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| panic!("class {name}"))
}
fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|e| panic!("{e}"))
}

/// Identical graph through both engines: names, a link chain, a containment
/// star, opposite out/incoming, leaves, two resources.
const N: usize = 40;
fn id(i: usize) -> String {
    format!("n{i:03}")
}
fn oracle(schema: &Schema) -> materialized::Model {
    let (node, leaf) = (key(schema, "Node"), key(schema, "Leaf"));
    let mut model = ok(BasicModel::new(schema.clone()));
    for i in 0..N {
        ok(model.create(&id(i), &node, if i % 2 == 0 { R0 } else { R1 }));
    }
    ok(model.create("leaf", &leaf, R0));
    let ids: Vec<String> = (0..N).map(id).collect();
    let mut edits: Vec<(&str, &str, Value)> =
        ids.iter().map(|i| (i.as_str(), "name", json!(i))).collect();
    for i in 0..N - 1 {
        edits.push((&ids[i], "links", json!([ids[i + 1]])));
        edits.push((&ids[i], "out", json!([ids[i + 1]])));
    }
    edits.push((
        &ids[0],
        "children",
        json!(
            ids[1..10]
                .iter()
                .chain(["leaf".to_owned()].iter())
                .collect::<Vec<_>>()
        ),
    ));
    edits.push(("leaf", "name", json!("leaf")));
    edits.push((&ids[3], "buddy", json!(ids[7])));
    ok(model.set_many(&edits));
    ok(model.validate());
    ok(storage::export(&model.into_store()))
}
fn frozen(mm: &Arc<Metamodel>) -> Result<Model, Diagnostics> {
    let node = mm
        .class(&key(mm.schema(), "Node"))
        .ok_or_else(|| Diagnostics(vec![]))?;
    let leaf = mm
        .class(&key(mm.schema(), "Leaf"))
        .ok_or_else(|| Diagnostics(vec![]))?;
    let f = |name: &str| mm.feature(node, name).ok_or_else(|| Diagnostics(vec![]));
    let (name, links, children, out, buddy) = (
        f("name")?,
        f("links")?,
        f("children")?,
        f("out")?,
        f("buddy")?,
    );
    let err = |e: FrozenError| Diagnostics(vec![Violation::Resolver(e.to_string())]);
    let mut b = ModelBuilder::new(mm.clone());
    let objs: Vec<ObjId> = (0..N)
        .map(|i| b.create(&id(i), node, if i % 2 == 0 { R0 } else { R1 }))
        .collect::<Result<_, _>>()
        .map_err(err)?;
    let leaf_obj = b.create("leaf", leaf, R0).map_err(err)?;
    for (i, obj) in objs.iter().enumerate() {
        b.set_str(*obj, name, &id(i)).map_err(err)?;
    }
    for i in 0..N - 1 {
        // Links resolve by name through a phase-local index; out by object handle.
        b.add_proxy(objs[i], links, &id(i + 1)).map_err(err)?;
        b.add_ref(objs[i], out, objs[i + 1]).map_err(err)?;
    }
    for child in &objs[1..10] {
        b.add_ref(objs[0], children, *child).map_err(err)?;
    }
    b.add_ref(objs[0], children, leaf_obj).map_err(err)?;
    let leaf_name = mm
        .feature(leaf, "name")
        .ok_or_else(|| Diagnostics(vec![]))?;
    b.set_str(leaf_obj, leaf_name, "leaf").map_err(err)?;
    b.add_ref(objs[3], buddy, objs[7]).map_err(err)?;
    b.link(&ByName::new(name))?.validate()
}
fn sorted(mut model: materialized::Model) -> materialized::Model {
    model.elements.sort_by(|a, b| a.id.cmp(&b.id));
    model
}
fn roots(model: &materialized::Model) -> Vec<(String, Vec<String>)> {
    model
        .resources
        .iter()
        .map(|(k, r)| (k.clone(), r.roots.to_vec()))
        .collect()
}

#[test]
fn freeze_compiles_dense_tables() {
    let schema = fixture_schema();
    let mm = ok(Metamodel::freeze(&schema));
    let node = mm
        .class(&key(&schema, "Node"))
        .unwrap_or_else(|| panic!("Node"));
    let leaf = mm
        .class(&key(&schema, "Leaf"))
        .unwrap_or_else(|| panic!("Leaf"));
    let named = mm
        .class(&key(&schema, "Named"))
        .unwrap_or_else(|| panic!("Named"));
    let name = mm.feature(node, "name").unwrap_or_else(|| panic!("name"));
    // Inherited features share one global identity across subclasses.
    assert_eq!(Some(name), mm.feature(leaf, "name"));
    assert!(mm.slot(node, name).is_some() && mm.slot(leaf, name).is_some());
    let links = mm.feature(node, "links").unwrap_or_else(|| panic!("links"));
    assert_eq!(None, mm.slot(leaf, links));
    let children = mm
        .feature(node, "children")
        .unwrap_or_else(|| panic!("children"));
    assert!(mm.conforms(leaf, children) && mm.conforms(node, children));
    assert!(mm.conforms(node, links) && !mm.conforms(leaf, links));
    let out = mm.feature(node, "out").unwrap_or_else(|| panic!("out"));
    let incoming = mm
        .feature(node, "incoming")
        .unwrap_or_else(|| panic!("incoming"));
    assert_eq!(
        mm.feature_info(out).and_then(|f| f.opposite),
        Some(incoming)
    );
    assert!(mm.class_info(named).is_some_and(|c| c.abstract_type));
}

#[test]
fn builder_graph_matches_oracle_exactly() {
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let expected = oracle(&schema);
    let actual = ok(ok(frozen(&mm)).to_model());
    assert_eq!(
        sorted(actual.clone()).elements,
        sorted(expected.clone()).elements
    );
    assert_eq!(roots(&actual), roots(&expected));
    // The existing full structural validator accepts the frozen export.
    ok(actual.validate(&schema));
}

#[test]
fn import_round_trip_is_lossless() {
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let expected = oracle(&schema);
    let model = ok(Model::from_model(mm, &expected));
    let mut actual = ok(model.to_model());
    // The export also marks assignments that equal the schema default
    // (`name: ""`), which element values alone cannot express; every value
    // and resource is otherwise unchanged.
    for element in &mut actual.elements {
        if let Some(state) = element.assignment_state.take() {
            assert!(state.set_features().all(|f| element.values.contains_key(f)));
        }
    }
    assert_eq!(actual.elements, expected.elements);
    assert_eq!(actual.resources, expected.resources);
}

#[test]
fn reads_need_no_checks_and_inverse_is_complete() {
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let model = ok(frozen(&mm));
    let n0 = model.lookup("n000").unwrap_or_else(|| panic!("n000"));
    let name = n0.feature("name").unwrap_or_else(|| panic!("name"));
    let incoming = n0.feature("incoming").unwrap_or_else(|| panic!("incoming"));
    let children = n0.feature("children").unwrap_or_else(|| panic!("children"));
    assert_eq!(n0.str(name), Some("n000"));
    let n1 = model.lookup("n001").unwrap_or_else(|| panic!("n001"));
    // Opposite completed during link.
    assert_eq!(
        n1.refs(incoming).map(|o| o.identity()).collect::<Vec<_>>(),
        ["n000"]
    );
    assert_eq!(
        n1.container().map(|(o, f)| (o.identity(), f)),
        Some(("n000", children))
    );
    assert_eq!(n0.refs(children).count(), 10);
    let inverse = model.inverse();
    let links = n0.feature("links").unwrap_or_else(|| panic!("links"));
    assert!(inverse.referrers(n1.id()).contains(&(n0.id(), links)));
    // links + out + incoming chains, ten children, one buddy.
    assert_eq!(inverse.edge_count(), 3 * (N - 1) + 10 + 1);
}

/// Each invalid edit must be refused by both the oracle and the frozen engine.
#[test]
fn violations_agree_with_oracle() {
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let base = oracle(&schema);
    let set = |model: &mut materialized::Model, id: &str, field: &str, value: Value| {
        let element = model
            .elements
            .iter_mut()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("{id}"));
        element.values.insert(field.into(), value);
    };
    type Mutation = Box<dyn Fn(&mut materialized::Model)>;
    let cases: Vec<(&str, Mutation)> = vec![
        (
            "lower bound",
            Box::new(move |m| set(m, "n005", "name", Value::Null)),
        ),
        (
            "upper bound",
            Box::new(move |m| set(m, "n005", "buddy", json!(["n001", "n002"]))),
        ),
        (
            "target type",
            Box::new(move |m| set(m, "n005", "links", json!(["leaf"]))),
        ),
        (
            "unresolved",
            Box::new(move |m| set(m, "n005", "links", json!(["missing"]))),
        ),
        (
            "duplicate",
            Box::new(move |m| set(m, "n020", "links", json!(["n001", "n001"]))),
        ),
        (
            "double container",
            Box::new(move |m| set(m, "n020", "children", json!(["n002"]))),
        ),
        (
            "containment cycle",
            Box::new(move |m| {
                set(m, "n020", "children", json!(["n021"]));
                set(m, "n021", "children", json!(["n020"]));
            }),
        ),
        (
            "missing opposite",
            Box::new(move |m| set(m, "n030", "out", json!(["n005", "n031"]))),
        ),
        (
            "attribute kind",
            Box::new(move |m| set(m, "n005", "name", json!(7))),
        ),
        (
            "abstract",
            Box::new(move |m| {
                if let Some(e) = m.elements.iter_mut().find(|e| e.id == "leaf") {
                    e.class = key(&fixture_schema(), "Named");
                    e.type_identity = fixture_schema().identity(&e.class);
                    e.values.remove("note");
                }
            }),
        ),
    ];
    for (label, mutate) in cases {
        let mut bad = base.clone();
        mutate(&mut bad);
        assert!(
            bad.validate(&schema).is_err(),
            "oracle must refuse: {label}"
        );
        assert!(
            Model::from_model(mm.clone(), &bad).is_err(),
            "frozen must refuse: {label}"
        );
    }
}

#[test]
fn link_reports_every_unresolved_and_ambiguous_name() {
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let node = mm
        .class(&key(&schema, "Node"))
        .unwrap_or_else(|| panic!("Node"));
    let name = mm.feature(node, "name").unwrap_or_else(|| panic!("name"));
    let links = mm.feature(node, "links").unwrap_or_else(|| panic!("links"));
    let mut b = ModelBuilder::new(mm.clone());
    let a = ok(b.create("a", node, R0));
    let c = ok(b.create("c", node, R0));
    ok(b.set_str(a, name, "same"));
    ok(b.set_str(c, name, "same"));
    ok(b.add_proxy(a, links, "nope1"));
    ok(b.add_proxy(a, links, "nope2"));
    let Err(Diagnostics(problems)) = b.link(&ByName::new(name)) else {
        panic!("unresolved names must fail")
    };
    assert_eq!(problems.len(), 2);

    let mut b = ModelBuilder::new(mm);
    let a = ok(b.create("a", node, R0));
    let c = ok(b.create("c", node, R0));
    ok(b.set_str(a, name, "same"));
    ok(b.set_str(c, name, "same"));
    ok(b.add_proxy(a, links, "same"));
    assert!(b.link(&ByName::new(name)).is_err(), "ambiguous name");
}

#[test]
fn builder_refuses_shape_errors_immediately() {
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let node = mm
        .class(&key(&schema, "Node"))
        .unwrap_or_else(|| panic!("Node"));
    let leaf = mm
        .class(&key(&schema, "Leaf"))
        .unwrap_or_else(|| panic!("Leaf"));
    let named = mm
        .class(&key(&schema, "Named"))
        .unwrap_or_else(|| panic!("Named"));
    let links = mm.feature(node, "links").unwrap_or_else(|| panic!("links"));
    let name = mm.feature(node, "name").unwrap_or_else(|| panic!("name"));
    let mut b = ModelBuilder::new(mm);
    assert!(matches!(
        b.create("x", named, R0),
        Err(FrozenError::Abstract(_))
    ));
    let l = ok(b.create("l", leaf, R0));
    assert!(matches!(
        b.create("l", leaf, R0),
        Err(FrozenError::Identity(_))
    ));
    assert!(matches!(
        b.add_ref(l, links, l),
        Err(FrozenError::NotAFeature { .. })
    ));
    assert!(matches!(b.add_ref(l, name, l), Err(FrozenError::Kind(_))));
}

#[test]
fn import_carries_assignment_state_unchanged() {
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let mut model = oracle(&schema);
    if let Some(e) = model.elements.first_mut() {
        e.assignment_state = Some(ok(materialized::AssignmentState::new(Vec::<String>::new())));
    }
    let exported = ok(ok(Model::from_model(mm, &model)).to_model());
    assert_eq!(exported.elements, model.elements);
}

/// FrozenStore journals nested transactions: rollback restores rewritten
/// objects and drops created ones; commit merges into the parent frame.
#[test]
fn frozen_store_nested_rollback_and_commit() {
    use intersect_model::core::storage::{ModelRead, ModelStore};
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let node = key(&schema, "Node");
    // Both public names denote the same mutable store, including checkpoints.
    let mut store: FrozenStore = ArenaStore::new(mm);
    let outer = ok(store.begin());
    ok(store.create_element(&schema, "a", &node, R0, 0, 1));
    ok(store.set_value("a", "name", Some(json!("first"))));
    let inner = ok(store.begin());
    ok(store.set_value("a", "name", Some(json!("second"))));
    ok(store.create_element(&schema, "b", &node, R0, 0, 1));
    // A reference to an identity created later in the same parse.
    ok(store.set_value("a", "links", Some(json!(["c"]))));
    ok(store.rollback(&inner));
    let a = ok(store.get("a")).unwrap_or_else(|| panic!("a"));
    assert_eq!(a.values.get("name"), Some(&json!("first")));
    assert_eq!(a.values.get("links"), Some(&json!([])));
    assert!(ok(store.get("b")).is_none(), "rolled-back creation is gone");

    let inner = ok(store.begin());
    ok(store.set_value("a", "links", Some(json!(["c"]))));
    ok(store.create_element(&schema, "c", &node, R0, 0, 1));
    ok(store.commit(&inner));
    ok(store.commit(&outer));
    let model = ok(ok(store.into_builder()).link(&ById)).validate();
    let model = ok(model);
    let a = model.lookup("a").unwrap_or_else(|| panic!("a"));
    let links = a.feature("links").unwrap_or_else(|| panic!("links"));
    assert_eq!(
        a.refs(links).map(|o| o.identity()).collect::<Vec<_>>(),
        ["c"]
    );
}

#[test]
fn frozen_store_removal_keeps_rows_and_nested_identity_rollback_consistent() {
    use intersect_model::core::storage::{ModelRead, ModelStore, Query};
    let schema = fixture_schema();
    let node = key(&schema, "Node");
    let mut store = ok(FrozenStore::for_schema(&schema));
    ok(storage::atomic(&mut store, |store| {
        for id in ["owner", "child", "observer"] {
            store.create_element(&schema, id, &node, R0, 0, 1)?;
        }
        store.set_value("owner", "children", Some(json!(["child"])))?;
        store.set_value("observer", "links", Some(json!(["owner"])))
    }));
    let original = ok(storage::export(&store));
    let outer = ok(store.begin());
    let child = ok(store.begin());
    ok(store.remove("owner"));
    assert_eq!(ok(store.query(Query::Class(&node))), ["child", "observer"]);
    assert_eq!(ok(store.page(None, 1)).first().map(|row| row.0), Some(1));
    let resource = ok(store.resource_record(R0)).unwrap();
    assert!(resource.roots.iter().any(|id| id == "child"));
    assert!(!resource.roots.iter().any(|id| id == "owner"));
    ok(store.create_element(&schema, "owner", &node, R0, 2, 3));
    ok(store.commit(&child));
    ok(store.rollback(&outer));
    assert_eq!(ok(storage::export(&store)), original);

    ok(storage::atomic(&mut store, |store| {
        store.remove("owner")?;
        store.create_element(&schema, "owner", &node, R0, 2, 3)
    }));
    let model = ok(ok(ok(store.into_builder()).link(&ById)).validate());
    assert_eq!(model.len(), 3);
    let observer = model.lookup("observer").unwrap();
    let links = observer.feature("links").unwrap();
    assert_eq!(
        observer
            .refs(links)
            .map(|o| o.identity())
            .collect::<Vec<_>>(),
        ["owner"]
    );
}

#[test]
fn frozen_store_class_replacement_keeps_row_and_restores_retired_layout_on_rollback() {
    use intersect_model::core::storage::{ModelRead, ModelStore};
    let schema = fixture_schema();
    let node = key(&schema, "Node");
    let leaf = key(&schema, "Leaf");
    let mut store = ok(FrozenStore::for_schema(&schema));
    ok(storage::atomic(&mut store, |store| {
        store.create_element(&schema, "changing", &node, R0, 0, 1)?;
        store.set_value("changing", "links", Some(json!(["unresolved-old-slot"])))
    }));
    let original = ok(storage::export(&store));
    let mut replacement = ok(materialized::Element::new(
        &schema, "changing", &leaf, R1, 2, 3,
    ));
    replacement
        .values
        .insert("note".into(), json!("new layout"));

    let outer = ok(store.begin());
    let mut invalid = replacement.clone();
    invalid.values.insert("unknown".into(), json!(true));
    assert!(store.put(invalid).is_err());
    assert_eq!(ok(storage::export(&store)), original);
    ok(store.put(replacement.clone()));
    assert_eq!(ok(store.page(None, 1)), [(0, replacement.clone())]);
    ok(store.rollback(&outer));
    assert_eq!(ok(storage::export(&store)), original);

    ok(storage::atomic(&mut store, |store| store.put(replacement)));
    let model = ok(ok(ok(store.into_builder()).link(&ById)).validate());
    let exported = ok(model.to_model());
    let element = exported.elements.first().unwrap();
    assert_eq!(element.class, leaf);
    assert_eq!(element.resource, R1);
    assert_eq!(element.values.get("note"), Some(&json!("new layout")));
}

#[test]
fn frozen_store_packs_only_live_references_and_strings_for_linking() {
    use intersect_model::core::storage::ModelStore;
    let schema = fixture_schema();
    let node = key(&schema, "Node");
    let mut store = ok(FrozenStore::for_schema(&schema));
    ok(storage::atomic(&mut store, |store| {
        store.create_element(&schema, "live-source", &node, R0, 0, 0)?;
        store.create_element(&schema, "live-target", &node, R0, 0, 0)?;
        store.set_value(
            "live-source",
            "links",
            Some(json!(["retired-identity", "?retired-scope"])),
        )?;
        store.set_value("live-source", "links", Some(json!(["live-target"])))
    }));
    let checkpoint = ok(store.begin());
    ok(store.create_element(&schema, "aborted-identity", &node, "aborted-resource", 0, 0));
    ok(store.rollback(&checkpoint));

    let model = ok(ok(ok(store.into_builder()).link(&ById)).validate());
    let source = model.lookup("live-source").unwrap();
    let links = source.feature("links").unwrap();
    assert_eq!(
        source.refs(links).map(|o| o.identity()).collect::<Vec<_>>(),
        ["live-target"]
    );
    let snapshot = ok(model.to_snapshot(&[0; 32]));
    let contains = |text: &[u8]| snapshot.windows(text.len()).any(|bytes| bytes == text);
    assert!(contains(b"live-target"));
    for retired in [
        "retired-identity",
        "retired-scope",
        "aborted-identity",
        "aborted-resource",
    ] {
        assert!(!contains(retired.as_bytes()), "retained {retired}");
    }
}

#[test]
fn frozen_store_materialized_wire_roundtrip_preserves_metadata_and_order() {
    use intersect_model::core::{
        resources::ResourceRecord,
        storage::{ModelRead, ModelStore},
    };
    let schema = fixture_schema();
    let mut source = oracle(&schema);
    let first = source.elements.first_mut().unwrap();
    first.implicit = true;
    let id = first.id.clone();
    let empty_uri = "memory:/frozen/empty";
    source
        .resources
        .insert(empty_uri.into(), ok(ResourceRecord::new(empty_uri)));
    let expected = ok(serde_json::to_vec(&source));

    let mut store = ok(FrozenStore::from_materialized(&schema, &source));
    let checkpoint = ok(store.begin());
    let mut changed = ok(store.get(&id)).unwrap();
    changed.implicit = false;
    ok(store.put(changed));
    assert!(!ok(store.get(&id)).unwrap().implicit);
    ok(store.rollback(&checkpoint));
    assert_eq!(ok(serde_json::to_vec(&ok(store.into_model()))), expected);

    let mut active = ok(FrozenStore::from_materialized(&schema, &source));
    let _checkpoint = ok(active.begin());
    assert!(active.into_model().is_err());

    source.elements.first_mut().unwrap().type_identity = "foreign#Node".into();
    assert!(
        matches!(FrozenStore::from_materialized(&schema, &source), Err(error) if error.contains("type identity"))
    );
}

const OPTIONAL_ENUM: &str = r#"rcore 1;
package frozenOptional version "1";
metamodel "urn:intersect:frozen-optional" prefix o {
  enum Mode { Day = 1; Night = 2; }
  class Node {
    attr mode: Mode [0,1];
    attr kind: Mode [1,1];
    ref buddy: Node [0,1];
  }
}"#;

/// While linking, an optional attribute nothing assigned reads as null rather
/// than its metamodel default (the enum's first literal), so a scope can tell
/// it from an assigned one; a required attribute still reads its default.
#[test]
fn link_view_reads_unassigned_optional_attributes_as_null() {
    use std::cell::RefCell;
    let schema =
        match intersect_model::compile_rcore_at("memory:/frozen-optional.rcore", OPTIONAL_ENUM) {
            Ok(package) => package.schema().clone(),
            Err(error) => panic!("fixture compiles: {error}"),
        };
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let node = mm
        .class(&key(&schema, "Node"))
        .unwrap_or_else(|| panic!("Node"));
    let feature = |name: &str| mm.feature(node, name).unwrap_or_else(|| panic!("{name}"));
    let (mode, kind, buddy) = (feature("mode"), feature("kind"), feature("buddy"));
    let mut b = ModelBuilder::new(mm.clone());
    let set = ok(b.create("set", node, R0));
    let unset = ok(b.create("unset", node, R0));
    ok(b.set_attribute(set, mode, json!({"kind": "enum", "literal": "Night"})));
    ok(b.add_proxy(unset, buddy, "set"));
    let seen = RefCell::new(Vec::new());
    let resolver = |view: &LinkView<'_>, owner: ObjId, _: FeatureId, _: &str| {
        let describe = |obj, feature| match view.value(obj, feature) {
            Ok(Some(LinkValue::Null)) => "null",
            Ok(Some(LinkValue::Default(_))) => "default",
            Ok(Some(LinkValue::Json(_))) => "assigned",
            _ => "other",
        };
        seen.borrow_mut().extend([
            describe(owner, mode),
            describe(owner, kind),
            describe(set, mode),
        ]);
        Ok(Some(set))
    };
    ok(b.link(&resolver)
        .map_err(|Diagnostics(problems)| format!("{problems:?}")));
    assert_eq!(seen.into_inner(), ["null", "default", "assigned"]);
}

/// An optional attribute explicitly set to its schema default survives an
/// element-level export and import as an assignment: the export lists it in
/// the element's assignment state, which `put` honours, so exporting again
/// still lists it. An attribute that was never set exports no state and stays
/// unassigned (the link view then reads it as null; see
/// `link_view_reads_unassigned_optional_attributes_as_null`).
#[test]
fn assigned_defaults_survive_export_and_import() {
    use intersect_model::core::storage::{ModelRead, ModelStore};
    let schema =
        match intersect_model::compile_rcore_at("memory:/frozen-optional.rcore", OPTIONAL_ENUM) {
            Ok(package) => package.schema().clone(),
            Err(error) => panic!("fixture compiles: {error}"),
        };
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let node_key = key(&schema, "Node");
    let node = mm.class(&node_key).unwrap_or_else(|| panic!("Node"));
    let mode = mm.feature(node, "mode").unwrap_or_else(|| panic!("mode"));
    let default = mm
        .definition(mode)
        .map(|d| d.default.clone())
        .unwrap_or_else(|| panic!("mode default"));
    let lists_mode = |e: &materialized::Element| {
        e.assignment_state
            .as_ref()
            .is_some_and(|s| s.explicitly_set("mode"))
    };

    let mut first = FrozenStore::new(mm.clone());
    ok(storage::atomic(&mut first, |store| {
        store.create_element(&schema, "set", &node_key, R0, 0, 1)?;
        store.create_element(&schema, "unset", &node_key, R0, 0, 1)?;
        store.set_value("set", "mode", Some(default))
    }));
    let set = ok(first.get("set")).unwrap_or_else(|| panic!("set"));
    let unset = ok(first.get("unset")).unwrap_or_else(|| panic!("unset"));
    assert!(lists_mode(&set), "an assigned default is listed");
    assert_eq!(unset.assignment_state, None, "values say it all");

    let mut second = FrozenStore::new(mm);
    ok(storage::atomic(&mut second, |store| {
        store.put(set)?;
        store.put(unset)
    }));
    let set = ok(second.get("set")).unwrap_or_else(|| panic!("set"));
    let unset = ok(second.get("unset")).unwrap_or_else(|| panic!("unset"));
    assert!(lists_mode(&set), "still assigned after import");
    assert_eq!(
        unset.assignment_state, None,
        "still unassigned after import"
    );
}

/// Containment survives an element-level export and import: the reimported
/// child still has its container after linking and validation.
#[test]
fn reimported_children_keep_their_container() {
    use intersect_model::core::storage::ModelStore;
    let schema = fixture_schema();
    let mm = Arc::new(ok(Metamodel::freeze(&schema)));
    let node = key(&schema, "Node");
    let leaf = key(&schema, "Leaf");
    let mut first = FrozenStore::new(mm.clone());
    ok(storage::atomic(&mut first, |store| {
        store.create_element(&schema, "parent", &node, R0, 0, 1)?;
        store.create_element(&schema, "child", &leaf, R0, 0, 1)?;
        store.set_value("parent", "children", Some(json!(["child"])))
    }));
    let elements = ok(storage::export(&first)).elements;
    let mut second = FrozenStore::new(mm);
    ok(storage::atomic(&mut second, |store| {
        for element in &elements {
            store.put(element.clone())?;
        }
        Ok(())
    }));
    let model = ok(ok(ok(second.into_builder()).link(&ById)).validate());
    let child = model.lookup("child").unwrap_or_else(|| panic!("child"));
    assert_eq!(child.container().map(|(c, _)| c.identity()), Some("parent"));
    let exported = ok(model.to_model());
    let roots: Vec<_> = exported
        .resources
        .values()
        .flat_map(|r| r.roots.iter().cloned())
        .collect();
    assert_eq!(roots, ["parent"]);
}
