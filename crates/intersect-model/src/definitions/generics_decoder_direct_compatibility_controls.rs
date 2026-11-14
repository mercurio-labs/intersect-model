//! Verification-only original generic source decoder observations.
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

use crate::definitions::generic_resolution::Resolver;

fn selected(node: &Node, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if node.name.namespace.is_none()
        && matches!(
            node.name.local.as_str(),
            "eGenericType"
                | "eGenericSuperTypes"
                | "eGenericExceptions"
                | "eTypeArguments"
                | "eBounds"
                | "eUpperBound"
                | "eLowerBound"
                | "eTypeParameters"
        )
    {
        out.push(path.clone());
    }
    for (index, child) in node.children.iter().enumerate() {
        path.push(index);
        selected(child, path, out);
        path.pop();
    }
}
fn capture(
    c: &Case,
    resources: &ResourceSet,
    resolver: &Resolver,
    id: &DeclarationId,
    operation: &str,
    control: Control,
) -> Value {
    let input = bundle(c);
    let source = serde_json::to_value(&input).unwrap();
    let pointer = resources.document(&id.source).unwrap() as *const _;
    let text_pointer = resources.document(&id.source).unwrap().source.text.as_ptr();
    let meter = Rc::new(Meter {
        control,
        ..Meter::default()
    });
    let lease = Rc::new(Lease(meter.clone()));
    let weak = Rc::downgrade(&lease);
    let output = {
        let ctx = InvocationContext::new(lease.clone());
        let obs = ParserObservation::new(&ctx);
        match operation {
            "parse_type" => encode(obs.finish(parse_type_observed(resources, id, &obs))),
            "parse_parameter" => encode(obs.finish(parse_parameter_observed(resources, id, &obs))),
            "resolve_type" => encode(resolver.resolve_type_observed(id, &ctx)),
            "resolve_parameter" => encode(resolver.resolve_parameter_observed(id, &ctx)),
            _ => panic!("unknown capture operation"),
        }
    };
    drop(lease);
    assert!(weak.upgrade().is_none());
    assert_eq!(resources.document(&id.source).unwrap() as *const _, pointer);
    assert_eq!(
        resources.document(&id.source).unwrap().source.text.as_ptr(),
        text_pointer
    );
    assert_eq!(source, serde_json::to_value(bundle(c)).unwrap());
    json!({"id":c.id,"operation":operation,"selection":id,"control":control,"source":source,"output":output,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get(),"fresh_lease_released":true,"source_and_document_identity_unchanged":true,"original_algorithm":"synchronous; no host quantum claimed"})
}
struct Sink {
    name: String,
    expected: Vec<String>,
    expected_stream: String,
    index: usize,
    stream: Sha256,
}
impl Sink {
    fn new(path: &std::path::Path) -> Self {
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let pins: Value = serde_json::from_str(include_str!(
            "generics_decoder_direct_original_full_hashes.json"
        ))
        .unwrap();
        let row = &pins[&name];
        Self {
            name,
            expected: serde_json::from_value(row["rows"].clone()).unwrap(),
            expected_stream: row["sha256"].as_str().unwrap().to_owned(),
            index: 0,
            stream: Sha256::new(),
        }
    }
    fn row(&mut self, row: Value) {
        let bytes = serde_json::to_vec(&row).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            Some(&digest),
            self.expected.get(self.index),
            "{} row{} id{} operation{} selection{} control{}",
            self.name,
            self.index,
            row["id"],
            row["operation"],
            row["selection"],
            row["control"]
        );
        self.stream.update(&bytes);
        self.stream.update(b"\n");
        self.index += 1;
    }
    fn finish(self, _: &std::path::Path) {
        assert_eq!(
            self.index,
            self.expected.len(),
            "{} complete row count",
            self.name
        );
        assert_eq!(
            format!("{:x}", self.stream.finalize()),
            self.expected_stream,
            "{} full serialized stream",
            self.name
        );
    }
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn frozen_original_direct_and_checked_outputs_every_callback_denial_and_exact_budgets() {
    let out = std::path::Path::new("no-output-direct-full-compatibility");
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic-source-decoding-before-cases.json")).unwrap();
    let mut main = Sink::new(&out.join("complete-main.jsonl"));
    let mut controls = Sink::new(&out.join("complete-controls.jsonl"));
    let chosen = [
        "inventory_multisource",
        "owner_rcore_long_unicode",
        "decode_leaf",
        "decode_wide_257",
        "decode_both_reference_bad_child_first",
        "decode_wildcard_both",
        "decode_href_257",
        "decode_child_href",
    ];
    for c in &cases {
        let input = bundle(c);
        let resources = match ResourceSet::new_diagnostic(&input) {
            Ok(value) => value,
            Err(e) => {
                main.row(json!({"id":c.id,"setup_error":format!("{e:?}"),"stage":"resources","source":input}));
                continue;
            }
        };
        let resolver = match Resolver::new(&input) {
            Ok(value) => value,
            Err(e) => {
                main.row(json!({"id":c.id,"setup_error":format!("{e:?}"),"stage":"resolver","source":input}));
                continue;
            }
        };
        let document = resources.document(&input.sources[0].id).unwrap();
        let mut paths = vec![vec![]];
        selected(&document.root, &mut vec![], &mut paths);
        let control_path = paths
            .iter()
            .find(|path| {
                let id = DeclarationId {
                    source: input.sources[0].id.clone(),
                    path: (*path).clone(),
                };
                resources.node(&id).is_ok_and(|node| {
                    node.name.namespace.is_none()
                        && matches!(
                            node.name.local.as_str(),
                            "eGenericType" | "eGenericSuperTypes" | "eBounds"
                        )
                })
            })
            .cloned();
        for path in paths {
            let id = DeclarationId {
                source: input.sources[0].id.clone(),
                path,
            };
            for operation in [
                "parse_type",
                "parse_parameter",
                "resolve_type",
                "resolve_parameter",
            ] {
                let full = capture(c, &resources, &resolver, &id, operation, Control::default());
                main.row(full.clone());
                if !chosen.contains(&c.id.as_str()) || control_path.as_ref() != Some(&id.path) {
                    continue;
                }
                let events = full["events"].as_array().unwrap();
                for deny in 1..=events.len() {
                    let row = capture(
                        c,
                        &resources,
                        &resolver,
                        &id,
                        operation,
                        Control {
                            deny: Some(deny),
                            ..Control::default()
                        },
                    );
                    assert_eq!(row["events"], json!(&events[..deny]));
                    assert!(row["output"].get("error").is_some());
                    controls.row(row);
                }
                let work = full["admitted_work"].as_u64().unwrap() as usize;
                let storage = full["admitted_storage"].as_u64().unwrap() as usize;
                for control in [
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
                    controls.row(capture(c, &resources, &resolver, &id, operation, control));
                }
                assert_eq!(
                    capture(c, &resources, &resolver, &id, operation, Control::default()),
                    full
                );
            }
        }
    }
    main.finish(&out.join("complete-main-index.json"));
    controls.finish(&out.join("complete-controls-index.json"));
}
