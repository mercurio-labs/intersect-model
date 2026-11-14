use super::*;
use crate::definitions::bundle::DefinitionSource;
use crate::definitions::conversion::{InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};
struct Meter {
    calls: Cell<usize>,
    bytes: Cell<usize>,
    work: Cell<usize>,
    stop: usize,
    byte_limit: usize,
    work_limit: usize,
}
impl Meter {
    fn new(stop: usize, byte_limit: usize, work_limit: usize) -> Rc<Self> {
        Rc::new(Self {
            calls: Cell::new(0),
            bytes: Cell::new(0),
            work: Cell::new(0),
            stop,
            byte_limit,
            work_limit,
        })
    }
    fn tick(&self) -> Result<(), String> {
        let n = self.calls.get();
        self.calls.set(n + 1);
        if n == self.stop {
            Err("resource preparation cancelled".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.tick()
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        let sum = self.work.get() + n;
        if sum > self.work_limit {
            return Err("resource work denied".into());
        }
        self.work.set(sum);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        let sum = self.bytes.get() + n;
        if sum > self.byte_limit {
            return Err("resource storage denied".into());
        }
        self.bytes.set(sum);
        Ok(())
    }
}
fn fixture() -> DefinitionBundle {
    serde_json::from_str(include_str!(
        "../../assets/fixtures/ecore-generics/resource-preparation/bundle.json"
    ))
    .unwrap()
}

fn run(
    bundle: &DefinitionBundle,
    meter: Rc<Meter>,
) -> Result<(ResourceSet, usize), ObservedImportError> {
    run_policy(bundle, meter, false)
}
fn run_policy(
    bundle: &DefinitionBundle,
    meter: Rc<Meter>,
    retain_bundle: bool,
) -> Result<(ResourceSet, usize), ObservedImportError> {
    let context = InvocationContext::new(meter.clone());
    let mut prep = if retain_bundle {
        ResourceSetPreparation::begin_with_retained_bundle(
            bundle,
            crate::definitions::ecore::ParseLimits::default(),
            &context,
        )?
    } else {
        ResourceSetPreparation::begin(
            bundle,
            crate::definitions::ecore::ParseLimits::default(),
            &context,
        )?
    };
    let mut advances = 0;
    loop {
        advances += 1;
        if prep.advance(&InvocationContext::new(meter.clone()))? {
            break;
        }
    }
    Ok((prep.finish()?, advances))
}
#[test]
fn retained_resource_sets_preserve_literal_root_package_id_reference_and_payload_identity() {
    let bundle = fixture();
    let before = bundle.clone();
    let (actual, advances) = run(&bundle, Meter::new(usize::MAX, usize::MAX, usize::MAX)).unwrap();
    assert!(advances > 80);
    assert_eq!(
        format!("{actual:?}"),
        format!("{:?}", ResourceSet::new(&bundle).unwrap())
    );
    assert_eq!(bundle, before);
    let a = &bundle.sources[1].id;
    let b = &bundle.sources[0].id;
    assert_eq!(
        actual.resources[a]
            .roots
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>(),
        [vec![0], vec![1]]
    );
    assert_eq!(
        actual.resources[a]
            .packages
            .iter()
            .map(|(uri, id)| (uri.as_str(), id.path.clone()))
            .collect::<Vec<_>>(),
        [
            ("urn:a", vec![0]),
            ("urn:a:inner", vec![0, 0]),
            ("urn:a:peer", vec![1])
        ]
    );
    for (name, path) in [
        ("aPkg", vec![0]),
        ("innerPkg", vec![0, 0]),
        ("innerNode", vec![0, 0, 0]),
        ("outerNode", vec![0, 1]),
        ("foreign", vec![0, 2]),
        ("peerPkg", vec![1]),
    ] {
        assert_eq!(
            actual.resources[a].ids[name],
            DeclarationId {
                source: a.clone(),
                path
            }
        );
    }
    for (from, href, target, path) in [
        (a, "urn:a:inner#//Node", a, vec![0, 0, 0]),
        (a, "#innerNode", a, vec![0, 0, 0]),
        (a, "#/0/@eSubpackages.0/@eClassifiers.0", a, vec![0, 0, 0]),
        (b, "a.ecore#innerNode", a, vec![0, 0, 0]),
        (a, "other.ecore#//Node", b, vec![0]),
    ] {
        assert_eq!(
            actual.resolve(from, href).unwrap(),
            DeclarationId {
                source: target.clone(),
                path
            }
        );
    }
    assert!(
        actual
            .resolve(a, "#//Node")
            .unwrap_err()
            .contains("Ambiguous default root")
    );
    assert_eq!(actual.document(a).unwrap().source, bundle.sources[1]);
}
#[test]
fn retained_resource_sets_observe_every_interruption_and_adjacent_limits() {
    let bundle = fixture();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let expected = run(&bundle, meter.clone()).unwrap().0;
    for stop in 0..meter.calls.get() {
        assert!(
            matches!(run(&bundle,Meter::new(stop,usize::MAX,usize::MAX)),Err(ObservedImportError::Observation(message)) if message=="resource preparation cancelled")
        );
    }
    for (bytes, work, success) in [
        (meter.bytes.get() - 1, meter.work.get(), false),
        (meter.bytes.get(), meter.work.get() - 1, false),
        (meter.bytes.get(), meter.work.get(), true),
    ] {
        assert_eq!(
            run(&bundle, Meter::new(usize::MAX, bytes, work)).is_ok(),
            success
        );
    }
    assert_eq!(
        format!(
            "{:?}",
            run(&bundle, Meter::new(usize::MAX, usize::MAX, usize::MAX))
                .unwrap()
                .0
        ),
        format!("{expected:?}")
    );
}
#[test]
fn retained_resource_sets_cancel_terminally_at_every_advance_and_do_not_publish_partial_maps() {
    let bundle = fixture();
    let (_, advances) = run(&bundle, Meter::new(usize::MAX, usize::MAX, usize::MAX)).unwrap();
    let context = InvocationContext::default();
    assert!(
        ResourceSetPreparation::begin(
            &bundle,
            crate::definitions::ecore::ParseLimits::default(),
            &context
        )
        .unwrap()
        .finish()
        .is_err()
    );
    for boundary in 0..=advances {
        let mut prep = ResourceSetPreparation::begin(
            &bundle,
            crate::definitions::ecore::ParseLimits::default(),
            &context,
        )
        .unwrap();
        for _ in 0..boundary {
            prep.advance(&context).unwrap();
        }
        let error = prep
            .advance(&InvocationContext::new(Meter::new(
                0,
                usize::MAX,
                usize::MAX,
            )))
            .unwrap_err();
        assert_eq!(
            error,
            ObservedImportError::Observation("resource preparation cancelled".into())
        );
        assert_eq!(prep.advance(&context).unwrap_err(), error);
        assert_eq!(prep.finish().unwrap_err(), error);
    }
}
#[test]
fn retained_resource_sets_keep_exact_revision_selection_and_ambiguity() {
    let mut bundle = fixture();
    let a = bundle.sources[1].clone();
    let mut other = DefinitionSource::new(
        a.id.uri.clone(),
        SourceKind::Ecore,
        a.text.replace("name=\"a\"", "name=\"revised\""),
    );
    other.dependencies = a.dependencies;
    let other_id = other.id.clone();
    bundle.sources.push(other);
    let actual = run(&bundle, Meter::new(usize::MAX, usize::MAX, usize::MAX))
        .unwrap()
        .0;
    assert_eq!(
        actual
            .resolve(&bundle.sources[0].id, "a.ecore#innerNode")
            .unwrap()
            .source,
        bundle.sources[1].id
    );
    bundle.sources[0].dependencies.push(other_id);
    let actual = run(&bundle, Meter::new(usize::MAX, usize::MAX, usize::MAX))
        .unwrap()
        .0;
    assert!(
        actual
            .resolve(&bundle.sources[0].id, "a.ecore#innerNode")
            .unwrap_err()
            .contains("Ambiguous resource revision")
    );
    assert_eq!(
        actual
            .resolve(&bundle.sources[0].id, "urn:a:inner#//Node")
            .unwrap()
            .source,
        bundle.sources[1].id
    );
}
#[test]
fn retained_resource_sets_preserve_first_error_kind_span_and_priority() {
    let mut variants = Vec::new();
    for text in [
        "<bad/>",
        "<x:XMI xmlns:x='http://www.omg.org/XMI'/>",
        "<x:XMI xmlns:x='http://www.omg.org/XMI' xmlns:e='http://www.eclipse.org/emf/2002/Ecore'><e:EPackage nsURI='urn:a'/><bad/></x:XMI>",
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='p' nsURI='urn:a'><eSubpackages name='n' nsURI=''/><eClassifiers name='X'/><eClassifiers name='X'/></e:EPackage>",
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='p' nsURI='urn:a'><eSubpackages name='n' nsURI='urn:a'/></e:EPackage>",
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.omg.org/XMI' name='p' nsURI='urn:a' x:id='same'><eClassifiers name='X' x:id='same'/></e:EPackage>",
    ] {
        variants.push(DefinitionBundle {
            version: 1,
            namespaces: vec![],
            sources: vec![DefinitionSource::new(
                "memory:bad.ecore",
                SourceKind::Ecore,
                text,
            )],
        });
    }
    variants.push(DefinitionBundle {
        version: 1,
        namespaces: vec![],
        sources: vec![DefinitionSource::new(
            "memory:fragment.ecore#bad",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='p' nsURI='urn:p'/>",
        )],
    });
    let mut bad = fixture();
    bad.version = 9;
    bad.sources[1].id.sha256 = "0".repeat(64);
    variants.push(bad);
    let mut bad = fixture();
    bad.namespaces[0].namespace = "urn:absent".into();
    variants.push(bad);
    for bundle in variants {
        let expected = ResourceSet::new_diagnostic(&bundle).unwrap_err();
        for retained in [false, true] {
            let actual = run_policy(
                &bundle,
                Meter::new(usize::MAX, usize::MAX, usize::MAX),
                retained,
            )
            .unwrap_err();
            assert_eq!(actual, ObservedImportError::Import(expected.clone()));
        }
    }
}

#[test]
fn retained_bundle_admission_defers_hashing_and_parsing_and_preserves_complete_fees() {
    let bundle = fixture();
    let before = bundle.clone();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let context = InvocationContext::new(meter.clone());
    let pending = ResourceSetPreparation::begin_with_retained_bundle(
        &bundle,
        crate::definitions::ecore::ParseLimits::default(),
        &context,
    )
    .unwrap();
    assert_eq!(meter.work.get(), 1); // Prefix only; no source scan or parse.
    assert!(pending.finish().is_err());
    let legacy = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let old = run(&bundle, legacy.clone()).unwrap();
    let current = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let new = run_policy(&bundle, current.clone(), true).unwrap();
    assert!(new.1 > old.1);
    assert_eq!(format!("{:?}", new.0), format!("{:?}", old.0));
    assert_eq!(
        (current.work.get(), current.bytes.get()),
        (legacy.work.get(), legacy.bytes.get())
    );
    assert_eq!(bundle, before);

    let observer = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let weak = Rc::downgrade(&observer);
    let mut pending = ResourceSetPreparation::begin_with_retained_bundle(
        &bundle,
        crate::definitions::ecore::ParseLimits::default(),
        &InvocationContext::new(observer),
    )
    .unwrap();
    assert!(weak.upgrade().is_none());
    assert!(!pending.advance(&InvocationContext::default()).unwrap());
}

#[test]
fn retained_bundle_admission_cancels_at_every_boundary_and_preserves_adjacent_limits() {
    let bundle = fixture();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let (_, advances) = run_policy(&bundle, meter.clone(), true).unwrap();
    for stop in 0..meter.calls.get() {
        assert!(matches!(
            run_policy(&bundle, Meter::new(stop, usize::MAX, usize::MAX), true),
            Err(ObservedImportError::Observation(message)) if message == "resource preparation cancelled"
        ));
    }
    let context = InvocationContext::default();
    for boundary in 0..=advances {
        let mut pending = ResourceSetPreparation::begin_with_retained_bundle(
            &bundle,
            crate::definitions::ecore::ParseLimits::default(),
            &context,
        )
        .unwrap();
        for _ in 0..boundary {
            pending.advance(&context).unwrap();
        }
        let error = pending
            .advance(&InvocationContext::new(Meter::new(
                0,
                usize::MAX,
                usize::MAX,
            )))
            .unwrap_err();
        assert_eq!(
            error,
            ObservedImportError::Observation("resource preparation cancelled".into())
        );
        assert_eq!(pending.advance(&context).unwrap_err(), error);
        assert_eq!(pending.finish().unwrap_err(), error);
    }
    for (bytes, work, success) in [
        (meter.bytes.get() - 1, meter.work.get(), false),
        (meter.bytes.get(), meter.work.get() - 1, false),
        (meter.bytes.get(), meter.work.get(), true),
    ] {
        assert_eq!(
            run_policy(&bundle, Meter::new(usize::MAX, bytes, work), true).is_ok(),
            success
        );
    }
    assert!(
        run_policy(
            &bundle,
            Meter::new(usize::MAX, usize::MAX, usize::MAX),
            true
        )
        .is_ok()
    );
}
