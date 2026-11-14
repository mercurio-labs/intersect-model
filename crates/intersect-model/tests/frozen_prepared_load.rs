//! Handwritten: loading a few project objects on top of a prepared model
//! (`FrozenStore::from_model`) gives the same model whether the load keeps
//! the prepared objects in place (`FrozenStore::into_builder`) or redoes the
//! whole-model work (`into_builder_repacked`), and a measurement of what
//! each load phase costs on a library-sized model either way.
use intersect_model::{
    core::{
        frozen::{
            ById, ClassId, DefaultItem, Defaults, FeatureId, FrozenError, FrozenStore,
            ImplicitObject, Implicits, Issue, LinkValue, LinkView, Metamodel, Model, ModelBuilder,
            ObjId, Validators,
        },
        model as materialized,
        resources::ResourceRecord,
        storage::{self, ModelRead, ModelStore, StorageResult},
    },
    definitions::schema::Schema,
};
use serde_json::json;
use std::{cell::RefCell, sync::Arc, time::Instant};

const SOURCE: &str = r#"rcore 1; package example.preparedLoad version "1";
metamodel "urn:prepared-load" prefix p {
  class Node {
    attr name: String [0,1];
    ref children: Node [0,*] containment opposite Node.parent;
    ref parent: Node [0,1] opposite Node.children;
    ref extras: Node [0,*] containment;
    ref out: Node [0,*] opposite Node.incoming;
    ref incoming: Node [0,*] opposite Node.out;
    ref uses: Node [0,*];
    ref only: Node [0,1] opposite Node.onlyOf;
    ref onlyOf: Node [0,1] opposite Node.only;
  }
  class Leaf extends Node { attr weight: Int32 [0,1]; }
  class Strict extends Node { ref must: Node [1,1]; }
}"#;
const PROJECT: &str = "memory:project";

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}

struct Fixture {
    schema: Schema,
    mm: Arc<Metamodel>,
    node: String,
    leaf: String,
    strict: String,
}

fn class_key(schema: &Schema, name: &str) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("Missing class {name}"))
}

fn fixture() -> Result<Fixture, String> {
    let schema = intersect_model::compile_rcore_at("memory:/prepared-load.rcore", SOURCE)
        .map_err(err)?
        .schema()
        .clone();
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(err)?);
    Ok(Fixture {
        node: class_key(&schema, "Node")?,
        leaf: class_key(&schema, "Leaf")?,
        strict: class_key(&schema, "Strict")?,
        schema,
        mm,
    })
}

impl Fixture {
    fn class(&self, key: &str) -> Result<ClassId, String> {
        self.mm.class(key).ok_or_else(|| key.to_owned())
    }
    fn feature(&self, name: &str) -> Result<FeatureId, String> {
        let class = self.class(if name == "must" {
            &self.strict
        } else {
            &self.node
        })?;
        self.mm.feature(class, name).ok_or_else(|| name.to_owned())
    }
}

fn lib(index: usize) -> String {
    format!("lib{index:06}")
}

/// A linked model of `groups` trees, one resource per hundred trees: a root
/// `Node` with `width - 1` `Leaf` children, each object pointing `out` at the
/// next one. Opposites (`parent`, `incoming`) are completed by `link`.
fn library(f: &Fixture, groups: usize, width: usize) -> Result<Model, String> {
    let (node, leaf) = (f.class(&f.node)?, f.class(&f.leaf)?);
    let (name, children, out) = (
        f.feature("name")?,
        f.feature("children")?,
        f.feature("out")?,
    );
    let count = groups * width;
    let mut b = ModelBuilder::with_capacity(f.mm.clone(), count);
    let mut objects = Vec::with_capacity(count);
    for index in 0..count {
        let class = if index % width == 0 { node } else { leaf };
        let resource = format!("memory:library/{}", index / width % 100);
        let obj = b.create(&lib(index), class, &resource).map_err(err)?;
        b.set_str(obj, name, &format!("n{}", index % 997))
            .map_err(err)?;
        objects.push(obj);
    }
    for index in 0..count {
        if index % width != 0 {
            b.add_ref(objects[index - index % width], children, objects[index])
                .map_err(err)?;
        }
        if index + 1 < count {
            b.add_ref(objects[index], out, objects[index + 1])
                .map_err(err)?;
        }
    }
    b.link(&ById).map_err(err)?.validate().map_err(err)
}

/// One edit of a prepared store before it is linked.
type Edit<'a> = &'a dyn Fn(&Fixture, &mut FrozenStore) -> StorageResult<()>;

