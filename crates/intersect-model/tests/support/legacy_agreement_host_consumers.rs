//! Public host controls shared by native and zero-import WASM verification.
//! The compatibility digest is frozen from the separately preserved V59 library.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{
        ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
    },
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
    schema::SharedRuntimeSourceArrayTarget,
};
use intersect_model::model_reference::ReferenceDomainState;
use intersect_model::{api::ModelEdit, compile_rcore_bundle_selected};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub fn fixture() -> Result<(RuntimeType, ReferenceContext), String> {
    let uri = format!(
        "memory:{}/legacy-host.rcore",
        "\u{03a9}\u{1f30d}".repeat(45)
    );
    let source = DefinitionSource::new(
        uri,
        SourceKind::Rcore,
        "rcore 1;package legacyHost version \"1\";metamodel \"urn:legacy-host\" prefix h {class Leaf {attr label:String [0,1];}class Box<T> {ref value:T [0,1];}class Child extends Box<Leaf> {}}",
    );
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let ty = |ordinal| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: entry.clone(),
                path: vec![ordinal],
            },
            arguments: vec![],
            raw: false,
        })
    };
    let child = ty(2)?;
    let leaf = ty(0)?;
    let requested = vec![child.clone(), leaf.clone()];
    let package =
        compile_rcore_bundle_selected(bundle.clone(), &entry, "urn:legacy-host", &requested)
            .map_err(|error| error.to_string())?;
    let mut model = package.new_model().map_err(|error| error.to_string())?;
    model.create_typed("leaf", &leaf, "memory:leaf-model")?;
    model.create_typed("child", &child, "memory:child-model")?;
    model.set("leaf", "label", json!("retained \u{1f30d} \u{03a9}"))?;
    model.set("child", "value", json!("leaf"))?;
    model.validate()?;
    let snapshot = serde_json::to_value(intersect_model::storage::export(&*model)?)
        .map_err(|error| error.to_string())?;
    Ok((
        child,
        ReferenceContext {
            bundle,
            entry,
            namespace: "urn:legacy-host".into(),
            conversions: vec![],
            materialized_types: requested,
            applied_conversions: vec![],
            constructors: vec![],
            model: snapshot,
        },
    ))
}

#[derive(Default)]
pub struct Stats {
    pub events: RefCell<Vec<(char, usize)>>,
    refused: Cell<Option<usize>>,
    work_limit: Cell<Option<usize>>,
    storage_limit: Cell<Option<usize>>,
    work: Cell<usize>,
    storage: Cell<usize>,
}
impl Stats {
    fn event(&self, kind: char, n: usize) -> Result<(), String> {
        self.events.borrow_mut().push((kind, n));
        let index = self.events.borrow().len() - 1;
        if self.refused.get() == Some(index) {
            return Err("public host observer refused".into());
        }
        let (total, limit) = match kind {
            'w' => (&self.work, self.work_limit.get()),
            's' => (&self.storage, self.storage_limit.get()),
            _ => return Ok(()),
        };
        total.set(total.get() + n);
        if limit.is_some_and(|limit| total.get() > limit) {
            return Err("public host budget refused".into());
        }
        Ok(())
    }
}
pub struct Lease {
    pub stats: Rc<Stats>,
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected host provider invocation".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.stats.event('c', 0)
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.stats.event('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.stats.event('s', n)
    }
}

