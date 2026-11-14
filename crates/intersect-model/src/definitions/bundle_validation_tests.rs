use super::*;
use std::cell::RefCell;

fn fixture() -> DefinitionBundle {
    let mut a = DefinitionSource::new("memory:a", SourceKind::Rcore, "a");
    let mut b = DefinitionSource::new("memory:b", SourceKind::Ecore, "b");
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let namespaces = vec![
        NamespaceBinding {
            namespace: "urn:a".into(),
            source: a.id.clone(),
        },
        NamespaceBinding {
            namespace: "urn:b".into(),
            source: b.id.clone(),
        },
    ];
    DefinitionBundle {
        version: 1,
        sources: vec![
            a,
            b,
            DefinitionSource::new("memory:g", SourceKind::Xtext, "g"),
        ],
        namespaces,
    }
}

#[test]
fn retained_bundle_keeps_literal_hook_order_and_releases_one_index_entry_per_step() {
    let bundle = fixture();
    let original = bundle.clone();
    let events = RefCell::new(Vec::new());
    let before = |step: ValidationStep<'_>| {
        let event = match step {
            ValidationStep::Source(source) => format!("source:{}", source.id.uri),
            ValidationStep::Identity(id) => format!("identity:{}", id.uri),
            ValidationStep::Dependency(id) => format!("dependency:{}", id.uri),
            ValidationStep::Namespace(binding) => format!("namespace:{}", binding.namespace),
            ValidationStep::Lookup(id) => format!("lookup:{}", id.uri),
        };
        events.borrow_mut().push(event);
        Ok(())
    };
    let mut pending = BundleValidation::new(&bundle);
    while !matches!(pending.phase, Phase::Release) {
        assert!(!pending.advance(&before).unwrap());
    }
    let expected = [
        "source:memory:a",
        "identity:memory:a",
        "source:memory:b",
        "identity:memory:b",
        "source:memory:g",
        "identity:memory:g",
        "dependency:memory:b",
        "dependency:memory:a",
        "namespace:urn:a",
        "lookup:memory:a",
        "namespace:urn:b",
        "lookup:memory:a",
        "lookup:memory:b",
    ];
    assert_eq!(events.borrow().as_slice(), expected);
    assert_eq!(pending.identities.len() + pending.namespaces.len(), 5);
    for count in (1..=5).rev() {
        assert!(
            !pending
                .advance(&|_| panic!("Index release invoked a callback"))
                .unwrap()
        );
        assert_eq!(
            pending.identities.len() + pending.namespaces.len(),
            count - 1
        );
    }
    assert!(pending.advance(&before).unwrap());
    assert!(pending.advance(&before).unwrap());
    assert_eq!(events.borrow().as_slice(), expected);
    assert_eq!(bundle, original);
}

#[test]
fn retained_bundle_hash_and_namespace_chunks_never_run_during_source_admission() {
    let mut source = DefinitionSource::new("memory:chunks", SourceKind::Rcore, "a".repeat(1025));
    let namespace = format!("urn:{}", "α".repeat(300));
    let bundle = DefinitionBundle {
        version: 1,
        namespaces: vec![NamespaceBinding {
            namespace,
            source: source.id.clone(),
        }],
        sources: vec![source.clone()],
    };
    let mut pending = BundleValidation::new(&bundle);
    assert!(!pending.advance(&|_| Ok(())).unwrap()); // Version only.
    assert!(!pending.advance(&|_| Ok(())).unwrap()); // Source only.
    assert!(pending.identities.is_empty());
    for _ in 0..6 {
        // One URI and five content chunks.
        assert!(matches!(pending.phase, Phase::SourceInput));
        assert!(
            !pending
                .advance(&|_| panic!("Input scanning invoked an admission callback"))
                .unwrap()
        );
        assert!(pending.identities.is_empty());
    }
    assert!(matches!(pending.phase, Phase::Identity));
    while !matches!(pending.phase, Phase::NamespaceText) {
        assert!(!pending.advance(&|_| Ok(())).unwrap());
    }
    for offset in [256, 512, 604] {
        assert!(
            !pending
                .advance(&|_| panic!("Namespace text invoked a callback"))
                .unwrap()
        );
        assert_eq!(pending.namespace_offset, offset);
    }
    assert!(matches!(pending.phase, Phase::NamespaceIdentity));
    while !pending.advance(&|_| Ok(())).unwrap() {}

    source.id.sha256 = "0".repeat(64);
    let bad = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let mut pending = BundleValidation::new(&bad);
    for _ in 0..7 {
        assert!(!pending.advance(&|_| Ok(())).unwrap());
    }
    let error = pending.advance(&|_| Ok(())).unwrap_err();
    assert_eq!(error, "Source content digest mismatch: memory:chunks");
    assert_eq!(
        pending
            .advance(&|_| panic!("Failed validation resumed"))
            .unwrap_err(),
        error
    );
}

#[test]
fn retained_bundle_hooks_are_transient_and_failure_is_sticky() {
    use std::rc::Rc;
    let bundle = fixture();
    let mut pending = BundleValidation::new(&bundle);
    let token = Rc::new(());
    let weak = Rc::downgrade(&token);
    let before = move |_: ValidationStep<'_>| {
        let _ = &token;
        Ok(())
    };
    assert!(!pending.advance(&before).unwrap());
    assert!(!pending.advance(&before).unwrap());
    drop(before);
    assert!(weak.upgrade().is_none());
    while !matches!(pending.phase, Phase::Identity) {
        assert!(!pending.advance(&|_| Ok(())).unwrap());
    }
    assert_eq!(
        pending
            .advance(&|_| Err("bundle hook cancelled".into()))
            .unwrap_err(),
        "bundle hook cancelled"
    );
    assert_eq!(
        pending
            .advance(&|_| panic!("Failed hook resumed"))
            .unwrap_err(),
        "bundle hook cancelled"
    );
    bundle.validate().unwrap();
}
