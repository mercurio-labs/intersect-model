//! Frozen original A/current B public owner observations.
use super::*;
fn inventory_public_b(case: &OwnerCase, quantum: usize, control: Control) -> Value {
    use crate::definitions::compiler::OwnedRuntimeSourceArrayPreparation;
    let bundle = bundle(&case.source_kind, &case.uri, &case.text, None);
    let resolver = match Resolver::new(&bundle) {
        Ok(value) => value,
        Err(error) => return early(&case.id, "public_materialization", "resolver_setup", error),
    };
    let entry = bundle.sources[0].id.clone();
    let root = resolver
        .resource_sets()
        .0
        .resolve(&entry, &case.receiver)
        .unwrap();
    let leaf = resolver
        .resource_sets()
        .0
        .resolve(&entry, &case.argument)
        .unwrap();
    let requested = [root, leaf]
        .into_iter()
        .map(|declaration| {
            RuntimeType::new(Type::Classifier {
                declaration,
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    drop(resolver);
    let original = serde_json::to_value(&bundle).unwrap();
    let pointer = bundle.sources[0].text.as_ptr();
    let meter = Rc::new(Meter {
        control,
        ..Meter::default()
    });
    let lease = Rc::new(PublicLease(meter.clone()));
    let weak = Rc::downgrade(&lease);
    let context = InvocationContext::new(lease.clone());
    let pending = OwnedRuntimeSourceArrayPreparation::begin_with_retained_generic_agreement(
        bundle,
        entry,
        "urn:legacy-freeze".into(),
        &requested,
        None,
        &context,
    );
    drop(context);
    drop(lease);
    assert!(weak.upgrade().is_none());
    let output = match pending {
        Err(value) => completed_public(*value, pointer, &original, None),
        Ok(mut job) => {
            let mut failure = None;
            loop {
                job = std::hint::black_box(job);
                let lease = Rc::new(PublicLease(meter.clone()));
                let weak = Rc::downgrade(&lease);
                let context = InvocationContext::new(lease.clone());
                let mut done = false;
                for _ in 0..quantum {
                    match job.advance(None, &context) {
                        Ok(true) => {
                            done = true;
                            break;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            failure = Some(format!("{error:?}"));
                            done = true;
                            break;
                        }
                    }
                }
                drop(context);
                drop(lease);
                assert!(weak.upgrade().is_none());
                if done {
                    break;
                }
            }
            completed_public(job.finish(), pointer, &original, failure)
        }
    };
    json!({"id":case.id,"operation":"public_materialization","quantum":quantum,"control":control,"observation":{"output":output,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get()},"fresh_leases_released":true})
}

#[test]
fn source_inventory_original_public_owners_complete_observations() {
    let fixture: Value = serde_json::from_str(include_str!(
        "generic_source_inventory_complete_digests.json"
    ))
    .unwrap();
    let mut hash = Sha256::new();
    let mut count = 0usize;
    let mut cases: Vec<OwnerCase> =
        serde_json::from_str(include_str!("generic_legacy_baseline_owners.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<OwnerCase>>(include_str!(
            "generic_legacy_supplement_owners.json"
        ))
        .unwrap(),
    );
    for case in &cases {
        for q in [1, 3, 256] {
            for retained in [false, true] {
                let row = if retained {
                    inventory_public_b(case, q, Control::default())
                } else {
                    capture_public_owner_inner(case, q, Control::default())
                };
                hash.update(serde_json::to_vec(&json!({"retained_current_b":retained,"case":case.id,"quantum":q,"complete_observation":row})).unwrap());
                hash.update(b"\n");
                count += 1;
            }
        }
    }
    assert_eq!(
        count as u64,
        fixture["original_public_owners"]["rows"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:x}", hash.finalize()),
        fixture["original_public_owners"]["complete_jsonl_sha256"]
            .as_str()
            .unwrap()
    );
}
