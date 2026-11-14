use super::*;
use crate::definitions::{
    bundle::DefinitionSource,
    conversion::{InvocationObserver, ProviderId},
};
use std::{cell::Cell, rc::Rc};
struct Meter {
    calls: Cell<usize>,
    bytes: Cell<usize>,
    work: Cell<usize>,
    stop: usize,
    bytes_limit: usize,
    work_limit: usize,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        if self.bytes.get() + n > self.bytes_limit {
            return Err("cursor storage denied".into());
        }
        self.bytes.set(self.bytes.get() + n);
        Ok(())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        if self.work.get() + n > self.work_limit {
            return Err("cursor work denied".into());
        }
        self.work.set(self.work.get() + n);
        Ok(())
    }
}
impl Meter {
    fn tick(&self) -> Result<(), String> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if call == self.stop {
            Err("cursor cancelled".into())
        } else {
            Ok(())
        }
    }
}
fn fixture() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        namespaces: vec![],
        sources: vec![DefinitionSource::new(
            "memory:cursor.ecore",
            SourceKind::Ecore,
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xmi="http://www.omg.org/XMI" name="p" nsURI="urn:cursor" xmi:id="package"><eClassifiers name="A" xmi:id="first"><eTypeParameters name="T" xmi:id="nested"/></eClassifiers><eSubpackages name="q" nsURI="urn:cursor:q" xmi:id="subpackage"><eClassifiers name="B" xmi:id="last"/></eSubpackages></e:EPackage>"#,
        )],
    }
}
#[test]
fn borrowed_index_cursor_preserves_every_exact_xmi_identity_and_diagnostic() {
    let bundle = fixture();
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    let actual = ResourceSet::new_context_with_cursor_policy(
        &bundle,
        &observation,
        crate::definitions::ecore::ParseLimits::default(),
        true,
    )
    .unwrap();
    let expected = ResourceSet::new(&bundle).unwrap();
    assert_eq!(actual.namespaces, expected.namespaces);
    let id = &bundle.sources[0].id;
    let actual = &actual.resources[id];
    let expected = &expected.resources[id];
    assert_eq!(actual.ids, expected.ids);
    assert_eq!(actual.packages, expected.packages);
    assert_eq!(actual.roots, expected.roots);
    assert_eq!(actual.document.source, bundle.sources[0]);
    for (name, path) in [
        ("package", vec![]),
        ("first", vec![0]),
        ("nested", vec![0, 0]),
        ("subpackage", vec![1]),
        ("last", vec![1, 0]),
    ] {
        assert_eq!(
            actual.ids[name],
            DeclarationId {
                source: id.clone(),
                path
            }
        );
    }
    for text in [
        bundle.sources[0]
            .text
            .replace("xmi:id=\"last\"", "xmi:id=\"first\""),
        bundle.sources[0]
            .text
            .replace("xmi:id=\"last\"", "xmi:id=\" \""),
    ] {
        let variant = DefinitionBundle {
            version: 1,
            namespaces: vec![],
            sources: vec![DefinitionSource::new(
                "memory:cursor.ecore",
                SourceKind::Ecore,
                text,
            )],
        };
        assert_eq!(
            ResourceSet::new_context_with_cursor_policy(
                &variant,
                &observation,
                crate::definitions::ecore::ParseLimits::default(),
                true
            )
            .unwrap_err(),
            ResourceSet::new_diagnostic(&variant).unwrap_err()
        );
    }
}
#[test]
fn borrowed_index_preflights_every_interruption_and_exact_work_storage_boundaries() {
    let bundle = fixture();
    let run = |stop, bytes_limit, work_limit| {
        let meter = Rc::new(Meter {
            calls: Cell::new(0),
            bytes: Cell::new(0),
            work: Cell::new(0),
            stop,
            bytes_limit,
            work_limit,
        });
        let context = InvocationContext::new(meter.clone());
        let observation = ParserObservation::new(&context);
        let result = observation.finish(ResourceSet::new_context_with_cursor_policy(
            &bundle,
            &observation,
            crate::definitions::ecore::ParseLimits::default(),
            true,
        ));
        (result, meter)
    };
    let (full, meter) = run(usize::MAX, usize::MAX, usize::MAX);
    assert!(full.is_ok());
    for stop in 0..meter.calls.get() {
        assert!(
            matches!(run(stop,usize::MAX,usize::MAX).0,Err(ObservedImportError::Observation(message)) if message=="cursor cancelled")
        );
    }
    for (bytes, work, success) in [
        (meter.bytes.get() - 1, meter.work.get(), false),
        (meter.bytes.get(), meter.work.get() - 1, false),
        (meter.bytes.get(), meter.work.get(), true),
    ] {
        assert_eq!(run(usize::MAX, bytes, work).0.is_ok(), success);
    }
    assert!(ResourceSet::new(&bundle).is_ok());
}