fn capture(
    retained: bool,
    quantum: usize,
    stats: Rc<Stats>,
) -> Result<(Value, Vec<(usize, usize)>), String> {
    let (target, context) = fixture()?;
    let original = context.clone();
    let digest = context.digest()?;
    let lease = Rc::new(Lease {
        stats: stats.clone(),
    });
    let weak = Rc::downgrade(&lease);
    let mut pending = {
        let observation = InvocationContext::new(lease.clone());
        let begin = if retained {
            ReferenceDomainState::<SharedRuntimeSourceArrayTarget>::begin_runtime_observed_with_retained_generic_agreement
        } else {
            ReferenceDomainState::<SharedRuntimeSourceArrayTarget>::begin_runtime_observed
        };
        begin(
            target.clone(),
            context,
            &ConversionRegistry::default(),
            &observation,
        )?
    };
    drop(lease);
    assert!(weak.upgrade().is_none());
    let mut polls = 0;
    let mut segments = vec![];
    let registry = ConversionRegistry::default();
    loop {
        pending = std::hint::black_box(pending);
        let lease = Rc::new(Lease {
            stats: stats.clone(),
        });
        let weak = Rc::downgrade(&lease);
        let done = {
            let observation = InvocationContext::new(lease.clone());
            let mut done = false;
            for _ in 0..quantum {
                polls += 1;
                let start = stats.events.borrow().len();
                let advance = pending.advance(&registry, &observation);
                let end = stats.events.borrow().len();
                segments.push((start, end));
                match advance {
                    Ok(true) => {
                        done = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        assert!(pending.advance(&registry, &observation).is_err());
                        assert_eq!(
                            stats.events.borrow().len(),
                            end,
                            "failed advance must remain poisoned"
                        );
                        assert!(pending.finish(&observation).is_err());
                        assert_eq!(
                            stats.events.borrow().len(),
                            end + 1,
                            "finish retains its original checkpoint"
                        );
                        return Err(error);
                    }
                }
            }
            done
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        if done {
            break;
        }
    }
    let lease = Rc::new(Lease {
        stats: stats.clone(),
    });
    let weak = Rc::downgrade(&lease);
    let domain = {
        let observation = InvocationContext::new(lease.clone());
        pending.finish(&observation)?
    };
    drop(lease);
    assert!(weak.upgrade().is_none());
    assert_eq!(domain.context(), &original);
    assert_eq!(domain.runtime_target(), Some(&target));
    let bound = domain.bind("child")?;
    assert_eq!(bound["context"], digest);
    let child = domain.resolve(&bound)?;
    assert_eq!(child.id, "child");
    assert_eq!(child.resource, "memory:child-model");
    assert_eq!(child.values["value"], "leaf");
    assert!(domain.bind("leaf").is_err());
    let events = stats.events.borrow().clone();
    let work: usize = events
        .iter()
        .filter(|(kind, _)| *kind == 'w')
        .map(|(_, n)| *n)
        .sum();
    let storage: usize = events
        .iter()
        .filter(|(kind, _)| *kind == 's')
        .map(|(_, n)| *n)
        .sum();
    let checkpoints = events.iter().filter(|(kind, _)| *kind == 'c').count();
    Ok((
        json!({"polls":polls,"work":work,"storage":storage,"checkpoints":checkpoints,
        "events":events,"output":{"context":domain.context(),"schema":domain.schema(),
        "class":domain.class(),"target":domain.runtime_target(),"bound":bound,"child":child},
        "fresh_leases_released":true}),
        segments,
    ))
}

fn digest(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn baseline() -> Result<Value, String> {
    serde_json::from_str(include_str!("legacy_agreement_host_original.json"))
        .map_err(|error| error.to_string())
}
fn fresh(retained: bool, quantum: usize) -> Result<(Value, Vec<(usize, usize)>), String> {
    capture(retained, quantum, Rc::new(Stats::default()))
}
fn compatibility() -> Result<(), String> {
    let original = baseline()?;
    for quantum in [1, 3, 256] {
        let (actual, _) = fresh(false, quantum)?;
        assert_eq!(digest(&actual)?, original["capture_sha256"]);
        assert_eq!(digest(&actual["events"])?, original["events_sha256"]);
        assert_eq!(digest(&actual["output"])?, original["output_sha256"]);
    }
    Ok(())
}
fn retained() -> Result<(Value, Vec<(usize, usize)>), String> {
    let original = baseline()?;
    let (a, old_segments) = fresh(false, 1)?;
    let (b, segments) = fresh(true, 1)?;
    assert_eq!(digest(&b["output"])?, original["output_sha256"]);
    assert_eq!(b["work"], original["work"]);
    assert_eq!(b["storage"], original["storage"]);
    assert!(b["polls"].as_u64() > a["polls"].as_u64());
    let positive = |v: &Value| {
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e[1] != 0)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(positive(&b), positive(&a));
    for quantum in [3, 256] {
        assert_eq!(fresh(true, quantum)?.0, b);
    }
    // Align complete public poll transcripts, preserving every original poll.
    // The generic quiet pattern also exists in Compatibility; only surplus
    // polls established by this ordered alignment are new public boundaries.
    let old_events = a["events"].as_array().ok_or("Missing original events")?;
    let new_events = b["events"].as_array().ok_or("Missing retained events")?;
    let mut original_poll = 0;
    let mut added = vec![];
    for (start, end) in segments {
        let current = &new_events[start..end];
        if let Some(&(old_start, old_end)) = old_segments.get(original_poll)
            && current == &old_events[old_start..old_end]
        {
            original_poll += 1;
            continue;
        }
        assert_eq!(
            current,
            &[
                json!(["c", 0]),
                json!(["c", 0]),
                json!(["w", 0]),
                json!(["c", 0])
            ]
        );
        added.push((start, end));
    }
    assert_eq!(original_poll, old_segments.len());
    assert_eq!(added.len(), 192);
    assert_eq!(new_events.len() - old_events.len(), added.len() * 4);
    Ok((b, added))
}
fn refusal_and_recovery(b: &Value, segments: &[(usize, usize)]) -> Result<(), String> {
    let events = b["events"].as_array().ok_or("Missing events")?;
    let silent = segments;
    assert!(
        !silent.is_empty(),
        "new public poll boundaries must be established by old/new ordered transcript alignment"
    );
    for (start, end) in [
        silent[0],
        silent[silent.len() / 2],
        silent[silent.len() - 1],
    ] {
        for index in start..end {
            let stats = Rc::new(Stats::default());
            stats.refused.set(Some(index));
            let result = capture(true, 3, stats.clone());
            assert!(result.is_err());
            assert_eq!(
                &stats.events.borrow()[..index + 1],
                &events[..index + 1]
                    .iter()
                    .map(|e| (
                        e[0].as_str().unwrap().chars().next().unwrap(),
                        e[1].as_u64().unwrap() as usize
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }
    assert_eq!(fresh(true, 256)?.0, *b);
    Ok(())
}
fn budgets(b: &Value) -> Result<(), String> {
    for quantum in [1, 3, 256] {
        let work = b["work"].as_u64().ok_or("Missing work")? as usize;
        let storage = b["storage"].as_u64().ok_or("Missing storage")? as usize;
        let exact = Rc::new(Stats::default());
        exact.work_limit.set(Some(work));
        exact.storage_limit.set(Some(storage));
        assert_eq!(capture(true, quantum, exact)?.0, *b);
        for (work_limit, storage_limit) in [(work - 1, storage), (work, storage - 1)] {
            let short = Rc::new(Stats::default());
            short.work_limit.set(Some(work_limit));
            short.storage_limit.set(Some(storage_limit));
            assert!(capture(true, quantum, short).is_err());
        }
    }
    assert_eq!(fresh(true, 1)?.0, *b);
    Ok(())
}
fn unsupported() -> Result<(), String> {
    use intersect_model::model_reference::ReferenceSchemaFactory;
    let (target, mut context) = fixture()?;
    let registry = ConversionRegistry::default();
    let observation = InvocationContext::default();
    assert!(
        ReferenceDomainState::<String>::begin_runtime_observed_with_retained_generic_agreement(
            target.clone(),
            context.clone(),
            &registry,
            &observation
        )
        .is_err()
    );
    context.materialized_types.clear();
    let original = context.clone();
    assert!(
        <SharedRuntimeSourceArrayTarget as ReferenceSchemaFactory>::begin_compilation(
            &mut context,
            &registry,
            &observation
        )?
        .is_none()
    );
    assert_eq!(context, original);
    assert!(<SharedRuntimeSourceArrayTarget as ReferenceSchemaFactory>::begin_compilation_with_retained_generic_agreement(&mut context,&registry,&observation).is_err());
    assert_eq!(context, original);
    assert!(ReferenceDomainState::<SharedRuntimeSourceArrayTarget>::begin_runtime_observed_with_retained_generic_agreement(target,context,&registry,&observation).is_err());
    Ok(())
}
pub fn verify() -> Result<u32, String> {
    compatibility()?;
    let (b, segments) = retained()?;
    refusal_and_recovery(&b, &segments)?;
    budgets(&b)?;
    unsupported()?;
    Ok(5)
}