/// `count` project objects: `proj0` contains the rest, every one points
/// `out` at a library object (so `incoming` is written into a library row)
/// and names another library object by spelling, resolved by `link`.
fn add_project(f: &Fixture, store: &mut FrozenStore, count: usize) -> StorageResult<()> {
    let schema = &f.schema;
    storage::atomic(store, |s| {
        storage::bind_schema(s, schema)?;
        let ids: Vec<String> = (0..count).map(|i| format!("proj{i}")).collect();
        for (index, id) in ids.iter().enumerate() {
            s.create_element(schema, id, &f.node, PROJECT, index, index + 1)?;
            s.set_value(id, "name", Some(json!(format!("project {index}"))))?;
            s.set_value(id, "out", Some(json!([lib(index * 5 + 1)])))?;
            s.set_value(id, "uses", Some(json!([format!("?{}", lib(index * 3))])))?;
        }
        s.set_value("proj0", "children", Some(json!(&ids[1..])))
    })
}

/// What every phase's view shows: recorded by the implicit, default and
/// validation hooks for each object they are asked about.
#[derive(Default)]
struct Seen(RefCell<Vec<String>>);
impl Seen {
    fn note(&self, phase: &str, view: &LinkView<'_>, obj: ObjId) {
        if view.len() > 10_000 {
            // The measurement: read what a scope reads, record nothing.
            let read = view.roots().len() + view.children(obj).len();
            assert!(read > 0 && view.container(obj).is_none_or(|(o, _)| o != obj));
            return;
        }
        let id = |o: ObjId| view.identity(o).unwrap_or("?").to_owned();
        let ids = |objects: &[ObjId]| objects.iter().map(|o| id(*o)).collect::<Vec<_>>();
        let mut up = Vec::new();
        let mut at = obj;
        while let Some((parent, feature)) = view.container(at) {
            up.push(format!(
                "{}.{}",
                id(parent),
                view.metamodel().feature_name(feature)
            ));
            // The parent's other children, and a prepared object's own.
            up.push(format!("{:?}", ids(view.children(parent))));
            at = parent;
            if up.len() > 64 {
                break;
            }
        }
        self.0.borrow_mut().push(format!(
            "{phase} {} up {up:?} children {:?} roots {:?} lib0 {:?} in {:?}",
            id(obj),
            ids(view.children(obj)),
            ids(view.roots()),
            view.lookup("lib000000").map(|o| ids(view.children(o))),
            view.lookup("lib000001")
                .and_then(|o| view.container(o))
                .map(|(o, _)| id(o)),
        ));
    }
}

struct Hooks<'a> {
    seen: &'a Seen,
    children: FeatureId,
    only_of: FeatureId,
    uses: FeatureId,
}
impl Implicits for Hooks<'_> {
    fn features(&self) -> Vec<FeatureId> {
        vec![self.children]
    }
    fn objects(
        &self,
        view: &LinkView<'_>,
        obj: ObjId,
        _: FeatureId,
    ) -> Result<Vec<ImplicitObject>, FrozenError> {
        self.seen.note("implicit", view, obj);
        // `implied` gets one implicit child that uses a library object.
        Ok(if view.identity(obj) == Some("implied") {
            vec![ImplicitObject {
                class: view.class(obj).ok_or(FrozenError::UnknownObject)?,
                values: vec![(
                    self.uses,
                    view.lookup("lib000002")
                        .map(intersect_model::core::frozen::ImplicitValue::Ref)
                        .into_iter()
                        .collect(),
                )],
            }]
        } else {
            Vec::new()
        })
    }
}
impl Defaults for Hooks<'_> {
    fn features(&self) -> Vec<FeatureId> {
        vec![self.only_of]
    }
    /// `onlyOf` of `proj1` defaults to what it uses, so a default writes an
    /// opposite end into a prepared object.
    fn values(
        &self,
        view: &LinkView<'_>,
        obj: ObjId,
        _: FeatureId,
    ) -> Result<Vec<DefaultItem>, FrozenError> {
        self.seen.note("default", view, obj);
        Ok(match view.item(obj, self.uses, 0) {
            Ok(Some(LinkValue::Ref(target))) if view.identity(obj) == Some("proj1") => {
                vec![DefaultItem::Object(target)]
            }
            _ => Vec::new(),
        })
    }
}
impl Validators for Hooks<'_> {
    fn is_empty(&self) -> bool {
        false
    }
    fn issues(&self, view: &LinkView<'_>, obj: ObjId) -> Result<Vec<Issue>, FrozenError> {
        self.seen.note("check", view, obj);
        Ok(Vec::new())
    }
}

