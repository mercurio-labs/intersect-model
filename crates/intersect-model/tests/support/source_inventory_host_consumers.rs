//! Verification-only actual public host consumer. No production algorithms live here.
//! Empty registry fixture; selected C boundaries are not attributed to internal phases.
use intersect_model::definitions::{
    conversion::{
        ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
    },
    generic_types::RuntimeType,
    schema::{
        SharedRuntimeArrayTarget, SharedRuntimeArrayViewTarget, SharedRuntimeSourceArrayTarget,
    },
};
use intersect_model::model_reference::{ReferenceDomainState, ReferenceSchemaFactory};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Compatibility,
    GenericAgreement,
    SourceInventoryAgreement,
}
#[derive(Default)]
pub struct Stats {
    pub events: RefCell<Vec<(char, usize)>>,
    refused: Cell<Option<usize>>,
    leases: RefCell<Vec<std::rc::Weak<Lease>>>,
    work_limit: Cell<Option<usize>>,
    storage_limit: Cell<Option<usize>>,
    work: Cell<usize>,
    storage: Cell<usize>,
}
impl Stats {
    fn event(&self, kind: char, n: usize) -> Result<(), String> {
        self.events.borrow_mut().push((kind, n));
        let index = self.events.borrow().len() - 1;
        if self.refused.get() == Some(index) {
            return Err("public host observer refused".into());
        }
        let (total, limit) = match kind {
            'w' => (&self.work, self.work_limit.get()),
            's' => (&self.storage, self.storage_limit.get()),
            _ => return Ok(()),
        };
        total.set(total.get() + n);
        if limit.is_some_and(|limit| total.get() > limit) {
            return Err("public host budget refused".into());
        }
        Ok(())
    }
}
pub struct Lease {
    pub stats: Rc<Stats>,
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected host provider invocation".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.stats.event('c', 0)
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.stats.event('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.stats.event('s', n)
    }
}

