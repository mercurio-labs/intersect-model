//! Actual retained module admission controls; whole compiler work package stays open.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    rcore_behavior::modules::{
        self, ModuleIndex, ModuleIndexPreparation, ModulePreparationError, ModulePreparationPhase,
    },
};
use std::{cell::Cell, rc::Rc};
fn check(ok: bool, message: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message.into()) }
}
#[derive(Default)]
struct Meter {
    calls: Cell<usize>,
    work: Cell<usize>,
    bytes: Cell<usize>,
    stop: Option<usize>,
    max_work: Option<usize>,
    max_bytes: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected semantic invocation during module indexing".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if self.stop == Some(call) {
            Err("module interrupted".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.checkpoint()?;
        let value = self.work.get() + n;
        if self.max_work.is_some_and(|max| value > max) {
            return Err("module work denied".into());
        }
        self.work.set(value);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.checkpoint()?;
        let value = self.bytes.get() + n;
        if self.max_bytes.is_some_and(|max| value > max) {
            return Err("module storage denied".into());
        }
        self.bytes.set(value);
        Ok(())
    }
}
struct Borrowed<'a>(&'a Meter);
impl InvocationObserver for Borrowed<'_> {
    fn before_invoke(&self, p: &ProviderId) -> Result<(), String> {
        self.0.before_invoke(p)
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.0.checkpoint()
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.0.work(n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.0.retain_storage(n)
    }
}
fn fixture() -> DefinitionBundle {
    let model = DefinitionSource::new(
        "memory:model.rcore",
        SourceKind::Rcore,
        "rcore 1;package schema version \"1\";metamodel \"urn:module-prep\" prefix m {class Node {attr name:String [0,1];}}",
    );
    let mut dep = DefinitionSource::new(
        "memory:dep.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package dep version \"1\";pub fn add(x:Int8)->Int8=x+1;",
    );
    let mut main = DefinitionSource::new(
        "memory:main.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package main version \"1\";import model \"urn:module-prep\" as m;import package dep version \"1\" as d;fn run(n:m.Node)->Int8=d.add(1);",
    );
    main.dependencies = vec![model.id.clone(), dep.id.clone()];
    dep.dependencies = vec![main.id.clone()];
    DefinitionBundle {
        version: 1,
        sources: vec![main, model, dep],
        namespaces: vec![],
    }
}
fn run(bundle: &DefinitionBundle, meter: Rc<Meter>) -> Result<ModuleIndex, ModulePreparationError> {
    let context = InvocationContext::new(meter);
    let mut pending =
        ModuleIndexPreparation::begin(bundle, &bundle.sources[0].id, Default::default(), &context)?;
    while !pending.advance(&context)? {}
    pending.finish()
}
fn phase_index(phase: ModulePreparationPhase) -> usize {
    match phase {
        ModulePreparationPhase::Validation => 0,
        ModulePreparationPhase::Resources => 1,
        ModulePreparationPhase::Selection => 2,
        ModulePreparationPhase::SourceCopy => 3,
        ModulePreparationPhase::Syntax => 4,
        ModulePreparationPhase::Interfaces => 5,
        ModulePreparationPhase::Complete => 6,
    }
}
fn grouping() -> Result<(), String> {
    let bundle = fixture();
    let entry = &bundle.sources[0].id;
    let expected = modules::index_modules(&bundle, entry).map_err(|e| format!("{e:?}"))?;
    check(
        expected.modules.len() == 3
            && expected
                .modules
                .iter()
                .find(|m| m.document.source.id == *entry)
                .is_some_and(|m| m.imports.len() == 2 && m.symbols.len() == 1),
        "Literal cyclic module/import inventory changed",
    )?;
    let pointers = bundle
        .sources
        .iter()
        .map(|s| (s.text.as_ptr(), s.dependencies.as_ptr()))
        .collect::<Vec<_>>();
    let mut receipts = None;
    for quantum in [1, 3, 256] {
        let meter = Meter::default();
        let operation = InvocationContext::new(Rc::new(Borrowed(&meter)));
        let mut pending =
            ModuleIndexPreparation::begin(&bundle, entry, Default::default(), &operation)
                .map_err(|e| format!("{e:?}"))?;
        drop(operation);
        let mut phases = [false; 7];
        let mut done = false;
        while !done {
            for _ in 0..quantum {
                phases[phase_index(pending.phase())] = true;
                let borrowed = Rc::new(Borrowed(&meter));
                let weak = Rc::downgrade(&borrowed);
                let operation = InvocationContext::new(borrowed.clone());
                done = pending.advance(&operation).map_err(|e| format!("{e:?}"))?;
                drop(operation);
                drop(borrowed);
                check(
                    weak.upgrade().is_none(),
                    "Module preparation retained a borrowed observer",
                )?;
                if done {
                    break;
                }
            }
        }
        phases[phase_index(pending.phase())] = true;
        check(
            phases.into_iter().all(|p| p),
            "Missing retained module phase",
        )?;
        check(
            pending.bundle() == &bundle
                && pending.finish().map_err(|e| format!("{e:?}"))? == expected,
            "Module resume grouping changed complete index",
        )?;
        let current = (meter.work.get(), meter.bytes.get());
        if let Some(previous) = receipts {
            check(
                current == previous,
                "Module work/storage changed across resume quanta",
            )?;
        }
        receipts = Some(current);
    }
    check(
        bundle
            .sources
            .iter()
            .zip(pointers)
            .all(|(s, p)| s.text.as_ptr() == p.0 && s.dependencies.as_ptr() == p.1),
        "Module input allocations changed",
    )?;
    Ok(())
}
fn interruptions() -> Result<(), String> {
    let bundle = fixture();
    let entry = &bundle.sources[0].id;
    let context = InvocationContext::default();
    let mut pending = ModuleIndexPreparation::begin(&bundle, entry, Default::default(), &context)
        .map_err(|e| format!("{e:?}"))?;
    let mut steps = 0;
    while !pending.advance(&context).map_err(|e| format!("{e:?}"))? {
        steps += 1;
    }
    check(steps > 100, "Module loading did not retain real progress")?;
    check(
        ModuleIndexPreparation::begin(&bundle, entry, Default::default(), &context)
            .map_err(|e| format!("{e:?}"))?
            .finish()
            .is_err(),
        "Incomplete module index published",
    )?;
    for prefix in 0..=steps {
        let mut pending =
            ModuleIndexPreparation::begin(&bundle, entry, Default::default(), &context)
                .map_err(|e| format!("{e:?}"))?;
        for _ in 0..prefix {
            check(
                !pending.advance(&context).map_err(|e| format!("{e:?}"))?,
                "Unexpected early module completion",
            )?;
        }
        let stopped = Rc::new(Meter {
            stop: Some(0),
            ..Default::default()
        });
        let weak = Rc::downgrade(&stopped);
        let operation = InvocationContext::new(stopped.clone());
        let error = pending
            .advance(&operation)
            .err()
            .ok_or("Missed module suspension interruption")?;
        drop(operation);
        drop(stopped);
        check(
            weak.upgrade().is_none()
                && matches!(&error,ModulePreparationError::Observation(m) if m=="module interrupted")
                && pending.advance(&context) == Err(error.clone())
                && matches!(pending.finish(),Err(e) if e==error),
            "Module cancellation retained observer, resumed or published",
        )?;
    }
    check(
        modules::index_modules(&bundle, entry).is_ok(),
        "Fresh module recovery failed",
    )?;
    Ok(())
}
fn budgets() -> Result<(), String> {
    let bundle = fixture();
    let meter = Rc::new(Meter::default());
    run(&bundle, meter.clone()).map_err(|e| format!("{e:?}"))?;
    let work = meter.work.get();
    let bytes = meter.bytes.get();
    let calls = meter.calls.get();
    for (work, bytes, ok) in [
        (work, bytes, true),
        (work - 1, bytes, false),
        (work, bytes - 1, false),
    ] {
        check(
            run(
                &bundle,
                Rc::new(Meter {
                    max_work: Some(work),
                    max_bytes: Some(bytes),
                    ..Default::default()
                }),
            )
            .is_ok()
                == ok,
            "Exact/one-short module budgets changed",
        )?;
    }
    for stop in 0..calls {
        check(
            matches!(run(&bundle,Rc::new(Meter{stop:Some(stop),..Default::default()})),Err(ModulePreparationError::Observation(m)) if m=="module interrupted"),
            "Missed nested module/resource observer interruption",
        )?;
    }
    Ok(())
}
fn rejection_order() -> Result<(), String> {
    let mut bundle = fixture();
    bundle.version = 99;
    bundle.sources[1] = DefinitionSource::new("memory:bad.ecore", SourceKind::Ecore, "<not xml");
    let result = modules::index_modules(&bundle, &bundle.sources[0].id)
        .err()
        .ok_or("Invalid bundle accepted")?;
    check(
        result.len() == 1 && result[0].message.contains("bundle version"),
        "Module bundle-before-resource diagnostic precedence changed",
    )?;
    let mut invalid_resource = fixture();
    let bad = DefinitionSource::new("memory:bad.ecore", SourceKind::Ecore, "<not xml");
    invalid_resource.sources[0].dependencies[0] = bad.id.clone();
    invalid_resource.sources[1] = bad.clone();
    let missing = DefinitionSource::new("memory:missing.rcore", SourceKind::RcoreBehavior, "").id;
    let errors = modules::index_modules(&invalid_resource, &missing)
        .err()
        .ok_or("Invalid resource accepted")?;
    check(
        errors.first().is_some_and(|e| e.span.source == bad.id),
        "Resource-before-entry diagnostic precedence changed",
    )?;
    let context = InvocationContext::default();
    let mut failed =
        ModuleIndexPreparation::begin(&invalid_resource, &missing, Default::default(), &context)
            .map_err(|e| format!("{e:?}"))?;
    let error = loop {
        match failed.advance(&context) {
            Ok(false) => {}
            Ok(true) => return Err("Invalid resource published module index".into()),
            Err(e) => break e,
        }
    };
    check(
        failed.advance(&context) == Err(error.clone())
            && matches!(failed.finish(),Err(e) if e==error),
        "Module diagnostic failure was not terminal",
    )?;
    let valid = fixture();
    let errors = modules::index_modules(&valid, &missing)
        .err()
        .ok_or("Missing exact entry accepted")?;
    check(
        errors.len() == 1
            && errors[0].message == "Entry must select an exact Rcore source"
            && errors[0].span.source == missing,
        "Exact entry rejection changed",
    )?;
    let mut bundle = fixture();
    bundle.sources[2].dependencies.clear();
    bundle.sources[2] = DefinitionSource::new(
        "memory:dep.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package dep version \"1\";private fn add(x:Int8)->Int8=x+1;",
    );
    bundle.sources[0].dependencies[1] = bundle.sources[2].id.clone();
    let index =
        modules::index_modules(&bundle, &bundle.sources[0].id).map_err(|e| format!("{e:?}"))?;
    check(
        index
            .imported_symbol(&bundle.sources[0].id, "d", &["add".into()])
            .is_err(),
        "Private module function became visible",
    )?;
    Ok(())
}
fn verify() -> Result<u32, String> {
    grouping()?;
    interruptions()?;
    budgets()?;
    rejection_order()?;
    Ok(4)
}
#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn verify_rcore_module_preparation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    println!(
        "{} retained module preparation groups passed; full compiler work package remains open",
        verify()?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phases_grouping_complete_indexes_and_fresh_borrowed_observers() {
        grouping().unwrap();
    }
    #[test]
    fn every_suspension_cancels_terminally_and_preserves_inputs() {
        interruptions().unwrap();
    }
    #[test]
    fn exact_budgets_and_every_nested_observer_interruption() {
        budgets().unwrap();
    }
    #[test]
    fn bundle_resource_entry_order_and_visibility_rejections() {
        rejection_order().unwrap();
    }
}