/// How a load finishes the store: keeping the prepared objects in place
/// where it can, or by the whole-model reference.
#[derive(Copy, Clone, PartialEq, Debug)]
enum Path {
    InPlace,
    Repacked,
}

/// Everything a consumer can read of a model, by identity.
#[derive(PartialEq, Debug)]
struct Observed {
    model: materialized::Model,
    containers: Vec<String>,
    prepared: usize,
}
fn observed(model: &Model) -> Result<Observed, String> {
    let containers = model
        .objects()
        .map(|obj| {
            format!(
                "{} in {:?}",
                obj.identity(),
                obj.container().map(|(owner, feature)| (
                    owner.identity(),
                    model.metamodel().feature_name(feature)
                ))
            )
        })
        .collect();
    Ok(Observed {
        model: model.to_model().map_err(err)?,
        containers,
        prepared: model.prepared(),
    })
}

struct Loaded {
    /// The validated model, or the phase that refused and its diagnostics.
    model: Result<Model, String>,
    /// What the phases' views showed.
    seen: Vec<String>,
    /// Whether the builder kept the prepared objects in place.
    in_place: bool,
    /// Milliseconds per phase, in order.
    times: Vec<(&'static str, f64)>,
}

/// One load on top of `prepared`: clone, edit, finish by `path`, link,
/// complete, validate (delegating `delegated`'s fields) and check.
fn load_with(
    f: &Fixture,
    prepared: &Model,
    edit: Edit<'_>,
    path: Path,
    delegated: Option<&dyn Fn(ClassId, FeatureId) -> bool>,
    after: &dyn Fn(&mut ModelBuilder) -> Result<(), String>,
) -> Result<Loaded, String> {
    let seen = Seen::default();
    let hooks = Hooks {
        seen: &seen,
        children: f.feature("children")?,
        only_of: f.feature("onlyOf")?,
        uses: f.feature("uses")?,
    };
    let mut times = Vec::new();
    let mut clock = Instant::now();
    let mut lap = |name: &'static str| {
        times.push((name, clock.elapsed().as_secs_f64() * 1e3));
        clock = Instant::now();
    };
    let copy = prepared.clone();
    lap("clone");
    let mut store = FrozenStore::from_model(copy);
    lap("from_model");
    edit(f, &mut store)?;
    lap("edit");
    let builder = match path {
        Path::InPlace => store.into_builder(),
        Path::Repacked => store.into_builder_repacked(),
    };
    lap("into_builder");
    let mut in_place = false;
    let model = (|| {
        let mut builder = builder.map_err(|e| format!("into_builder: {e:?}"))?;
        after(&mut builder)?;
        in_place = builder.keeps_prepared_in_place();
        let linked = builder.link(&ById).map_err(|e| format!("link: {e:?}"))?;
        lap("link");
        let linked = linked
            .complete_implicit(&hooks)
            .map_err(|e| format!("implicit: {e:?}"))?;
        lap("implicit");
        let linked = linked
            .complete_defaults(&hooks)
            .map_err(|e| format!("defaults: {e:?}"))?;
        lap("defaults");
        let model = linked
            .validate_delegating(delegated)
            .map_err(|e| format!("validate: {e:?}"))?;
        lap("validate");
        assert!(model.check(&hooks).is_empty());
        lap("check");
        Ok(model)
    })();
    Ok(Loaded {
        model,
        seen: seen.0.into_inner(),
        in_place,
        times,
    })
}

fn load(f: &Fixture, prepared: &Model, edit: Edit<'_>, path: Path) -> Result<Loaded, String> {
    load_with(f, prepared, edit, path, None, &|_| Ok(()))
}

/// Load both ways and require the same outcome: the same model, or the same
/// refusal, and the same views in every phase. Returns the in-place load.
fn same(f: &Fixture, prepared: &Model, edit: Edit<'_>, in_place: bool) -> Result<Loaded, String> {
    let fast = load(f, prepared, edit, Path::InPlace)?;
    let reference = load(f, prepared, edit, Path::Repacked)?;
    assert!(!reference.in_place);
    assert_eq!(
        fast.in_place, in_place,
        "kept the prepared objects in place"
    );
    assert_same(&fast, &reference)?;
    Ok(fast)
}

fn assert_same(fast: &Loaded, reference: &Loaded) -> Result<(), String> {
    match (&fast.model, &reference.model) {
        (Ok(fast), Ok(reference)) => assert_eq!(observed(fast)?, observed(reference)?),
        (fast, reference) => assert_eq!(fast.as_ref().err(), reference.as_ref().err()),
    }
    assert_eq!(fast.seen, reference.seen);
    Ok(())
}

