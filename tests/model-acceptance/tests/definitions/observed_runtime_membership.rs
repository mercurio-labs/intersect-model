//! Shared membership predicates under explicit fresh caller observations.
#[path = "common/runtime_lower_bounds.rs"]
mod common;
use intersect_model::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    generic_resolution::Resolver,
};
use std::{cell::Cell, rc::Rc};
#[derive(Default)]
struct Meter {
    polls: Cell<usize>,
    work: Cell<usize>,
    bytes: Cell<usize>,
    cancel_at: Option<usize>,
    work_cap: Option<usize>,
    storage_cap: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("No executable provider expected".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        let next = self.polls.get() + 1;
        self.polls.set(next);
        if self.cancel_at.is_some_and(|at| next >= at) {
            Err("Observed membership cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, amount: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self.work.get().checked_add(amount).ok_or("Work overflow")?;
        if self.work_cap.is_some_and(|cap| next > cap) {
            return Err("Observed membership work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, amount: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self
            .bytes
            .get()
            .checked_add(amount)
            .ok_or("Storage overflow")?;
        if self.storage_cap.is_some_and(|cap| next > cap) {
            return Err("Observed membership storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
#[test]
fn observed_membership_matches_lower_bounds_invariance_and_inherited_source() {
    let f = common::Fixture::new();
    let resolver = Resolver::new(&f.bundle).unwrap();
    let base = f.atom(0);
    let sub = f.atom(1);
    let other = f.atom(2);
    let cases = [
        (f.ty(4, vec![], true), f.pair(&base, &sub)),
        (f.ty(4, vec![], true), f.pair(&sub, &sub)),
        (
            f.ty(4, vec![sub.expression().clone()], false),
            f.pair(&sub, &sub),
        ),
        (
            f.ty(4, vec![sub.expression().clone()], false),
            f.pair(&base, &base),
        ),
        (f.ty(5, vec![], true), f.pair(&base, &sub)),
        (f.ty(4, vec![], true), f.pair(&base, &other)),
        (
            f.ty(10, vec![sub.expression().clone()], false),
            f.pair(&f.pair(&sub, &base), &sub),
        ),
    ];
    for (receiver, actual) in cases {
        let domain = resolver
            .runtime_feature_domains(&receiver)
            .unwrap()
            .into_iter()
            .find(|d| d.projection.source.name == "value")
            .unwrap();
        let expected = resolver.runtime_feature_accepts(&domain, &actual).unwrap();
        let meter = Rc::new(Meter::default());
        let weak = Rc::downgrade(&meter);
        {
            let operation = InvocationContext::new(meter.clone());
            assert_eq!(
                resolver
                    .runtime_feature_accepts_observed(&domain, &actual, &operation)
                    .unwrap(),
                expected
            );
        }
        assert!(meter.work.get() > 0 && meter.bytes.get() > 0);
        drop(meter);
        assert!(weak.upgrade().is_none());
        let mut forged = domain.clone();
        forged.projection.source.span.end += 1;
        assert!(
            resolver
                .runtime_feature_accepts_observed(&forged, &actual, &InvocationContext::default())
                .is_err()
        );
    }
}
#[test]
fn observed_membership_honors_boundaries_and_rechecks_fresh_observers() {
    let f = common::Fixture::new();
    let resolver = Resolver::new(&f.bundle).unwrap();
    let receiver = f.ty(4, vec![], true);
    let actual = f.pair(&f.atom(0), &f.atom(1));
    let domain = resolver
        .runtime_feature_domains(&receiver)
        .unwrap()
        .into_iter()
        .find(|d| d.projection.source.name == "value")
        .unwrap();
    let baseline = Rc::new(Meter::default());
    assert!(
        resolver
            .runtime_feature_accepts_observed(
                &domain,
                &actual,
                &InvocationContext::new(baseline.clone())
            )
            .unwrap()
    );
    for (work_cap, storage_cap) in [
        (Some(0), None),
        (Some(baseline.work.get() - 1), None),
        (None, Some(0)),
        (None, Some(baseline.bytes.get() - 1)),
    ] {
        let meter = Rc::new(Meter {
            work_cap,
            storage_cap,
            ..Meter::default()
        });
        assert!(
            resolver
                .runtime_feature_accepts_observed(
                    &domain,
                    &actual,
                    &InvocationContext::new(meter.clone())
                )
                .is_err()
        );
        assert!(work_cap.is_none_or(|cap| meter.work.get() <= cap));
        assert!(storage_cap.is_none_or(|cap| meter.bytes.get() <= cap));
        assert!(resolver.runtime_feature_accepts(&domain, &actual).unwrap());
    }
    let exact = Rc::new(Meter {
        work_cap: Some(baseline.work.get()),
        storage_cap: Some(baseline.bytes.get()),
        ..Meter::default()
    });
    assert!(
        resolver
            .runtime_feature_accepts_observed(
                &domain,
                &actual,
                &InvocationContext::new(exact.clone())
            )
            .unwrap()
    );
    assert_eq!(exact.work.get(), baseline.work.get());
    assert_eq!(exact.bytes.get(), baseline.bytes.get());
    for cancel_at in [1, baseline.polls.get() / 2, baseline.polls.get()] {
        let cancelled = Rc::new(Meter {
            cancel_at: Some(cancel_at),
            ..Meter::default()
        });
        assert!(
            resolver
                .runtime_feature_accepts_observed(
                    &domain,
                    &actual,
                    &InvocationContext::new(cancelled)
                )
                .is_err()
        );
        let fresh = Rc::new(Meter::default());
        assert!(
            resolver
                .runtime_feature_accepts_observed(
                    &domain,
                    &actual,
                    &InvocationContext::new(fresh.clone())
                )
                .unwrap()
        );
        assert_eq!(fresh.work.get(), baseline.work.get());
        assert_eq!(fresh.bytes.get(), baseline.bytes.get());
    }
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn observed_membership_preserves_126_127_parameter_depth_rejection_without_extra_stack() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        ecore::ErrorKind,
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    for count in [126, 127] {
        let parameters = (0..count)
            .rev()
            .map(|index| {
                let bound = if index + 1 == count {
                    "<eBounds eClassifier='#//Base'/>".to_owned()
                } else {
                    format!("<eBounds eTypeParameter='#//Owner/T{}'/>", index + 1)
                };
                format!("<eTypeParameters name='T{index}'>{bound}</eTypeParameters>")
            })
            .collect::<String>();
        let source = DefinitionSource::new(
            "memory:observed-lower-depth.ecore",
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='limits' nsURI='urn:lower-limits'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Box'><eTypeParameters name='E'/></eClassifiers><eClassifiers x:type='e:EClass' name='Owner'>{parameters}<eStructuralFeatures x:type='e:EReference' name='value'><eGenericType eClassifier='#//Box'><eTypeArguments><eLowerBound eTypeParameter='#//Owner/T0'/></eTypeArguments></eGenericType></eStructuralFeatures></eClassifiers></e:EPackage>"
            ),
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let ty = |index, args, raw| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![index],
                },
                arguments: args,
                raw,
            })
            .unwrap()
        };
        let owner = ty(3, vec![], true);
        let domain = resolver
            .runtime_feature_domains(&owner)
            .unwrap()
            .into_iter()
            .find(|d| d.projection.source.name == "value")
            .unwrap();
        let actual = ty(2, vec![ty(1, vec![], false).expression().clone()], false);
        let result = resolver.runtime_feature_accepts_observed(
            &domain,
            &actual,
            &InvocationContext::default(),
        );
        if count == 127 {
            let intersect_model::definitions::ecore::ObservedImportError::Import(e) =
                result.unwrap_err()
            else {
                panic!("Depth rejection must remain an import error");
            };
            assert_eq!(e.kind, ErrorKind::ResourceLimit);
            assert_eq!(e.message, "Generic conformance budget exceeded");
        } else {
            assert!(!result.unwrap());
        }
    }
}
