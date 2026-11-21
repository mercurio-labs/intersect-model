use super::*;
use crate::{
    core::{
        frozen::FrozenStore,
        model::Element,
        storage::{ModelStore, Query, Row, StorageResult, atomic},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::{InvocationObserver, ProviderId},
    },
};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

const SOURCE: &str = r#"rcore 1;
package read.cache version "1";
metamodel "urn:read-cache" prefix m {
 class Base {attr name:String [0,1]; attr values:String [0,*]; ref target:Base [0,1];}
 class Child extends Base {attr note:String [0,1];}
 class Other {attr name:String [0,1];}
 class Restricted {attr calculated:String [0,1] derived; attr live:String [0,1] volatile;}
}"#;

struct Meter {
    storage: Cell<usize>,
    limit: usize,
    cancelled: Cell<bool>,
    work: Cell<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("metadata invoked a provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.cancelled.get() {
            Err("cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.checkpoint()?;
        self.work.set(self.work.get() + n);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self.storage.get().checked_add(n).ok_or("overflow")?;
        if next > self.limit {
            self.cancelled.set(true);
            return Err("storage exhausted".into());
        }
        self.storage.set(next);
        Ok(())
    }
}
fn meter(limit: usize) -> (Rc<Meter>, InvocationContext<'static>) {
    let m = Rc::new(Meter {
        storage: Cell::new(0),
        limit,
        cancelled: Cell::new(false),
        work: Cell::new(0),
    });
    (m.clone(), InvocationContext::new(m))
}
fn schema(text: &str) -> Result<Schema, String> {
    let source = DefinitionSource::new("memory:read-cache.rcore", SourceKind::Rcore, text);
    let id = source.id.clone();
    crate::definitions::compiler::compile_structure_bundle(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &id,
        "urn:read-cache",
    )
    .map_err(|e| format!("{e:?}"))
}
fn class(s: &Schema, name: &str) -> String {
    s.declarations
        .iter()
        .find(|(key, d)| d.name == name && s.classes.contains_key(*key))
        .map(|(key, _)| key.clone())
        .unwrap()
}
fn field(s: &Schema, owner: &str, name: &str) -> DeclarationId {
    let owner = class(s, owner);
    let fields = s.effective_features(&owner).unwrap();
    let field = fields.iter().find(|f| f.feature.name == name).unwrap();
    match &field.identity {
        MemberIdentity::Source { declaration } => declaration.clone(),
        _ => panic!("authored field"),
    }
}
struct Store {
    inner: RefCell<FrozenStore>,
    foreign: Cell<bool>,
    revision_mismatch: Cell<bool>,
    reads: Cell<usize>,
}
impl Store {
    fn new(s: &Schema) -> Result<Self, String> {
        let mut inner = FrozenStore::for_schema(s).map_err(|e| e.to_string())?;
        atomic(&mut inner, |st| {
            for (id, kind) in [
                ("a", "Child"),
                ("b", "Base"),
                ("c", "Other"),
                ("d", "Restricted"),
            ] {
                st.create_element(s, id, &class(s, kind), "memory:data", 0, 1)?;
            }
            st.set_value("a", "name", Some(json!("first")))?;
            st.set_value("a", "values", Some(json!(["one", "two", "three"])))?;
            st.set_value("a", "target", Some(json!("?waiting")))
        })?;
        Ok(Self {
            inner: RefCell::new(inner),
            foreign: Cell::new(false),
            revision_mismatch: Cell::new(false),
            reads: Cell::new(0),
        })
    }
    fn set(&self, name: &str, value: Value) -> Result<(), String> {
        atomic(&mut *self.inner.borrow_mut(), |s| {
            s.set_value("a", name, Some(value))
        })
    }
}
impl ModelRead for Store {
    fn revision(&self) -> StorageResult<String> {
        if self.revision_mismatch.get() {
            Ok("foreign revision".into())
        } else {
            self.inner.borrow().revision()
        }
    }
    fn get(&self, id: &str) -> StorageResult<Option<Element>> {
        let mut value = self.inner.borrow().get(id)?;
        if self.foreign.get()
            && let Some(e) = &mut value
        {
            e.type_identity = "foreign".into()
        }
        Ok(value)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.inner.borrow().class(id)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.inner.borrow().page(after, limit)
    }
    fn query(&self, q: Query<'_>) -> StorageResult<Vec<String>> {
        self.inner.borrow().query(q)
    }
    fn matches(&self, id: &str, q: Query<'_>) -> StorageResult<bool> {
        self.inner.borrow().matches(id, q)
    }
    fn inspect_stored_value_observed(
        &self,
        id: &str,
        name: &str,
        many: bool,
        o: &InvocationContext<'_>,
        f: &mut dyn FnMut(ValueView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.reads.set(self.reads.get() + 1);
        self.inner
            .borrow()
            .inspect_stored_value_observed(id, name, many, o, f)
    }
}
fn value(v: ScopeValue) -> Value {
    match v {
        ScopeValue::Value(v) => v,
        ScopeValue::Absent => Value::Null,
        _ => panic!("expected a stored value"),
    }
}
fn page(v: ScopePage) -> Option<Value> {
    match v {
        ScopePage::Value(v) => Some(v),
        ScopePage::End => None,
        _ => panic!("expected a stored page"),
    }
}

#[test]
fn invocation_pages_preserve_values_and_reduce_repeated_preparation() -> Result<(), String> {
    let s = schema(SOURCE)?;
    let store = Store::new(&s)?;
    let decl = field(&s, "Child", "values");
    let cached = SourceModelReads::for_invocation(&store, &s);
    let plain = SourceModelReads::new(&store, &s);
    let (cm, co) = meter(usize::MAX);
    let (pm, po) = meter(usize::MAX);
    for _ in 0..20 {
        for cursor in 0..4 {
            assert_eq!(
                page(cached.page("a", &decl, cursor, &co)?),
                page(plain.page("a", &decl, cursor, &po)?)
            );
        }
    }
    assert!(
        cm.storage.get() * 4 < pm.storage.get(),
        "cached={}, original={}",
        cm.storage.get(),
        pm.storage.get()
    );
    assert_eq!(
        store.reads.get(),
        160,
        "cache must retain no object/page values"
    );
    assert!(plain.members.is_none());
    Ok(())
}

#[test]
fn cached_metadata_keeps_live_values_and_all_source_guards() -> Result<(), String> {
    let s = schema(SOURCE)?;
    let store = Store::new(&s)?;
    let reader = SourceModelReads::for_invocation(&store, &s);
    let decl = field(&s, "Base", "name");
    let (_, o) = meter(usize::MAX);
    assert_eq!(value(reader.singular("a", &decl, &o)?), json!("first"));
    store.set("name", json!("changed"))?;
    assert_eq!(value(reader.singular("a", &decl, &o)?), json!("changed"));
    assert!(
        reader.singular("c", &decl, &o).is_err(),
        "same name on another class is not the same declaration"
    );
    let mut foreign = decl.clone();
    foreign.source.sha256 = "0".repeat(64);
    assert!(reader.singular("a", &foreign, &o).is_err());
    store.foreign.set(true);
    assert!(
        reader
            .singular("a", &decl, &o)
            .err()
            .is_some_and(|e| e.contains("foreign source/type"))
    );
    store.foreign.set(false);
    store.revision_mismatch.set(true);
    assert!(
        reader
            .singular("a", &decl, &o)
            .err()
            .is_some_and(|e| e.contains("different schema revision"))
    );
    Ok(())
}

#[test]
fn independent_invocations_reprepare_and_cancelled_hits_stay_terminal() -> Result<(), String> {
    let s = schema(SOURCE)?;
    let store = Store::new(&s)?;
    let decl = field(&s, "Base", "name");
    let first = SourceModelReads::for_invocation(&store, &s);
    let (a, ao) = meter(usize::MAX);
    first.singular("a", &decl, &ao)?;
    let full = a.storage.get();
    let second = SourceModelReads::for_invocation(&store, &s);
    let (b, bo) = meter(usize::MAX);
    second.singular("a", &decl, &bo)?;
    assert_eq!(b.storage.get(), full);
    a.cancelled.set(true);
    let reads = store.reads.get();
    assert!(
        first
            .singular("a", &decl, &ao)
            .err()
            .is_some_and(|e| e.contains("cancelled"))
    );
    assert_eq!(store.reads.get(), reads);
    Ok(())
}

#[test]
fn invalid_members_multiplicity_and_failed_precharge_publish_no_proof() -> Result<(), String> {
    let s = schema(SOURCE)?;
    let store = Store::new(&s)?;
    let reader = SourceModelReads::for_invocation(&store, &s);
    let (_, o) = meter(usize::MAX);
    let list = field(&s, "Base", "values");
    assert!(reader.singular("a", &list, &o).is_err());
    for name in ["calculated", "live"] {
        assert!(
            reader
                .singular("d", &field(&s, "Restricted", name), &o)
                .is_err()
        );
    }
    assert!(reader.members.as_ref().unwrap().borrow().is_empty());
    let decl = field(&s, "Base", "name");
    let measured = SourceModelReads::for_invocation(&store, &s);
    let (m, mo) = meter(usize::MAX);
    measured.member(&class(&s, "Child"), &decl, false, &mo)?;
    let (short, so) = meter(m.storage.get() - 1);
    assert!(
        reader
            .member(&class(&s, "Child"), &decl, false, &so)
            .is_err()
    );
    assert!(short.cancelled.get());
    assert!(reader.members.as_ref().unwrap().borrow().is_empty());
    Ok(())
}

#[test]
fn pending_and_unavailable_references_are_read_again_after_metadata_reuse() -> Result<(), String> {
    let s = schema(SOURCE)?;
    let store = Store::new(&s)?;
    let reader = SourceModelReads::for_invocation(&store, &s);
    let decl = field(&s, "Base", "target");
    let (_, o) = meter(usize::MAX);
    assert!(matches!(
        reader.singular("a", &decl, &o)?,
        ScopeValue::Pending
    ));
    store.set("target", json!("not-resident"))?;
    assert!(matches!(
        reader.singular("a", &decl, &o)?,
        ScopeValue::Unavailable
    ));
    store.set("target", json!("b"))?;
    assert_eq!(value(reader.singular("a", &decl, &o)?), json!("b"));
    Ok(())
}

#[test]
fn redefined_declarations_preserve_exact_checked_resolution_on_cache_hits() -> Result<(), String> {
    let s = schema(
        r##"rcore 1; package read.cache version "1";
        metamodel "urn:read-cache" prefix m {
          class Base { attr name:String [0,1]; }
          class Renamed extends Base { attr label:String [0,1]
            annotation "redefines" { reference "#//@eClassifiers.0/@eStructuralFeatures.0"; }; }
          class Other { attr label:String [0,1]; }
        }"##,
    )?;
    let store = FrozenStore::for_schema(&s).map_err(|e| e.to_string())?;
    let cached = SourceModelReads::for_invocation(&store, &s);
    let original = SourceModelReads::new(&store, &s);
    let (_, o) = meter(usize::MAX);
    let declarations = [
        field(&s, "Base", "name"),
        field(&s, "Renamed", "label"),
        field(&s, "Other", "label"),
    ];
    assert_ne!(declarations[1], declarations[2]);
    for _ in 0..2 {
        for kind in ["Base", "Renamed", "Other"] {
            for declaration in &declarations {
                let describe = |result: Result<&Feature, String>| {
                    result.map(|f| (f.name.clone(), f.lower, f.upper))
                };
                assert_eq!(
                    describe(cached.member(&class(&s, kind), declaration, false, &o)),
                    describe(original.member(&class(&s, kind), declaration, false, &o))
                );
            }
        }
    }
    Ok(())
}