/// Twelve trees of four objects, `lib000000` to `lib000047`, one resource
/// each; the roots are the multiples of four.
fn small(f: &Fixture) -> Result<Model, String> {
    library(f, 12, 4)
}

fn create(f: &Fixture, s: &mut FrozenStore, id: &str, class: &str) -> StorageResult<()> {
    s.create_element(&f.schema, id, class, PROJECT, 0, 1)
}

#[test]
fn added_objects_and_references_into_the_prepared_model_agree() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let loaded = same(
        &f,
        &prepared,
        &|f, store| {
            add_project(f, store, 6)?;
            storage::atomic(store, |s| {
                // A prepared root becomes a child of a new object, and a new
                // object a child of a prepared one, through `parent`.
                s.set_value("proj2", "children", Some(json!([lib(4)])))?;
                create(f, s, "adopted", &f.leaf)?;
                s.set_value("adopted", "parent", Some(json!(lib(8))))?;
                s.set_value("proj3", "extras", Some(json!([lib(12)])))?;
                // A single-valued opposite end written into a prepared row.
                s.set_value("proj4", "only", Some(json!(lib(5))))?;
                create(f, s, "implied", &f.node)
            })
        },
        true,
    )?;
    let model = loaded.model?;
    let get = |id: &str| model.lookup(id).ok_or_else(|| id.to_owned());
    let refs = |id: &str, feature: &str| -> Result<Vec<&str>, String> {
        let object = get(id)?;
        let feature = object.feature(feature).ok_or("feature")?;
        Ok(object.refs(feature).map(|o| o.identity()).collect())
    };
    assert_eq!(refs(&lib(1), "incoming")?, [lib(0).as_str(), "proj0"]);
    assert_eq!(refs(&lib(5), "onlyOf")?, ["proj4"]);
    assert_eq!(refs(&lib(8), "children")?.last(), Some(&"adopted"));
    assert_eq!(refs(&lib(3), "only")?, ["proj1"], "the default's opposite");
    assert_eq!(refs("implied/implicit/children/0", "uses")?, [lib(2)]);
    let container = |id: &str| -> Result<Option<&str>, String> {
        Ok(get(id)?.container().map(|(owner, _)| owner.identity()))
    };
    assert_eq!(container(&lib(4))?, Some("proj2"));
    assert_eq!(container(&lib(12))?, Some("proj3"));
    assert_eq!(container("adopted")?, Some(lib(8).as_str()));
    assert_eq!(container(&lib(9))?, Some(lib(8).as_str()));
    assert_eq!(model.prepared(), 48);
    Ok(())
}

#[test]
fn a_second_load_on_a_loaded_model_agrees() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let first: Edit<'_> = &|f, store| add_project(f, store, 3);
    let on_fast = load(&f, &prepared, first, Path::InPlace)?.model?;
    let on_reference = load(&f, &prepared, first, Path::Repacked)?.model?;
    let second: Edit<'_> = &|f, store| {
        storage::atomic(store, |s| {
            create(f, s, "later", &f.leaf)?;
            s.set_value("later", "out", Some(json!(["proj1", lib(2)])))?;
            s.set_value("later", "children", Some(json!(["proj0"])))
        })
    };
    let fast = same(&f, &on_fast, second, true)?;
    let reference = load(&f, &on_reference, second, Path::Repacked)?;
    assert_same(&fast, &reference)?;
    assert_eq!(fast.model?.prepared(), 51);
    Ok(())
}

