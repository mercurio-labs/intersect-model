//! Observational producer only: complete original outputs/errors and ordered events.
//! Compact expectations hash complete pre-edit serialized outputs and all ordered callbacks.
use super::*;
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{ObservedImportError, ParserObservation},
    generic_types::{ParameterBinding, RuntimeType, Substitution, Type},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Clone, Deserialize)]
struct Dependency {
    uri: String,
    text: String,
}
#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    source_kind: String,
    uri: String,
    text: String,
    owner: String,
    feature: String,
    dependency: Option<Dependency>,
    generic_fragment: Option<String>,
    selected_path: Option<Vec<usize>>,
}
#[derive(Clone, Deserialize)]
struct OwnerCase {
    id: String,
    source_kind: String,
    uri: String,
    text: String,
    receiver: String,
    signature: String,
    parameter: String,
    argument: String,
}
#[derive(Clone, Copy, Default, Serialize)]
struct Control {
    deny: Option<usize>,
    max_work: Option<usize>,
    max_storage: Option<usize>,
}
#[derive(Default)]
struct Meter {
    events: RefCell<Vec<(char, usize)>>,
    work: Cell<usize>,
    storage: Cell<usize>,
    control: Control,
}
impl Meter {
    fn record(&self, kind: char, n: usize) -> Result<(), String> {
        let mut events = self.events.borrow_mut();
        events.push((kind, n));
        if self.control.deny == Some(events.len()) {
            return Err("legacy baseline denied".into());
        }
        match kind {
            'w' => {
                let next = self.work.get() + n;
                if self.control.max_work.is_some_and(|max| next > max) {
                    return Err("legacy work denied".into());
                }
                self.work.set(next);
            }
            's' => {
                let next = self.storage.get() + n;
                if self.control.max_storage.is_some_and(|max| next > max) {
                    return Err("legacy storage denied".into());
                }
                self.storage.set(next);
            }
            _ => {}
        }
        Ok(())
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        self.record('p', 0)?;
        Err("Unexpected legacy provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.record('c', 0)
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
fn encoded<T: Serialize>(result: Result<T, ObservedImportError>) -> Value {
    match result {
        Ok(value) => json!({"ok":value}),
        Err(failure) => json!({"error":format!("{failure:?}")}),
    }
}
fn observe(
    control: Control,
    run: impl FnOnce(&InvocationContext<'_>, &ParserObservation<'_, '_>) -> Value,
) -> Value {
    let meter = Rc::new(Meter {
        control,
        ..Meter::default()
    });
    let context = InvocationContext::new(meter.clone());
    let observation = ParserObservation::new(&context);
    let output = run(&context, &observation);
    json!({"output":output,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get()})
}
fn source(kind: &str, uri: &str, text: &str) -> DefinitionSource {
    DefinitionSource::new(
        uri,
        if kind == "rcore" {
            SourceKind::Rcore
        } else {
            SourceKind::Ecore
        },
        text,
    )
}
fn bundle(kind: &str, uri: &str, text: &str, dependency: Option<&Dependency>) -> DefinitionBundle {
    let mut sources = vec![source(kind, uri, text)];
    if let Some(dependency) = dependency {
        sources.push(source("ecore", &dependency.uri, &dependency.text));
    }
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}
fn early(id: &str, operation: &str, stage: &str, error: impl std::fmt::Debug) -> Value {
    json!({"id":id,"operation":operation,"stage":stage,"error":format!("{error:?}"),"events":[]})
}
fn capture_case(case: &Case, operation: &str, control: Control) -> Value {
    let bundle = bundle(
        &case.source_kind,
        &case.uri,
        &case.text,
        case.dependency.as_ref(),
    );
    let before = serde_json::to_value(&bundle).unwrap();
    let resolver = match Resolver::new(&bundle) {
        Ok(value) => value,
        Err(error) => return early(&case.id, operation, "resolver_setup", error),
    };
    let owner = match resolver
        .resource_sets()
        .0
        .resolve(&bundle.sources[0].id, &case.owner)
    {
        Ok(value) => value,
        Err(error) => return early(&case.id, operation, "owner_selection", error),
    };
    let selected = if let Some(path) = &case.selected_path {
        Some(DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: path.clone(),
        })
    } else if let Some(fragment) = &case.generic_fragment {
        match resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, fragment)
        {
            Ok(value) => Some(value),
            Err(error) => return early(&case.id, operation, "generic_selection", error),
        }
    } else {
        None
    };
    let document = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let observation = observe(control, |context, observation| match operation {
        "legacy" => encoded(observation.finish(resolver.legacy_references_context(
            &owner,
            &case.feature,
            observation,
        ))),
        "agreement" => {
            encoded(observation.finish(resolver.check_legacy_context(&owner, observation)))
        }
        "inventory" => encoded(resolver.resolve_all_observed(context)),
        "normalization" => encoded(resolver.type_expression_observed(
            selected.as_ref().expect("Selected normalization case"),
            context,
        )),
        _ => panic!("Unknown capture operation"),
    });
    assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    assert_eq!(
        resolver
            .resource_sets()
            .0
            .document(&bundle.sources[0].id)
            .unwrap() as *const _,
        document
    );
    json!({"id":case.id,"operation":operation,"control":control,"observation":observation,"source_bundle_unchanged":true,"parsed_document_identity_unchanged":true})
}
fn capture_owner_inner(case: &OwnerCase, profile: usize, mode: &str, control: Control) -> Value {
    let bundle = bundle(&case.source_kind, &case.uri, &case.text, None);
    let before = serde_json::to_value(&bundle).unwrap();
    let input_pointer = bundle.sources[0].text.as_ptr();
    let mut resolver = match Resolver::new(&bundle) {
        Ok(value) => value,
        Err(error) => return early(&case.id, mode, "owner_resolver_setup", error),
    };
    let select = |fragment: &str| {
        resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, fragment)
    };
    let receiver_id = match select(&case.receiver) {
        Ok(value) => value,
        Err(error) => return early(&case.id, mode, "receiver_selection", error),
    };
    let owner = match select(&case.signature) {
        Ok(value) => value,
        Err(error) => return early(&case.id, mode, "signature_selection", error),
    };
    let parameter = match select(&case.parameter) {
        Ok(value) => value,
        Err(error) => return early(&case.id, mode, "parameter_selection", error),
    };
    let argument = match select(&case.argument) {
        Ok(value) => value,
        Err(error) => return early(&case.id, mode, "argument_selection", error),
    };
    let receiver = RuntimeType::new(Type::Classifier {
        declaration: receiver_id,
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let environment = Substitution::new(if profile >= 3 {
        vec![]
    } else {
        vec![ParameterBinding {
            parameter,
            argument: Type::Classifier {
                declaration: argument,
                arguments: vec![],
                raw: false,
            },
        }]
    })
    .unwrap();
    let span = SourceSpan {
        source: bundle.sources[0].id.clone(),
        start: 17,
        end: 29,
    };
    let mut preparation = vec![];
    if profile > 0 {
        let row = observe(Control::default(), |_, observation| {
            encoded(
                observation
                    .finish(resolver.resolve_materialization_inventory_observed(observation)),
            )
        });
        let failed = row["output"].get("error").is_some();
        preparation.push(json!({"stage":"inventory","observation":row}));
        if failed {
            return json!({"id":case.id,"profile":profile,"operation":mode,"stage":"prepared_inventory_rejected","preparation":preparation});
        }
    }
    if profile >= 2 {
        resolver = resolver.with_borrowed_owner_inputs();
    }
    if profile >= 3 {
        let row = observe(Control::default(), |_, observation| {
            encoded(
                observation
                    .finish(resolver.enable_compiled_feature_signatures_observed(observation)),
            )
        });
        let failed = row["output"].get("error").is_some();
        preparation.push(json!({"stage":"compiled_signatures","observation":row}));
        if failed {
            return json!({"id":case.id,"profile":profile,"operation":mode,"stage":"compiled_setup_rejected","preparation":preparation});
        }
    }
    if profile == 4 {
        preparation.push(json!({"stage":"warm_owner","observation":observe(Control::default(),|_,observation|encoded(observation.finish(resolver.legacy_owner_capture(mode,&receiver,&owner,&environment,&span,observation))))}));
    }
    let observation = observe(control, |_, observation| {
        encoded(observation.finish(resolver.legacy_owner_capture(
            mode,
            &receiver,
            &owner,
            &environment,
            &span,
            observation,
        )))
    });
    assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    assert_eq!(bundle.sources[0].text.as_ptr(), input_pointer);
    json!({"id":case.id,"profile":profile,"operation":mode,"control":control,"preparation":preparation,"observation":observation,"cache_after":resolver.legacy_owner_cache_capture(),"source_bundle_unchanged":true,"source_buffer_identity_unchanged":true})
}

// Production public owner consumption, with full returned source/result fields.
// Host-owned fixture cloning occurs before admission and grants no source proof.
struct PublicLease(Rc<Meter>);
impl InvocationObserver for PublicLease {
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
fn completed_public(
    value: crate::definitions::compiler::OwnedRuntimeSourceArrayPreparationResult,
    pointer: *const u8,
    original: &Value,
    advance_error: Option<String>,
) -> Value {
    assert_eq!(
        value.materialization.bundle.sources[0].text.as_ptr(),
        pointer
    );
    assert_eq!(
        serde_json::to_value(&value.materialization.bundle).unwrap(),
        *original
    );
    json!({"entry_source":value.entry_source,"namespace":value.namespace,"bundle":value.materialization.bundle,"result":encoded(value.materialization.result),"advance_error":advance_error,"source_buffer_identity_unchanged":true})
}
fn capture_public_owner_inner(case: &OwnerCase, quantum: usize, control: Control) -> Value {
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
    let pending = OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources(
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

fn capture_owner(case: &OwnerCase, profile: usize, mode: &str, control: Control) -> Value {
    let mut row = capture_owner_inner(case, profile, mode, control);
    row["profile"] = json!(profile);
    row["signature_environment"] = json!(if profile >= 3 {
        "empty_for_compiled_eligibility"
    } else {
        "source_parameter_argument"
    });
    row["control"] = json!(control);
    row
}
fn capture_public_owner(case: &OwnerCase, quantum: usize, control: Control) -> Value {
    let mut row = capture_public_owner_inner(case, quantum, control);
    row["quantum"] = json!(quantum);
    row["control"] = json!(control);
    row
}
struct RawWriter {
    digest: Sha256,
    expected: Option<Vec<Value>>,
    count: usize,
    expected_count: usize,
    expected_sha: String,
}
impl RawWriter {
    fn new(profiles: Option<Vec<Value>>, count: usize, sha: &str) -> Self {
        Self {
            digest: Sha256::new(),
            expected: profiles,
            count: 0,
            expected_count: count,
            expected_sha: sha.into(),
        }
    }
    fn row(&mut self, value: Value) {
        let bytes = serde_json::to_vec(&value).unwrap();
        if let Some(profiles) = &self.expected {
            let actual = format!("{:x}", Sha256::digest(&bytes));
            assert_eq!(
                actual,
                profiles[self.count]["complete_sha256"].as_str().unwrap(),
                "complete original row {}: {} {} profile={} quantum={}",
                self.count,
                value["id"],
                value["operation"],
                value["profile"],
                value["quantum"]
            );
        }
        self.digest.update(&bytes);
        self.digest.update(b"\n");
        self.count += 1;
    }
    fn finish(self) {
        assert_eq!(self.count, self.expected_count);
        assert_eq!(format!("{:x}", self.digest.finalize()), self.expected_sha);
    }
}
#[test]
fn retained_legacy_complete_original_370_and_9779_observations() {
    let frozen: Value =
        serde_json::from_str(include_str!("generic_legacy_baseline_digests.json")).unwrap();
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic_legacy_baseline_cases.json")).unwrap();
    let owners: Vec<OwnerCase> =
        serde_json::from_str(include_str!("generic_legacy_baseline_owners.json")).unwrap();
    let mut main = RawWriter::new(
        Some(frozen["main_rows"].as_array().unwrap().clone()),
        370,
        frozen["main_jsonl_sha256"].as_str().unwrap(),
    );
    for case in &cases {
        for operation in ["legacy", "agreement", "inventory"] {
            main.row(capture_case(case, operation, Control::default()));
        }
        if case.selected_path.is_some() || case.generic_fragment.is_some() {
            main.row(capture_case(case, "normalization", Control::default()));
        }
    }
    for owner in &owners {
        for profile in 0..5 {
            for mode in [
                "parents",
                "signatures",
                "compiled_signatures",
                "sources_owned",
                "sources_views",
                "runtime_owned",
                "runtime_materialization",
            ] {
                main.row(capture_owner(owner, profile, mode, Control::default()));
            }
        }
    }
    for owner in &owners {
        for quantum in [1, 3, 256] {
            main.row(capture_public_owner(owner, quantum, Control::default()));
        }
    }
    let literal = cases.iter().find(|case| case.id == "literal_556").unwrap();
    let full = capture_case(literal, "legacy", Control::default());
    assert_eq!(full["observation"]["admitted_storage"], 556);
    assert!(full["observation"]["output"].get("ok").is_some());
    let mut controls = RawWriter::new(
        None,
        9779,
        frozen["controls_jsonl_sha256"].as_str().unwrap(),
    );
    for (limit, retained) in [
        (0, 0),
        (67, 0),
        (68, 68),
        (395, 68),
        (396, 396),
        (555, 396),
        (556, 556),
    ] {
        let row = capture_case(
            literal,
            "legacy",
            Control {
                max_storage: Some(limit),
                ..Control::default()
            },
        );
        assert_eq!(row["observation"]["admitted_storage"], retained);
        assert_eq!(
            row["observation"]["output"].get("ok").is_some(),
            limit == 556
        );
        controls.row(row);
    }
    for (id, storage, work) in [("literal_259", 19279, 259), ("literal_390", 29275, 390)] {
        let case = cases.iter().find(|case| case.id == id).unwrap();
        let row = capture_case(case, "normalization", Control::default());
        assert_eq!(row["observation"]["admitted_storage"], storage);
        assert_eq!(row["observation"]["admitted_work"], work);
        controls.row(row);
    }
    for (id, operation) in [
        ("plain_attribute", "legacy"),
        ("child_hint", "legacy"),
        ("comments_whitespace", "legacy"),
        ("shadowed_good", "legacy"),
        ("ordered_exact", "agreement"),
        ("parameter_erasure", "agreement"),
        ("applied_generic", "agreement"),
        ("agreement_mismatch", "agreement"),
        ("owner_rcore_generic", "inventory"),
    ] {
        let case = cases.iter().find(|case| case.id == id).unwrap();
        let full = capture_case(case, operation, Control::default());
        let events = full["observation"]["events"]
            .as_array()
            .expect("Selected denial fixture must reach original algorithm");
        assert!(!events.is_empty());
        for deny in 1..=events.len() {
            let row = capture_case(
                case,
                operation,
                Control {
                    deny: Some(deny),
                    ..Control::default()
                },
            );
            assert!(
                row["observation"]["output"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("legacy baseline denied")
            );
            assert_eq!(row["observation"]["events"], json!(&events[..deny]));
            controls.row(row);
        }
        let work = full["observation"]["admitted_work"].as_u64().unwrap() as usize;
        let storage = full["observation"]["admitted_storage"].as_u64().unwrap() as usize;
        for control in [
            Control {
                max_work: Some(work),
                max_storage: Some(storage),
                deny: None,
            },
            Control {
                max_work: Some(work - 1),
                max_storage: Some(storage),
                deny: None,
            },
            Control {
                max_work: Some(work),
                max_storage: Some(storage - 1),
                deny: None,
            },
        ] {
            controls.row(capture_case(case, operation, control));
        }
    }
    for (id, mode) in [
        ("owner_ecore_qualified", "signatures"),
        ("owner_rcore_generic", "runtime_materialization"),
    ] {
        let case = owners.iter().find(|case| case.id == id).unwrap();
        let full = capture_owner(case, 0, mode, Control::default());
        let events = full["observation"]["events"].as_array().unwrap();
        for deny in 1..=events.len() {
            let row = capture_owner(
                case,
                0,
                mode,
                Control {
                    deny: Some(deny),
                    ..Control::default()
                },
            );
            assert!(
                row["observation"]["output"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("legacy baseline denied")
            );
            assert_eq!(row["observation"]["events"], json!(&events[..deny]));
            controls.row(row);
        }
    }
    main.finish();
    controls.finish();
}
fn supplement_case(case: &Case, operation: &str, control: Control) -> Value {
    let expected = source(&case.source_kind, &case.uri, &case.text).id;
    let mut row = capture_case(case, operation, control);
    if let Some(ok) = row.get("observation").and_then(|o| o["output"].get("ok")) {
        if operation == "legacy" {
            if let Some(refs) = ok.as_array() {
                for reference in refs {
                    assert_eq!(
                        reference["span"]["source"],
                        serde_json::to_value(&expected).unwrap()
                    );
                }
            }
        } else if operation == "agreement" {
            for signature in ok.as_array().unwrap() {
                assert_eq!(
                    signature["owner"]["source"],
                    serde_json::to_value(&expected).unwrap()
                );
            }
        }
    }
    row["input_source_id"] = serde_json::to_value(expected).unwrap();
    if let Some(dependency) = &case.dependency {
        row["input_dependency_source_id"] =
            serde_json::to_value(source("ecore", &dependency.uri, &dependency.text).id).unwrap();
    }
    row
}
fn supplement_parent(case: &Case, control: Control) -> Value {
    let bundle = bundle(
        &case.source_kind,
        &case.uri,
        &case.text,
        case.dependency.as_ref(),
    );
    let resolver = match Resolver::new(&bundle) {
        Ok(value) => value,
        Err(error) => return early(&case.id, "parent_duplicate", "resolver_setup", error),
    };
    let owner = resolver
        .resource_sets()
        .0
        .resolve(&bundle.sources[0].id, &case.owner)
        .unwrap();
    let span = resolver.node(&owner).unwrap().opening.clone(); // fixture setup only
    let receiver = Type::Classifier {
        declaration: owner,
        arguments: vec![],
        raw: false,
    };
    let observation = observe(control, |_, observation| {
        encoded(observation.finish(resolver.direct_parent_types_observed(
            &receiver,
            &span,
            observation,
        )))
    });
    json!({"id":case.id,"operation":"parent_duplicate","control":control,"observation":observation,"input_source_id":bundle.sources[0].id})
}
#[test]
fn retained_legacy_complete_original_unicode_88_and_3063_observations() {
    let frozen: Value =
        serde_json::from_str(include_str!("generic_legacy_supplement_digests.json")).unwrap();
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic_legacy_supplement_cases.json")).unwrap();
    let owners: Vec<OwnerCase> =
        serde_json::from_str(include_str!("generic_legacy_supplement_owners.json")).unwrap();
    let mut main = RawWriter::new(
        Some(frozen["complete_main_profiles"].as_array().unwrap().clone()),
        88,
        frozen["complete_main_jsonl_sha256"].as_str().unwrap(),
    );
    for case in &cases {
        for operation in ["legacy", "agreement", "inventory"] {
            main.row(supplement_case(case, operation, Control::default()));
        }
    }
    for id in [
        "duplicate_long_targets",
        "unicode_whitespace_many",
        "xml_entity_whitespace",
        "xml_literal_whitespace",
        "ordered_long_mismatch",
    ] {
        main.row(supplement_parent(
            cases.iter().find(|case| case.id == id).unwrap(),
            Control::default(),
        ));
    }
    for owner in &owners {
        for profile in 0..5 {
            for mode in [
                "parents",
                "signatures",
                "compiled_signatures",
                "sources_owned",
                "sources_views",
                "runtime_owned",
                "runtime_materialization",
            ] {
                main.row(capture_owner(owner, profile, mode, Control::default()));
            }
        }
        for quantum in [1, 3, 256] {
            main.row(capture_public_owner(owner, quantum, Control::default()));
        }
    }
    let mut controls = RawWriter::new(
        None,
        3063,
        frozen["complete_controls_jsonl_sha256"].as_str().unwrap(),
    );
    for case in &cases {
        for operation in ["legacy", "agreement"] {
            let full = supplement_case(case, operation, Control::default());
            let Some(events) = full
                .get("observation")
                .and_then(|row| row["events"].as_array())
            else {
                controls.row(json!({"id":case.id,"operation":operation,"original_rejection_before_requested_layer":full,"no_support_or_denial_award":true}));
                continue;
            };
            for deny in 1..=events.len() {
                let row = supplement_case(
                    case,
                    operation,
                    Control {
                        deny: Some(deny),
                        ..Control::default()
                    },
                );
                assert!(
                    row["observation"]["output"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("legacy baseline denied")
                );
                assert_eq!(row["observation"]["events"], json!(&events[..deny]));
                controls.row(row);
            }
            let work = full["observation"]["admitted_work"].as_u64().unwrap() as usize;
            let storage = full["observation"]["admitted_storage"].as_u64().unwrap() as usize;
            let mut budgets = vec![Control {
                max_work: Some(work),
                max_storage: Some(storage),
                deny: None,
            }];
            if work > 0 {
                budgets.push(Control {
                    max_work: Some(work - 1),
                    max_storage: Some(storage),
                    deny: None,
                });
            }
            if storage > 0 {
                budgets.push(Control {
                    max_work: Some(work),
                    max_storage: Some(storage - 1),
                    deny: None,
                });
            }
            for control in budgets {
                controls.row(supplement_case(case, operation, control));
            }
        }
    }
    let duplicate = cases
        .iter()
        .find(|case| case.id == "duplicate_long_targets")
        .unwrap();
    let full = supplement_parent(duplicate, Control::default());
    let events = full["observation"]["events"].as_array().unwrap();
    for deny in 1..=events.len() {
        let row = supplement_parent(
            duplicate,
            Control {
                deny: Some(deny),
                ..Control::default()
            },
        );
        assert!(
            row["observation"]["output"]["error"]
                .as_str()
                .unwrap()
                .contains("legacy baseline denied")
        );
        assert_eq!(row["observation"]["events"], json!(&events[..deny]));
        controls.row(row);
    }
    main.finish();
    controls.finish();
}

#[path = "generic_legacy_cursor_controls.rs"]
mod cursor_controls;

#[path = "generic_source_inventory_owner_compatibility_tests.rs"]
mod inventory_compatibility_tests;
