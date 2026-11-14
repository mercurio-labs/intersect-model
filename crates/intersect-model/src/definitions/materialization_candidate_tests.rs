//! Controls for partial candidate scans over fresh, source-bound reports.
impl<'source> MaterializationState<'source> {
    fn test_ready(&self) -> &MaterializationContext<'source> {
        match self {
            Self::Ready(state) => state,
            Self::Loading(_) => panic!("compatibility fixture is not ready"),
        }
    }
    fn test_ready_mut(&mut self) -> &mut MaterializationContext<'source> {
        match self {
            Self::Ready(state) => state,
            Self::Loading(_) => panic!("compatibility fixture is not ready"),
        }
    }
}
use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Meter {
    cancelled: Cell<bool>,
}
impl InvocationObserver for Meter {
    fn checkpoint(&self) -> Result<(), String> {
        self.work(0)
    }
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider invocation".into())
    }
    fn work(&self, _: usize) -> Result<(), String> {
        if self.cancelled.get() {
            Err("Candidate scan cancelled".into())
        } else {
            Ok(())
        }
    }
}

fn fixture(
    operation: &InvocationContext<'_>,
    exercise: impl FnOnce(
        &mut Context<'_, '_>,
        &crate::definitions::generic_resolution::CheckedRuntimeFeatureDomain<'_>,
        &[RuntimeCandidate<'_>],
        &Xml,
        &[String],
    ),
) {
    let source = DefinitionSource::new(
        "memory:candidates.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:candidates'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'><eBounds eClassifier='#//A'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='r' upperBound='-1'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures></eClassifiers><eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B' eSuperTypes='#//A'/><eClassifiers x:type='e:EClass' name='C'/></e:EPackage>",
    );
    let source = crate::definitions::migration_test_fixture::source(source);
    let types: Vec<_> = (0..4)
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
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let observation = ParserObservation::new(operation);
    let standard_source = standard_datatype_source_observed(&observation).unwrap();
    let mut resolver =
        Resolver::with_standard_source_and_inputs(&bundle, standard_source, None, &observation)
            .unwrap();
    resolver
        .resolve_materialization_inventory_observed(&observation)
        .unwrap();
    let (resources, standard, standard_id) = resolver.resource_sets();
    let mut state = MaterializationContext {
        source_inventory: None,
        reuse_compiler_type_inputs: false,
        pending_revision: None,
        progress: AssemblyProgress::Empty,
        readonly_xml: Default::default(),
        open_features: BTreeMap::new(),
        reference_top: None,
        bundle: &bundle,
        resolver: &resolver,
        resources,
        standard,
        standard_id,
        types: BTreeMap::new(),
        enums: BTreeMap::new(),
        datatypes: BTreeMap::new(),
        declarations: BTreeMap::new(),
    };
    let mut context = Context {
        state: &mut state,
        observation: &observation,
    };
    let reports: Vec<_> = types
        .iter()
        .map(|ty| {
            (
                key(ty),
                resolver
                    .prepare_runtime_members_observed(ty, &observation)
                    .unwrap(),
            )
        })
        .collect();
    let candidates: Vec<_> = reports
        .iter()
        .map(|(key, report)| RuntimeCandidate::reference(key, report, &context).unwrap())
        .collect();
    let admission = reports[0].1.checked_feature_domains().next().unwrap();
    let domain = admission.source();
    let field = context
        .xml_read(domain.projection.source.declaration)
        .unwrap();
    let expected = vec![key(&types[1]), key(&types[2])];
    exercise(&mut context, &admission, &candidates, &field, &expected);
}

#[test]
fn candidate_scan_yields_each_comparison_and_publishes_only_complete_selection() {
    fixture(
        &InvocationContext::default(),
        |context, admission, candidates, field, expected| {
            let mut storage = OwnedAlternatives;
            let mut incomplete_scan =
                CandidateScan::<String>::new(admission.source().reference, vec![999]);
            let incomplete = CandidatePreparation::new(admission, candidates, &mut incomplete_scan);
            assert!(
                incomplete
                    .finish(&mut storage, context.observation, field)
                    .is_err()
            );
            let mut scan = CandidateScan::<String>::new(admission.source().reference, vec![999]);
            let mut preparation = CandidatePreparation::new(admission, candidates, &mut scan);
            let mut remaining = candidates.len();
            for index in 0..candidates.len() {
                assert_eq!(
                    preparation
                        .advance(context, field, &mut storage, &mut remaining)
                        .unwrap(),
                    index + 1 == candidates.len()
                );
                assert_eq!(remaining, candidates.len() - index - 1);
            }
            assert!(
                preparation
                    .advance(context, field, &mut storage, &mut remaining)
                    .unwrap()
            );
            assert_eq!(remaining, 0);
            let CandidateSelection {
                alternatives,
                unresolved,
                reference_selection: selection,
            } = preparation
                .finish(&mut storage, context.observation, field)
                .unwrap();
            assert_eq!(&alternatives, expected);
            assert!(unresolved.is_empty());
            assert!(selection.is_empty());
        },
    );
}

#[test]
fn failed_candidate_scans_are_terminal_and_fresh_attempts_recover() {
    let meter = Rc::new(Meter::default());
    let operation = InvocationContext::new(meter.clone());
    fixture(&operation, |context, admission, candidates, field, _| {
        let mut storage = OwnedAlternatives;
        let mut scan = CandidateScan::<String>::new(admission.source().reference, vec![]);
        let mut preparation = CandidatePreparation::new(admission, candidates, &mut scan);
        let mut remaining = candidates.len();
        assert!(
            !preparation
                .advance(context, field, &mut storage, &mut remaining)
                .unwrap()
        );
        meter.cancelled.set(true);
        assert!(
            preparation
                .advance(context, field, &mut storage, &mut remaining)
                .is_err()
        );
        meter.cancelled.set(false);
        assert!(
            preparation
                .advance(context, field, &mut storage, &mut remaining)
                .is_err()
        );
        assert!(
            preparation
                .finish(&mut storage, context.observation, field)
                .is_err()
        );
    });
    fixture(
        &operation,
        |context, admission, candidates, field, expected| {
            let mut storage = OwnedAlternatives;
            let mut scan = CandidateScan::<String>::new(admission.source().reference, vec![]);
            let mut preparation = CandidatePreparation::new(admission, candidates, &mut scan);
            let mut remaining = candidates.len();
            while !preparation
                .advance(context, field, &mut storage, &mut remaining)
                .unwrap()
            {}
            assert_eq!(
                &preparation
                    .finish(&mut storage, context.observation, field)
                    .unwrap()
                    .alternatives,
                expected
            );
        },
    );
}

#[test]
fn owned_scan_survives_transient_bindings_and_cannot_publish_twice() {
    fixture(
        &InvocationContext::default(),
        |context, admission, candidates, field, expected| {
            let mut storage = OwnedAlternatives;
            let mut scan = CandidateScan::<String>::new(admission.source().reference, vec![]);
            let mut remaining = candidates.len();
            for index in 0..candidates.len() {
                let mut view = CandidatePreparation::new(admission, candidates, &mut scan);
                assert_eq!(
                    view.advance(context, field, &mut storage, &mut remaining)
                        .unwrap(),
                    index + 1 == candidates.len()
                );
            }
            assert_eq!(scan.next_candidate, candidates.len());
            let result = CandidatePreparation::new(admission, candidates, &mut scan)
                .finish(&mut storage, context.observation, field)
                .unwrap();
            assert_eq!(&result.alternatives, expected);
            assert_eq!(remaining, 0);
            let mut rebound = CandidatePreparation::new(admission, candidates, &mut scan);
            assert!(
                rebound
                    .advance(context, field, &mut storage, &mut remaining)
                    .is_err()
            );
            assert!(
                rebound
                    .finish(&mut storage, context.observation, field)
                    .is_err()
            );
        },
    );
}

#[test]
fn publication_cancellation_is_terminal_across_source_view_rebinding() {
    let meter = Rc::new(Meter::default());
    let operation = InvocationContext::new(meter.clone());
    fixture(&operation, |context, admission, candidates, field, _| {
        let mut storage = SharedArrayAlternatives::<
            crate::definitions::schema::SharedRuntimeSourceArrayTarget,
        >::default();
        let mut scan = CandidateScan::new(admission.source().reference, vec![]);
        let mut remaining = candidates.len();
        for _ in candidates {
            CandidatePreparation::new(admission, candidates, &mut scan)
                .advance(context, field, &mut storage, &mut remaining)
                .unwrap();
        }
        meter.cancelled.set(true);
        assert!(
            CandidatePreparation::new(admission, candidates, &mut scan)
                .finish(&mut storage, context.observation, field)
                .is_err()
        );
        assert!(scan.failed);
        meter.cancelled.set(false);
        let mut rebound = CandidatePreparation::new(admission, candidates, &mut scan);
        assert!(
            rebound
                .advance(context, field, &mut storage, &mut remaining)
                .is_err()
        );
        assert!(
            rebound
                .finish(&mut storage, context.observation, field)
                .is_err()
        );
    });
}

fn feature_fixture(
    operation: &InvocationContext<'_>,
    exercise: impl FnOnce(
        FeatureDomainPreparation<'_, '_, OwnedAlternatives>,
        &[String],
        &ParserObservation<'_, '_>,
    ),
) {
    fixture(
        operation,
        |context, admission, candidates, field, expected| {
            let resolver = context.state.resolver;
            let report = resolver
                .prepare_runtime_members_observed(candidates[0].receiver(), context.observation)
                .unwrap();
            assert_eq!(
                report.checked_feature_domain(0).unwrap().source(),
                admission.source()
            );
            assert!(report.checked_feature_domain(1).is_none());
            let reports = vec![(key(candidates[0].receiver()), Rc::new(report))];
            let mut storage = OwnedAlternatives;
            let observation = context.observation;
            let preparation = FeatureDomainPreparation::new(
                context.state,
                &reports,
                CandidateInputs::Views(candidates),
                CandidateInputs::Views(&[]),
                &mut storage,
                field,
            );
            exercise(preparation, expected, observation);
        },
    );
}

#[test]
fn staged_feature_domains_publish_only_after_complete_source_bound_preparation() {
    feature_fixture(&InvocationContext::default(), |preparation, _, _| {
        assert!(preparation.finish().is_err());
    });
    feature_fixture(
        &InvocationContext::default(),
        |mut preparation, expected, observation| {
            let mut phases = Vec::new();
            loop {
                phases.push(preparation.progress.phase);
                if preparation.advance(None, observation).unwrap() {
                    break;
                }
            }
            assert!(phases.contains(&FeaturePreparationPhase::Scan));
            assert!(phases.contains(&FeaturePreparationPhase::Publish));
            assert!(phases.contains(&FeaturePreparationPhase::Commit));
            let selection = preparation.finish().unwrap();
            assert_eq!(selection.domains.len(), 1);
            let domains = selection.domains.values().next().unwrap();
            assert_eq!(domains.len(), 1);
            assert_eq!(&domains[0].alternatives, expected);
            assert!(domains[0].unresolved.is_empty());
            assert!(selection.attribute_defaults.is_empty());
        },
    );
}

#[test]
fn staged_feature_domain_cancellation_is_terminal_at_every_advance_and_fresh_runs_recover() {
    let mut advances = 0;
    feature_fixture(
        &InvocationContext::default(),
        |mut preparation, _, observation| {
            loop {
                advances += 1;
                if preparation.advance(None, observation).unwrap() {
                    break;
                }
            }
            preparation.finish().unwrap();
        },
    );
    for prefix in 0..advances {
        let meter = Rc::new(Meter::default());
        let operation = InvocationContext::new(meter.clone());
        feature_fixture(&operation, |mut preparation, _, observation| {
            for _ in 0..prefix {
                assert!(!preparation.advance(None, observation).unwrap());
            }
            meter.cancelled.set(true);
            assert!(
                preparation.advance(None, observation).is_err(),
                "prefix {prefix}"
            );
            meter.cancelled.set(false);
            assert_eq!(preparation.progress.phase, FeaturePreparationPhase::Failed);
            assert!(preparation.advance(None, observation).is_err());
            assert!(preparation.finish().is_err());
        });
        feature_fixture(&operation, |mut preparation, expected, observation| {
            while !preparation.advance(None, observation).unwrap() {}
            let selection = preparation.finish().unwrap();
            assert_eq!(
                &selection.domains.values().next().unwrap()[0].alternatives,
                expected
            );
        });
    }
}

#[test]
fn staged_advances_release_old_observers_and_use_fresh_cancellation() {
    feature_fixture(&InvocationContext::default(), |mut preparation, _, _| {
        let meter = Rc::new(Meter::default());
        let old_observer = Rc::downgrade(&meter);
        {
            let operation = InvocationContext::new(meter);
            let observation = ParserObservation::new(&operation);
            assert!(!preparation.advance(None, &observation).unwrap());
        }
        assert!(old_observer.upgrade().is_none());
        let meter = Rc::new(Meter::default());
        meter.cancelled.set(true);
        let cancelled_observer = Rc::downgrade(&meter);
        {
            let operation = InvocationContext::new(meter);
            let observation = ParserObservation::new(&operation);
            assert!(preparation.advance(None, &observation).is_err());
        }
        assert!(cancelled_observer.upgrade().is_none());
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        assert!(preparation.advance(None, &observation).is_err());
        assert!(preparation.finish().is_err());
    });
}

#[test]
fn retained_source_views_check_fresh_observers_without_retaining_them() {
    fixture(
        &InvocationContext::default(),
        |context, admission, _, field, _| {
            let id = admission.source().projection.source.declaration;
            let first;
            let meter = Rc::new(Meter::default());
            let old_observer = Rc::downgrade(&meter);
            {
                let operation = InvocationContext::new(meter);
                let observation = ParserObservation::new(&operation);
                let view = Context {
                    state: context.state,
                    observation: &observation,
                };
                first = view.xml_read(id).unwrap();
            }
            assert!(old_observer.upgrade().is_none());
            assert!(std::ptr::eq(first.as_ref(), field));
            let meter = Rc::new(Meter::default());
            meter.cancelled.set(true);
            {
                let operation = InvocationContext::new(meter);
                let observation = ParserObservation::new(&operation);
                let view = Context {
                    state: context.state,
                    observation: &observation,
                };
                assert!(view.xml_read(id).is_err());
            }
            {
                let operation = InvocationContext::default();
                let observation = ParserObservation::new(&operation);
                let view = Context {
                    state: context.state,
                    observation: &observation,
                };
                assert!(Rc::ptr_eq(&first, &view.xml_read(id).unwrap()));
            }
        },
    );
}

struct OwnedReportFixture {
    holder: OwnedMaterializationContext,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    fallback: SourceSpan,
}
fn owned_report_fixture() -> OwnedReportFixture {
    let mut output = None;
    fixture(
        &InvocationContext::default(),
        |context, _, candidates, _, _| {
            let bundle = context.state.bundle.clone();
            let requested: Vec<_> = candidates
                .iter()
                .map(|candidate| candidate.receiver().clone())
                .collect();
            let operation = InvocationContext::default();
            let observation = ParserObservation::new(&operation);
            let InitializedMaterialization {
                resolver,
                inventory,
                fallback,
            } = initialize_materialization::<OwnedAlternatives>(
                &bundle,
                &requested,
                None,
                None,
                &observation,
            )
            .unwrap();
            let owner = MaterializationSourceInputs {
                bundle,
                subset: Default::default(),
                resolver: resolver.into(),
                loading: None,
            };
            let mut holder = OwnedMaterializationContext::try_new(owner, |inputs| {
                new_materialization_context(
                    inputs.selected_bundle(),
                    inputs.resolver()?,
                    &observation,
                )
                .map(|state| MaterializationState::Ready(Box::new(state)))
            })
            .unwrap();
            holder.with_dependent_mut(|_, state| {
                let state = state.test_ready_mut();
                state.set_reports(ReportPreparation::new(&requested, &fallback).unwrap());
            });
            output = Some(OwnedReportFixture {
                holder,
                inventory,
                fallback,
            });
        },
    );
    output.unwrap()
}
impl OwnedReportFixture {
    fn advance(&mut self, operation: &InvocationContext<'_>) -> Result<bool, ImportError> {
        let observation = ParserObservation::new(operation);
        self.holder.with_dependent_mut(|_, state| {
            let state = state.test_ready_mut();
            let mut preparation = state.take_reports().unwrap();
            let mut context = Context {
                state,
                observation: &observation,
            };
            let result = preparation.advance(&mut context, &mut self.inventory, &self.fallback);
            context.state.set_reports(preparation);
            result
        })
    }
    fn phase(&self) -> ReportPreparationPhase {
        self.holder
            .borrow_dependent()
            .test_ready()
            .reports()
            .unwrap()
            .phase
    }
}

#[test]
fn owned_report_progress_survives_moves_parent_advances_and_observer_release() {
    let mut attempt = owned_report_fixture();
    let expected_bundle = attempt.holder.borrow_owner().bundle.clone();
    let mut phases = Vec::new();
    loop {
        phases.push(attempt.phase());
        let meter = Rc::new(Meter::default());
        let weak = Rc::downgrade(&meter);
        let done = {
            let operation = InvocationContext::new(meter);
            attempt.advance(&operation).unwrap()
        };
        assert!(weak.upgrade().is_none());
        // Moving the holder must never invalidate its checked source views.
        let mut moved = vec![attempt];
        attempt = moved.pop().unwrap();
        if done {
            break;
        }
    }
    assert!(phases.contains(&ReportPreparationPhase::Members));
    assert!(phases.contains(&ReportPreparationPhase::Parents));
    assert!(phases.contains(&ReportPreparationPhase::Register));
    assert_eq!(
        attempt.holder.borrow_dependent().test_ready().types.len(),
        4
    );
    attempt.holder.with_dependent_mut(|_, state| {
        let state = state.test_ready_mut();
        let selections = state
            .take_reports()
            .unwrap()
            .finish(&attempt.fallback)
            .unwrap();
        assert_eq!(selections.len(), 4);
        for (key, (_, span)) in selections {
            assert_eq!(span.source, expected_bundle.sources[0].id);
            assert_eq!(state.types[&key].declaration_span.as_ref(), span.as_ref());
        }
    });
    assert_eq!(attempt.holder.into_owner().bundle, expected_bundle);
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn owned_report_cancellation_is_terminal_at_every_advance_and_preserves_input_recovery() {
    let mut baseline = owned_report_fixture();
    let mut advances = 0;
    loop {
        advances += 1;
        if baseline.advance(&InvocationContext::default()).unwrap() {
            break;
        }
    }
    for prefix in 0..advances {
        let mut attempt = owned_report_fixture();
        let expected = attempt.holder.borrow_owner().bundle.clone();
        for _ in 0..prefix {
            assert!(!attempt.advance(&InvocationContext::default()).unwrap());
        }
        let meter = Rc::new(Meter::default());
        meter.cancelled.set(true);
        assert!(attempt.advance(&InvocationContext::new(meter)).is_err());
        assert_eq!(attempt.phase(), ReportPreparationPhase::Failed);
        assert!(attempt.advance(&InvocationContext::default()).is_err());
        attempt.holder.with_dependent_mut(|_, state| {
            let state = state.test_ready_mut();
            assert!(
                state
                    .take_reports()
                    .unwrap()
                    .finish(&attempt.fallback)
                    .is_err()
            );
        });
        assert_eq!(attempt.holder.into_owner().bundle, expected);
        let mut fresh = owned_report_fixture();
        while !fresh.advance(&InvocationContext::default()).unwrap() {}
        assert_eq!(fresh.holder.borrow_dependent().test_ready().types.len(), 4);
    }
}

impl OwnedReportFixture {
    fn begin_classes(&mut self) {
        while !self.advance(&InvocationContext::default()).unwrap() {}
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        self.holder.with_dependent_mut(|_, state| {
            let state = state.test_ready_mut();
            state
                .take_reports()
                .unwrap()
                .finish(&self.fallback)
                .unwrap();
            let mut package =
                definition_id(state.types.values().next().unwrap().receiver.expression())
                    .unwrap()
                    .clone();
            package.path.pop();
            let context = Context {
                state,
                observation: &observation,
            };
            let mut root = context.xml_header(&package).unwrap();
            root.name = "ecore:EPackage".into();
            root.attrs.remove("xmi:id");
            root.attrs.remove("xsi:type");
            let reports = context
                .state
                .types
                .iter()
                .map(|(key, report)| (key.clone(), report.clone()))
                .collect();
            context
                .state
                .set_class(ClassPreparation::new(root, reports));
        });
    }
    fn advance_class(&mut self, operation: &InvocationContext<'_>) -> Result<bool, ImportError> {
        let observation = ParserObservation::new(operation);
        self.holder.with_dependent_mut(|_, state| {
            let state = state.test_ready_mut();
            let mut preparation = state.take_class().unwrap();
            let mut context = Context {
                state,
                observation: &observation,
            };
            let result = preparation.advance(&mut context);
            context.state.set_class(preparation);
            result
        })
    }
}

#[test]
fn owned_class_preparation_retains_order_source_identity_and_releases_observers_across_moves() {
    let mut attempt = owned_report_fixture();
    attempt.begin_classes();
    let bundle = attempt.holder.borrow_owner().bundle.clone();
    let keys: Vec<_> = attempt
        .holder
        .borrow_dependent()
        .test_ready()
        .types
        .keys()
        .cloned()
        .collect();
    for index in 0..keys.len() {
        let meter = Rc::new(Meter::default());
        let weak = Rc::downgrade(&meter);
        {
            let operation = InvocationContext::new(meter);
            assert!(!attempt.advance_class(&operation).unwrap());
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(
            attempt
                .holder
                .borrow_dependent()
                .test_ready()
                .class()
                .unwrap()
                .root
                .children
                .len(),
            index + 1
        );
        let mut moved = vec![attempt];
        attempt = moved.pop().unwrap();
    }
    assert!(
        attempt
            .advance_class(&InvocationContext::default())
            .unwrap()
    );
    attempt.holder.with_dependent_mut(|_, state| {
        let state = state.test_ready_mut();
        let output = state.take_class().unwrap().finish().unwrap();
        assert_eq!(
            output
                .root
                .children
                .iter()
                .map(|node| node.attrs["name"].clone())
                .collect::<Vec<_>>(),
            keys
        );
        assert!(
            output
                .root
                .children
                .iter()
                .all(|node| node.opening.source == bundle.sources[0].id)
        );
        assert_eq!(output.operation_templates.len(), 4);
        assert_eq!(output.instantiations.len(), 1);
    });
    assert_eq!(attempt.holder.into_owner().bundle, bundle);
}

#[test]
fn owned_class_cancellation_is_terminal_at_each_boundary_and_never_publishes_partial_output() {
    for prefix in 0..=4 {
        let mut attempt = owned_report_fixture();
        attempt.begin_classes();
        let bundle = attempt.holder.borrow_owner().bundle.clone();
        for _ in 0..prefix {
            assert!(
                !attempt
                    .advance_class(&InvocationContext::default())
                    .unwrap()
            );
        }
        let meter = Rc::new(Meter::default());
        meter.cancelled.set(true);
        assert!(
            attempt
                .advance_class(&InvocationContext::new(meter))
                .is_err()
        );
        assert!(
            attempt
                .advance_class(&InvocationContext::default())
                .is_err()
        );
        attempt.holder.with_dependent_mut(|_, state| {
            let state = state.test_ready_mut();
            assert!(state.take_class().unwrap().finish().is_err());
        });
        assert_eq!(attempt.holder.into_owner().bundle, bundle);
        let mut fresh = owned_report_fixture();
        fresh.begin_classes();
        while !fresh.advance_class(&InvocationContext::default()).unwrap() {}
        assert_eq!(
            fresh
                .holder
                .borrow_dependent()
                .test_ready()
                .class()
                .unwrap()
                .root
                .children
                .len(),
            4
        );
    }
}

#[test]
fn feature_progress_survives_transient_binding_release_and_moves_without_rechecking_completed_candidates()
 {
    feature_fixture(
        &InvocationContext::default(),
        |preparation, expected, observation| {
            let FeatureDomainPreparation {
                nominal: _,
                context,
                reports,
                reference_candidates,
                attribute_candidates,
                storage,
                at,
                mut progress,
            } = preparation;
            loop {
                let previous = progress.comparisons;
                let scanning = progress.phase == FeaturePreparationPhase::Scan;
                let mut view = FeatureDomainPreparation::new(
                    context,
                    reports,
                    reference_candidates,
                    attribute_candidates,
                    storage,
                    at,
                )
                .with_progress(progress);
                let done = view.advance(None, observation).unwrap();
                progress = view.into_progress();
                assert_eq!(previous - progress.comparisons, usize::from(scanning));
                let mut moved = vec![progress];
                progress = moved.pop().unwrap();
                if done {
                    break;
                }
            }
            let output = FeatureDomainPreparation::new(
                context,
                reports,
                reference_candidates,
                attribute_candidates,
                storage,
                at,
            )
            .with_progress(progress)
            .finish()
            .unwrap();
            assert_eq!(
                &output.domains.values().next().unwrap()[0].alternatives,
                expected
            );
            assert!(output.attribute_defaults.is_empty());
        },
    );
}

#[test]
fn feature_progress_keeps_terminal_cancellation_after_transient_binding_is_dropped() {
    feature_fixture(
        &InvocationContext::default(),
        |preparation, _, observation| {
            let FeatureDomainPreparation {
                nominal: _,
                context,
                reports,
                reference_candidates,
                attribute_candidates,
                storage,
                at,
                mut progress,
            } = preparation;
            let mut view = FeatureDomainPreparation::new(
                context,
                reports,
                reference_candidates,
                attribute_candidates,
                storage,
                at,
            )
            .with_progress(progress);
            assert!(!view.advance(None, observation).unwrap());
            progress = view.into_progress();
            let meter = Rc::new(Meter::default());
            meter.cancelled.set(true);
            {
                let operation = InvocationContext::new(meter);
                let cancelled = ParserObservation::new(&operation);
                let mut view = FeatureDomainPreparation::new(
                    context,
                    reports,
                    reference_candidates,
                    attribute_candidates,
                    storage,
                    at,
                )
                .with_progress(progress);
                assert!(view.advance(None, &cancelled).is_err());
                progress = view.into_progress();
            }
            assert_eq!(progress.phase, FeaturePreparationPhase::Failed);
            let mut rebound = FeatureDomainPreparation::new(
                context,
                reports,
                reference_candidates,
                attribute_candidates,
                storage,
                at,
            )
            .with_progress(progress);
            assert!(rebound.advance(None, observation).is_err());
            assert!(rebound.finish().is_err());
        },
    );
}

fn owned_driver_fixture() -> (OwnedMaterializationPreparation, Vec<RuntimeType>) {
    owned_driver_fixture_with_features(false)
}
fn owned_feature_driver_fixture() -> (OwnedMaterializationPreparation, Vec<RuntimeType>) {
    owned_driver_fixture_with_features(true)
}
fn owned_driver_fixture_with_features(
    features: bool,
) -> (OwnedMaterializationPreparation, Vec<RuntimeType>) {
    let source = DefinitionSource::new(
        "memory:owned-driver.ecore",
        SourceKind::Ecore,
        if features {
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:owned-driver'><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'><eBounds eClassifier='#//Anchor'/></eTypeParameters><eTypeParameters name='D'><eBounds eClassifier='http://www.eclipse.org/emf/2002/Ecore#//EString'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='generals' upperBound='-1'><eGenericType eTypeParameter='#//Base/T'/></eStructuralFeatures><eStructuralFeatures x:type='e:EAttribute' name='name'><eGenericType eTypeParameter='#//Base/D'/></eStructuralFeatures><eOperations name='parent' eType='#//Anchor'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Anchor'/></e:EPackage>"
        } else {
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:owned-driver'><eClassifiers x:type='e:EClass' name='Base'><eOperations name='parent' eType='#//Base'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Base'/></e:EPackage>"
        },
    );
    let source = crate::definitions::migration_test_fixture::source(source);
    let indices: &[usize] = if features { &[1, 2] } else { &[1] };
    let requested: Vec<_> = indices
        .iter()
        .copied()
        .map(|index| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![index],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        })
        .collect();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let operation = InvocationContext::default();
    let observation = ParserObservation::new(&operation);
    let InitializedMaterialization {
        resolver,
        inventory,
        fallback,
    } = initialize_materialization::<
        SharedArrayAlternatives<crate::definitions::schema::SharedRuntimeSourceArrayTarget>,
    >(&bundle, &requested, None, None, &observation)
    .unwrap();
    let owner = MaterializationSourceInputs {
        bundle,
        subset: Default::default(),
        resolver: resolver.into(),
        loading: None,
    };
    let holder = OwnedMaterializationContext::try_new(owner, |inputs| {
        let mut state = new_materialization_context(
            inputs.selected_bundle(),
            inputs.resolver()?,
            &observation,
        )?;
        state.set_reports(ReportPreparation::new(&requested, &fallback)?);
        Ok::<_, ImportError>(MaterializationState::Ready(Box::new(state)))
    })
    .unwrap();
    (
        OwnedMaterializationPreparation {
            generic_agreement_polling: GenericAgreementPolling::Compatibility,
            nominal_bitmaps: false,
            policy: CompilationPolicyInputs::default(),
            holder,
            inventory: Some(inventory),
            fallback,
            observe_membership: false,
            buffered_revision: false,
            incremental_revision: false,
            publication_started: false,
            reports_complete: false,
            features_complete: false,
            classes_complete: false,
            feature_progress: None,
            storage: SharedArrayAlternatives::default(),
            failed: false,
        },
        requested,
    )
}

#[test]
fn owned_driver_resumes_completed_reports_without_losing_inherited_operations() {
    let (mut attempt, requested) = owned_driver_fixture();
    let bundle = attempt.holder.borrow_owner().bundle.clone();
    let operation = InvocationContext::default();
    let expected =
        materialize_context_with_storage_observed(
            &bundle,
            &requested,
            None,
            None,
            &mut SharedArrayAlternatives::<
                crate::definitions::schema::SharedRuntimeSourceArrayTarget,
            >::default(),
            &ParserObservation::new(&operation),
        )
        .unwrap();
    assert!(
        attempt
            .finish(None, &ParserObservation::new(&operation))
            .is_err()
    );
    let mut advances = 0;
    loop {
        let meter = Rc::new(Meter::default());
        let weak = Rc::downgrade(&meter);
        let done = {
            let operation = InvocationContext::new(meter);
            attempt
                .advance_reports(None, &ParserObservation::new(&operation))
                .unwrap()
        };
        assert!(weak.upgrade().is_none());
        advances += 1;
        let mut moved = Some(attempt);
        attempt = moved.take().unwrap();
        if done {
            break;
        }
    }
    assert!(advances > 2);
    while !attempt
        .advance_features(None, &ParserObservation::new(&operation))
        .unwrap()
    {}
    let schema = attempt
        .finish(None, &ParserObservation::new(&operation))
        .unwrap();
    assert_eq!(
        serde_json::to_value(&schema).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert!(
        schema
            .declared_operation_selections
            .as_ref()
            .unwrap()
            .values()
            .any(|selection| !selection.groups.is_empty())
    );
    assert!(
        attempt
            .finish(None, &ParserObservation::new(&operation))
            .is_err()
    );
    assert_eq!(attempt.holder.into_owner().bundle, bundle);
}

#[test]
fn owned_driver_cancellation_is_terminal_before_each_report_and_final_publication() {
    let (mut baseline, _) = owned_driver_fixture();
    let operation = InvocationContext::default();
    let mut advances = 0;
    loop {
        advances += 1;
        if baseline
            .advance_reports(None, &ParserObservation::new(&operation))
            .unwrap()
        {
            break;
        }
    }
    for prefix in 0..=advances {
        let (mut attempt, _) = owned_driver_fixture();
        let bundle = attempt.holder.borrow_owner().bundle.clone();
        for _ in 0..prefix {
            attempt
                .advance_reports(None, &ParserObservation::new(&operation))
                .unwrap();
        }
        let meter = Rc::new(Meter::default());
        meter.cancelled.set(true);
        let cancelled = InvocationContext::new(meter);
        let observation = ParserObservation::new(&cancelled);
        if prefix == advances {
            while !attempt
                .advance_features(None, &ParserObservation::new(&operation))
                .unwrap()
            {}
        }
        let stopped = if prefix == advances {
            attempt.finish(None, &observation).map(|_| true)
        } else {
            attempt.advance_reports(None, &observation)
        };
        assert!(matches!(
            observation.finish(stopped),
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                _
            ))
        ));
        assert!(
            attempt
                .advance_reports(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert!(
            attempt
                .finish(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert_eq!(attempt.holder.into_owner().bundle, bundle);
        let (mut recovery, _) = owned_driver_fixture();
        while !recovery
            .advance_reports(None, &ParserObservation::new(&operation))
            .unwrap()
        {}
        while !recovery
            .advance_features(None, &ParserObservation::new(&operation))
            .unwrap()
        {}
        recovery
            .finish(None, &ParserObservation::new(&operation))
            .unwrap();
    }
}

#[test]
fn retained_feature_driver_shares_complete_inputs_across_moves_and_fresh_observers() {
    let (mut attempt, _) = owned_feature_driver_fixture();
    let operation = InvocationContext::default();
    while !attempt
        .advance_reports(None, &ParserObservation::new(&operation))
        .unwrap()
    {}
    assert!(
        attempt
            .finish(None, &ParserObservation::new(&operation))
            .is_err()
    );
    assert!(
        !attempt
            .advance_features(None, &ParserObservation::new(&operation))
            .unwrap()
    );
    let input_identity = |attempt: &OwnedMaterializationPreparation| {
        let inputs = attempt
            .holder
            .borrow_dependent()
            .test_ready()
            .features()
            .unwrap();
        (
            inputs.reports.as_ptr().cast::<()>(),
            inputs.candidates.as_ptr(),
            inputs
                .reports
                .iter()
                .map(|(_, report)| Rc::as_ptr(report).cast::<()>())
                .collect::<Vec<_>>(),
        )
    };
    let identity = input_identity(&attempt);
    let mut scans = 0;
    let mut phases = Vec::new();
    loop {
        let progress = attempt.feature_progress.as_ref().unwrap();
        let before = progress.comparisons;
        let phase = progress.phase;
        phases.push(phase);
        let meter = Rc::new(Meter::default());
        let weak = Rc::downgrade(&meter);
        let done = {
            let operation = InvocationContext::new(meter);
            attempt
                .advance_features(None, &ParserObservation::new(&operation))
                .unwrap()
        };
        assert!(weak.upgrade().is_none());
        let after = attempt.feature_progress.as_ref().unwrap().comparisons;
        assert_eq!(
            before - after,
            usize::from(phase == FeaturePreparationPhase::Scan)
        );
        scans += before - after;
        assert_eq!(input_identity(&attempt), identity);
        let mut moved = Some(attempt);
        attempt = moved.take().unwrap();
        if done {
            break;
        }
    }
    assert!(scans >= 4);
    assert!(phases.contains(&FeaturePreparationPhase::Publish));
    assert!(phases.contains(&FeaturePreparationPhase::Commit));
    let schema = attempt
        .finish(None, &ParserObservation::new(&operation))
        .unwrap();
    schema.validate_instantiations().unwrap();
    assert!(
        attempt
            .holder
            .borrow_dependent()
            .test_ready()
            .features()
            .is_none()
    );
}

#[test]
fn retained_feature_driver_cancels_at_every_boundary_and_recovers_exact_sources() {
    let operation = InvocationContext::default();
    let prepare = || {
        let (mut attempt, _) = owned_feature_driver_fixture();
        while !attempt
            .advance_reports(None, &ParserObservation::new(&operation))
            .unwrap()
        {}
        attempt
    };
    let mut baseline = prepare();
    let mut advances = 0;
    loop {
        advances += 1;
        if baseline
            .advance_features(None, &ParserObservation::new(&operation))
            .unwrap()
        {
            break;
        }
    }
    let expected = serde_json::to_value(
        baseline
            .finish(None, &ParserObservation::new(&operation))
            .unwrap(),
    )
    .unwrap();
    for prefix in 0..=advances {
        let mut attempt = prepare();
        let bundle = attempt.holder.borrow_owner().bundle.clone();
        for _ in 0..prefix {
            attempt
                .advance_features(None, &ParserObservation::new(&operation))
                .unwrap();
        }
        let meter = Rc::new(Meter::default());
        meter.cancelled.set(true);
        let cancelled = InvocationContext::new(meter);
        let observation = ParserObservation::new(&cancelled);
        assert!(matches!(
            observation.finish(attempt.advance_features(None, &observation)),
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                _
            ))
        ));
        assert!(
            attempt
                .advance_features(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert!(
            attempt
                .finish(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert_eq!(attempt.holder.into_owner().bundle, bundle);
        let mut recovery = prepare();
        while !recovery
            .advance_features(None, &ParserObservation::new(&operation))
            .unwrap()
        {}
        let schema = recovery
            .finish(None, &ParserObservation::new(&operation))
            .unwrap();
        assert_eq!(serde_json::to_value(schema).unwrap(), expected);
    }
}

#[test]
fn owned_driver_rejects_changed_provider_selections_before_report_feature_or_publication() {
    use crate::definitions::conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry,
    };
    struct Provider(ConversionContract);
    impl ConversionProvider for Provider {
        fn contract(&self) -> &ConversionContract {
            &self.0
        }
        fn parse(&self, _: &str) -> Result<Value, String> {
            Err("Unexpected semantic call".into())
        }
        fn format(&self, _: &Value) -> Result<String, String> {
            Err("Unexpected semantic call".into())
        }
        fn validate(&self, _: &Value) -> Result<(), String> {
            Err("Unexpected semantic call".into())
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Err("Unexpected semantic call".into())
        }
    }
    let source = DefinitionSource::new(
        "memory:policy-drift.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:policy-drift'><eClassifiers x:type='e:EDataType' name='Custom' instanceClassName='demo.Custom'/></e:EPackage>",
    );
    let source = crate::definitions::migration_test_fixture::source(source);
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![0],
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let datatype = crate::definitions::compiler::import_datatype_bundle(&bundle, &id).unwrap();
    let provider = Provider(ConversionContract {
        datatype: id,
        provider: ProviderId {
            name: "test.policy".into(),
            version: "1".into(),
        },
        serializable: true,
        declarations: vec![],
        members: vec![],
    });
    let mut changed = ConversionRegistry::default();
    changed.register(&datatype, &provider).unwrap();
    let operation = InvocationContext::default();
    for stage in 0..5 {
        let (mut attempt, _) = owned_feature_driver_fixture();
        let original = attempt.holder.borrow_owner().bundle.clone();
        if stage >= 1 {
            while !attempt
                .advance_reports(None, &ParserObservation::new(&operation))
                .unwrap()
            {}
        }
        if stage >= 2 {
            while !attempt
                .advance_features(None, &ParserObservation::new(&operation))
                .unwrap()
            {}
        }
        if stage == 4 {
            while !attempt
                .advance_classes(None, &ParserObservation::new(&operation))
                .unwrap()
            {}
        }
        let observation = ParserObservation::new(&operation);
        let failure = match stage {
            0 => attempt.advance_reports(Some(&changed), &observation),
            1 => attempt.advance_features(Some(&changed), &observation),
            3 | 4 => attempt.advance_classes(Some(&changed), &observation),
            _ => attempt.finish(Some(&changed), &observation).map(|_| true),
        }
        .unwrap_err();
        assert_eq!(failure.kind, ErrorKind::InvalidDefinition);
        assert!(failure.message.contains("provider selections changed"));
        assert!(
            attempt
                .advance_reports(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert!(
            attempt
                .finish(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert_eq!(attempt.holder.into_owner().bundle, original);
    }
}

#[test]
fn owned_class_driver_lowers_one_real_class_per_advance_without_retaining_observers() {
    let (mut attempt, _) = owned_driver_fixture_with_features(true);
    let operation = InvocationContext::default();
    while !attempt
        .advance_reports(None, &ParserObservation::new(&operation))
        .unwrap()
    {}
    while !attempt
        .advance_features(None, &ParserObservation::new(&operation))
        .unwrap()
    {}
    let classes = attempt.holder.borrow_dependent().test_ready().types.len();
    let owner = attempt.holder.borrow_owner().bundle.sources[0]
        .text
        .as_ptr();
    let mut advances = 0;
    loop {
        let before = attempt
            .holder
            .borrow_dependent()
            .test_ready()
            .class()
            .map_or(0, |progress| progress.root.children.len());
        let meter = Rc::new(Meter::default());
        let weak = Rc::downgrade(&meter);
        let done = {
            let operation = InvocationContext::new(meter);
            attempt
                .advance_classes(None, &ParserObservation::new(&operation))
                .unwrap()
        };
        assert!(weak.upgrade().is_none());
        let state = attempt.holder.borrow_dependent().test_ready();
        let progress = state.class().unwrap();
        assert_eq!(progress.root.children.len() - before, usize::from(!done));
        assert!(state.features().is_none());
        assert!(state.class_inputs().is_some());
        assert_eq!(
            attempt.holder.borrow_owner().bundle.sources[0]
                .text
                .as_ptr(),
            owner
        );
        advances += 1;
        let mut moved = Some(attempt);
        attempt = moved.take().unwrap();
        if done {
            break;
        }
    }
    assert_eq!(advances, classes + 1);
    let schema = attempt
        .finish(None, &ParserObservation::new(&operation))
        .unwrap();
    schema.validate_instantiations().unwrap();
    assert_eq!(schema.classes.len(), classes);
    assert!(
        attempt
            .holder
            .borrow_dependent()
            .test_ready()
            .class()
            .is_none()
    );
    assert!(
        attempt
            .holder
            .borrow_dependent()
            .test_ready()
            .class_inputs()
            .is_none()
    );
}
#[test]
fn owned_class_driver_cancels_at_every_class_boundary_and_recovers_freshly() {
    let operation = InvocationContext::default();
    let start = || {
        let (mut attempt, _) = owned_driver_fixture_with_features(true);
        while !attempt
            .advance_reports(None, &ParserObservation::new(&operation))
            .unwrap()
        {}
        while !attempt
            .advance_features(None, &ParserObservation::new(&operation))
            .unwrap()
        {}
        attempt
    };
    let mut baseline = start();
    let classes = baseline.holder.borrow_dependent().test_ready().types.len();
    while !baseline
        .advance_classes(None, &ParserObservation::new(&operation))
        .unwrap()
    {}
    let expected = serde_json::to_value(
        baseline
            .finish(None, &ParserObservation::new(&operation))
            .unwrap(),
    )
    .unwrap();
    for prefix in 0..=classes + 1 {
        let mut attempt = start();
        for _ in 0..prefix {
            attempt
                .advance_classes(None, &ParserObservation::new(&operation))
                .unwrap();
        }
        let exact = attempt.holder.borrow_owner().bundle.clone();
        let cancelled = Rc::new(Meter::default());
        cancelled.cancelled.set(true);
        let stopped = InvocationContext::new(cancelled);
        assert!(
            attempt
                .advance_classes(None, &ParserObservation::new(&stopped))
                .is_err()
        );
        assert!(
            attempt
                .advance_classes(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert!(
            attempt
                .finish(None, &ParserObservation::new(&operation))
                .is_err()
        );
        assert_eq!(attempt.holder.borrow_owner().bundle, exact);
        let mut fresh = start();
        while !fresh
            .advance_classes(None, &ParserObservation::new(&operation))
            .unwrap()
        {}
        assert_eq!(
            serde_json::to_value(
                fresh
                    .finish(None, &ParserObservation::new(&operation))
                    .unwrap()
            )
            .unwrap(),
            expected
        );
    }
}