#[test]
fn removed_and_reclassified_new_objects_agree() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let reclassify = |f: &Fixture, s: &mut FrozenStore, id: &str| -> StorageResult<()> {
        let mut element = s.get(id)?.ok_or("element")?;
        element.class = f.leaf.clone();
        element.type_identity = f.schema.identity(&f.leaf);
        element.values.insert("weight".into(), json!(7));
        s.put(element)
    };
    let loaded = same(
        &f,
        &prepared,
        &|f, store| {
            add_project(f, store, 8)?;
            storage::atomic(store, |s| {
                // Leave gaps: remove two objects nothing names, one of them
                // created again afterwards, and replace a class.
                s.set_value(
                    "proj0",
                    "children",
                    Some(json!(["proj1", "proj2", "proj4"])),
                )?;
                s.remove("proj3")?;
                s.remove("proj6")?;
                create(f, s, "proj6", &f.leaf)?;
                s.set_value("proj6", "uses", Some(json!(["proj7", lib(1)])))?;
                reclassify(f, s, "proj2")?;
                s.set_value("proj5", "out", Some(json!(["proj6", "proj2"])))
            })
        },
        true,
    )?;
    let model = loaded.model?;
    assert_eq!(model.len(), 48 + 7);
    assert_eq!(model.lookup("proj2").map(|o| o.class_key()), Some(&*f.leaf));
    assert!(model.lookup("proj3").is_none());
    // The model kept in place saves and loads as a snapshot like any other.
    let key = [5; 32];
    let bytes = model.to_snapshot(&key).map_err(err)?;
    let restored = Model::from_snapshot(
        f.mm.clone(),
        &bytes,
        &key,
        intersect_model::core::frozen::SnapshotTrust::Revalidate,
    )
    .map_err(err)?;
    assert_eq!(observed(&restored)?.model, observed(&model)?.model);
    assert_eq!(
        observed(&restored)?.containers,
        observed(&model)?.containers
    );
    // A reference to a removed object is refused the same way.
    let dangling = same(
        &f,
        &prepared,
        &|f, store| {
            add_project(f, store, 4)?;
            storage::atomic(store, |s| {
                s.set_value("proj0", "children", Some(json!(["proj1", "proj3"])))?;
                s.remove("proj2")?;
                s.set_resolved_value("proj1", "uses", Some(json!(["proj3", "proj2"])))
            })
        },
        // No builder comes back to keep anything in place.
        false,
    )?;
    assert_eq!(
        dangling.model.err().as_deref(),
        Some(r#"into_builder: Identity("proj2")"#)
    );
    Ok(())
}

#[test]
fn a_changed_prepared_object_falls_back_to_the_whole_model() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let edits: [(&str, Edit<'_>); 5] = [
        ("value", &|f, store| {
            add_project(f, store, 3)?;
            storage::atomic(store, |s| {
                s.set_value(&lib(2), "uses", Some(json!(["proj1"])))
            })
        }),
        ("span", &|f, store| {
            add_project(f, store, 2)?;
            storage::atomic(store, |s| s.set_span(&lib(2), 3, 9))
        }),
        ("remove", &|f, store| {
            add_project(f, store, 2)?;
            storage::atomic(store, |s| {
                // Detach the last leaf from what names it, then remove it.
                s.set_value(&lib(44), "children", Some(json!([lib(45), lib(46)])))?;
                s.set_value(&lib(46), "out", Some(json!([])))?;
                s.remove(&lib(47))
            })
        }),
        ("class", &|f, store| {
            storage::atomic(store, |s| {
                let mut element = s.get(&lib(0))?.ok_or("element")?;
                element.class = f.leaf.clone();
                element.type_identity = f.schema.identity(&f.leaf);
                s.put(element)
            })
        }),
        ("explicit default", &|_, store| {
            storage::atomic(store, |s| s.set_value(&lib(0), "uses", Some(json!([]))))
        }),
    ];
    for (name, edit) in edits {
        same(&f, &prepared, edit, false)
            .map_err(|e| format!("{name}: {e}"))?
            .model
            .map_err(|e| format!("{name}: {e}"))?;
    }
    // A write that is rolled back leaves the prepared objects as they were.
    same(
        &f,
        &prepared,
        &|f, store| {
            add_project(f, store, 2)?;
            let attempt = store.begin()?;
            store.set_value(&lib(2), "name", Some(json!("renamed")))?;
            store.remove(&lib(11))?;
            store.rollback(&attempt)
        },
        true,
    )?
    .model?;
    // So does a write of the value an object already has.
    same(
        &f,
        &prepared,
        &|_, store| storage::atomic(store, |s| s.set_value(&lib(2), "name", Some(json!("n2")))),
        true,
    )?
    .model?;
    Ok(())
}

#[test]
fn a_builder_write_to_a_prepared_object_falls_back() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let (name, uses) = (f.feature("name")?, f.feature("uses")?);
    let edit: Edit<'_> = &|f, store| add_project(f, store, 3);
    let write = |builder: &mut ModelBuilder| -> Result<(), String> {
        let (target, new) = (
            builder.lookup(&lib(6)).ok_or("lib")?,
            builder.lookup("proj2").ok_or("proj2")?,
        );
        // A write to a new object keeps the prepared ones in place.
        let kept = builder.keeps_prepared_in_place();
        builder.set_str(new, name, "kept").map_err(err)?;
        assert_eq!(builder.keeps_prepared_in_place(), kept);
        builder.add_ref(target, uses, new).map_err(err)
    };
    let fast = load_with(&f, &prepared, edit, Path::InPlace, None, &write)?;
    let reference = load_with(&f, &prepared, edit, Path::Repacked, None, &write)?;
    assert!(!fast.in_place);
    assert_same(&fast, &reference)?;
    let model = fast.model?;
    let object = model.lookup(&lib(6)).ok_or("lib")?;
    assert_eq!(
        object.refs(uses).map(|o| o.identity()).collect::<Vec<_>>(),
        ["proj2"]
    );
    Ok(())
}

