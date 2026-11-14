//! Controls for ownership policy over the same source/package authority.
use super::*;
use crate::definitions::bundle::DefinitionSource;
use crate::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};
struct Meter {
    bytes: Cell<usize>,
    work: Cell<usize>,
    max_work: usize,
    max_bytes: usize,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("owner selection cannot invoke providers")
    }
    fn work(&self, units: usize) -> Result<(), String> {
        let next = self.work.get() + units;
        if next > self.max_work {
            return Err("owner work interrupted".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self.bytes.get() + bytes;
        if next > self.max_bytes {
            return Err("owner storage interrupted".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
fn source(body: &str) -> DefinitionSource {
    DefinitionSource::new("memory:owner-inputs.ecore", SourceKind::Ecore, body)
}
fn resolve(input: &DefinitionSource, borrowed: bool) -> Resolver {
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![input.clone()],
        namespaces: vec![],
    })
    .unwrap();
    if borrowed {
        resolver.with_borrowed_owner_inputs()
    } else {
        resolver
    }
}
fn run(
    resolver: &Resolver,
    id: &DeclarationId,
    max_work: usize,
    max_bytes: usize,
) -> (
    Result<TargetKind, crate::definitions::ecore::ObservedImportError>,
    usize,
    usize,
) {
    let meter = Rc::new(Meter {
        bytes: Cell::new(0),
        work: Cell::new(0),
        max_work,
        max_bytes,
    });
    let context = InvocationContext::new(meter.clone());
    let observation = crate::definitions::ecore::ParserObservation::new(&context);
    let result = observation.finish(resolver.classifier_observed(id, &observation));
    (result, meter.bytes.get(), meter.work.get())
}
#[test]
fn borrowed_owner_paths_keep_nested_xmi_and_invalid_source_results() {
    for (body, path) in [
        (
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:owners"><eClassifiers x:type="e:EClass" name="C"/></e:EPackage>"#,
            vec![0],
        ),
        (
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:owners"><eSubpackages name="nested" nsURI="urn:nested"><eClassifiers x:type="e:EClass" name="C"/></eSubpackages></e:EPackage>"#,
            vec![0, 0],
        ),
        (
            r#"<xmi:XMI xmlns:xmi="http://www.omg.org/XMI" xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance"><e:EPackage name="m" nsURI="urn:owners"><eClassifiers x:type="e:EClass" name="C"/></e:EPackage></xmi:XMI>"#,
            vec![0, 0],
        ),
    ] {
        let input = source(body);
        let owned = resolve(&input, false);
        let borrowed = resolve(&input, true);
        let id = DeclarationId {
            source: input.id.clone(),
            path,
        };
        let a = run(&owned, &id, usize::MAX, usize::MAX);
        let b = run(&borrowed, &id, usize::MAX, usize::MAX);
        assert_eq!(a.0, b.0);
        assert_eq!(b.0, Ok(TargetKind::Class));
        assert!(a.1 > 0);
        assert_eq!(b.1, 0);
        for invalid in [
            DeclarationId {
                source: input.id.clone(),
                path: vec![],
            },
            DeclarationId {
                source: input.id.clone(),
                path: vec![999],
            },
            DeclarationId {
                source: SourceId {
                    uri: input.id.uri.clone(),
                    sha256: "0".repeat(64),
                },
                path: id.path.clone(),
            },
        ] {
            let a = run(&owned, &invalid, usize::MAX, usize::MAX).0;
            let b = run(&borrowed, &invalid, usize::MAX, usize::MAX).0;
            assert!(a.is_err());
            assert_eq!(a, b);
        }
        let mut changed = input.clone();
        changed.text.push(' ');
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![changed],
            namespaces: vec![],
        };
        assert!(Resolver::new(&bundle).is_err());
    }
}
#[test]
fn borrowed_owner_paths_cancel_each_observed_work_boundary_and_recover() {
    let input = source(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:owners"><eSubpackages name="nested" nsURI="urn:nested"><eClassifiers x:type="e:EClass" name="C"/></eSubpackages></e:EPackage>"#,
    );
    let resolver = resolve(&input, true);
    let id = DeclarationId {
        source: input.id.clone(),
        path: vec![0, 0],
    };
    let baseline = run(&resolver, &id, usize::MAX, 0);
    assert_eq!(baseline.0, Ok(TargetKind::Class));
    assert_eq!(baseline.1, 0);
    for work in 0..baseline.2 {
        assert!(
            matches!(run(&resolver,&id,work,0).0,Err(crate::definitions::ecore::ObservedImportError::Observation(message)) if message=="owner work interrupted")
        );
    }
    assert_eq!(run(&resolver, &id, baseline.2, 0), baseline);
    assert_eq!(run(&resolver, &id, usize::MAX, 0), baseline);
}
