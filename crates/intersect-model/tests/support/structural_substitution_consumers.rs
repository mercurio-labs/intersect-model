//! Production public API consumers shared by native and zero-import WASM hosts.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{self, OwnedRuntimeSourceArrayPreparation},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    generic_types::{ParameterBinding, RuntimeType, Substitution, Type},
    rcore_behavior::{
        checking,
        function_lowering::compile_function,
        ir_v2::{EntrySelection, Limits},
    },
    resources::DeclarationId,
};
use std::{cell::Cell, rc::Rc};
fn fixture() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:substitution-consumers.rcore",
            SourceKind::Rcore,
            "rcore 1;package substitution version \"1\";metamodel \"urn:substitution-consumers\" prefix s {class Leaf {}class Box<T> {ref value:T [0,1];}class Child extends Box<Leaf> {}}behavior {import model \"urn:substitution-consumers\" as m;fn read(x:m.Child)->m.Leaf?=x.value;}",
        )],
        namespaces: vec![],
    }
}
#[derive(Default)]
struct Stats {
    work: Cell<usize>,
    storage: Cell<usize>,
    checkpoints: Cell<usize>,
}
struct Lease {
    stats: Rc<Stats>,
    cancel: bool,
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected structural substitution provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.stats.checkpoints.set(self.stats.checkpoints.get() + 1);
        if self.cancel {
            Err("substitution consumer cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.stats.work.set(self.stats.work.get() + n);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.stats.storage.set(self.stats.storage.get() + n);
        Ok(())
    }
}
fn requested(bundle: &DefinitionBundle) -> Result<Vec<RuntimeType>, String> {
    [2, 0]
        .into_iter()
        .map(|ordinal| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: bundle.sources[0].id.clone(),
                    path: vec![ordinal],
                },
                arguments: vec![],
                raw: false,
            })
        })
        .collect()
}
fn public_values() -> Result<(), String> {
    let bundle = fixture();
    let id = |n| DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: vec![n],
    };
    let p = Type::Parameter { declaration: id(7) };
    let q = Type::Parameter { declaration: id(8) };
    let leaf = Type::Classifier {
        declaration: id(0),
        arguments: vec![],
        raw: false,
    };
    let environment = Substitution::new(vec![
        ParameterBinding {
            parameter: id(7),
            argument: q.clone(),
        },
        ParameterBinding {
            parameter: id(8),
            argument: leaf.clone(),
        },
    ])?;
    assert_eq!(environment.apply(&p)?, leaf);
    assert_eq!(environment.apply_simultaneous(&p)?, q);
    let cycle = Substitution::new(vec![
        ParameterBinding {
            parameter: id(7),
            argument: q,
        },
        ParameterBinding {
            parameter: id(8),
            argument: p.clone(),
        },
    ])?;
    assert!(
        cycle
            .apply(&p)
            .unwrap_err()
            .starts_with("Cyclic generic substitution at ")
    );
    assert_eq!(environment.apply(&p)?, leaf);
    Ok(())
}
fn retained_materialization() -> Result<(), String> {
    let bundle = fixture();
    let root = requested(&bundle)?;
    let entry = bundle.sources[0].id.clone();
    let expected = compiler::materialize_runtime_bundle(
        &bundle,
        &entry,
        "urn:substitution-consumers",
        &root,
        None,
    )
    .map_err(|e| e.to_string())?;
    let original = serde_json::to_value(&bundle).map_err(|e| e.to_string())?;
    let mut same = None;
    for quantum in [1, 3, 256] {
        let inputs = bundle.clone();
        let pointer = inputs.sources[0].text.as_ptr();
        let stats = Rc::new(Stats::default());
        let mut job = OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources(
            inputs,
            entry.clone(),
            "urn:substitution-consumers".into(),
            &root,
            None,
            &InvocationContext::default(),
        )
        .map_err(|_| "Retained source admission failed")?;
        let mut advances = 0;
        loop {
            job = std::hint::black_box(job);
            let lease = Rc::new(Lease {
                stats: stats.clone(),
                cancel: false,
            });
            let weak = Rc::downgrade(&lease);
            let done = {
                let context = InvocationContext::new(lease.clone());
                let mut done = false;
                for _ in 0..quantum {
                    advances += 1;
                    if job.advance(None, &context).map_err(|e| e.to_string())? {
                        done = true;
                        break;
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
        let completed = job.finish();
        assert_eq!(
            completed.materialization.bundle.sources[0].text.as_ptr(),
            pointer
        );
        assert_eq!(
            serde_json::to_value(&completed.materialization.bundle).map_err(|e| e.to_string())?,
            original
        );
        let output = completed
            .materialization
            .result
            .map_err(|e| e.to_string())?;
        assert_eq!(
            serde_json::to_value(output).map_err(|e| e.to_string())?,
            serde_json::to_value(&expected).map_err(|e| e.to_string())?
        );
        let observation = (
            advances,
            stats.work.get(),
            stats.storage.get(),
            stats.checkpoints.get(),
        );
        assert!(advances > 30);
        assert_eq!(*same.get_or_insert(observation), observation);
    }
    // Exercise terminal cancellation after actual preparation has progressed.
    let stats = Rc::new(Stats::default());
    let mut pending = OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources(
        bundle.clone(),
        entry.clone(),
        "urn:substitution-consumers".into(),
        &root,
        None,
        &InvocationContext::default(),
    )
    .map_err(|_| "Retained source admission failed")?;
    for _ in 0..12 {
        assert!(
            !pending
                .advance(None, &InvocationContext::default())
                .map_err(|e| e.to_string())?
        );
    }
    let cancel = InvocationContext::new(Rc::new(Lease {
        stats,
        cancel: true,
    }));
    assert!(pending.advance(None, &cancel).is_err());
    assert!(
        pending
            .advance(None, &InvocationContext::default())
            .is_err()
    );
    assert!(pending.finish().materialization.result.is_err());
    assert_eq!(
        serde_json::to_value(
            compiler::materialize_runtime_bundle(
                &bundle,
                &entry,
                "urn:substitution-consumers",
                &root,
                None
            )
            .map_err(|e| e.to_string())?
        )
        .map_err(|e| e.to_string())?,
        serde_json::to_value(expected).map_err(|e| e.to_string())?
    );
    Ok(())
}
fn complete_compiler() -> Result<(), String> {
    let bundle = fixture();
    let entry = &bundle.sources[0].id;
    let original = serde_json::to_value(&bundle).map_err(|e| e.to_string())?;
    let report = checking::check(&bundle, entry).map_err(|e| format!("{e:?}"))?;
    assert!(
        report
            .modules
            .iter()
            .flat_map(|m| m.members.values())
            .any(|m| m.declaration.source == *entry && m.declaration.path == [1, 0])
    );
    let symbol = report
        .signatures
        .functions
        .first()
        .ok_or("Missing read function")?
        .symbol
        .clone();
    let selection = EntrySelection {
        symbol,
        types: vec![],
        arguments: vec![],
    };
    let image = compile_function(&bundle, entry, &selection, &Limits::default())
        .map_err(|e| format!("{e:?}"))?;
    let replay: DefinitionBundle =
        serde_json::from_value(original.clone()).map_err(|e| e.to_string())?;
    let fresh = compile_function(&replay, entry, &selection, &Limits::default())
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(image.program(), fresh.program());
    assert_eq!(
        serde_json::to_value(&bundle).map_err(|e| e.to_string())?,
        original
    );
    let bad = DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            entry.uri.clone(),
            SourceKind::Rcore,
            bundle.sources[0].text.replace("x.value", "x.missing"),
        )],
        namespaces: vec![],
    };
    let errors = checking::check(&bad, &bad.sources[0].id).unwrap_err();
    assert!(errors.iter().any(|e| e.message.contains("Unknown feature")));
    assert!(errors.iter().all(|e| e.span.source == bad.sources[0].id));
    Ok(())
}
pub fn verify() -> Result<u32, String> {
    public_values()?;
    retained_materialization()?;
    complete_compiler()?;
    Ok(3)
}
