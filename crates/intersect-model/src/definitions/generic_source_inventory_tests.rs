use super::*;
use crate::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};
struct Meter {
    bytes: Cell<usize>,
    work: Cell<usize>,
    calls: Cell<usize>,
    stop: usize,
    byte_limit: usize,
    work_limit: usize,
}
impl Meter {
    fn new(stop: usize, byte_limit: usize, work_limit: usize) -> Rc<Self> {
        Rc::new(Self {
            bytes: Cell::new(0),
            work: Cell::new(0),
            calls: Cell::new(0),
            stop,
            byte_limit,
            work_limit,
        })
    }
    fn tick(&self) -> Result<(), String> {
        let n = self.calls.get();
        self.calls.set(n + 1);
        if n == self.stop {
            Err("inventory cancelled".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.tick()?;
        let n = self.bytes.get() + bytes;
        if n > self.byte_limit {
            return Err("inventory storage denied".into());
        }
        self.bytes.set(n);
        Ok(())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.tick()?;
        let n = self.work.get() + units;
        if n > self.work_limit {
            return Err("inventory work denied".into());
        }
        self.work.set(n);
        Ok(())
    }
}
const BODY: &str = r##"<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EClass" name="Box" eSuperTypes="e:EClass #//Base">
<eTypeParameters name="T"><eBounds eClassifier="#//Base"/><eAnnotations source="parameter"/></eTypeParameters>
<eStructuralFeatures x:type="e:EReference" name="value" eType="#//Base"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures>
<eOperations name="run" eType="#//Base"><eParameters name="arg" eType="#//Base"/><eGenericExceptions eClassifier="#//Base"/></eOperations>
<eAnnotations source="class"><details key="k" value="v"/></eAnnotations>
</eClassifiers>
<eSubpackages name="nested" nsURI="urn:inventory:nested"><eClassifiers x:type="e:EClass" name="Other" eSuperTypes=""/></eSubpackages>
<f:payload xmlns:f="urn:foreign">opaque content</f:payload>"##;
fn fixture(
    body: &str,
) -> (
    crate::definitions::bundle::DefinitionSource,
    DefinitionBundle,
    Resolver,
) {
    let source = crate::definitions::bundle::DefinitionSource::new(
        "memory:inventory",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:inventory">{body}</e:EPackage>"#
        ),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    (source, bundle, resolver)
}
#[test]
fn observed_inventory_retains_literal_identities_order_legacy_empty_and_opaque_payloads() {
    let (source, bundle, resolver) = fixture(BODY);
    let before = serde_json::to_value(&bundle).unwrap();
    let full = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let actual = resolver
        .resolve_all_observed(&InvocationContext::new(full.clone()))
        .unwrap();
    assert_eq!(actual.sources, vec![source.id.clone()]);
    assert_eq!(
        actual
            .parameters
            .iter()
            .map(|p| p.syntax.declaration.path.clone())
            .collect::<Vec<_>>(),
        [vec![1, 0]]
    );
    assert_eq!(
        actual.parameters[0].bounds[0].raw_type,
        DeclarationId {
            source: source.id.clone(),
            path: vec![0]
        }
    );
    assert_eq!(
        actual
            .types
            .iter()
            .map(|t| t.syntax.declaration.path.clone())
            .collect::<Vec<_>>(),
        [vec![1, 1, 0], vec![1, 2, 1]]
    );
    assert!(actual.types.iter().all(|t| t.raw_type.path == vec![0]));
    let rows = actual
        .raw_signatures
        .iter()
        .map(|r| {
            (
                r.owner.path.clone(),
                r.feature.as_str(),
                r.legacy.as_ref().map(Vec::len),
                r.generic.iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
                r.raw_types
                    .iter()
                    .map(|i| i.path.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            (vec![0], "eSuperTypes", None, vec![], vec![]),
            (vec![1], "eSuperTypes", Some(1), vec![], vec![vec![0]]),
            (
                vec![1, 1],
                "eType",
                Some(1),
                vec![vec![1, 1, 0]],
                vec![vec![0]]
            ),
            (vec![1, 2], "eType", Some(1), vec![], vec![vec![0]]),
            (
                vec![1, 2],
                "eExceptions",
                None,
                vec![vec![1, 2, 1]],
                vec![vec![0]]
            ),
            (vec![1, 2, 0], "eType", Some(1), vec![], vec![vec![0]]),
            (vec![2, 0], "eSuperTypes", Some(0), vec![], vec![])
        ]
    );
    assert_eq!(
        actual.raw_signatures[1].legacy.as_ref().unwrap()[0]
            .kind_hint
            .as_deref(),
        Some("EClass")
    );
    let expected_spans = [
        r#"<eAnnotations source="parameter"/>"#,
        r#"<eAnnotations source="class"><details key="k" value="v"/></eAnnotations>"#,
        r#"<f:payload xmlns:f="urn:foreign">opaque content</f:payload>"#,
    ]
    .map(|text| {
        let start = source.text.find(text).unwrap();
        SourceSpan {
            source: source.id.clone(),
            start,
            end: start + text.len(),
        }
    });
    assert_eq!(actual.retained_payloads, expected_spans);
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(resolver.resolve_all().unwrap()).unwrap()
    );
    assert_eq!(serde_json::to_value(bundle).unwrap(), before);
    // Every observer interruption must use the typed failure channel; reusing this
    // immutable resolver afterward must produce the complete original inventory.
    for stop in 0..full.calls.get() {
        assert!(
            matches!(resolver.resolve_all_observed(&InvocationContext::new(Meter::new(stop,usize::MAX,usize::MAX))),Err(crate::definitions::ecore::ObservedImportError::Observation(message)) if message=="inventory cancelled")
        );
    }
    for (bytes, work, success) in [
        (full.bytes.get() - 1, full.work.get(), false),
        (full.bytes.get(), full.work.get() - 1, false),
        (full.bytes.get(), full.work.get(), true),
        (full.bytes.get() + 1, full.work.get() + 1, true),
    ] {
        assert_eq!(
            resolver
                .resolve_all_observed(&InvocationContext::new(Meter::new(usize::MAX, bytes, work)))
                .is_ok(),
            success
        );
    }
    assert_eq!(
        serde_json::to_value(
            resolver
                .resolve_all_observed(&InvocationContext::default())
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(actual).unwrap()
    );
}
#[test]
fn observed_inventory_keeps_legacy_diagnostics_and_first_error_source_spans() {
    for body in [
        BODY.replace(
            "eSuperTypes=\"e:EClass #//Base\"",
            "eSuperTypes=\"e:EClass\"",
        ),
        BODY.replace(
            "name=\"value\" eType=\"#//Base\"",
            "name=\"value\" eType=\"#//Box\"",
        ),
        BODY.replace(
            "<eGenericType eTypeParameter=\"#//Box/T\"/>",
            "<eType href=\"#//Base\"/><eGenericType eTypeParameter=\"#//Box/T\"/>",
        ),
    ] {
        let (_, _, resolver) = fixture(&body);
        let expected = resolver.resolve_all().unwrap_err();
        let actual = resolver
            .resolve_all_observed(&InvocationContext::new(Meter::new(
                usize::MAX,
                usize::MAX,
                usize::MAX,
            )))
            .unwrap_err();
        let crate::definitions::ecore::ObservedImportError::Import(actual) = actual else {
            panic!("Source error changed category")
        };
        assert_eq!(actual, expected);
        assert!(actual.message.contains("Legacy") || actual.message.contains("legacy"));
    }
}

#[test]
fn original_large_inventory_keeps_its_exact_source_and_complete_signature_counts() {
    let source = crate::definitions::bundle::DefinitionSource::new(
        "memory:job-expansion.ecore",
        SourceKind::Ecore,
        include_str!("../../assets/fixtures/ecore-generics/large-inventory.ecore"),
    );
    assert_eq!(
        source.id.sha256,
        "f1e38abc4457ce56324d15be77e39b28f30d4d8fe4c8a856c5bc070802371557"
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap().with_borrowed_owner_inputs();
    let ordinary = resolver.resolve_all().unwrap();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let observed = resolver
        .resolve_all_observed(&InvocationContext::new(meter.clone()))
        .unwrap();
    assert_eq!(observed.parameters.len(), 1);
    assert_eq!(observed.types.len(), 4);
    assert_eq!(observed.raw_signatures.len(), 132);
    assert_eq!(
        serde_json::to_value(observed).unwrap(),
        serde_json::to_value(ordinary).unwrap()
    );
    eprintln!(
        "DIAGNOSTIC_SOURCE_INVENTORY_ONLY work={} retained_bytes={} checkpoints={}",
        meter.work.get(),
        meter.bytes.get(),
        meter.calls.get()
    );
}

#[test]
fn retained_inventory_yields_replaces_observers_and_rejects_failed_or_incomplete_publication() {
    let (_, _, resolver) = fixture(BODY);
    let expected = resolver.resolve_all().unwrap();
    let default_context = InvocationContext::default();
    let default_observation = crate::definitions::ecore::ParserObservation::new(&default_context);
    let mut preparation = SourceInventoryPreparation::new(&resolver, &default_observation).unwrap();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let mut advances = 0;
    loop {
        // Each invocation has its own context: the continuation must not retain it.
        let context = InvocationContext::new(meter.clone());
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        advances += 1;
        if observation
            .finish(preparation.advance(&observation))
            .unwrap()
        {
            break;
        }
    }
    assert!(
        advances > 30,
        "Source traversal must yield between declarations and edges"
    );
    assert!(preparation.advance(&default_observation).unwrap());
    assert_eq!(preparation.finish().unwrap(), expected);
    assert!(
        SourceInventoryPreparation::new(&resolver, &default_observation)
            .unwrap()
            .finish()
            .is_err()
    );

    // Cancel at each retained boundary, including the completed state. Failed
    // preparation cannot be resumed or published, even with a fresh observer.
    for boundary in 0..=advances {
        let mut preparation =
            SourceInventoryPreparation::new(&resolver, &default_observation).unwrap();
        for _ in 0..boundary {
            preparation.advance(&default_observation).unwrap();
        }
        let context = InvocationContext::new(Meter::new(0, usize::MAX, usize::MAX));
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        assert!(
            matches!(observation.finish(preparation.advance(&observation)),
            Err(crate::definitions::ecore::ObservedImportError::Observation(message)) if message == "inventory cancelled")
        );
        assert!(preparation.advance(&default_observation).is_err());
        assert!(preparation.finish().is_err());
    }
    assert_eq!(resolver.resolve_all().unwrap(), expected);
}

#[test]
fn retained_inventory_visits_multiple_sources_and_limits_a_subtree_to_its_root() {
    let (first, _, _) = fixture(BODY);
    let second = crate::definitions::bundle::DefinitionSource::new(
        "memory:inventory-second",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="q" nsURI="urn:inventory-second"><eClassifiers x:type="e:EClass" name="Last"/></e:EPackage>"#,
    );
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![second.clone(), first.clone()],
        namespaces: vec![],
    })
    .unwrap();
    let inventory = resolver.resolve_all().unwrap();
    assert_eq!(inventory.sources, [first.id.clone(), second.id.clone()]);
    assert_eq!(inventory.raw_signatures.len(), 8);
    assert_eq!(
        inventory.raw_signatures[7].owner,
        DeclarationId {
            source: second.id,
            path: vec![0]
        }
    );
    let context = InvocationContext::default();
    let observation = crate::definitions::ecore::ParserObservation::new(&context);
    let root = DeclarationId {
        source: first.id,
        path: vec![1],
    };
    let mut subtree = SourceInventoryPreparation::subtree(&resolver, &root, &observation).unwrap();
    while !subtree.advance(&observation).unwrap() {}
    let subtree = subtree.finish().unwrap();
    assert!(subtree.sources.is_empty());
    assert_eq!(subtree.raw_signatures, inventory.raw_signatures[1..6]);
    assert_eq!(subtree.parameters, inventory.parameters);
    assert_eq!(subtree.types, inventory.types);
    assert_eq!(subtree.retained_payloads, inventory.retained_payloads[..2]);
}

#[test]
fn exact_reference_inputs_reduce_repeated_search_without_changing_complete_inventory() {
    let source = crate::definitions::bundle::DefinitionSource::new(
        "memory:job-expansion.ecore",
        SourceKind::Ecore,
        include_str!("../../assets/fixtures/ecore-generics/large-inventory.ecore"),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let uncached = Resolver::new(&bundle).unwrap().with_borrowed_owner_inputs();
    let baseline = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let expected = uncached
        .resolve_all_observed(&InvocationContext::new(baseline.clone()))
        .unwrap();
    let resolver = Resolver::new(&bundle).unwrap().with_borrowed_owner_inputs();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let context = InvocationContext::new(meter.clone());
    let observation = crate::definitions::ecore::ParserObservation::new(&context);
    resolver
        .enable_source_reference_inputs(&observation)
        .unwrap();
    let actual = resolver.resolve_all_observed(&context).unwrap();
    assert_eq!(actual, expected);
    assert!(meter.work.get() < baseline.work.get());
    assert!(meter.bytes.get() < baseline.bytes.get());
    eprintln!(
        "DIAGNOSTIC_REFERENCE_INPUTS work={} retained_bytes={} checkpoints={} baseline_work={} baseline_bytes={}",
        meter.work.get(),
        meter.bytes.get(),
        meter.calls.get(),
        baseline.work.get(),
        baseline.bytes.get()
    );
}

#[test]
fn cached_reference_selection_keeps_fresh_kind_errors_exact_revisions_and_error_spans() {
    let malformed = BODY.replace(
        "eSuperTypes=\"e:EClass #//Base\"",
        "eSuperTypes=\"e:EEnum #//Base\"",
    );
    let (source, _, resolver) = fixture(&malformed);
    let context = InvocationContext::default();
    let observation = crate::definitions::ecore::ParserObservation::new(&context);
    resolver
        .enable_source_reference_inputs(&observation)
        .unwrap();
    let usage = DeclarationId {
        source: source.id.clone(),
        path: vec![1],
    };
    let reference = TypeReference {
        href: "#//Base".into(),
        kind_hint: None,
        span: SourceSpan {
            source: source.id.clone(),
            start: 17,
            end: 23,
        },
    };
    assert_eq!(
        resolver
            .reference_observed(&usage, &reference, &observation)
            .unwrap()
            .path,
        vec![0]
    );
    // A cached address does not bypass the legacy kind agreement predicate.
    let actual = resolver.resolve_all_observed(&context).unwrap_err();
    let expected = fixture(&malformed).2.resolve_all().unwrap_err();
    assert_eq!(
        actual,
        crate::definitions::ecore::ObservedImportError::Import(expected)
    );
    let missing = TypeReference {
        href: "#//Absent".into(),
        ..reference.clone()
    };
    for span in [
        reference.span.clone(),
        SourceSpan {
            start: 51,
            end: 63,
            ..reference.span.clone()
        },
    ] {
        let reference = TypeReference {
            span: span.clone(),
            ..missing.clone()
        };
        let error = resolver
            .reference_observed(&usage, &reference, &observation)
            .unwrap_err();
        assert_eq!(error.span, span);
        assert_eq!(
            error.message,
            "Missing named fragment Absent in memory:inventory"
        );
    }
    let shifted = format!("<eClassifiers x:type=\"e:EClass\" name=\"Padding\"/>{BODY}");
    let (new_source, _, new_resolver) = fixture(&shifted);
    assert_eq!(new_source.id.uri, source.id.uri);
    assert_ne!(new_source.id.sha256, source.id.sha256);
    new_resolver
        .enable_source_reference_inputs(&observation)
        .unwrap();
    let new_usage = DeclarationId {
        source: new_source.id.clone(),
        path: vec![2],
    };
    let target = new_resolver
        .reference_observed(&new_usage, &reference, &observation)
        .unwrap();
    assert_eq!(
        target,
        DeclarationId {
            source: new_source.id,
            path: vec![1]
        }
    );
}

#[test]
fn reference_input_preflights_cancel_without_retaining_partial_selections() {
    let (source, _, _) = fixture(BODY);
    let usage = DeclarationId {
        source: source.id.clone(),
        path: vec![1],
    };
    let reference = TypeReference {
        href: "#//Base".into(),
        kind_hint: None,
        span: SourceSpan {
            source: source.id,
            start: 17,
            end: 23,
        },
    };
    let default_context = InvocationContext::default();
    let default_observation = crate::definitions::ecore::ParserObservation::new(&default_context);
    for warm in [false, true] {
        let run = |stop, byte_limit, work_limit| {
            let (_, _, resolver) = fixture(BODY);
            resolver
                .enable_source_reference_inputs(&default_observation)
                .unwrap();
            if warm {
                resolver
                    .reference_observed(&usage, &reference, &default_observation)
                    .unwrap();
            }
            let meter = Meter::new(stop, byte_limit, work_limit);
            let context = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&context);
            let result =
                observation.finish(resolver.reference_observed(&usage, &reference, &observation));
            // Rejected lookup admission cannot poison recovery.
            assert_eq!(
                resolver
                    .reference_observed(&usage, &reference, &default_observation)
                    .unwrap()
                    .path,
                vec![0]
            );
            (result, meter)
        };
        let (full, meter) = run(usize::MAX, usize::MAX, usize::MAX);
        assert_eq!(full.unwrap().path, vec![0]);
        for stop in 0..meter.calls.get() {
            assert!(matches!(run(stop,usize::MAX,usize::MAX).0,
                Err(crate::definitions::ecore::ObservedImportError::Observation(message)) if message == "inventory cancelled"));
        }
        for (bytes, work, success) in [
            (meter.bytes.get() - 1, meter.work.get(), false),
            (meter.bytes.get(), meter.work.get() - 1, false),
            (meter.bytes.get(), meter.work.get(), true),
        ] {
            assert_eq!(run(usize::MAX, bytes, work).0.is_ok(), success);
        }
    }
}
