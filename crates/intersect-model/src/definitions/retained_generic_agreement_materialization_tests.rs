//! Public A/B contracts, using an independently captured ORIGINAL provider row.
use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, InvocationContext,
        InvocationObserver, ProviderId,
    },
    resources::ResourceSet,
};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
const TEXT: &str = "rcore 1;package legacy version \"1\";metamodel \"urn:legacy-freeze\" prefix l {class Leaf {}class Box<T> {ref value:T [0,1];}class Child extends Box<Leaf> {}datatype Token instanceClass \"example.Token\" {}}";
struct Provider {
    contract: ConversionContract,
    changed: ConversionContract,
    stale: Cell<bool>,
}
impl ConversionProvider for Provider {
    fn contract(&self) -> &ConversionContract {
        if self.stale.get() {
            &self.changed
        } else {
            &self.contract
        }
    }
    fn parse(&self, _: &str) -> Result<Value, String> {
        Err("unexpected provider parse".into())
    }
    fn format(&self, _: &Value) -> Result<String, String> {
        Err("unexpected provider format".into())
    }
    fn validate(&self, _: &Value) -> Result<(), String> {
        Err("unexpected provider validate".into())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Err("unexpected provider default".into())
    }
}
struct Fixture {
    bundle: DefinitionBundle,
    requested: [RuntimeType; 2],
    datatype: crate::definitions::datatypes::Datatype,
    provider: Rc<Provider>,
}
impl Fixture {
    fn new() -> Self {
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![DefinitionSource::new(
                "memory:legacy-provider-original.rcore",
                SourceKind::Rcore,
                TEXT,
            )],
            namespaces: vec![],
        };
        let resources = ResourceSet::new(&bundle).unwrap();
        let entry = &bundle.sources[0].id;
        let select = |s| resources.resolve(entry, s).unwrap();
        let token = select("#//Token");
        let datatype =
            crate::definitions::compiler::import_datatype_bundle(&bundle, &token).unwrap();
        let contract = ConversionContract {
            datatype: token,
            provider: ProviderId {
                name: "legacy.provider.original".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        };
        let mut changed = contract.clone();
        changed.provider.version = "stale".into();
        let provider = Rc::new(Provider {
            contract,
            changed,
            stale: Cell::new(false),
        });
        let requested = ["#//Child", "#//Leaf"].map(|name| {
            RuntimeType::new(crate::definitions::generic_types::Type::Classifier {
                declaration: select(name),
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        });
        Self {
            bundle,
            requested,
            datatype,
            provider,
        }
    }
    fn registry(&self) -> ConversionRegistry<'_> {
        let mut registry = ConversionRegistry::default();
        registry
            .register_shared(&self.datatype, self.provider.clone())
            .unwrap();
        registry
            .retain_compilation_dependency(&self.provider.contract.datatype)
            .unwrap();
        registry
    }
}
#[derive(Default)]
struct Meter {
    events: RefCell<Vec<(char, usize)>>,
    work: Cell<usize>,
    storage: Cell<usize>,
    deny: Option<usize>,
    max_work: Option<usize>,
    max_storage: Option<usize>,
}
impl Meter {
    fn record(&self, kind: char, n: usize) -> Result<(), String> {
        let mut events = self.events.borrow_mut();
        events.push((kind, n));
        if self.deny == Some(events.len()) {
            return Err("retained generic agreement denied".into());
        }
        if kind == 'w' {
            let next = self.work.get() + n;
            if self.max_work.is_some_and(|m| next > m) {
                return Err("retained generic work denied".into());
            }
            self.work.set(next)
        }
        if kind == 's' {
            let next = self.storage.get() + n;
            if self.max_storage.is_some_and(|m| next > m) {
                return Err("retained generic storage denied".into());
            }
            self.storage.set(next)
        }
        Ok(())
    }
}
struct Lease(Rc<Meter>);
impl InvocationObserver for Lease {
    fn checkpoint(&self) -> Result<(), String> {
        self.0.record('c', 0)
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.0.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.0.record('s', n)
    }
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("unexpected provider invocation".into())
    }
}
fn continuation(job: &OwnedRuntimeSourceArrayPreparation) -> bool {
    job.preparation
        .holder
        .with_dependent(|_, state| match state {
            MaterializationState::Ready(state) => state
                .reports()
                .is_some_and(ReportPreparation::is_silent_agreement_step),
            MaterializationState::Loading(_) => false,
        })
}
fn drive(
    retained: bool,
    quantum: usize,
    meter: Rc<Meter>,
    stale_at: Option<usize>,
    drop_at: Option<usize>,
) -> (Value, Vec<(usize, usize)>) {
    let fixture = Fixture::new();
    let bundle = fixture.bundle.clone();
    let before = serde_json::to_value(&bundle).unwrap();
    let pointer = bundle.sources[0].text.as_ptr();
    let entry = bundle.sources[0].id.clone();
    let lease = Rc::new(Lease(meter.clone()));
    let weak = Rc::downgrade(&lease);
    let mut job = {
        let context = InvocationContext::new(lease.clone());
        let registry = fixture.registry();
        let begin = if retained {
            OwnedRuntimeSourceArrayPreparation::begin_with_retained_generic_agreement
        } else {
            OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources
        };
        begin(
            bundle,
            entry,
            "urn:legacy-freeze".into(),
            &fixture.requested,
            Some(&registry),
            &context,
        )
        .unwrap_or_else(|_| panic!("fixture initialization failed"))
    };
    drop(lease);
    assert!(weak.upgrade().is_none());
    assert_eq!(
        job.preparation.generic_agreement_polling == GenericAgreementPolling::Retained,
        retained
    );
    let mut polls = 0;
    let mut quiet = vec![];
    let mut error = None;
    'outer: loop {
        job = std::hint::black_box(job);
        let lease = Rc::new(Lease(meter.clone()));
        let weak = Rc::downgrade(&lease);
        let mut done = false;
        {
            let context = InvocationContext::new(lease.clone());
            for _ in 0..quantum {
                if drop_at == Some(polls) {
                    done = true;
                    break;
                }
                if continuation(&job) {
                    quiet.push((polls + 1, meter.events.borrow().len()))
                }
                polls += 1;
                assert!(polls < 100_000);
                if stale_at == Some(polls) {
                    fixture.provider.stale.set(true)
                }
                let registry = fixture.registry();
                match job.advance(Some(&registry), &context) {
                    Ok(true) => {
                        done = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(failure) => {
                        error = Some(failure.to_string());
                        done = true;
                        break;
                    }
                }
            }
        }
        drop(lease);
        assert!(weak.upgrade().is_none(), "public job retained a lease");
        if done {
            break 'outer;
        }
    }
    if error.is_some() {
        fixture.provider.stale.set(false);
        let registry = fixture.registry();
        let quiet_meter = Rc::new(Meter::default());
        let lease = Rc::new(Lease(quiet_meter.clone()));
        let context = InvocationContext::new(lease);
        assert!(job.advance(Some(&registry), &context).is_err());
        assert!(
            quiet_meter.events.borrow().is_empty(),
            "failed attempt repeated observations"
        );
    }
    let events_before = meter.events.borrow().len();
    let result = job.finish();
    assert_eq!(meter.events.borrow().len(), events_before);
    assert_eq!(
        result.materialization.bundle.sources[0].text.as_ptr(),
        pointer
    );
    assert_eq!(
        serde_json::to_value(&result.materialization.bundle).unwrap(),
        before
    );
    if error.is_some() || drop_at.is_some() {
        assert!(
            result.materialization.result.is_err(),
            "partial schema published"
        )
    }
    let output = match result.materialization.result {
        Ok(schema) => serde_json::json!({"ok":schema}),
        Err(error) => serde_json::json!({"error":format!("{error:?}")}),
    };
    (
        serde_json::json!({"fixture":"provider-bearing original public A","source":before,"output":output,"entry_source":result.entry_source,"namespace":result.namespace,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get(),"polls":polls,"exact_input_buffer_recovered":true}),
        quiet,
    )
}
#[test]
fn retained_generic_agreement_original_provider_a_full_transcript_is_unchanged() {
    let expected: Value =
        serde_json::from_str(include_str!("generic_legacy_provider_original_digest.json")).unwrap();
    for quantum in [1, 3, 256] {
        let (row, quiet) = drive(false, quantum, Rc::new(Meter::default()), None, None);
        assert!(quiet.is_empty());
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(&row).unwrap())),
            expected["complete_canonical_sha256"].as_str().unwrap()
        );
    }
}
#[test]
fn retained_generic_agreement_provider_b_fees_callbacks_stale_each_new_poll_drop_recovery() {
    let (a, _) = drive(false, 1, Rc::new(Meter::default()), None, None);
    let (b, quiet) = drive(true, 1, Rc::new(Meter::default()), None, None);
    let literal: Value =
        serde_json::from_str(include_str!("generic_legacy_retained_provider_digest.json")).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(serde_json::to_vec(&b).unwrap())),
        literal["complete_canonical_sha256"].as_str().unwrap()
    );
    assert_eq!(b["output"], a["output"]);
    assert_eq!(b["source"], a["source"]);
    assert_eq!(b["admitted_work"], a["admitted_work"]);
    assert_eq!(b["admitted_storage"], a["admitted_storage"]);
    assert_eq!(quiet.len(), 158);
    assert_eq!(b["polls"], 3697);
    assert_eq!(b["events"].as_array().unwrap().len(), 46430);
    for quantum in [3, 256] {
        assert_eq!(
            drive(true, quantum, Rc::new(Meter::default()), None, None).0,
            b
        )
    }
    let full_controls: Value = serde_json::from_str(include_str!(
        "generic_legacy_retained_provider_controls.json"
    ))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&quiet).unwrap(),
        full_controls["positions"]
    );
    let mut controls = full_controls["controls"].as_array().unwrap().iter();
    let mut check_control = |kind: &str, poll: usize, callback: Option<usize>, row: &Value| {
        let expected = controls.next().unwrap();
        let complete = serde_json::json!({"kind":kind,"poll":poll,"callback":callback,"complete_observation":row});
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&complete).unwrap())
            ),
            expected["complete_sha256"].as_str().unwrap()
        );
    };
    for (poll, start) in quiet {
        let (row, _) = drive(true, 1, Rc::new(Meter::default()), Some(poll), None);
        assert!(
            row["output"]["error"]
                .as_str()
                .unwrap()
                .contains("Compilation provider selections changed")
        );
        check_control("stale", poll, None, &row);
        let (partial, _) = drive(true, 1, Rc::new(Meter::default()), None, Some(poll));
        check_control("drop", poll, None, &partial);
        assert!(partial["output"].get("error").is_some());
        for offset in 1..=3 {
            let denial = start + offset;
            let (row, _) = drive(
                true,
                1,
                Rc::new(Meter {
                    deny: Some(denial),
                    ..Meter::default()
                }),
                None,
                None,
            );
            check_control("deny", poll, Some(denial), &row);
            assert!(
                row["output"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("retained generic agreement denied")
            );
            assert_eq!(
                row["events"],
                serde_json::json!(&b["events"].as_array().unwrap()[..denial])
            );
        }
    }
    assert!(controls.next().is_none());
    for quantum in [1, 3, 256] {
        let exact = Rc::new(Meter {
            max_work: Some(20807),
            max_storage: Some(2272343),
            ..Meter::default()
        });
        assert_eq!(drive(true, quantum, exact, None, None).0, b);
        for (work, storage, message) in [
            (20806, 2272343, "retained generic work denied"),
            (20807, 2272342, "retained generic storage denied"),
        ] {
            let (row, _) = drive(
                true,
                quantum,
                Rc::new(Meter {
                    max_work: Some(work),
                    max_storage: Some(storage),
                    ..Meter::default()
                }),
                None,
                None,
            );
            assert!(row["output"]["error"].as_str().unwrap().contains(message));
        }
    }
    let (retry, _) = drive(true, 1, Rc::new(Meter::default()), None, None);
    assert_eq!(retry, b);
}

#[path = "retained_source_inventory_materialization_tests.rs"]
mod retained_source_inventory_controls;

#[path = "generic_source_inventory_provider_compatibility_tests.rs"]
mod inventory_compatibility_tests;
