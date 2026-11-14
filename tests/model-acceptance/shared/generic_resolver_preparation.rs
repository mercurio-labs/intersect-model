//! Shared native/raw-WASM controls for actual resolver source-input jobs.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{ObservedImportError, ParseLimits},
    generic_resolution::{Resolver, ResolverPreparation, ResolverPreparationPhase},
    generic_types::{RuntimeType, Type},
    resources::{DeclarationId, ResourceSet},
};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Stats {
    work: Cell<usize>,
    storage: Cell<usize>,
    calls: Cell<usize>,
}
struct Lease {
    stats: Rc<Stats>,
    cancel: bool,
}
impl Lease {
    fn tick(&self) -> Result<(), String> {
        self.stats.calls.set(self.stats.calls.get() + 1);
        if self.cancel {
            Err("resolver job cancelled".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Source preparation cannot invoke a semantic provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.tick()
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        self.stats.work.set(self.stats.work.get() + n);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        self.stats.storage.set(self.stats.storage.get() + n);
        Ok(())
    }
}
fn observed<T>(
    stats: &Rc<Stats>,
    cancel: bool,
    run: impl FnOnce(&InvocationContext<'_>) -> Result<T, ObservedImportError>,
) -> Result<T, ObservedImportError> {
    let observer = Rc::new(Lease {
        stats: Rc::clone(stats),
        cancel,
    });
    let weak = Rc::downgrade(&observer);
    let result = {
        let context = InvocationContext::new(observer.clone());
        run(&context)
    };
    assert_eq!(
        Rc::strong_count(&observer),
        1,
        "Job retained an old observer"
    );
    drop(observer);
    assert!(weak.upgrade().is_none());
    result
}
fn fixture(kind: SourceKind) -> DefinitionBundle {
    let text = if kind == SourceKind::Ecore {
        "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:resolver-job'><eClassifiers xsi:type='ecore:EClass' name='Node'><eTypeParameters name='T'/><eStructuralFeatures xsi:type='ecore:EAttribute' name='name' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers></ecore:EPackage>"
    } else {
        "rcore 1;package p version \"1\";metamodel \"urn:resolver-job\" prefix p {class Node<T> {attr name:String [0,1];}}"
    };
    let model = DefinitionSource::new(format!("memory:resolver-{kind:?}"), kind, text);
    let mut semantic = DefinitionSource::new(
        "memory:resolver-semantic.rcore",
        SourceKind::RcoreBehavior,
        "Its language consumer owns interpretation; the resolver admits source metadata.",
    );
    semantic.dependencies.push(model.id.clone());
    DefinitionBundle {
        version: 1,
        sources: vec![model, semantic],
        namespaces: vec![],
    }
}
fn output(resolver: &Resolver, bundle: &DefinitionBundle) -> Result<Value, String> {
    let receiver = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: true,
    })?;
    Ok(
        json!({"inventory":resolver.resolve_all().map_err(|e|e.to_string())?,
        "members":resolver.runtime_members(&receiver).map_err(|e|e.to_string())?}),
    )
}
type Receipt = (usize, usize, usize);
type DriveOutcome = (Result<Resolver, ObservedImportError>, usize, Receipt);

fn drive(
    bundle: &DefinitionBundle,
    parsed: Option<ResourceSet>,
    limits: ParseLimits,
    quantum: usize,
) -> Result<DriveOutcome, String> {
    let stats = Rc::new(Stats::default());
    let mut job = match observed(&stats, false, |context| {
        ResolverPreparation::begin(bundle, parsed, limits, context)
    }) {
        Ok(job) => Some(job),
        Err(error) => {
            return Ok((
                Err(error),
                0,
                (stats.work.get(), stats.storage.get(), stats.calls.get()),
            ));
        }
    };
    let mut steps = 0;
    loop {
        for _ in 0..quantum {
            let mut moving = job.take().ok_or("Missing movable resolver job")?;
            steps += 1;
            if steps > 100_000 {
                return Err("Resolver source job did not finish".into());
            }
            match observed(&stats, false, |context| moving.advance(context)) {
                Ok(true) => {
                    return Ok((
                        moving.finish(),
                        steps,
                        (stats.work.get(), stats.storage.get(), stats.calls.get()),
                    ));
                }
                Ok(false) => job = Some(moving),
                Err(error) => {
                    return Ok((
                        Err(error),
                        steps,
                        (stats.work.get(), stats.storage.get(), stats.calls.get()),
                    ));
                }
            }
        }
    }
}

pub fn quantum_inputs_and_fresh_observers() -> Result<(), String> {
    for kind in [SourceKind::Ecore, SourceKind::Rcore] {
        let bundle = fixture(kind);
        let active = Resolver::new(&bundle).map_err(|e| e.to_string())?;
        let expected = output(&active, &bundle)?;
        for reuse in [false, true] {
            let mut baseline = None;
            for quantum in [1, 3, 256] {
                let parsed = if reuse {
                    Some(ResourceSet::new(&bundle)?)
                } else {
                    None
                };
                let (result, steps, receipt) = drive(&bundle, parsed, Default::default(), quantum)?;
                let resolver = result.map_err(|e| e.to_string())?;
                assert_eq!(output(&resolver, &bundle)?, expected);
                assert!(steps > 10 && receipt.0 > 0 && receipt.1 > 0);
                if let Some(previous) = baseline {
                    assert_eq!((steps, receipt), previous);
                } else {
                    baseline = Some((steps, receipt));
                }
            }
        }
        assert_eq!(output(&active, &bundle)?, expected);
    }
    Ok(())
}

pub fn metadata_and_policy_errors_match_ordinary_resolver() -> Result<(), String> {
    for kind in [SourceKind::Ecore, SourceKind::Rcore] {
        let original = fixture(kind);
        let resources = ResourceSet::new(&original)?;
        let mut cases = vec![];
        let mut changed = original.clone();
        changed.sources[1].id.sha256 = "0".repeat(64);
        cases.push(changed);
        let mut changed = original.clone();
        changed.sources[1].text.push(' ');
        cases.push(changed);
        let mut changed = original.clone();
        changed.version = 2;
        changed.sources[1].id.sha256 = "0".repeat(64);
        cases.push(changed);
        let mut changed = original.clone();
        changed.sources.push(changed.sources[1].clone());
        cases.push(changed);
        let mut changed = original.clone();
        changed.sources[1]
            .dependencies
            .push(DefinitionSource::new("memory:missing", SourceKind::Xtext, "missing").id);
        cases.push(changed);
        for changed in cases {
            let expected = Resolver::new(&changed)
                .err()
                .ok_or("Ordinary resolver accepted invalid metadata")?;
            for quantum in [1, 3, 256] {
                let (actual, _, _) = drive(
                    &changed,
                    Some(resources.clone()),
                    Default::default(),
                    quantum,
                )?;
                assert_eq!(
                    actual.err(),
                    Some(ObservedImportError::Import(expected.clone()))
                );
            }
        }
        let base = &original.sources[0];
        let text = if kind == SourceKind::Ecore {
            base.text.replace(
                "</ecore:EPackage>",
                &format!("<!--{}--></ecore:EPackage>", "x".repeat(260_000)),
            )
        } else {
            format!("{}/*{}*/", base.text, "x".repeat(260_000))
        };
        let source = DefinitionSource::new(&base.id.uri, kind, text);
        let limits = ParseLimits {
            source_bytes: source.text.len(),
        };
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let resources = ResourceSet::new_with_limits(&bundle, limits).map_err(|e| e.to_string())?;
        for requested in [
            ParseLimits::default(),
            ParseLimits {
                source_bytes: limits.source_bytes - 1,
            },
        ] {
            let expected = Resolver::new_with_limits(&bundle, requested)
                .err()
                .ok_or("Ordinary limit rejection missing")?;
            let (actual, _, _) = drive(&bundle, Some(resources.clone()), requested, 3)?;
            assert_eq!(actual.err(), Some(ObservedImportError::Import(expected)));
        }
        let expected = output(
            &Resolver::new_with_limits(&bundle, limits).map_err(|e| e.to_string())?,
            &bundle,
        )?;
        let (actual, _, _) = drive(&bundle, Some(resources), limits, 256)?;
        assert_eq!(
            output(&actual.map_err(|e| e.to_string())?, &bundle)?,
            expected
        );
    }
    Ok(())
}

pub fn cancellation_each_advance_is_terminal_and_recovers() -> Result<(), String> {
    let bundle = fixture(SourceKind::Ecore);
    let active = Resolver::new(&bundle).map_err(|e| e.to_string())?;
    let expected = output(&active, &bundle)?;
    for reuse in [false, true] {
        let parsed = if reuse {
            Some(ResourceSet::new(&bundle)?)
        } else {
            None
        };
        let (_, steps, _) = drive(&bundle, parsed, Default::default(), 1)?;
        for prefix in 0..=steps {
            let context = InvocationContext::default();
            let parsed = if reuse {
                Some(ResourceSet::new(&bundle)?)
            } else {
                None
            };
            let mut job = ResolverPreparation::begin(&bundle, parsed, Default::default(), &context)
                .map_err(|e| e.to_string())?;
            assert!(job.phase() == ResolverPreparationPhase::Admission);
            for _ in 0..prefix {
                job.advance(&context).map_err(|e| e.to_string())?;
            }
            let stats = Rc::new(Stats::default());
            let error = observed(&stats, true, |context| job.advance(context))
                .err()
                .ok_or("Cancellation did not stop resolver job")?;
            assert_eq!(
                error,
                ObservedImportError::Observation("resolver job cancelled".into())
            );
            assert_eq!(job.advance(&context).err(), Some(error.clone()));
            assert_eq!(job.finish().err(), Some(error));
            assert_eq!(output(&active, &bundle)?, expected);
            let parsed = if reuse {
                Some(ResourceSet::new(&bundle)?)
            } else {
                None
            };
            let (fresh, _, _) = drive(&bundle, parsed, Default::default(), 3)?;
            assert_eq!(
                output(&fresh.map_err(|e| e.to_string())?, &bundle)?,
                expected
            );
        }
    }
    let context = InvocationContext::default();
    let unfinished = ResolverPreparation::begin(&bundle, None, Default::default(), &context)
        .map_err(|e| e.to_string())?;
    assert!(unfinished.finish().is_err());
    Ok(())
}

// Complete public raw/member/error replay supplements the source-job controls.
// Expected reports were captured at 498093ef before member orchestration changed.
pub fn frozen_public_runtime_member_reports() -> Result<(), String> {
    let baseline: Value = serde_json::from_str(include_str!(
        "../../../crates/intersect-model/src/definitions/generic_runtime_members_baseline.json"
    ))
    .map_err(|e| e.to_string())?;
    let source = DefinitionSource::new(
        "memory:member-report-domains.ecore",
        SourceKind::Ecore,
        include_str!(
            "../../../crates/intersect-model/assets/fixtures/ecore-generics/raw-feature-domains.ecore"
        ),
    );
    let plain = |index, arguments, raw| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![index],
        },
        arguments,
        raw,
    };
    for case in 12..36 {
        let (bundle, expression) = if case == 35 {
            let source = DefinitionSource::new(
                "memory:member-report-method.ecore",
                SourceKind::Ecore,
                r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:report-method"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures><eOperations name="pick"><eTypeParameters name="U"><eBounds eClassifier="#//Base"/></eTypeParameters><eGenericType eTypeParameter="#//Box/T"/><eParameters name="input"><eGenericType eTypeParameter="#//Box/pick/U"/></eParameters></eOperations></eClassifiers></e:EPackage>"##,
            );
            let expression = Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![1],
                },
                arguments: vec![],
                raw: true,
            };
            (
                DefinitionBundle {
                    version: 1,
                    sources: vec![source],
                    namespaces: vec![],
                },
                expression,
            )
        } else {
            let expression = match case {
                12..=29 => plain(case - 12, vec![], true),
                30 => plain(5, vec![plain(1, vec![], false)], false),
                31 => plain(5, vec![plain(3, vec![], false)], false),
                32 => plain(12, vec![plain(11, vec![], false)], false),
                33 => plain(12, vec![plain(1, vec![], false)], false),
                _ => plain(
                    17,
                    vec![plain(0, vec![], false), plain(1, vec![], false)],
                    false,
                ),
            };
            (
                DefinitionBundle {
                    version: 1,
                    sources: vec![source.clone()],
                    namespaces: vec![],
                },
                expression,
            )
        };
        let original = serde_json::to_value(&bundle).map_err(|e| e.to_string())?;
        let receiver = RuntimeType::new(expression)?;
        let resolver = Resolver::new(&bundle).map_err(|e| e.to_string())?;
        let output = match resolver.runtime_members(&receiver) {
            Ok(report) => json!({"ok":report}),
            Err(error) => json!({"error":format!("Import({error:?})")}),
        };
        assert_eq!(
            output,
            baseline[case * 5]["output"],
            "raw report case {case}"
        );
        assert_eq!(
            serde_json::to_value(&bundle).map_err(|e| e.to_string())?,
            original
        );
    }
    Ok(())
}

