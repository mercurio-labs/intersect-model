use super::*;
use crate::definitions::{
    bundle::DefinitionSource,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
use std::{cell::Cell, rc::Rc};
struct Meter {
    calls: Cell<usize>,
    units: Cell<usize>,
    stop: usize,
    bytes: Cell<usize>,
    max_bytes: usize,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        let next = self.bytes.get() + n;
        if next > self.max_bytes {
            return Err("Visit allocation denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        let at = self.calls.get();
        self.calls.set(at + 1);
        if at == self.stop {
            return Err("Fresh attempt cancelled".into());
        }
        self.units.set(self.units.get() + n);
        Ok(())
    }
}
fn meter(stop: usize) -> Rc<Meter> {
    Rc::new(Meter {
        calls: Cell::new(0),
        units: Cell::new(0),
        stop,
        bytes: Cell::new(0),
        max_bytes: usize::MAX,
    })
}
fn fixture() -> (DefinitionBundle, Vec<DeclarationId>) {
    let children = (0..6)
        .map(|i| format!("<eClassifiers x:type='e:EClass' name='C{i}'/>"))
        .collect::<String>();
    let source = DefinitionSource::new(
        "memory:attempt.ecore",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:attempt'>{children}<eClassifiers x:type='e:EEnum' name='Choice'/></e:EPackage>"
        ),
    );
    let ids = (0..7)
        .map(|i| DeclarationId {
            source: source.id.clone(),
            path: vec![i],
        })
        .collect();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        ids,
    )
}
#[test]
fn exact_source_reads_capacity_fallback_and_attempt_local_work() {
    let (bundle, ids) = fixture();
    let resolver = Resolver::new(&bundle).unwrap();
    let context = InvocationContext::default();
    let obs = ParserObservation::new(&context);
    let mut attempt = AttemptClassifiers::new(&resolver);
    for id in &ids {
        assert_eq!(
            attempt.kind(id, &obs).unwrap(),
            resolver.classifier(id).unwrap()
        );
    }
    let measure = |warm| {
        let mut local = AttemptClassifiers::new(&resolver);
        if warm {
            local.kind(&ids[0], &obs).unwrap();
        }
        let m = meter(usize::MAX);
        let context = InvocationContext::new(m.clone());
        local
            .kind(&ids[0], &ParserObservation::new(&context))
            .unwrap();
        m.units.get()
    };
    assert!(measure(true) < measure(false));
    let baseline = measure(false);
    assert_eq!(measure(false), baseline);
    for field in 0..4 {
        let mut changed = ids[0].clone();
        match field {
            0 => changed.source.uri.push('x'),
            1 => changed.source.sha256 = "0".repeat(64),
            2 => changed.path = vec![99],
            _ => changed.path.clear(),
        }
        assert!(attempt.kind(&changed, &obs).is_err());
    }
    assert_eq!(
        attempt.kind(&ids[6], &obs).unwrap(),
        TargetKind::Enumeration
    );
    assert_eq!(attempt.entries.iter().flatten().count(), 4);
}
#[test]
fn every_cold_and_warm_work_interruption_recovers_without_partial_authority() {
    let (bundle, ids) = fixture();
    let resolver = Resolver::new(&bundle).unwrap();
    let context = InvocationContext::default();
    let obs = ParserObservation::new(&context);
    for warm in [false, true] {
        let run = |stop| {
            let mut local = AttemptClassifiers::new(&resolver);
            if warm {
                local.kind(&ids[0], &obs).unwrap();
            }
            let m = meter(stop);
            let context = InvocationContext::new(m.clone());
            let observed = ParserObservation::new(&context);
            let result = observed.finish(local.kind(&ids[0], &observed));
            (
                result,
                m.calls.get(),
                local.entries.iter().flatten().count(),
            )
        };
        let (_, calls, _) = run(usize::MAX);
        for stop in 0..calls {
            let (result, _, retained) = run(stop);
            assert!(result.is_err());
            assert_eq!(retained, usize::from(warm));
            let mut fresh = AttemptClassifiers::new(&resolver);
            assert_eq!(fresh.kind(&ids[0], &obs).unwrap(), TargetKind::Class);
        }
    }
}
#[test]
fn complete_membership_results_and_invalid_raw_inputs_remain_fresh() {
    let source = DefinitionSource::new(
        "memory:fresh-domain.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:fresh-domain'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'><eBounds eClassifier='#//Root'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='r'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/><eClassifiers x:type='e:EClass' name='Other'/></e:EPackage>",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let mut resolver = Resolver::new(&bundle).unwrap().with_borrowed_owner_inputs();
    let context = InvocationContext::default();
    let obs = ParserObservation::new(&context);
    resolver
        .resolve_materialization_inventory_observed(&obs)
        .unwrap();
    resolver.enable_parameter_bound_inputs(&obs).unwrap();
    let candidate = |index, raw| {
        crate::definitions::generic_types::RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![index],
            },
            arguments: vec![],
            raw,
        })
        .unwrap()
    };
    let receiver = candidate(0, true);
    let prepared = resolver
        .prepare_runtime_members_observed(&receiver, &obs)
        .unwrap();
    let domain = prepared.checked_feature_domain(0).unwrap();
    let candidates = [candidate(0, true), candidate(1, false), candidate(2, false)];
    let legacy = candidates
        .iter()
        .map(|c| domain.accepts_observed(c, &obs).unwrap())
        .collect::<Vec<_>>();
    resolver.enable_attempt_classifiers();
    assert_eq!(
        candidates
            .iter()
            .map(|c| domain.accepts_observed(c, &obs).unwrap())
            .collect::<Vec<_>>(),
        legacy
    );
    assert_eq!(legacy, vec![true, true, false]);
    let invalid = candidate(0, false);
    assert!(domain.accepts_observed(&invalid, &obs).is_err());
    let m = meter(usize::MAX);
    let measured = InvocationContext::new(m.clone());
    domain
        .accepts_observed(&candidates[1], &ParserObservation::new(&measured))
        .unwrap();
    for stop in 0..m.calls.get() {
        let context = InvocationContext::new(meter(stop));
        assert!(
            domain
                .accepts_observed(&candidates[1], &ParserObservation::new(&context))
                .is_err()
        );
        assert!(domain.accepts_observed(&candidates[1], &obs).unwrap());
        assert!(domain.accepts_observed(&invalid, &obs).is_err());
    }
}

