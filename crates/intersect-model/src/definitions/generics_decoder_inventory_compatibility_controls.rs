//! Complete original A/C inventory compatibility over all114 frozen graphs.
use super::*;
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ObservedImportError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    io::Write,
    rc::Rc,
};
#[derive(Clone, Deserialize)]
struct Dependency {
    uri: String,
    text: String,
}
#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    source_kind: String,
    uri: String,
    text: String,
    owner: String,
    dependency: Option<Dependency>,
}
#[derive(Clone, Copy, Default, Serialize)]
struct Control {
    deny: Option<usize>,
    max_work: Option<usize>,
    max_storage: Option<usize>,
    drop_at: Option<usize>,
}
#[derive(Default)]
struct Meter {
    events: RefCell<Vec<(char, usize)>>,
    work: Cell<usize>,
    storage: Cell<usize>,
    control: Control,
}
impl Meter {
    fn record(&self, kind: char, n: usize) -> Result<(), String> {
        let mut e = self.events.borrow_mut();
        e.push((kind, n));
        if self.control.deny == Some(e.len()) {
            return Err("inventory baseline denied".into());
        }
        if kind == 'w' {
            let next = self.work.get() + n;
            if self.control.max_work.is_some_and(|m| next > m) {
                return Err("inventory work denied".into());
            }
            self.work.set(next)
        }
        if kind == 's' {
            let next = self.storage.get() + n;
            if self.control.max_storage.is_some_and(|m| next > m) {
                return Err("inventory storage denied".into());
            }
            self.storage.set(next)
        }
        Ok(())
    }
}
struct Lease(Rc<Meter>);
impl InvocationObserver for Lease {
    fn checkpoint(&self) -> Result<(), String> {
        self.0.record('c', 0)
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.0.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.0.record('s', n)
    }
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("unexpected inventory provider".into())
    }
}
fn encode<T: Serialize>(result: Result<T, ObservedImportError>) -> Value {
    match result {
        Ok(v) => json!({"ok":v}),
        Err(e) => json!({"error":format!("{e:?}")}),
    }
}
fn bundle(c: &Case) -> DefinitionBundle {
    let mut sources = vec![DefinitionSource::new(
        &c.uri,
        if c.source_kind == "rcore" {
            SourceKind::Rcore
        } else {
            SourceKind::Ecore
        },
        &c.text,
    )];
    if let Some(d) = &c.dependency {
        sources.push(DefinitionSource::new(&d.uri, SourceKind::Ecore, &d.text))
    }
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}
fn phase(p: Phase) -> Value {
    match p {
        Phase::Sources => json!("sources"),
        Phase::Root => json!("root"),
        Phase::Visit => json!("visit"),
        Phase::Children(n) => json!({"children":n}),
        Phase::Ascend => json!("ascend"),
        Phase::Complete => json!("complete"),
        Phase::Failed => json!("failed"),
    }
}
fn run(c: &Case, mode: &str, quantum: usize, control: Control) -> Value {
    let bundle = bundle(c);
    let before = serde_json::to_value(&bundle).unwrap();
    let buffers: Vec<_> = bundle.sources.iter().map(|s| s.text.as_ptr()).collect();
    let resolver = match super::Resolver::new(&bundle) {
        Ok(r) => r,
        Err(e) => {
            return json!({"id":c.id,"mode":mode,"quantum":quantum,"control":control,"source":before,"setup_error":format!("{e:?}"),"stage":"resolver","events":[]});
        }
    };
    let root = if mode == "subtree" {
        match resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, &c.owner)
        {
            Ok(id) => Some(id),
            Err(e) => {
                return json!({"id":c.id,"mode":mode,"quantum":quantum,"control":control,"source":before,"setup_error":format!("{e:?}"),"stage":"root_selection","events":[]});
            }
        }
    } else {
        None
    };
    let document = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let meter = Rc::new(Meter {
        control,
        ..Meter::default()
    });
    let mut steps = vec![];
    let mut polls = 0;
    let mut failure = None;
    let mut poison = None;
    let output = if mode == "sync" {
        let lease = Rc::new(Lease(meter.clone()));
        let weak = Rc::downgrade(&lease);
        let result = {
            let ctx = InvocationContext::new(lease.clone());
            encode(resolver.resolve_all_observed(&ctx))
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        result
    } else {
        let lease = Rc::new(Lease(meter.clone()));
        let weak = Rc::downgrade(&lease);
        let start = {
            let ctx = InvocationContext::new(lease.clone());
            let obs = ParserObservation::new(&ctx);
            obs.finish(if let Some(id) = &root {
                SourceInventoryPreparation::subtree(&resolver, id, &obs)
            } else {
                SourceInventoryPreparation::new(&resolver, &obs)
            })
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        match start {
            Err(e) => json!({"error":format!("{e:?}"),"stage":"initialization"}),
            Ok(mut job) => {
                let mut abandoned_leases = Vec::new();
                'all: loop {
                    let lease = Rc::new(Lease(meter.clone()));
                    let weak = Rc::downgrade(&lease);
                    abandoned_leases.push(weak.clone());
                    {
                        let ctx = InvocationContext::new(lease.clone());
                        let obs = ParserObservation::new(&ctx);
                        for _ in 0..quantum {
                            if control.drop_at == Some(polls) {
                                break 'all;
                            }
                            let mut relocation = vec![job];
                            job = relocation.pop().unwrap();
                            let begin = meter.events.borrow().len();
                            let oldphase = match &job.state {
                                State::Walking { phase: p, .. } => phase(*p),
                                _ => panic!("compatibility exposed retained inventory phase"),
                            };
                            let oldcursor = job.cursor.clone();
                            polls += 1;
                            assert!(polls < 100_000);
                            let result = obs.finish(job.advance(&obs));
                            steps.push(json!({"poll":polls,"phase":oldphase,"cursor":oldcursor,"event_start":begin,"event_end":meter.events.borrow().len(),"result":format!("{result:?}")}));
                            match result {
                                Ok(true) => {
                                    drop(ctx);
                                    drop(lease);
                                    assert!(weak.upgrade().is_none());
                                    break 'all;
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    failure = Some(format!("{e:?}"));
                                    drop(ctx);
                                    drop(lease);
                                    assert!(weak.upgrade().is_none());
                                    break 'all;
                                }
                            }
                        }
                    }
                    drop(lease);
                    assert!(weak.upgrade().is_none());
                }
                assert!(
                    abandoned_leases
                        .iter()
                        .all(|lease| lease.upgrade().is_none()),
                    "abandoned attempt retained observer"
                );
                if failure.is_some() {
                    let quiet = Rc::new(Meter::default());
                    let ctx = InvocationContext::new(Rc::new(Lease(quiet.clone())));
                    let obs = ParserObservation::new(&ctx);
                    let again = obs.finish(job.advance(&obs));
                    assert!(again.is_err());
                    assert!(quiet.events.borrow().is_empty());
                    poison = Some(format!("{again:?}"));
                }
                let before_finish = meter.events.borrow().len();
                let ctx = InvocationContext::default();
                let obs = ParserObservation::new(&ctx);
                let finish = encode(obs.finish(job.finish()));
                assert_eq!(before_finish, meter.events.borrow().len());
                if failure.is_some() || control.drop_at.is_some() {
                    assert!(finish.get("error").is_some())
                }
                finish
            }
        }
    };
    assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    assert_eq!(
        bundle
            .sources
            .iter()
            .map(|s| s.text.as_ptr())
            .collect::<Vec<_>>(),
        buffers
    );
    assert_eq!(
        resolver
            .resource_sets()
            .0
            .document(&bundle.sources[0].id)
            .unwrap() as *const _,
        document
    );
    json!({"id":c.id,"mode":mode,"quantum":quantum,"control":control,"source":before,"selected_subtree":root,"output":output,"failure":failure,"poison":poison,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get(),"polls":polls,"steps":steps,"fresh_leases_released":true,"source_buffers_unchanged":true,"parsed_document_identity_unchanged":true})
}
#[path = "generics_decoder_frozen_rows.rs"]
mod frozen_rows;
use frozen_rows::FrozenWriter;
struct Writer(FrozenWriter);
impl Writer {
    fn new(name: &str) -> Self {
        Self(FrozenWriter::new(&format!("inventory_A/{name}")))
    }
    fn row(&mut self, row: Value) {
        self.0.row(&row)
    }
    fn finish(self, _: &str) {
        self.0.finish()
    }
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn frozen_original_a_whole_subtree_complete_callbacks_denials_budgets_and_drop() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic-source-decoding-before-cases.json")).unwrap();
    let mut main = Writer::new("complete-main");
    let mut controls = Writer::new("complete-controls");
    let mut summary = vec![];
    for c in &cases {
        for mode in ["sync", "whole", "subtree"] {
            for q in if mode == "sync" {
                vec![1]
            } else {
                vec![1, 3, 256]
            } {
                let row = run(c, mode, q, Control::default());
                summary.push(json!({"id":c.id,"mode":mode,"quantum":q,"setup_error":row["setup_error"],"error":row["output"]["error"],"events":row["events"].as_array().map(Vec::len),"work":row["admitted_work"],"storage":row["admitted_storage"],"polls":row["polls"]}));
                main.row(row)
            }
        }
    }
    let selected = [
        "empty_many",
        "exception_order",
        "duplicate_long_targets",
        "inventory_multisource",
    ];
    for id in selected {
        let c = cases.iter().find(|c| c.id == id).unwrap();
        for mode in ["whole", "subtree"] {
            let full = run(c, mode, 1, Control::default());
            if full.get("setup_error").is_some() {
                continue;
            }
            let events = full["events"].as_array().unwrap();
            for deny in 1..=events.len() {
                let row = run(
                    c,
                    mode,
                    1,
                    Control {
                        deny: Some(deny),
                        ..Control::default()
                    },
                );
                assert_eq!(row["events"], json!(&events[..deny]));
                controls.row(row)
            }
            let work = full["admitted_work"].as_u64().unwrap() as usize;
            let storage = full["admitted_storage"].as_u64().unwrap() as usize;
            for q in [1, 3, 256] {
                for ctrl in [
                    Control {
                        max_work: Some(work),
                        max_storage: Some(storage),
                        ..Control::default()
                    },
                    Control {
                        max_work: Some(work.saturating_sub(1)),
                        max_storage: Some(storage),
                        ..Control::default()
                    },
                    Control {
                        max_work: Some(work),
                        max_storage: Some(storage.saturating_sub(1)),
                        ..Control::default()
                    },
                ] {
                    controls.row(run(c, mode, q, ctrl))
                }
            }
            for max in [0, 511, 512] {
                controls.row(run(
                    c,
                    mode,
                    1,
                    Control {
                        max_storage: Some(max),
                        ..Control::default()
                    },
                ))
            }
            for poll in 0..full["polls"].as_u64().unwrap() as usize {
                controls.row(run(
                    c,
                    mode,
                    1,
                    Control {
                        drop_at: Some(poll),
                        ..Control::default()
                    },
                ))
            }
            let retry = run(c, mode, 1, Control::default());
            assert_eq!(retry, full);
        }
    }
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&summary).unwrap())
        ),
        "6265d8fa4ea8ae3b12311fc5e3928a43ca273efbff33e71aa69a80721ed6df7b"
    );
    main.finish("complete-main");
    controls.finish("complete-controls");
}

#[path = "generics_decoder_inventory_c_compatibility_controls.rs"]
mod retained_controls;