pub fn source_member_consumers() -> Result<(), String> {
    use intersect_model::definitions::generic_types::ClosedType;
    let body = r##"
<eClassifiers x:type="e:EClass" name="Value"/>
<eClassifiers x:type="e:EClass" name="Base">
  <eTypeParameters name="T"/>
  <eStructuralFeatures x:type="e:EReference" name="value" lowerBound="1" upperBound="-1"><eGenericType eTypeParameter="#//Base/T"/><eAnnotations source="retained"/></eStructuralFeatures>
  <eOperations name="convert">
    <eTypeParameters name="T"><eBounds eTypeParameter="#//Base/T"/></eTypeParameters>
    <eGenericType eTypeParameter="#//Base/convert/T"/>
    <eParameters name="input"><eGenericType eTypeParameter="#//Base/T"/></eParameters>
    <eGenericExceptions eClassifier="#//Value"/>
  </eOperations>
</eClassifiers>
<eClassifiers x:type="e:EClass" name="Child"><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="child"><eGenericType eClassifier="#//Child"/></eStructuralFeatures></eClassifiers>
"##;
    for bad in [false, true] {
        let body = if bad {
            body.replace("e:EReference\" name=\"value", "e:EAttribute\" name=\"value")
        } else {
            body.to_string()
        };
        let source = DefinitionSource::new(
            "memory:source-projection-consumer.ecore",
            SourceKind::Ecore,
            format!(
                r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">{body}</e:EPackage>"#
            ),
        );
        let id = |path| DeclarationId {
            source: source.id.clone(),
            path,
        };
        let usage = id(vec![3, 0, 0]);
        let receiver = Type::Classifier {
            declaration: id(vec![2]),
            arguments: vec![],
            raw: false,
        };
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).map_err(|e| e.to_string())?;
        let result = resolver.member_types(&usage);
        if bad {
            assert!(
                result
                    .unwrap_err()
                    .message
                    .contains("attribute/reference kind")
            );
            continue;
        }
        let report = result.map_err(|e| e.to_string())?;
        let closed = resolver
            .instantiate_members(&ClosedType::new(receiver)?)
            .map_err(|e| e.to_string())?;
        assert_eq!(report.members, closed.members);
        assert_eq!(report.members.len(), 2);
        let value = Type::Classifier {
            declaration: id(vec![0]),
            arguments: vec![],
            raw: false,
        };
        let feature = &report.members[0];
        assert_eq!(feature.declaration, id(vec![1, 1]));
        assert_eq!(feature.signatures[0].specialized, vec![value.clone()]);
        assert!(feature.source_payload.contains("upperBound=\"-1\""));
        assert!(feature.source_payload.contains("source=\"retained\""));
        let operation = &report.members[1];
        assert_eq!(operation.declaration, id(vec![1, 2]));
        assert_eq!(operation.formals[0].bounds, vec![value.clone()]);
        assert_eq!(
            operation.formals[0].declaration.declaration,
            id(vec![1, 2, 0])
        );
        assert_eq!(
            operation.signatures[0].specialized,
            vec![Type::Parameter {
                declaration: id(vec![1, 2, 0])
            }]
        );
        assert_eq!(operation.signatures[1].feature, "eExceptions");
        assert_eq!(operation.signatures[1].specialized, vec![value.clone()]);
        assert_eq!(operation.signatures[2].owner, id(vec![1, 2, 2]));
        assert_eq!(operation.signatures[2].specialized, vec![value]);
        assert_eq!(bundle.sources[0].text, source.text);
    }
    Ok(())
}

pub fn verify() -> Result<u32, String> {
    quantum_inputs_and_fresh_observers()?;
    metadata_and_policy_errors_match_ordinary_resolver()?;
    cancellation_each_advance_is_terminal_and_recovers()?;
    frozen_public_runtime_member_reports()?;
    source_member_consumers()?;
    Ok(5)
}