#[test]
fn invalid_loads_are_refused_alike() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let edits: [(&str, &str, Edit<'_>); 5] = [
        ("two containers", "MultipleContainers", &|f, store| {
            add_project(f, store, 2)?;
            storage::atomic(store, |s| {
                s.set_value("proj1", "extras", Some(json!([lib(1)])))
            })
        }),
        ("containment cycle", "ContainmentCycle", &|f, store| {
            add_project(f, store, 2)?;
            storage::atomic(store, |s| {
                s.set_value("proj1", "extras", Some(json!([lib(0)])))?;
                s.set_value("proj0", "parent", Some(json!(lib(1))))
            })
        }),
        ("one opposite end, two owners", "Opposite", &|f, store| {
            add_project(f, store, 3)?;
            storage::atomic(store, |s| {
                s.set_value("proj1", "only", Some(json!(lib(5))))?;
                s.set_value("proj2", "only", Some(json!(lib(5))))
            })
        }),
        ("unresolved", "Unresolved", &|f, store| {
            add_project(f, store, 2)?;
            storage::atomic(store, |s| {
                s.set_value("proj1", "uses", Some(json!(["?nowhere", "?lib000001"])))
            })
        }),
        ("required value", "Multiplicity", &|f, store| {
            add_project(f, store, 2)?;
            storage::atomic(store, |s| create(f, s, "strict", &f.strict))
        }),
    ];
    for (name, violation, edit) in edits {
        let refusal = same(&f, &prepared, edit, true)
            .map_err(|e| format!("{name}: {e}"))?
            .model
            .err()
            .ok_or_else(|| format!("{name}: accepted"))?;
        assert!(refusal.contains(violation), "{name}: {refusal}");
    }
    Ok(())
}

/// A prepared model validated with a field delegated may break that field's
/// bounds. A load that delegates nothing reports it for the prepared object
/// too, whichever way it finishes.
#[test]
fn prepared_objects_valid_only_by_delegation_are_still_checked() -> Result<(), String> {
    let f = fixture()?;
    let (strict, must) = (f.class(&f.strict)?, f.feature("must")?);
    let delegated = move |class: ClassId, feature: FeatureId| class == strict && feature == must;
    let mut b = ModelBuilder::new(f.mm.clone());
    b.create("lenient", strict, "memory:library/0")
        .map_err(err)?;
    b.create(&lib(0), f.class(&f.node)?, "memory:library/0")
        .map_err(err)?;
    let prepared = b
        .link(&ById)
        .map_err(err)?
        .validate_delegating(Some(&delegated))
        .map_err(err)?;
    let edit: Edit<'_> = &|f, store| {
        storage::atomic(store, |s| {
            storage::bind_schema(s, &f.schema)?;
            create(f, s, "proj0", &f.node)?;
            s.set_value("proj0", "out", Some(json!([lib(0)])))
        })
    };
    for delegated in [
        None,
        Some(&delegated as &dyn Fn(ClassId, FeatureId) -> bool),
    ] {
        let fast = load_with(&f, &prepared, edit, Path::InPlace, delegated, &|_| Ok(()))?;
        let reference = load_with(&f, &prepared, edit, Path::Repacked, delegated, &|_| Ok(()))?;
        assert!(fast.in_place);
        assert_same(&fast, &reference)?;
        assert_eq!(fast.model.is_ok(), delegated.is_some());
        if let Err(refusal) = fast.model {
            assert!(refusal.contains("Multiplicity") && refusal.contains("lenient"));
        }
    }
    Ok(())
}

