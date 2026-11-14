use super::*;
use crate::definitions::{
    bundle::SourceKind,
    conversion::{InvocationObserver, ProviderId},
};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Meter {
    work: Cell<usize>,
    bytes: Cell<usize>,
    calls: Cell<usize>,
    stop: Option<usize>,
    max_work: Option<usize>,
    max_bytes: Option<usize>,
}
impl Meter {
    fn touch(&self) -> Result<(), String> {
        let next = self.calls.get();
        self.calls.set(next + 1);
        if self.stop == Some(next) {
            Err("Source copy interrupted".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider call".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.touch()
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.touch()?;
        let next = self.work.get() + n;
        if self.max_work.is_some_and(|limit| next > limit) {
            return Err("Source copy work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.touch()?;
        let next = self.bytes.get() + n;
        if self.max_bytes.is_some_and(|limit| next > limit) {
            return Err("Source copy storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
fn small(count: usize) -> DefinitionSource {
    DefinitionSource {
        id: SourceId {
            uri: "a".into(),
            sha256: "b".into(),
        },
        kind: SourceKind::Ecore,
        text: "c".into(),
        dependencies: vec![
            SourceId {
                uri: "d".into(),
                sha256: "e".into()
            };
            count
        ],
    }
}
fn drive(source: &DefinitionSource, meter: Rc<Meter>) -> Result<DefinitionSource, String> {
    let context = InvocationContext::new(meter);
    let mut preparation = SourceCopyPreparation::new(source, true);
    while !preparation.advance(&context)? {}
    preparation.finish().map(AdmittedSourceCopy::into_source)
}
#[test]
fn source_copy_keeps_literal_serde_fees_and_adjacent_work_storage_boundaries() {
    // Source has 13 nodes/51 policy bytes; each dependency adds 5 nodes/17 bytes.
    for (count, work, bytes) in [(0, 20, 1715), (1, 28, 2372), (2, 35, 3029)] {
        let source = small(count);
        let meter = Rc::new(Meter::default());
        assert_eq!(drive(&source, meter.clone()).unwrap(), source);
        assert_eq!((meter.work.get(), meter.bytes.get()), (work, bytes));
        let legacy = Rc::new(Meter::default());
        let context = InvocationContext::new(legacy.clone());
        let size = retained_storage::reserve_serialized(&source, &context).unwrap();
        context.work(size.div_ceil(256)).unwrap();
        assert_eq!((legacy.work.get(), legacy.bytes.get()), (work, bytes));
        for (max_work, max_bytes, success) in [
            (work, bytes, true),
            (work - 1, bytes, false),
            (work, bytes - 1, false),
        ] {
            let meter = Rc::new(Meter {
                max_work: Some(max_work),
                max_bytes: Some(max_bytes),
                ..Meter::default()
            });
            assert_eq!(drive(&source, meter).is_ok(), success);
        }
    }
    for count in [13_104, 13_105, 65_536] {
        let source = small(count);
        let legacy = Rc::new(Meter::default());
        let baseline =
            retained_storage::reserve_serialized(&source, &InvocationContext::new(legacy.clone()));
        let meter = Rc::new(Meter::default());
        let context = InvocationContext::new(meter.clone());
        let mut preparation = SourceCopyPreparation::new(&source, true);
        let outcome = loop {
            match preparation.advance(&context) {
                Ok(_) if preparation.phase == Phase::Reserve => break Ok(()),
                Ok(_) => assert!(preparation.copied.is_none()),
                Err(error) => break Err(error),
            }
        };
        assert_eq!(outcome.is_ok(), baseline.is_ok());
        assert_eq!(meter.work.get(), legacy.work.get());
        assert_eq!(meter.bytes.get(), 0);
        if let Err(error) = outcome {
            assert_eq!(error, baseline.unwrap_err());
            assert!(preparation.finish().is_err());
        }
    }
}
fn text(source: &DefinitionSource, field: Field, dependency: usize) -> &str {
    match field {
        Field::Uri => &source.id.uri,
        Field::Digest => &source.id.sha256,
        Field::Text => &source.text,
        Field::DependencyUri => &source.dependencies[dependency].uri,
        Field::DependencyDigest => &source.dependencies[dependency].sha256,
    }
}
#[test]
fn source_copy_chunks_preserve_utf8_and_never_reallocate_a_copied_prefix() {
    let mut source = small(3);
    source.id.uri = "é🚀".repeat(140);
    source.id.sha256 = "α".repeat(130);
    source.text = "α🚀\r\n".repeat(2000);
    source.dependencies[0].uri = "".into();
    source.dependencies[1].uri = "λ🚀".repeat(200);
    source.dependencies[2].sha256 = "中".repeat(200);
    let meter = Rc::new(Meter::default());
    let context = InvocationContext::new(meter.clone());
    let mut preparation = SourceCopyPreparation::new(&source, true);
    let mut advances = 0;
    loop {
        let before = if preparation.phase == Phase::Copy {
            let field = preparation.field;
            let dependency = preparation.dependency;
            let value = text(preparation.copied.as_ref().unwrap(), field, dependency);
            Some((field, dependency, value.as_ptr(), value.len()))
        } else {
            None
        };
        let done = preparation.advance(&context).unwrap();
        advances += 1;
        if let Some((field, dependency, pointer, start)) = before {
            let output = text(preparation.copied.as_ref().unwrap(), field, dependency);
            assert_eq!(output.as_ptr(), pointer);
            assert!(output.len() - start <= 256);
            assert_eq!(output, &text(&source, field, dependency)[..output.len()]);
        }
        if done {
            break;
        }
    }
    assert!(advances > source.text.len() / 256);
    assert_eq!(preparation.finish().unwrap().into_source(), source);
    let mut unobserved = SourceCopyPreparation::new(&source, false);
    let observer = Rc::new(Meter::default());
    let context = InvocationContext::new(observer.clone());
    while !unobserved.advance(&context).unwrap() {}
    assert_eq!(unobserved.finish().unwrap().into_source(), source);
    assert_eq!((observer.work.get(), observer.bytes.get()), (0, 0));
}
#[test]
fn source_copy_interruption_is_terminal_at_every_observation_and_retains_no_observer() {
    let mut source = small(2);
    source.text = "é🚀".repeat(500);
    let baseline = Rc::new(Meter::default());
    drive(&source, baseline.clone()).unwrap();
    for stop in 0..baseline.calls.get() {
        let meter = Rc::new(Meter {
            stop: Some(stop),
            ..Meter::default()
        });
        let context = InvocationContext::new(meter.clone());
        let mut preparation = SourceCopyPreparation::new(&source, true);
        let error = loop {
            match preparation.advance(&context) {
                Ok(false) => {}
                Ok(true) => panic!("Missed observation {stop}"),
                Err(error) => break error,
            }
        };
        assert_eq!(error, "Source copy interrupted");
        drop(context);
        assert_eq!(Rc::strong_count(&meter), 1);
        assert!(preparation.advance(&InvocationContext::default()).is_err());
        assert!(preparation.finish().is_err());
    }
    assert_eq!(drive(&source, Rc::new(Meter::default())).unwrap(), source);
}
