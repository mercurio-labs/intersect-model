//! Production public API consumers shared by native and zero-import WASM hosts.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{self, OwnedRuntimeSourceArrayPreparation},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    generic_resolution::Resolver,
    generic_types::{RuntimeType, Type},
    rcore_behavior::{
        checking,
        function_lowering::compile_function,
        ir_v2::{EntrySelection, Limits},
    },
    resources::{DeclarationId, ResourceSet},
};
use std::{cell::Cell, rc::Rc};
fn fixture() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:checked-normalization-consumers.rcore",
            SourceKind::Rcore,
            "rcore 1;package substitution version \"1\";metamodel \"urn:checked-normalization-consumers\" prefix s {class Leaf {}class Box<T> {ref value:T [0,1];}class Child extends Box<Leaf> {}}behavior {import model \"urn:checked-normalization-consumers\" as m;fn read(x:m.Child)->m.Leaf?=x.value;}",
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
        Err("Unexpected checked normalization provider".into())
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
fn authored(case: usize) -> (DefinitionBundle, String) {
    let rcore = case >= 12;
    let kind = case % 12;
    let generic = match kind {
        0 => "<eGenericType eClassifier='#//Leaf'/>",
        1 => "<eGenericType eClassifier='#//Box'/>",
        2 => "<eGenericType eTypeParameter='#//Host/P'/>",
        3 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments eClassifier='#//Leaf'/></eGenericType>"
        }
        4 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments><eUpperBound eClassifier='#//Base'/></eTypeArguments></eGenericType>"
        }
        5 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments><eLowerBound eClassifier='#//Leaf'/></eTypeArguments></eGenericType>"
        }
        6 => {
            "<eGenericType eClassifier='#//Pair'><eTypeArguments eClassifier='#//Box'><eTypeArguments eClassifier='#//Leaf'/></eTypeArguments><eTypeArguments/></eGenericType>"
        }
        7 => "<eGenericType eClassifier='#//Missing'/>",
        8 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments eClassifier='#//Leaf'/><eTypeArguments eClassifier='#//Base'/></eGenericType>"
        }
        9 => "<eGenericType eClassifier='#//Leaf' eTypeParameter='#//Host/P'/>",
        10 => "<eGenericType eClassifier='memory:normalization-dependency.ecore#//Foreign'/>",
        _ => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments><eUpperBound eClassifier='#//Base'/><eLowerBound eClassifier='#//Leaf'/></eTypeArguments></eGenericType>"
        }
    };
    let text = if rcore {
        let ty = match kind {
            0 => "Leaf",
            1 => "Box",
            2 => "P",
            3 => "Box<Leaf>",
            4 => "Box<? extends Base>",
            5 => "Box<? super Leaf>",
            6 => "Pair<Box<Leaf>,?>",
            7 => "Missing",
            8 => "Box<Leaf,Base>",
            9 => "Pair<P,Box<Leaf>>",
            10 => "Pair<Box<Box<Leaf>>,Base>",
            _ => "Box<?>",
        };
        format!(
            "rcore 1;package normalize version \"1\";metamodel \"urn:normalize\" prefix n {{class Base {{}}class Leaf extends Base {{}}class Box<T> {{}}class Pair<A,B> {{}}class Host<P> {{ref value:{ty} [0,1];}}}}"
        )
    } else {
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='normalize' nsURI='urn:normalize'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Leaf' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Box'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Pair'><eTypeParameters name='A'/><eTypeParameters name='B'/></eClassifiers><eClassifiers x:type='e:EClass' name='Host'><eTypeParameters name='P'/><eStructuralFeatures x:type='e:EReference' name='value'>{generic}</eStructuralFeatures></eClassifiers></e:EPackage>"
        )
    };
    let source = DefinitionSource::new(
        if kind == 6 {
            "memory:normalization-\u{1f30d}-\u{e9}.rcore"
        } else {
            "memory:normalization-root"
        },
        if rcore {
            SourceKind::Rcore
        } else {
            SourceKind::Ecore
        },
        text,
    );
    let mut sources = vec![source];
    if !rcore && kind == 10 {
        sources.push(DefinitionSource::new("memory:normalization-dependency.ecore",SourceKind::Ecore,"<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='dep' nsURI='urn:dep'><eClassifiers x:type='e:EClass' name='Foreign'/></e:EPackage>"));
    }
    (
        DefinitionBundle {
            version: 1,
            sources,
            namespaces: vec![],
        },
        "#//Host/value/@eGenericType.0".into(),
    )
}
fn public_values() -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::cell::RefCell;
    struct Ordered {
        events: Rc<RefCell<Vec<(char, usize)>>>,
    }
    impl InvocationObserver for Ordered {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn checkpoint(&self) -> Result<(), String> {
            self.events.borrow_mut().push(('c', 0));
            Ok(())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            self.events.borrow_mut().push(('w', n));
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            self.events.borrow_mut().push(('s', n));
            Ok(())
        }
    }
    let frozen: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../src/definitions/generic_normalization_preparation_baseline.json"
    ))
    .map_err(|e| e.to_string())?;
    for (case, original_profile) in frozen.iter().take(24).enumerate() {
        let (bundle, fragment) = authored(case);
        let original = serde_json::to_value(&bundle).map_err(|e| e.to_string())?;
        let observation = match Resolver::new(&bundle) {
            Err(e) => serde_json::json!({"setup_error":format!("{e:?}")}),
            Ok(resolver) => {
                match ResourceSet::new(&bundle)?.resolve(&bundle.sources[0].id, &fragment) {
                    Err(e) => serde_json::json!({"selection_error":e}),
                    Ok(selected) => {
                        let events = Rc::new(RefCell::new(vec![]));
                        let context = InvocationContext::new(Rc::new(Ordered {
                            events: events.clone(),
                        }));
                        let output = match resolver.type_expression_observed(&selected, &context) {
                            Ok(v) => serde_json::json!({"ok":v}),
                            Err(e) => serde_json::json!({"error":format!("{e:?}")}),
                        };
                        let captured = events.borrow().clone();
                        serde_json::json!({"output":output,"events":captured})
                    }
                }
            }
        };
        let complete = serde_json::json!({"authored":case,"observation":observation});
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&complete).map_err(|e| e.to_string())?)
        );
        if hash
            != original_profile["complete_sha256"]
                .as_str()
                .ok_or("Missing frozen hash")?
        {
            return Err(format!("Public complete profile{case} changed"));
        }
        assert_eq!(
            serde_json::to_value(&bundle).map_err(|e| e.to_string())?,
            original
        );
    }
    Ok(())
}
fn retained_materialization() -> Result<(), String> {
    let bundle = fixture();
    let root = requested(&bundle)?;
    let entry = bundle.sources[0].id.clone();
    let expected = compiler::materialize_runtime_bundle(
        &bundle,
        &entry,
        "urn:checked-normalization-consumers",
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
            "urn:checked-normalization-consumers".into(),
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
        "urn:checked-normalization-consumers".into(),
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
                "urn:checked-normalization-consumers",
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