#[test]
fn resource_records_agree() -> Result<(), String> {
    let f = fixture()?;
    // A prepared model with a recorded resource that holds no object.
    let empty = "memory:library/empty";
    let mut store = FrozenStore::from_model(small(&f)?);
    storage::atomic(&mut store, |s| {
        s.set_resource_record(ResourceRecord::new(empty)?)
    })?;
    let prepared = store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .validate()
        .map_err(err)?;
    assert!(observed(&prepared)?.model.resources.contains_key(empty));
    // Kept, with the project's own resource beside it.
    let kept = same(&f, &prepared, &|f, store| add_project(f, store, 2), true)?.model?;
    let resources = observed(&kept)?.model.resources;
    assert!(resources.contains_key(empty) && resources.contains_key(PROJECT));
    // Its record removed: the resource is gone, by repacking.
    let removed = same(
        &f,
        &prepared,
        &|f, store| {
            add_project(f, store, 2)?;
            storage::atomic(store, |s| s.remove_resource_record(empty))
        },
        false,
    )?
    .model?;
    assert!(!observed(&removed)?.model.resources.contains_key(empty));
    // A new resource whose only object was removed again, with and without
    // a record of its own.
    for (recorded, in_place) in [(true, true), (false, false)] {
        let loaded = same(
            &f,
            &prepared,
            &|f, store| {
                storage::atomic(store, |s| {
                    storage::bind_schema(s, &f.schema)?;
                    s.create_element(&f.schema, "gone", &f.node, "memory:gone", 0, 1)?;
                    s.remove("gone")?;
                    if !recorded {
                        s.remove_resource_record("memory:gone")?;
                    }
                    Ok(())
                })
            },
            in_place,
        )?
        .model?;
        assert_eq!(
            observed(&loaded)?
                .model
                .resources
                .contains_key("memory:gone"),
            recorded
        );
    }
    Ok(())
}

/// A store keeps answering class and resource queries about the prepared
/// objects, which it indexes on the first such query.
#[test]
fn prepared_objects_are_queried_like_new_ones() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let roots: Vec<String> = (0..12).map(|group| lib(group * 4)).collect();
    let with = |added: &[&str]| -> Vec<String> {
        let added = added.iter().map(|id| (*id).to_owned());
        roots.iter().cloned().chain(added).collect()
    };
    let mut store = FrozenStore::from_model(prepared.clone());
    add_project(&f, &mut store, 2)?;
    assert_eq!(
        store.query(storage::Query::Class(&f.node))?,
        with(&["proj0", "proj1"])
    );
    assert_eq!(
        store.query(storage::Query::Resource("memory:library/3"))?,
        [12, 13, 14, 15].map(lib)
    );
    storage::atomic(&mut store, |s| s.remove("proj0"))?;
    assert_eq!(
        store.query(storage::Query::Class(&f.node))?,
        with(&["proj1"])
    );
    // Asked before any write, and across a rolled-back one.
    let mut store = FrozenStore::from_model(prepared);
    assert_eq!(store.query(storage::Query::Class(&f.leaf))?.len(), 36);
    let attempt = store.begin()?;
    store.remove(&lib(1))?;
    assert_eq!(store.query(storage::Query::Class(&f.leaf))?.len(), 35);
    store.rollback(&attempt)?;
    assert_eq!(store.query(storage::Query::Class(&f.leaf))?.len(), 36);
    assert!(store.handle(&lib(1)).is_some());
    Ok(())
}

/// A fixed pseudo-random sequence (no clock, no global state).
struct Sequence(u64);
impl Sequence {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % below
    }
}

/// One arbitrary edit of a store on the small library: creations, values
/// naming prepared, new and missing objects by identity or by spelling,
/// removals, class replacements, rolled-back writes to prepared objects and,
/// rarely, a kept one. A store operation that is refused is skipped.
fn arbitrary_edit(f: &Fixture, store: &mut FrozenStore, seed: u64) -> StorageResult<()> {
    let mut random = Sequence(seed);
    let target = |random: &mut Sequence| match random.next(10) {
        0..=4 => lib(random.next(48)),
        5..=8 => format!("proj{}", random.next(6)),
        _ => format!("?{}", lib(random.next(48))),
    };
    storage::atomic(store, |s| {
        storage::bind_schema(s, &f.schema)?;
        for _ in 0..(4 + random.next(14)) {
            let id = format!("proj{}", random.next(6));
            let kind = random.next(20);
            // Refused operations roll back alone.
            let _ = storage::atomic(s, |s| match kind {
                0..=5 => {
                    let class = if random.next(3) == 0 {
                        &f.leaf
                    } else {
                        &f.node
                    };
                    create(f, s, &id, class)
                }
                6..=11 => {
                    let feature = ["out", "uses", "children", "extras", "incoming"][random.next(5)];
                    let targets: Vec<String> =
                        (0..random.next(4)).map(|_| target(&mut random)).collect();
                    s.set_value(&id, feature, Some(json!(targets)))
                }
                12..=14 => {
                    let feature = ["only", "parent", "onlyOf"][random.next(3)];
                    s.set_value(&id, feature, Some(json!(target(&mut random))))
                }
                15 => s.remove(&id),
                16 => {
                    let mut element = s.get(&id)?.ok_or("element")?;
                    let class = if element.class == f.leaf {
                        &f.node
                    } else {
                        &f.leaf
                    };
                    element.class = class.clone();
                    element.type_identity = f.schema.identity(class);
                    element.values.remove("weight");
                    s.put(element)
                }
                17 | 18 => {
                    let attempt = s.begin()?;
                    s.set_value(&lib(random.next(48)), "name", Some(json!("changed")))?;
                    s.set_value(&lib(random.next(48)), "uses", Some(json!([id])))?;
                    s.rollback(&attempt)
                }
                _ if random.next(4) == 0 => {
                    s.set_value(&lib(random.next(48)), "uses", Some(json!([lib(1)])))
                }
                _ => Ok(()),
            });
        }
        Ok(())
    })
}

