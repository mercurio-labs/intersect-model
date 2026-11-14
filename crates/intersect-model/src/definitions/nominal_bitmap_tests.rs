use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
use std::cell::Cell;

struct Meter {
    calls: Cell<usize>,
    bytes: Cell<usize>,
    stop: usize,
    cap: usize,
}
impl Meter {
    fn tick(&self) -> Result<(), String> {
        let at = self.calls.get();
        self.calls.set(at + 1);
        if at == self.stop {
            Err("Bitmap interrupted".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected bitmap provider".into())
    }
    fn work(&self, _: usize) -> Result<(), String> {
        self.tick()
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        let total = self.bytes.get() + n;
        if total > self.cap {
            return Err("Bitmap storage denied".into());
        }
        self.bytes.set(total);
        Ok(())
    }
}
fn meter(stop: usize, cap: usize) -> Rc<Meter> {
    Rc::new(Meter {
        calls: Cell::new(0),
        bytes: Cell::new(0),
        stop,
        cap,
    })
}
fn fixture(bounds: &str) -> (DefinitionBundle, Vec<RuntimeType>) {
    let source = DefinitionSource::new(
        "memory:bitmap.ecore",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:bitmap'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'>{bounds}</eTypeParameters><eStructuralFeatures x:type='e:EReference' name='r'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures><eStructuralFeatures x:type='e:EReference' name='q'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures></eClassifiers><eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B'/><eClassifiers x:type='e:EClass' name='AB' eSuperTypes='#//A #//B'/><eClassifiers x:type='e:EClass' name='OnlyA' eSuperTypes='#//A'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Leaf' eSuperTypes='#//AB'/></e:EPackage>"
        ),
    );
    let types = (0..7)
        .map(|index| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![index],
                },
                arguments: vec![],
                raw: index == 0,
            })
            .unwrap()
        })
        .collect();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        types,
    )
}
fn with_reports(
    bundle: &DefinitionBundle,
    types: &[RuntimeType],
    exercise: impl FnOnce(
        &Resolver,
        &[(String, Rc<PreparedRuntimeMembers<'_>>)],
        &Xml,
        &ParserObservation<'_, '_>,
    ),
) {
    let context = InvocationContext::default();
    let obs = ParserObservation::new(&context);
    let mut resolver = Resolver::new(bundle).unwrap().with_borrowed_owner_inputs();
    resolver
        .resolve_materialization_inventory_observed(&obs)
        .unwrap();
    resolver.enable_parameter_bound_inputs(&obs).unwrap();
    let reports = types
        .iter()
        .map(|ty| {
            (
                key(ty),
                Rc::new(resolver.prepare_runtime_members_observed(ty, &obs).unwrap()),
            )
        })
        .collect::<Vec<_>>();
    let source = &bundle.sources[0];
    let xml = Xml::from_node(
        &resolver
            .resource_sets()
            .0
            .document(&source.id)
            .unwrap()
            .root,
        &source.text,
    )
    .unwrap();
    exercise(&resolver, &reports, &xml, &obs);
}
fn positions(mut mask: NominalMaskScan, obs: &ParserObservation<'_, '_>, xml: &Xml) -> Vec<usize> {
    let mut scan = CandidateScan::<String>::new(true, vec![]);
    while !mask.advance(&mut scan, obs, xml).unwrap() {}
    assert_eq!(scan.next_candidate, mask.len);
    scan.reference_selection
}
#[test]
fn nominal_conjunction_unbounded_and_top_masks_match_every_old_checker_bit() {
    for bounds in [
        "<eBounds eClassifier='#//A'/><eBounds eClassifier='#//B'/>",
        "",
        "<eBounds eClassifier='http://www.eclipse.org/emf/2002/Ecore#//EObject'/>",
    ] {
        let (bundle, types) = fixture(bounds);
        with_reports(&bundle, &types, |_, reports, xml, obs| {
            let index = NominalReferenceIndex::from_reports(reports, xml, obs)
                .unwrap()
                .unwrap();
            let root = &reports[0].1;
            for domain in root.checked_feature_domains() {
                let old = reports
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (_, candidate))| {
                        domain
                            .accepts_observed(&candidate.receiver, obs)
                            .unwrap()
                            .then_some(i)
                    })
                    .collect::<Vec<_>>();
                let selected = positions(index.select(&domain, obs).unwrap().unwrap(), obs, xml);
                assert_eq!(selected, old);
                if bounds.contains("#//B") {
                    assert_eq!(selected, vec![3, 6]);
                } else {
                    assert_eq!(selected.len(), 7);
                }
            }
            assert_eq!(index.rules.borrow().len(), 1); // Both source fields share code, not a result mask.
            assert!(index.active_scan.borrow().is_none());
            let fresh = NominalReferenceIndex::from_reports(reports, xml, obs)
                .unwrap()
                .unwrap();
            assert!(fresh.rules.borrow().is_empty());
        });
    }
}
#[test]
fn masks_reject_foreign_context_and_applied_types_use_original_checker() {
    let (bundle, types) = fixture("<eBounds eClassifier='#//A'/>");
    with_reports(&bundle, &types, |_, reports, xml, obs| {
        let index = NominalReferenceIndex::from_reports(reports, xml, obs)
            .unwrap()
            .unwrap();
        with_reports(&bundle, &types, |_, foreign, _, _| {
            assert!(
                index
                    .select(&foreign[0].1.checked_feature_domain(0).unwrap(), obs)
                    .unwrap()
                    .is_none()
            );
        });
    });
    let mut applied = types.clone();
    applied[0] = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        },
        arguments: vec![types[1].expression().clone()],
        raw: false,
    })
    .unwrap();
    with_reports(&bundle, &applied, |_, reports, xml, obs| {
        assert!(
            NominalReferenceIndex::from_reports(reports, xml, obs)
                .unwrap()
                .is_none()
        );
    });
    let context = InvocationContext::default();
    let obs = ParserObservation::new(&context);
    let resolver = Resolver::new(&bundle).unwrap();
    let invalid = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    assert!(
        resolver
            .prepare_runtime_members_observed(&invalid, &obs)
            .is_err()
    );
}
#[test]
fn bitmap_words_keep_order_tail_and_terminal_cancellation() {
    let (bundle, types) = fixture("");
    with_reports(&bundle, &types, |_, _, xml, obs| {
        for len in [0usize, 1, 63, 64, 65, 128, 129] {
            let fresh = || {
                let mut bits = vec![u64::MAX; len.div_ceil(64)];
                if len % 64 != 0
                    && let Some(last) = bits.last_mut()
                {
                    *last = (1u64 << (len % 64)) - 1;
                }
                NominalMaskScan { bits, len, word: 0 }
            };
            assert_eq!(positions(fresh(), obs, xml), (0..len).collect::<Vec<_>>());
            let measured = meter(usize::MAX, usize::MAX);
            let context = InvocationContext::new(measured.clone());
            positions(fresh(), &ParserObservation::new(&context), xml);
            for stop in 0..measured.calls.get() {
                let stopped = meter(stop, usize::MAX);
                let context = InvocationContext::new(stopped);
                let mut bitmap = fresh();
                let mut scan = CandidateScan::<String>::new(true, vec![]);
                let observed = ParserObservation::new(&context);
                loop {
                    match bitmap.advance(&mut scan, &observed, xml) {
                        Ok(true) => panic!("interruption did not happen"),
                        Ok(false) => {}
                        Err(_) => break,
                    }
                }
                assert!(scan.failed);
                assert!(bitmap.advance(&mut scan, obs, xml).is_err());
                assert_eq!(positions(fresh(), obs, xml), (0..len).collect::<Vec<_>>());
            }
        }
    });
}
#[test]
fn index_and_fresh_selection_precharge_each_allocation_and_recover() {
    let (bundle, types) = fixture("<eBounds eClassifier='#//A'/>");
    with_reports(&bundle, &types, |_, reports, xml, obs| {
        let run = |stop, cap| {
            let m = meter(stop, cap);
            let context = InvocationContext::new(m.clone());
            let observed = ParserObservation::new(&context);
            let result =
                observed.finish(NominalReferenceIndex::from_reports(reports, xml, &observed));
            (result, m.calls.get(), m.bytes.get())
        };
        let (_, calls, bytes) = run(usize::MAX, usize::MAX);
        assert!(run(usize::MAX, bytes - 1).0.is_err());
        assert!(run(usize::MAX, bytes).0.is_ok());
        for stop in 0..calls {
            assert!(run(stop, usize::MAX).0.is_err());
        }
        let domain = reports[0].1.checked_feature_domain(0).unwrap();
        let run = |stop, cap| {
            let index = NominalReferenceIndex::from_reports(reports, xml, obs)
                .unwrap()
                .unwrap();
            index.select(&domain, obs).unwrap();
            let m = meter(stop, cap);
            let context = InvocationContext::new(m.clone());
            let observed = ParserObservation::new(&context);
            let result = observed.finish(index.select(&domain, &observed));
            (result, m.calls.get(), m.bytes.get())
        };
        let (_, calls, bytes) = run(usize::MAX, usize::MAX);
        assert!(run(usize::MAX, bytes - 1).0.is_err());
        assert!(run(usize::MAX, bytes).0.is_ok());
        for stop in 0..calls {
            assert!(run(stop, usize::MAX).0.is_err());
        }
    });
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn cold_opt_in_session_matches_complete_schema_and_every_advance_recovers() {
    let (bundle, types) = fixture("<eBounds eClassifier='#//A'/><eBounds eClassifier='#//B'/>");
    let bundle = crate::definitions::migration_test_fixture::bundle(bundle);
    let types = types
        .into_iter()
        .map(|ty| {
            let Type::Classifier {
                declaration,
                arguments,
                raw,
            } = ty.expression()
            else {
                panic!("classifier expected")
            };
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: bundle.sources[0].id.clone(),
                    path: declaration.path.clone(),
                },
                arguments: arguments.clone(),
                raw: *raw,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    let context = InvocationContext::default();
    let entry = bundle.sources[0].id.clone();
    let old = crate::definitions::compiler::materialize_runtime_bundle(
        &bundle,
        &entry,
        "urn:bitmap",
        &types,
        None,
    )
    .unwrap();
    let start = || {
        OwnedRuntimeSourceArrayPreparation::begin_with_nominal_bitmaps(
            bundle.clone(),
            entry.clone(),
            "urn:bitmap".into(),
            &types,
            None,
            &context,
        )
        .unwrap_or_else(|_| panic!("valid source failed"))
    };
    let mut session = start();
    let mut advances = 0;
    while !session.advance(None, &context).unwrap() {
        advances += 1;
    }
    let current = session.finish().materialization.result.unwrap();
    assert_eq!(
        serde_json::to_vec(&current).unwrap(),
        serde_json::to_vec(&old).unwrap()
    );
    for prefix in 0..=advances {
        let mut session = start();
        for _ in 0..prefix {
            assert!(!session.advance(None, &context).unwrap());
        }
        let stopped = InvocationContext::new(meter(0, usize::MAX));
        assert!(session.advance(None, &stopped).is_err());
        assert!(session.advance(None, &context).is_err());
        assert!(session.finish().materialization.result.is_err());
    }
    let mut fresh = start();
    while !fresh.advance(None, &context).unwrap() {}
    assert_eq!(
        serde_json::to_vec(&fresh.finish().materialization.result.unwrap()).unwrap(),
        serde_json::to_vec(&old).unwrap()
    );
}

#[test]
fn equal_names_from_distinct_sources_keep_distinct_bitmap_positions() {
    let (mut bundle, types) = fixture("<eBounds eClassifier='#//A'/>");
    let foreign = DefinitionSource::new(
        "memory:foreign.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='f' nsURI='urn:foreign'><eClassifiers x:type='e:EClass' name='A'/></e:EPackage>",
    );
    bundle.sources[0].dependencies.push(foreign.id.clone());
    let other = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: foreign.id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    bundle.sources.push(foreign);
    let selected = vec![types[0].clone(), types[1].clone(), other];
    with_reports(&bundle, &selected, |_, reports, xml, obs| {
        let index = NominalReferenceIndex::from_reports(reports, xml, obs)
            .unwrap()
            .unwrap();
        let domain = reports[0].1.checked_feature_domain(0).unwrap();
        let actual = positions(index.select(&domain, obs).unwrap().unwrap(), obs, xml);
        assert_eq!(actual, vec![1]);
        for (i, (_, report)) in reports.iter().enumerate() {
            assert_eq!(
                actual.contains(&i),
                domain.accepts_observed(&report.receiver, obs).unwrap()
            );
        }
    });
}
#[test]
fn lower_arguments_and_established_receiver_bindings_keep_full_matcher_fallback() {
    let (mut bundle, _) = fixture("<eBounds eClassifier='#//A'/>");
    let original = &bundle.sources[0];
    let lower=DefinitionSource::new(original.id.uri.clone(),SourceKind::Ecore,original.text.replacen("<eGenericType eTypeParameter='#//Root/T'/>","<eGenericType eClassifier='#//Root'><eTypeArguments><eLowerBound eClassifier='#//A'/></eTypeArguments></eGenericType>",1));
    let lower_types = (0..7)
        .map(|i| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: lower.id.clone(),
                    path: vec![i],
                },
                arguments: vec![],
                raw: i == 0,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    bundle.sources[0] = lower;
    with_reports(&bundle, &lower_types, |_, reports, xml, obs| {
        let index = NominalReferenceIndex::from_reports(reports, xml, obs)
            .unwrap()
            .unwrap();
        let domain = reports[0].1.checked_feature_domain(0).unwrap();
        assert!(index.select(&domain, obs).unwrap().is_none());
        assert!(domain.compile_nominal_rule_observed(obs).unwrap().is_none());
        for (_, report) in reports {
            domain.accepts_observed(&report.receiver, obs).unwrap();
        }
    });
    let (mut bundle, _) = fixture("<eBounds eClassifier='#//A'/>");
    let original = &bundle.sources[0];
    // Raw Child<U> establishes Root<T> -> U without making the feature concrete.
    let child = DefinitionSource::new(
        original.id.uri.clone(),
        SourceKind::Ecore,
        original.text.replace("</e:EPackage>", "<eClassifiers x:type='e:EClass' name='Child'><eTypeParameters name='U'><eBounds eClassifier='#//A'/></eTypeParameters><eGenericSuperTypes eClassifier='#//Root'><eTypeArguments eTypeParameter='#//Child/U'/></eGenericSuperTypes></eClassifiers></e:EPackage>"),
    );
    let selected = vec![
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: child.id.clone(),
                path: vec![7],
            },
            arguments: vec![],
            raw: true,
        })
        .unwrap(),
    ];
    bundle.sources[0] = child;
    with_reports(&bundle, &selected, |_, reports, xml, obs| {
        let domain = reports[0]
            .1
            .checked_feature_domains()
            .find(|domain| !domain.source().projection.bindings.is_empty())
            .unwrap();
        assert!(!domain.source().projection.bindings.is_empty());
        assert!(domain.compile_nominal_rule_observed(obs).unwrap().is_none());
        assert!(
            NominalReferenceIndex::from_reports(reports, xml, obs)
                .unwrap()
                .is_none()
        );
    });
}
