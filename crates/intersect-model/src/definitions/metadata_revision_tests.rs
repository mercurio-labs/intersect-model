use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
};
use std::{cell::Cell, rc::Rc};
struct Meter {
    work: Cell<usize>,
    bytes: Cell<usize>,
    calls: Cell<usize>,
    stop: usize,
    work_limit: usize,
    byte_limit: usize,
}
impl Meter {
    fn new(stop: usize, work_limit: usize, byte_limit: usize) -> Rc<Self> {
        Rc::new(Self {
            work: Cell::new(0),
            bytes: Cell::new(0),
            calls: Cell::new(0),
            stop,
            work_limit,
            byte_limit,
        })
    }
    fn touch(&self) -> Result<(), String> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if call == self.stop {
            Err("metadata cancelled".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.touch()?;
        let next = self.work.get() + units;
        if next > self.work_limit {
            return Err("metadata work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.touch()?;
        let next = self.bytes.get() + bytes;
        if next > self.byte_limit {
            return Err("metadata storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
#[derive(serde::Deserialize)]
struct Cases {
    cases: Vec<Case>,
}
#[derive(serde::Deserialize)]
struct Case {
    name: String,
    prior_revision: String,
    raw_reference_bounds: RawBounds,
    runtime_warnings: Vec<Warning>,
    expected_json: String,
    expected_sha256: String,
    expected_bytes: usize,
}
fn cases() -> Vec<Case> {
    serde_json::from_str::<Cases>(include_str!(
        "../../assets/fixtures/ecore-generics/metadata-revision-oracles.json"
    ))
    .unwrap()
    .cases
}
fn source() -> DefinitionSource {
    DefinitionSource::new("memory:metadata-hash", SourceKind::Ecore, "<a/>")
}
fn pending_for(
    case: &Case,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<MetadataRevisionPreparation, ImportError> {
    MetadataRevisionPreparation::new(
        case.prior_revision.clone(),
        case.raw_reference_bounds.clone(),
        case.runtime_warnings.clone(),
        source,
        observation,
    )
}
fn run(
    case: &Case,
    meter: Rc<Meter>,
) -> Result<(String, usize), crate::definitions::ecore::ObservedImportError> {
    let source = source();
    let operation = InvocationContext::new(meter);
    let observation = ParserObservation::new(&operation);
    let result = (|| {
        let mut pending = pending_for(case, &source.id, &observation)?;
        let mut steps = 1usize;
        while !pending.advance(&source.id, &observation)? {
            steps += 1;
        }
        let values = pending.finish().map_err(|e| error(&source.id, e))?;
        assert_eq!(values.raw_reference_bounds, case.raw_reference_bounds);
        assert_eq!(values.runtime_warnings, case.runtime_warnings);
        Ok((values.revision, steps))
    })();
    observation.finish(result)
}
#[test]
fn metadata_revision_matches_independent_literal_bytes_and_every_interrupt_limit_and_recovery() {
    for case in cases() {
        let original = serde_json::to_vec(&(
            &case.prior_revision,
            &case.raw_reference_bounds,
            &case.runtime_warnings,
        ))
        .unwrap();
        assert_eq!(original, case.expected_json.as_bytes(), "{}", case.name);
        assert_eq!(original.len(), case.expected_bytes);
        assert_eq!(
            format!("{:x}", Sha256::digest(&original)),
            case.expected_sha256
        );
        let full = Meter::new(usize::MAX, usize::MAX, usize::MAX);
        let (digest, steps) = run(&case, full.clone()).unwrap();
        assert_eq!(digest, case.expected_sha256, "{}", case.name);
        // The result uses 64 digest bytes plus the existing 64-byte string slot.
        assert_eq!(full.bytes.get(), STATE_BYTES + 128);
        let bound_count: usize = case
            .raw_reference_bounds
            .values()
            .flat_map(|f| f.values())
            .map(Vec::len)
            .sum();
        assert!(steps >= 3 + bound_count + case.runtime_warnings.len());
        for call in 0..full.calls.get() {
            assert!(
                matches!(
                    run(&case, Meter::new(call, usize::MAX, usize::MAX)),
                    Err(crate::definitions::ecore::ObservedImportError::Observation(message)) if message == "metadata cancelled"
                ),
                "{} call {}",
                case.name,
                call
            );
            assert_eq!(
                run(&case, Meter::new(usize::MAX, usize::MAX, usize::MAX))
                    .unwrap()
                    .0,
                case.expected_sha256
            );
        }
        for (work, bytes, success) in [
            (full.work.get() - 1, full.bytes.get(), false),
            (full.work.get(), full.bytes.get() - 1, false),
            (full.work.get(), full.bytes.get(), true),
            (full.work.get() + 1, full.bytes.get() + 1, true),
        ] {
            let result = run(&case, Meter::new(usize::MAX, work, bytes));
            assert_eq!(
                result.is_ok(),
                success,
                "{} work {} bytes {}",
                case.name,
                work,
                bytes
            );
            if success {
                assert_eq!(result.unwrap().0, case.expected_sha256);
            }
        }
        assert_eq!(
            serde_json::to_vec(&(
                &case.prior_revision,
                &case.raw_reference_bounds,
                &case.runtime_warnings
            ))
            .unwrap(),
            original
        );
    }
}
#[test]
fn metadata_revision_precharges_before_state_and_has_terminal_failure_without_observer_retention() {
    let source = source();
    let case = cases().into_iter().next().unwrap();
    let denied = Meter::new(usize::MAX, usize::MAX, STATE_BYTES - 1);
    let operation = InvocationContext::new(denied.clone());
    assert!(pending_for(&case, &source.id, &ParserObservation::new(&operation)).is_err());
    assert_eq!(denied.bytes.get(), 0);
    let operation = InvocationContext::default();
    for case in cases() {
        let observation = ParserObservation::new(&operation);
        assert!(
            pending_for(&case, &source.id, &observation)
                .unwrap()
                .finish()
                .is_err()
        );
        let mut pending = pending_for(&case, &source.id, &observation).unwrap();
        loop {
            let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
            let weak = Rc::downgrade(&meter);
            let done = {
                let operation = InvocationContext::new(meter.clone());
                pending
                    .advance(&source.id, &ParserObservation::new(&operation))
                    .unwrap()
            };
            drop(meter);
            assert!(weak.upgrade().is_none());
            if done {
                break;
            }
        }
        assert_eq!(pending.finish().unwrap().revision, case.expected_sha256);
        let mut pending = pending_for(&case, &source.id, &observation).unwrap();
        // Complete the prefix before cancellation, so failure cannot be retried
        // from a partially emitted tuple under a fresh host observer.
        assert!(!pending.advance(&source.id, &observation).unwrap());
        let cancelled = InvocationContext::new(Meter::new(0, usize::MAX, usize::MAX));
        assert!(
            pending
                .advance(&source.id, &ParserObservation::new(&cancelled))
                .is_err()
        );
        assert!(pending.advance(&source.id, &observation).is_err());
        assert!(pending.finish().is_err());
    }
}

#[test]
fn metadata_revision_moves_original_metadata_allocations_without_copying_keys_or_payloads() {
    let case = cases().pop().unwrap();
    let key = case.raw_reference_bounds.keys().next().unwrap().as_ptr() as usize;
    let field = case
        .raw_reference_bounds
        .values()
        .next()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .as_ptr() as usize;
    let bound = case
        .raw_reference_bounds
        .values()
        .next()
        .unwrap()
        .values()
        .next()
        .unwrap()[0]
        .as_ptr() as usize;
    let warning = case.runtime_warnings[0].message.as_ptr() as usize;
    let source = source();
    let operation = InvocationContext::default();
    let observation = ParserObservation::new(&operation);
    let mut pending = MetadataRevisionPreparation::new(
        case.prior_revision,
        case.raw_reference_bounds,
        case.runtime_warnings,
        &source.id,
        &observation,
    )
    .unwrap();
    while !pending.advance(&source.id, &observation).unwrap() {}
    let values = pending.finish().unwrap();
    assert_eq!(values.revision, case.expected_sha256);
    assert_eq!(
        values.raw_reference_bounds.keys().next().unwrap().as_ptr() as usize,
        key
    );
    assert_eq!(
        values
            .raw_reference_bounds
            .values()
            .next()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .as_ptr() as usize,
        field
    );
    assert_eq!(
        values
            .raw_reference_bounds
            .values()
            .next()
            .unwrap()
            .values()
            .next()
            .unwrap()[0]
            .as_ptr() as usize,
        bound
    );
    assert_eq!(
        values.runtime_warnings[0].message.as_ptr() as usize,
        warning
    );
}
