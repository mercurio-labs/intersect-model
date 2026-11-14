//! Verification-only native/WASM consumer of shared retained bundle admission.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, NamespaceBinding, SourceId, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{ErrorKind, ObservedImportError, ParseLimits},
    resources::{ResourceSet, ResourceSetPreparation},
};
use std::{cell::Cell, rc::Rc};

fn fixture() -> DefinitionBundle {
    let mut a = DefinitionSource::new(
        "memory:a.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='a' nsURI='urn:a'/>",
    );
    let mut b = DefinitionSource::new(
        "memory:b.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='b' nsURI='urn:b'/>",
    );
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
            DefinitionSource::new("memory:g.xtext", SourceKind::Xtext, "grammar example.Probe"),
        ],
        namespaces,
    }
}
fn long_fixture() -> DefinitionBundle {
    let namespace = format!("urn:{}", "α".repeat(300));
    let source = DefinitionSource::new(
        format!("memory:{}.ecore", "α".repeat(300)),
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='wide' nsURI='{namespace}'/>"
        ),
    );
    DefinitionBundle {
        version: 1,
        namespaces: vec![NamespaceBinding {
            namespace,
            source: source.id.clone(),
        }],
        sources: vec![source],
    }
}
struct Meter {
    calls: Cell<usize>,
    work: Cell<usize>,
    bytes: Cell<usize>,
    stop: usize,
    work_limit: usize,
    storage_limit: usize,
}
impl Meter {
    fn new(stop: usize, work_limit: usize, storage_limit: usize) -> Rc<Self> {
        Rc::new(Self {
            calls: Cell::new(0),
            work: Cell::new(0),
            bytes: Cell::new(0),
            stop,
            work_limit,
            storage_limit,
        })
    }
    fn tick(&self) -> Result<(), String> {
        let calls = self.calls.get();
        self.calls.set(calls + 1);
        if calls == self.stop {
            Err("bundle cancelled".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider invocation".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.tick()
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.tick()?;
        let next = self
            .work
            .get()
            .checked_add(units)
            .ok_or("bundle work overflow")?;
        if next > self.work_limit {
            return Err("bundle work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.tick()?;
        let next = self
            .bytes
            .get()
            .checked_add(bytes)
            .ok_or("bundle storage overflow")?;
        if next > self.storage_limit {
            return Err("bundle storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
fn begin<'a>(
    bundle: &'a DefinitionBundle,
    context: &InvocationContext<'_>,
    retained: bool,
) -> Result<ResourceSetPreparation<'a>, ObservedImportError> {
    if retained {
        ResourceSetPreparation::begin_with_retained_bundle(bundle, ParseLimits::default(), context)
    } else {
        ResourceSetPreparation::begin(bundle, ParseLimits::default(), context)
    }
}
fn run(
    bundle: &DefinitionBundle,
    meter: Rc<Meter>,
    retained: bool,
) -> Result<(ResourceSet, usize), ObservedImportError> {
    let context = InvocationContext::new(meter);
    let mut pending = begin(bundle, &context, retained)?;
    let mut steps = 0;
    loop {
        steps += 1;
        if pending.advance(&context)? {
            return Ok((pending.finish()?, steps));
        }
    }
}
fn unlimited() -> Rc<Meter> {
    Meter::new(usize::MAX, usize::MAX, usize::MAX)
}
fn verify_good() -> Result<(), String> {
    let mut bundle = fixture();
    let old = bundle.sources[0].clone();
    let mut revised = DefinitionSource::new(
        old.id.uri.clone(),
        SourceKind::Ecore,
        old.text.replace("name='a'", "name='revised'"),
    );
    revised.dependencies = old.dependencies;
    bundle.sources.push(revised);
    bundle.validate()?;
    let (actual, _) = run(&bundle, unlimited(), true).map_err(|e| e.to_string())?;
    let expected = ResourceSet::new_diagnostic(&bundle).map_err(|e| e.to_string())?;
    if format!("{actual:?}") != format!("{expected:?}")
        || actual.document(&bundle.sources[0].id)?.source != bundle.sources[0]
        || actual.document(&bundle.sources[3].id)?.source != bundle.sources[3]
    {
        return Err("Cyclic/revised source selection or complete resources changed".into());
    }
    let source = DefinitionSource::new(
        "memory:model.rcore",
        SourceKind::Rcore,
        "rcore 1; package probe version \"1\"; metamodel \"urn:r\" prefix r { class Item {} }",
    );
    let rcore = DefinitionBundle {
        version: 1,
        namespaces: vec![NamespaceBinding {
            namespace: "urn:r".into(),
            source: source.id.clone(),
        }],
        sources: vec![source],
    };
    rcore.validate()?;
    run(&rcore, unlimited(), true).map_err(|e| e.to_string())?;
    let empty = DefinitionBundle {
        version: 1,
        sources: vec![],
        namespaces: vec![],
    };
    empty.validate()?;
    run(&empty, unlimited(), true).map_err(|e| e.to_string())?;
    Ok(())
}
fn bad_cases() -> Vec<(DefinitionBundle, String, usize)> {
    let mut cases = Vec::new();
    let mut bad = fixture();
    bad.version = 9;
    cases.push((bad, "Unsupported definition bundle version 9".into(), 0));
    let mut bad = fixture();
    bad.sources.push(bad.sources[0].clone());
    cases.push((bad, "Duplicate source identity: memory:a.ecore".into(), 0));
    let mut bad = fixture();
    bad.sources[1].id.sha256 = "0".repeat(64);
    cases.push((
        bad,
        "Source content digest mismatch: memory:b.ecore".into(),
        1,
    ));
    let mut bad = fixture();
    bad.sources[0].id.uri = " \u{2003} ".into();
    cases.push((
        bad,
        "Definition source URI must be nonempty and contain no controls".into(),
        0,
    ));
    let mut bad = fixture();
    bad.sources[0].id.uri = format!("memory:{}\0", "α".repeat(300));
    cases.push((
        bad,
        "Definition source URI must be nonempty and contain no controls".into(),
        0,
    ));
    let missing = SourceId {
        uri: "memory:missing".into(),
        sha256: "0".repeat(64),
    };
    let mut bad = fixture();
    bad.sources[1].dependencies.push(missing.clone());
    cases.push((
        bad,
        format!(
            "Missing exact source dependency memory:missing@{} required by memory:b.ecore",
            missing.sha256
        ),
        0,
    ));
    let mut bad = fixture();
    let dependency = bad.sources[0].id.clone();
    bad.sources[1].dependencies.push(dependency);
    cases.push((bad, "Duplicate dependency in memory:b.ecore".into(), 0));
    let mut bad = fixture();
    bad.namespaces[1].namespace = "urn:a".into();
    bad.namespaces[1].source = missing.clone();
    cases.push((bad, "Ambiguous namespace binding: urn:a".into(), 0));
    for namespace in [
        "".into(),
        " \u{2003} ".into(),
        format!("urn:{}\0", "α".repeat(300)),
    ] {
        let mut bad = fixture();
        bad.namespaces[0].namespace = namespace;
        cases.push((
            bad,
            "Namespace identity must be nonempty and contain no controls".into(),
            0,
        ));
    }
    let mut bad = fixture();
    bad.namespaces[0].source = missing.clone();
    cases.push((
        bad,
        format!(
            "Missing exact definition source memory:missing@{}",
            missing.sha256
        ),
        0,
    ));
    let mut bad = fixture();
    bad.namespaces[0].source.sha256 = "0".repeat(64);
    cases.push((
        bad,
        format!(
            "Missing exact definition source memory:a.ecore@{}",
            missing.sha256
        ),
        0,
    ));
    let mut bad = fixture();
    bad.namespaces[0].source = bad.sources[2].id.clone();
    cases.push((bad, "Namespace urn:a must bind an Ecore source".into(), 0));
    cases
}
fn verify_rules() -> Result<(), String> {
    for (bundle, message, source_index) in bad_cases() {
        if bundle.validate() != Err(message.clone()) {
            return Err(format!("Bundle rule changed: {message}"));
        }
        for retained in [false, true] {
            let error = run(&bundle, unlimited(), retained)
                .err()
                .ok_or("Invalid bundle accepted")?;
            match error {
                ObservedImportError::Import(error)
                    if error.kind == ErrorKind::InvalidDefinition
                        && error.message == message
                        && error.span.source == bundle.sources[source_index].id
                        && error.span.start == 0
                        && error.span.end == 0 => {}
                error => {
                    return Err(format!(
                        "Resource diagnostic changed for {message}: {error}"
                    ));
                }
            }
        }
    }
    Ok(())
}
fn verify_priority() -> Result<(), String> {
    let mut bad = fixture();
    bad.version = 9;
    bad.sources[1].id.sha256 = "0".repeat(64);
    if bad.validate() != Err("Unsupported definition bundle version 9".into()) {
        return Err("Ordinary bundle version no longer wins".into());
    }
    for retained in [false, true] {
        match run(&bad, unlimited(), retained) {
            Err(ObservedImportError::Import(error))
                if error.message == "Source content digest mismatch: memory:b.ecore"
                    && error.span.source == bad.sources[1].id => {}
            _ => return Err("Resource source-first prepass priority changed".into()),
        }
    }
    let mut bad = fixture();
    bad.sources.insert(1, bad.sources[0].clone());
    bad.sources[2].id.sha256 = "0".repeat(64);
    if bad.validate() != Err("Duplicate source identity: memory:a.ecore".into()) {
        return Err("Ordinary source/duplicate ordering changed".into());
    }
    match run(&bad, unlimited(), true) {
        Err(ObservedImportError::Import(error))
            if error.message == "Source content digest mismatch: memory:b.ecore"
                && error.span.source == bad.sources[2].id => {}
        _ => return Err("Late source failure did not precede duplicate validation".into()),
    }
    Ok(())
}
fn verify_cancellation() -> Result<(), String> {
    let bundle = long_fixture();
    let meter = unlimited();
    let (_, advances) = run(&bundle, meter.clone(), true).map_err(|e| e.to_string())?;
    for stop in 0..meter.calls.get() {
        if !matches!(run(&bundle, Meter::new(stop, usize::MAX, usize::MAX), true),
            Err(ObservedImportError::Observation(message)) if message == "bundle cancelled")
        {
            return Err(format!(
                "Observer interruption {stop} did not stop admission"
            ));
        }
    }
    let context = InvocationContext::default();
    if begin(&bundle, &context, true)
        .map_err(|e| e.to_string())?
        .finish()
        .is_ok()
    {
        return Err("Unvalidated resources published".into());
    }
    for boundary in 0..=advances {
        let mut pending = begin(&bundle, &context, true).map_err(|e| e.to_string())?;
        for _ in 0..boundary {
            pending.advance(&context).map_err(|e| e.to_string())?;
        }
        let error = ObservedImportError::Observation("bundle cancelled".into());
        if pending.advance(&InvocationContext::new(Meter::new(
            0,
            usize::MAX,
            usize::MAX,
        ))) != Err(error.clone())
            || pending.advance(&context) != Err(error.clone())
            || !matches!(pending.finish(), Err(actual) if actual == error)
        {
            return Err(format!("Cancellation boundary {boundary} was not terminal"));
        }
    }
    Ok(())
}
fn verify_limits() -> Result<(), String> {
    let bundle = long_fixture();
    let old_meter = unlimited();
    let meter = unlimited();
    let old = run(&bundle, old_meter.clone(), false).map_err(|e| e.to_string())?;
    let new = run(&bundle, meter.clone(), true).map_err(|e| e.to_string())?;
    if (meter.work.get(), meter.bytes.get()) != (old_meter.work.get(), old_meter.bytes.get())
        || new.1 <= old.1
        || format!("{:?}", new.0) != format!("{:?}", old.0)
    {
        return Err("Complete resource fees/output changed".into());
    }
    for (work, storage, success) in [
        (meter.work.get() - 1, meter.bytes.get(), false),
        (meter.work.get(), meter.bytes.get() - 1, false),
        (meter.work.get(), meter.bytes.get(), true),
    ] {
        if run(&bundle, Meter::new(usize::MAX, work, storage), true).is_ok() != success {
            return Err("Adjacent admission limit changed".into());
        }
    }
    let transient = unlimited();
    let weak = Rc::downgrade(&transient);
    let mut pending =
        begin(&bundle, &InvocationContext::new(transient), true).map_err(|e| e.to_string())?;
    if weak.upgrade().is_some() {
        return Err("Initialization retained an observer".into());
    }
    for _ in 0..10 {
        let transient = unlimited();
        let weak = Rc::downgrade(&transient);
        pending
            .advance(&InvocationContext::new(transient))
            .map_err(|e| e.to_string())?;
        if weak.upgrade().is_some() {
            return Err("An advance retained its observer".into());
        }
    }
    run(&bundle, unlimited(), true).map_err(|e| e.to_string())?;
    Ok(())
}
fn verify() -> Result<u32, String> {
    verify_good()?;
    verify_rules()?;
    verify_priority()?;
    verify_cancellation()?;
    verify_limits()?;
    verify_source_copy()?;
    Ok(6)
}
fn verify_source_copy() -> Result<(), String> {
    // Independent literal receipts pin the serializer authority used by copying.
    for (count, work, bytes) in [(0, 20, 1715), (1, 28, 2372), (2, 35, 3029)] {
        let source = DefinitionSource {
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
        };
        let meter = unlimited();
        let context = InvocationContext::new(meter.clone());
        let size =
            intersect_model::definitions::retained_storage::reserve_serialized(&source, &context)?;
        context.work(size.div_ceil(256))?;
        if (meter.work.get(), meter.bytes.get()) != (work, bytes) {
            return Err("Literal source metadata receipts changed".into());
        }
    }
    let mut bundle = long_fixture();
    let source = &bundle.sources[0];
    let text = format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='wide' nsURI='{}'><!--{}--></e:EPackage>",
        bundle.namespaces[0].namespace,
        "λ🚀".repeat(1200)
    );
    let mut copied = DefinitionSource::new(source.id.uri.clone(), SourceKind::Ecore, text);
    copied.dependencies.push(copied.id.clone());
    bundle.namespaces[0].source = copied.id.clone();
    bundle.sources[0] = copied;
    let pointer = bundle.sources[0].text.as_ptr();
    let old_meter = unlimited();
    let (old, _) = run(&bundle, old_meter.clone(), false).map_err(|e| e.to_string())?;
    let meter = unlimited();
    let (current, steps) = run(&bundle, meter.clone(), true).map_err(|e| e.to_string())?;
    if format!("{current:?}") != format!("{old:?}")
        || current.document(&bundle.sources[0].id)?.source != bundle.sources[0]
        || (meter.work.get(), meter.bytes.get()) != (old_meter.work.get(), old_meter.bytes.get())
        || bundle.sources[0].text.as_ptr() != pointer
    {
        return Err("Chunked source copy changed exact document, owner or fees".into());
    }
    for boundary in 0..=steps {
        let context = InvocationContext::new(unlimited());
        let mut pending = begin(&bundle, &context, true).map_err(|e| e.to_string())?;
        for _ in 0..boundary {
            pending.advance(&context).map_err(|e| e.to_string())?;
        }
        let observer = Meter::new(0, usize::MAX, usize::MAX);
        let weak = Rc::downgrade(&observer);
        let expected = ObservedImportError::Observation("bundle cancelled".into());
        if pending.advance(&InvocationContext::new(observer)) != Err(expected.clone())
            || weak.upgrade().is_some()
            || pending.advance(&InvocationContext::default()) != Err(expected)
            || pending.finish().is_ok()
        {
            return Err("Copied-source cancellation or observer retention changed".into());
        }
    }
    for quantum in [1, 3, 256] {
        let meter = unlimited();
        let context = InvocationContext::new(meter.clone());
        let mut pending = begin(&bundle, &context, true).map_err(|e| e.to_string())?;
        let mut done = false;
        while !done {
            for _ in 0..quantum {
                done = pending.advance(&context).map_err(|e| e.to_string())?;
                if done {
                    break;
                }
            }
        }
        if format!("{:?}", pending.finish().map_err(|e| e.to_string())?) != format!("{old:?}")
            || (meter.work.get(), meter.bytes.get())
                != (old_meter.work.get(), old_meter.bytes.get())
        {
            return Err("Source-copy resume grouping changed complete resources or fees".into());
        }
    }
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_bundle_validation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    println!("{} retained bundle validation groups passed", verify()?);
    Ok(())
}