fn capture_inner(
    mode: Mode,
    quantum: usize,
    stats: Rc<Stats>,
) -> Result<(Value, Vec<(usize, usize)>), String> {
    let (target, context) = super::legacy::fixture()?;
    let original = context.clone();
    let digest = context.digest()?;
    let lease = Rc::new(Lease {
        stats: stats.clone(),
    });
    let weak = Rc::downgrade(&lease);
    stats.leases.borrow_mut().push(weak.clone());
    let mut pending = {
        let observation = InvocationContext::new(lease.clone());
        let begin = match mode {
            Mode::Compatibility => ReferenceDomainState::<SharedRuntimeSourceArrayTarget>::begin_runtime_observed,
            Mode::GenericAgreement => ReferenceDomainState::<SharedRuntimeSourceArrayTarget>::begin_runtime_observed_with_retained_generic_agreement,
            Mode::SourceInventoryAgreement => ReferenceDomainState::<SharedRuntimeSourceArrayTarget>::begin_runtime_observed_with_retained_source_inventory_agreement,
        };
        begin(
            target.clone(),
            context,
            &ConversionRegistry::default(),
            &observation,
        )?
    };
    drop(lease);
    assert!(weak.upgrade().is_none());
    let mut polls = 0;
    let mut segments = vec![];
    let registry = ConversionRegistry::default();
    loop {
        pending = std::hint::black_box(pending);
        let lease = Rc::new(Lease {
            stats: stats.clone(),
        });
        let weak = Rc::downgrade(&lease);
        stats.leases.borrow_mut().push(weak.clone());
        let done = {
            let observation = InvocationContext::new(lease.clone());
            let mut done = false;
            for _ in 0..quantum {
                polls += 1;
                let start = stats.events.borrow().len();
                let advance = pending.advance(&registry, &observation);
                let end = stats.events.borrow().len();
                segments.push((start, end));
                match advance {
                    Ok(true) => {
                        done = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        assert!(pending.advance(&registry, &observation).is_err());
                        assert_eq!(
                            stats.events.borrow().len(),
                            end,
                            "failed advance must remain poisoned"
                        );
                        assert!(pending.finish(&observation).is_err());
                        assert_eq!(
                            stats.events.borrow().len(),
                            end + 1,
                            "finish retains its original checkpoint"
                        );
                        return Err(error);
                    }
                }
            }
            done
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        if done {
            break;
        }
    }
    let lease = Rc::new(Lease {
        stats: stats.clone(),
    });
    let weak = Rc::downgrade(&lease);
    stats.leases.borrow_mut().push(weak.clone());
    let domain = {
        let observation = InvocationContext::new(lease.clone());
        pending.finish(&observation)?
    };
    drop(lease);
    assert!(weak.upgrade().is_none());
    assert_eq!(domain.context(), &original);
    assert_eq!(domain.runtime_target(), Some(&target));
    let bound = domain.bind("child")?;
    assert_eq!(bound["context"], digest);
    let child = domain.resolve(&bound)?;
    assert_eq!(child.id, "child");
    assert_eq!(child.resource, "memory:child-model");
    assert_eq!(child.values["value"], "leaf");
    assert!(domain.bind("leaf").is_err());
    let events = stats.events.borrow().clone();
    let work: usize = events
        .iter()
        .filter(|(kind, _)| *kind == 'w')
        .map(|(_, n)| *n)
        .sum();
    let storage: usize = events
        .iter()
        .filter(|(kind, _)| *kind == 's')
        .map(|(_, n)| *n)
        .sum();
    let checkpoints = events.iter().filter(|(kind, _)| *kind == 'c').count();
    Ok((
        json!({"polls":polls,"work":work,"storage":storage,"checkpoints":checkpoints,
        "events":events,"output":{"context":domain.context(),"schema":domain.schema(),
        "class":domain.class(),"target":domain.runtime_target(),"bound":bound,"child":child},
        "fresh_leases_released":true}),
        segments,
    ))
}

fn digest(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

type Segments = Vec<(usize, usize)>;
fn capture(mode: Mode, quantum: usize, stats: Rc<Stats>) -> Result<(Value, Segments), String> {
    if ![1, 3, 256].contains(&quantum) {
        return Err("Unsupported verification quantum".into());
    }
    let result = capture_inner(mode, quantum, stats.clone());
    assert!(
        stats
            .leases
            .borrow()
            .iter()
            .all(|weak| weak.upgrade().is_none()),
        "No observation lease may outlive a completed/dropped capture"
    );
    result
}
fn fresh(mode: Mode, quantum: usize) -> Result<(Value, Segments), String> {
    capture(mode, quantum, Rc::new(Stats::default()))
}
fn row(mode: Mode, quantum: usize, observation: Value, segments: Segments) -> Value {
    let source = observation["output"]["context"].clone();
    let mut result = json!({"all_poll_segments":segments,"complete_observation":observation,"quantum":quantum,"source":source});
    match mode {
        Mode::Compatibility => result["retained_current_b"] = json!(false),
        Mode::GenericAgreement => result["retained_current_b"] = json!(true),
        Mode::SourceInventoryAgreement => {
            result["polling_mode"] = json!("retained_source_inventory_agreement")
        }
    }
    result
}
/// Complete raw row for first capture and native/WASM equality, without a C golden.
pub fn capture_raw(mode: Mode, quantum: usize) -> Result<Value, String> {
    let (observation, segments) = fresh(mode, quantum)?;
    Ok(row(mode, quantum, observation, segments))
}
fn compatibility() -> Result<Value, String> {
    let original: Value =
        serde_json::from_str(include_str!("source_inventory_host_original_a_b_pins.json"))
            .map_err(|e| e.to_string())?;
    let mut b = None;
    for mode in [Mode::Compatibility, Mode::GenericAgreement] {
        for quantum in [1, 3, 256] {
            let actual = capture_raw(mode, quantum)?;
            let expected = original["rows"]
                .as_array()
                .ok_or("Missing frozen original rows")?
                .iter()
                .find(|r| {
                    r["quantum"] == quantum
                        && r["retained_current_b"] == matches!(mode, Mode::GenericAgreement)
                })
                .ok_or("Missing original A/B row")?;
            let observation = &actual["complete_observation"];
            assert_eq!(digest(&actual)?, expected["row_sha256"]);
            assert_eq!(digest(observation)?, expected["observation_sha256"]);
            assert_eq!(digest(&observation["events"])?, expected["events_sha256"]);
            assert_eq!(
                digest(&actual["all_poll_segments"])?,
                expected["poll_segments_sha256"]
            );
            assert_eq!(digest(&observation["output"])?, expected["output_sha256"]);
            assert_eq!(digest(&actual["source"])?, expected["source_sha256"]);
            if matches!(mode, Mode::GenericAgreement) && quantum == 1 {
                b = Some(observation.clone());
            }
        }
    }
    b.ok_or("Missing current B baseline".into())
}
fn events(value: &Value) -> Result<Vec<(char, usize)>, String> {
    value["events"]
        .as_array()
        .ok_or("Missing events")?
        .iter()
        .map(|e| {
            Ok((
                e[0].as_str()
                    .and_then(|s| s.chars().next())
                    .ok_or("Missing event kind")?,
                usize::try_from(e[1].as_u64().ok_or("Missing event units")?)
                    .map_err(|e| e.to_string())?,
            ))
        })
        .collect()
}
fn count(value: &Value, key: &str) -> Result<usize, String> {
    usize::try_from(value[key].as_u64().ok_or("Missing capture count")?).map_err(|e| e.to_string())
}
// Fixed additive C identity was derived only after the original full native
// capture was frozen. Installation requires initial full native/WASM equality.
fn assert_first_c(
    quantum: usize,
    observation: &Value,
    segments: &[(usize, usize)],
) -> Result<(), String> {
    let original: Value =
        serde_json::from_str(include_str!("source_inventory_host_c_first_pins.json"))
            .map_err(|e| e.to_string())?;
    let expected = original["rows"]
        .as_array()
        .ok_or("Missing first C rows")?
        .iter()
        .find(|r| r["quantum"] == quantum)
        .ok_or("Missing first C quantum")?;
    let actual = row(
        Mode::SourceInventoryAgreement,
        quantum,
        observation.clone(),
        segments.to_vec(),
    );
    assert_eq!(digest(&actual)?, expected["row_sha256"]);
    assert_eq!(digest(observation)?, expected["observation_sha256"]);
    assert_eq!(digest(&observation["events"])?, expected["events_sha256"]);
    assert_eq!(
        digest(&actual["all_poll_segments"])?,
        expected["poll_segments_sha256"]
    );
    assert_eq!(digest(&observation["output"])?, expected["output_sha256"]);
    assert_eq!(digest(&actual["source"])?, expected["source_sha256"]);
    Ok(())
}
fn inventory(b: &Value) -> Result<(Value, Segments), String> {
    let (c, segments) = fresh(Mode::SourceInventoryAgreement, 1)?;
    assert_first_c(1, &c, &segments)?;
    assert_eq!(c["output"], b["output"]);
    assert_eq!(c["work"], b["work"]);
    assert_eq!(c["storage"], b["storage"]);
    let b_events = events(b)?;
    let c_events = events(&c)?;
    let nonzero = |v: &[(char, usize)]| {
        v.iter()
            .copied()
            .filter(|(_, n)| *n != 0)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        nonzero(&c_events),
        nonzero(&b_events),
        "Full ordered original nonzero fees must remain exact"
    );
    let extra = count(&c, "polls")?
        .checked_sub(count(b, "polls")?)
        .ok_or("C lost public polls")?;
    assert!(extra > 0);
    // Source audit: each extra inventory subdivision crosses host advance's
    // checkpoint, OwnedRuntimeSourceArrayPreparation's checkpoint, and the
    // unchanged reports/inventory work(0) calls. Original inner fees may move
    // between polls. This checks totals, NOT old/new whole-poll alignment and
    // NOT an attribution of a quiet-looking C poll to a specific private phase.
    assert_eq!(
        count(&c, "checkpoints")? - count(b, "checkpoints")?,
        extra * 2
    );
    let zeros = |v: &[(char, usize)]| v.iter().filter(|(kind, n)| *kind == 'w' && *n == 0).count();
    assert_eq!(zeros(&c_events) - zeros(&b_events), extra * 2);
    assert_eq!(c_events.len() - b_events.len(), extra * 4);
    for quantum in [3, 256] {
        let (next, next_segments) = fresh(Mode::SourceInventoryAgreement, quantum)?;
        assert_first_c(quantum, &next, &next_segments)?;
        assert_eq!(next, c);
        assert_eq!(next_segments, segments);
    }
    Ok((c, segments))
}
fn refusals(c: &Value, segments: &[(usize, usize)]) -> Result<(), String> {
    let original = events(c)?;
    assert!(!segments.is_empty());
    // These are honestly selected first/middle/last ACTUAL C poll boundaries.
    // They are not claimed to be exclusively new inventory boundaries.
    for (start, end) in [
        segments[0],
        segments[segments.len() / 2],
        segments[segments.len() - 1],
    ] {
        for index in start..end {
            let stats = Rc::new(Stats::default());
            stats.refused.set(Some(index));
            assert!(capture(Mode::SourceInventoryAgreement, 3, stats.clone()).is_err());
            let actual = stats.events.borrow();
            assert_eq!(&actual[..=index], &original[..=index]);
            assert_eq!(
                actual.len(),
                index + 2,
                "One finish checkpoint follows the failed/poisoned advance"
            );
            assert_eq!(actual[index + 1], ('c', 0));
        }
    }
    assert_eq!(fresh(Mode::SourceInventoryAgreement, 256)?.0, *c);
    Ok(())
}
fn budgets(c: &Value) -> Result<(), String> {
    let work = count(c, "work")?;
    let storage = count(c, "storage")?;
    for quantum in [1, 3, 256] {
        let exact = Rc::new(Stats::default());
        exact.work_limit.set(Some(work));
        exact.storage_limit.set(Some(storage));
        assert_eq!(
            capture(Mode::SourceInventoryAgreement, quantum, exact)?.0,
            *c
        );
        for (w, s) in [
            (work.checked_sub(1).ok_or("No work to refuse")?, storage),
            (work, storage.checked_sub(1).ok_or("No storage to refuse")?),
        ] {
            let short = Rc::new(Stats::default());
            short.work_limit.set(Some(w));
            short.storage_limit.set(Some(s));
            assert!(capture(Mode::SourceInventoryAgreement, quantum, short).is_err());
        }
    }
    assert_eq!(fresh(Mode::SourceInventoryAgreement, 1)?.0, *c);
    Ok(())
}
fn unsupported_family<A: ReferenceSchemaFactory>(
    target: &RuntimeType,
    context: &ReferenceContext,
) -> Result<(), String> {
    let registry = ConversionRegistry::default();
    let observation = InvocationContext::default();
    let expected = "Retained source inventory agreement is unsupported by this schema factory";
    let mut copy = context.clone();
    let result = A::begin_compilation_with_retained_source_inventory_agreement(
        &mut copy,
        &registry,
        &observation,
    );
    assert_eq!(
        result.err().ok_or("Unsupported factory accepted C")?,
        expected
    );
    assert_eq!(&copy, context);
    let result =
        ReferenceDomainState::<A>::begin_runtime_observed_with_retained_source_inventory_agreement(
            target.clone(),
            context.clone(),
            &registry,
            &observation,
        );
    assert_eq!(
        result
            .err()
            .ok_or("Unsupported public host family accepted C")?,
        expected
    );
    Ok(())
}
fn unsupported() -> Result<(), String> {
    let (target, mut context) = super::legacy::fixture()?;
    unsupported_family::<String>(&target, &context)?;
    unsupported_family::<SharedRuntimeArrayTarget>(&target, &context)?;
    unsupported_family::<SharedRuntimeArrayViewTarget>(&target, &context)?;
    let registry = ConversionRegistry::default();
    let observation = InvocationContext::default();
    context.materialized_types.clear();
    let original = context.clone();
    assert!(
        <SharedRuntimeSourceArrayTarget as ReferenceSchemaFactory>::begin_compilation(
            &mut context,
            &registry,
            &observation
        )?
        .is_none()
    );
    assert_eq!(context, original);
    let expected = "Retained source inventory agreement requires materialized runtime types";
    let result = <SharedRuntimeSourceArrayTarget as ReferenceSchemaFactory>::begin_compilation_with_retained_source_inventory_agreement(&mut context, &registry, &observation);
    assert_eq!(
        result
            .err()
            .ok_or("Empty C selection accepted by factory")?,
        expected
    );
    assert_eq!(context, original);
    let result = ReferenceDomainState::<SharedRuntimeSourceArrayTarget>::begin_runtime_observed_with_retained_source_inventory_agreement(target, context, &registry, &observation);
    assert_eq!(
        result
            .err()
            .ok_or("Empty C selection accepted by public host")?,
        expected
    );
    Ok(())
}
/// Five C public-host groups, shared verbatim by native and actual WASM exports.
/// The parent module additionally runs the unmodified five legacy host groups.
pub fn verify() -> Result<u32, String> {
    let b = compatibility()?;
    let (c, segments) = inventory(&b)?;
    refusals(&c, &segments)?;
    budgets(&c)?;
    unsupported()?;
    Ok(5)
}