#[test]
fn inline_visits_keep_complete_inputs_and_precharge_promotion_before_moving() {
    let (bundle, ids) = fixture();
    let span = SourceSpan {
        source: bundle.sources[0].id.clone(),
        start: 0,
        end: 1,
    };
    let context = InvocationContext::default();
    let obs = ParserObservation::new(&context);
    let inputs = (0..18)
        .map(|index| {
            let mut id = ids[0].clone();
            id.path.push(index);
            Rc::new(Type::Classifier {
                declaration: id,
                arguments: vec![],
                raw: false,
            })
        })
        .collect::<Vec<_>>();
    let mut denied_slot = SharedApplicationVisits::inline();
    let cancelled_slot = InvocationContext::new(meter(0));
    assert!(
        denied_slot
            .insert_observed(
                inputs[0].clone(),
                &span,
                &ParserObservation::new(&cancelled_slot)
            )
            .is_err()
    );
    assert!(denied_slot.is_empty());
    denied_slot
        .insert_observed(inputs[0].clone(), &span, &obs)
        .unwrap();
    assert!(Rc::ptr_eq(
        &inputs[0],
        denied_slot.get(inputs[0].as_ref()).unwrap()
    ));
    let mut inline = SharedApplicationVisits::inline();
    let m = meter(usize::MAX);
    let measured = InvocationContext::new(m.clone());
    let observed = ParserObservation::new(&measured);
    for input in &inputs[..16] {
        inline
            .insert_observed(input.clone(), &span, &observed)
            .unwrap();
    }
    assert_eq!(m.bytes.get(), 0); // All sixteen slots are in the local value.
    for input in &inputs[..16] {
        assert!(Rc::ptr_eq(input, inline.get(input.as_ref()).unwrap()));
    }
    let denied = InvocationContext::new(Rc::new(Meter {
        calls: Cell::new(0),
        units: Cell::new(0),
        stop: usize::MAX,
        bytes: Cell::new(0),
        max_bytes: 17 * 128 - 1,
    }));
    assert!(
        inline
            .insert_observed(inputs[16].clone(), &span, &ParserObservation::new(&denied))
            .is_err()
    );
    assert!(inline.get(inputs[16].as_ref()).is_none());
    for input in &inputs[..16] {
        assert!(Rc::ptr_eq(input, inline.get(input.as_ref()).unwrap()));
    }
    for input in &inputs[16..] {
        inline
            .insert_observed(input.clone(), &span, &observed)
            .unwrap();
    }
    assert_eq!(m.bytes.get(), 18 * 128);
    for input in &inputs {
        assert!(inline.contains_observed(input, &span, &obs).unwrap());
    }
    let mut changed = inputs[0].as_ref().clone();
    if let Type::Classifier { declaration, .. } = &mut changed {
        declaration.source.sha256 = "0".repeat(64);
    }
    assert!(!inline.contains_observed(&changed, &span, &obs).unwrap());
    let mut cancelled = SharedApplicationVisits::inline();
    for input in &inputs[..16] {
        cancelled
            .insert_observed(input.clone(), &span, &obs)
            .unwrap();
    }
    let denied = InvocationContext::new(meter(0));
    assert!(
        cancelled
            .insert_observed(inputs[16].clone(), &span, &ParserObservation::new(&denied))
            .is_err()
    );
    for input in &inputs[..16] {
        assert!(Rc::ptr_eq(input, cancelled.get(input.as_ref()).unwrap()));
    }
    cancelled
        .insert_observed(inputs[16].clone(), &span, &obs)
        .unwrap();
    assert!(cancelled.get(inputs[16].as_ref()).is_some());
}
