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
        let call = self.calls.get();
        self.calls.set(call + 1);
        if call == self.stop {
            Err("legacy observation cancelled".into())
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
        let next = self.bytes.get() + bytes;
        if next > self.byte_limit {
            return Err("legacy storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.tick()?;
        let next = self.work.get() + units;
        if next > self.work_limit {
            return Err("legacy work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
}
#[test]
fn legacy_reference_precharges_literal_string_span_record_and_slot_before_retention() {
    let source = crate::definitions::bundle::DefinitionSource::new(
        "memory:x",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='C'><eStructuralFeatures x:type='e:EReference' name='r' eType='#//C'/></eClassifiers></e:EPackage>",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    let owner = DeclarationId {
        source: source.id.clone(),
        path: vec![0, 0],
    };
    let run = |meter: Rc<Meter>| {
        let context = InvocationContext::new(meter);
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        observation.finish(resolver.legacy_references_context(&owner, "eType", &observation))
    };
    let full = Meter::new(usize::MAX, 556, usize::MAX);
    let refs = run(full.clone()).unwrap().unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].href, "#//C");
    assert_eq!(refs[0].kind_hint, None);
    let start = source.text.find("eType='#//C'").unwrap();
    assert_eq!(
        refs[0].span,
        SourceSpan {
            source: source.id.clone(),
            start,
            end: start + 12
        }
    );
    // href 4+64; span record128+two slots128+uri8+sha64; reference128+vector32.
    assert_eq!(full.bytes.get(), 556);
    for (limit, retained) in [(0, 0), (67, 0), (68, 68), (395, 68), (396, 396), (555, 396)] {
        let meter = Meter::new(usize::MAX, limit, usize::MAX);
        assert!(
            matches!(run(meter.clone()),Err(crate::definitions::ecore::ObservedImportError::Observation(message)) if message=="legacy storage denied")
        );
        assert_eq!(meter.bytes.get(), retained);
    }
    for stop in 0..full.calls.get() {
        assert!(
            matches!(run(Meter::new(stop,usize::MAX,usize::MAX)),Err(crate::definitions::ecore::ObservedImportError::Observation(message)) if message=="legacy observation cancelled")
        );
    }
    for (limit, success) in [
        (full.work.get() - 1, false),
        (full.work.get(), true),
        (full.work.get() + 1, true),
    ] {
        assert_eq!(run(Meter::new(usize::MAX, 556, limit)).is_ok(), success);
    }
    assert_eq!(
        run(Meter::new(usize::MAX, 556, usize::MAX))
            .unwrap()
            .unwrap(),
        refs
    );
}