/// Many arbitrary loads agree, whether accepted or refused, and so does a
/// further load on top of each accepted one.
#[test]
fn arbitrary_loads_agree() -> Result<(), String> {
    let f = fixture()?;
    let prepared = small(&f)?;
    let (mut accepted, mut refused, mut in_place) = (0, 0, 0);
    for seed in 0..400u64 {
        let edit: Edit<'_> = &|f, store| arbitrary_edit(f, store, seed);
        let fast = load(&f, &prepared, edit, Path::InPlace)?;
        let reference = load(&f, &prepared, edit, Path::Repacked)?;
        assert_same(&fast, &reference).map_err(|e| format!("seed {seed}: {e}"))?;
        in_place += usize::from(fast.in_place);
        let (Ok(fast), Ok(reference)) = (fast.model, reference.model) else {
            refused += 1;
            continue;
        };
        accepted += 1;
        let next: Edit<'_> = &|f, store| arbitrary_edit(f, store, seed + 1_000);
        let again = load(&f, &fast, next, Path::InPlace)?;
        assert_same(&again, &load(&f, &reference, next, Path::Repacked)?)?;
        assert_same(&again, &load(&f, &fast, next, Path::Repacked)?)?;
    }
    // The sequences exercise both outcomes, the in-place path and the
    // fallback.
    assert!(accepted >= 40 && refused >= 40, "{accepted} {refused}");
    assert!((100..400).contains(&in_place), "{in_place}");
    Ok(())
}

/// Whole-model costs of one small load on a library-sized prepared model.
/// It prints the phase times of the whole-model reference and of the load
/// that keeps the prepared objects in place, alternated, taking each
/// phase's best of three; run with `--nocapture` to read them. It asserts
/// only that both give the same model.
#[test]
fn a_small_load_on_a_large_prepared_model_is_measured() -> Result<(), String> {
    let f = fixture()?;
    let prepared = library(&f, 1_000, 100)?;
    assert_eq!(prepared.len(), 100_000);
    let edit: Edit<'_> = &|f, store| add_project(f, store, 20);
    let mut best: [Vec<(&'static str, f64)>; 2] = [Vec::new(), Vec::new()];
    let mut loads = Vec::new();
    for round in 0..3 {
        for (slot, path) in [(0, Path::Repacked), (1, Path::InPlace)] {
            let loaded = load(&f, &prepared, edit, path)?;
            if best[slot].is_empty() {
                best[slot].clone_from(&loaded.times);
            } else {
                for (kept, (_, ms)) in best[slot].iter_mut().zip(&loaded.times) {
                    kept.1 = kept.1.min(*ms);
                }
            }
            if round == 0 {
                loads.push(loaded);
            }
        }
    }
    println!("phase,whole_model_ms,in_place_ms,ratio");
    let mut totals = (0.0, 0.0);
    for ((name, whole), (_, in_place)) in best[0].iter().zip(&best[1]) {
        println!(
            "{name},{whole:.3},{in_place:.3},{:.1}",
            whole / in_place.max(1e-6)
        );
        totals = (totals.0 + whole, totals.1 + in_place);
    }
    println!(
        "total,{:.3},{:.3},{:.1}",
        totals.0,
        totals.1,
        totals.0 / totals.1.max(1e-6)
    );
    assert!(loads[1].in_place && !loads[0].in_place);
    assert_same(&loads[1], &loads[0])?;
    assert_eq!(loads[1].model.as_ref().map(Model::len), Ok(100_020));
    Ok(())
}
