//! Native host benchmark for checked model edits. No production instrumentation.
//!
//! Compile this same source against each revision being compared. Setup uses raw
//! bulk storage, then full validation; measured edits use the public checked API.
//! Per-unit clocks and latency recording have overhead, particularly on tiny
//! models. `transaction_boundary` includes begin, final validation and completion,
//! rather than claiming to measure commit alone. Handshake waits are not timed.

use intersect_model::core::api::{BasicModel, ModelEdit, TransactionalModel};
use intersect_model::core::frozen::FrozenStore;
use intersect_model::core::storage::{self, ModelRead, ModelStore, StorageResult};
use intersect_model::definitions::schema::Schema;

use serde_json::{Value, json};
use std::io::{self, Write};
use std::time::{Duration, Instant};

#[path = "model_edit_metrics/allocation.rs"]
mod allocation;

const RESOURCE: &str = "memory:/model-edit-metrics";
const CANCELLED: &str = "model-edit-metrics: intentional rollback";
const SCENARIOS: &[&str] = &[
    "basic-create",
    "transaction-create",
    "basic-scalar-edit",
    "transaction-scalar-edit",
    "set-many",
    "relationship-edit",
    "rollback",
    "create-rollback",
    "validate",
];

struct Config {
    scenario: String,
    size: usize,
    edits: usize,
    pattern: String,
    handshake: bool,
}

impl Config {
    fn parse() -> StorageResult<Self> {
        let mut config = Self {
            scenario: "basic-scalar-edit".into(),
            size: 1_000,
            edits: 100,
            pattern: "spread".into(),
            handshake: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--handshake" {
                config.handshake = true;
                continue;
            }
            let value = args.next().ok_or_else(|| format!("Missing value: {arg}"))?;
            match arg.as_str() {
                "--scenario" => config.scenario = value,
                "--pattern" => config.pattern = value,
                "--size" | "--edits" => {
                    let count = value.parse().map_err(|_| format!("Invalid count: {arg}"))?;
                    if arg == "--size" {
                        config.size = count;
                    } else {
                        config.edits = count;
                    }
                }
                _ => return Err(format!("Unknown argument: {arg}")),
            }
        }
        if !SCENARIOS.contains(&config.scenario.as_str()) {
            return Err(format!("Unknown scenario; choose {}", SCENARIOS.join(", ")));
        }
        if !matches!(config.pattern.as_str(), "spread" | "hotspot") {
            return Err("Pattern must be spread or hotspot".into());
        }
        if config.size == 0 || config.edits == 0 || config.edits >= i32::MAX as usize {
            return Err("Size and edits must be positive; edits must be below EInt maximum".into());
        }
        Ok(config)
    }

    fn creates(&self) -> bool {
        matches!(
            self.scenario.as_str(),
            "basic-create" | "transaction-create"
        )
    }

    fn units(&self) -> usize {
        if self.creates() {
            self.size
        } else {
            self.edits
        }
    }

    fn target(&self, index: usize) -> usize {
        if self.pattern == "hotspot" {
            0
        } else {
            index % self.size
        }
    }
}

fn copy_id(index: usize) -> String {
    format!("copy-{index:08}")
}

fn handshake(config: &Config, phase: &str) -> StorageResult<()> {
    if config.handshake {
        println!("{}", json!({"event": "ready", "phase": phase}));
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut acknowledgement = String::new();
        if io::stdin()
            .read_line(&mut acknowledgement)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err(format!("Handshake input closed at {phase}"));
        }
    }
    Ok(())
}

struct FixtureClasses {
    library: String,
    book_copy: String,
    loan: String,
}

fn class_key(schema: &Schema, name: &str) -> StorageResult<String> {
    schema
        .declarations
        .iter()
        .find(|(key, declaration)| declaration.name == name && schema.classes.contains_key(*key))
        .map(|(key, _)| key.clone())
        .ok_or_else(|| format!("Unknown fixture class {name}"))
}

fn fixture(config: &Config) -> StorageResult<(BasicModel, FixtureClasses)> {
    let schema = intersect_model::compile_rcore(include_str!(
        "../assets/fixtures/library/library-bindings.rcore"
    ))
    .map_err(|diagnostics| format!("Library fixture compilation failed: {diagnostics:?}"))?
    .schema()
    .clone();
    let classes = FixtureClasses {
        library: class_key(&schema, "Library")?,
        book_copy: class_key(&schema, "BookCopy")?,
        loan: class_key(&schema, "Loan")?,
    };
    let mut store = FrozenStore::for_schema(&schema).map_err(|error| error.to_string())?;
    storage::atomic(&mut store, |store| {
        store.configure_schema(&schema)?;
        for id in ["library-a", "library-b"] {
            store.create_element(&schema, id, &classes.library, RESOURCE, 0, 0)?;
            store.set_value(id, "name", Some(json!(id)))?;
        }
        if !config.creates() {
            for index in 0..config.size {
                let id = copy_id(index);
                store.create_element(&schema, &id, &classes.book_copy, RESOURCE, 0, 0)?;
                store.set_value(&id, "name", Some(json!(id)))?;
            }
            // Fixed relationship degree: background copies stay unrelated as N grows.
            store.set_value("library-a", "copies", Some(json!([copy_id(0)])))?;
            store.set_value(&copy_id(0), "library", Some(json!("library-a")))?;
        }
        Ok(())
    })?;
    let model = BasicModel::from_store(schema, store)?;
    model.validate()?;
    Ok((model, classes))
}

enum Action {
    Create(String, Value),
    Scalar(String, Value),
    Batch(String, Value, Value),
    Move(&'static str, &'static str, Value),
    Validate,
}

fn actions(config: &Config) -> Vec<Action> {
    (0..config.units())
        .map(|index| {
            let id = copy_id(config.target(index));
            match config.scenario.as_str() {
                "basic-create" | "transaction-create" => {
                    let id = copy_id(index);
                    Action::Create(id.clone(), json!(id))
                }
                "create-rollback" => {
                    let id = format!("temporary-{index:08}");
                    Action::Create(id.clone(), json!(id))
                }
                "set-many" => {
                    Action::Batch(id, json!(index + 2), json!(format!("title-{index:08}")))
                }
                "relationship-edit" => {
                    let (from, to) = if index % 2 == 0 {
                        ("library-a", "library-b")
                    } else {
                        ("library-b", "library-a")
                    };
                    Action::Move(from, to, json!([copy_id(0)]))
                }
                "validate" => Action::Validate,
                _ => Action::Scalar(id, json!(index + 2)),
            }
        })
        .collect()
}

fn run_actions(
    model: &mut impl ModelEdit,
    book_copy: &str,
    actions: &[Action],
    latencies: &mut Vec<u64>,
) -> StorageResult<()> {
    for action in actions {
        let started = Instant::now();
        match action {
            Action::Create(id, name) => {
                model.create(id, book_copy, RESOURCE)?;
                model.set(id, "name", name.clone())?;
            }
            Action::Scalar(id, edition) => model.set(id, "edition", edition.clone())?,
            Action::Batch(id, edition, title) => model.set_many(&[
                (id, "edition", edition.clone()),
                (id, "title", title.clone()),
            ])?,
            Action::Move(from, to, copies) => {
                model.set_many(&[(from, "copies", json!([])), (to, "copies", copies.clone())])?
            }
            Action::Validate => storage::validate(model, model.schema())?,
        }
        latencies.push(ns(started.elapsed()));
    }
    Ok(())
}

fn run(
    config: &Config,
    mut model: BasicModel,
    book_copy: &str,
    actions: &[Action],
    latencies: &mut Vec<u64>,
) -> StorageResult<(BasicModel, Duration, Duration)> {
    if matches!(
        config.scenario.as_str(),
        "transaction-create" | "transaction-scalar-edit" | "rollback" | "create-rollback"
    ) {
        let mut model = TransactionalModel::new(model);
        let mut body = Duration::ZERO;
        let started = Instant::now();
        let result = model.transaction(|edit| {
            let body_started = Instant::now();
            let result = run_actions(edit, book_copy, actions, latencies);
            body = body_started.elapsed();
            result?;
            if matches!(config.scenario.as_str(), "rollback" | "create-rollback") {
                Err(CANCELLED.into())
            } else {
                Ok(())
            }
        });
        let total = started.elapsed();
        if matches!(config.scenario.as_str(), "rollback" | "create-rollback") {
            if result.as_ref().map_err(String::as_str) != Err(CANCELLED) {
                return Err(format!("Unexpected rollback outcome: {result:?}"));
            }
        } else {
            result?;
        }
        Ok((model.into_basic(), body, total.saturating_sub(body)))
    } else {
        let started = Instant::now();
        run_actions(&mut model, book_copy, actions, latencies)?;
        Ok((model, started.elapsed(), Duration::ZERO))
    }
}

// Stable non-cryptographic equivalence checksum. This does not hash handles or
// allocation addresses; source identity, insertion order and all values count.
struct Fingerprint(u64);

impl Fingerprint {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

impl Write for Fingerprint {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn checksum(model: &impl ModelRead) -> StorageResult<(String, usize)> {
    let mut fingerprint = Fingerprint::new();
    serde_json::to_writer(&mut fingerprint, &model.revision()?).map_err(|e| e.to_string())?;
    let mut count = 0;
    storage::visit(model, |element| {
        serde_json::to_writer(&mut fingerprint, &element).map_err(|e| e.to_string())?;
        fingerprint.write_all(b"\n").map_err(|e| e.to_string())?;
        count += 1;
        Ok(())
    })?;
    Ok((fingerprint.hex(), count))
}

fn require_value(
    model: &impl ModelRead,
    id: &str,
    field: &str,
    expected: Value,
) -> StorageResult<()> {
    let actual = model.value(id, field)?;
    if actual != Some(expected.clone()) {
        return Err(format!(
            "Unexpected {id}.{field}: {actual:?}, expected {expected}"
        ));
    }
    Ok(())
}

fn verify_values(
    config: &Config,
    model: &BasicModel,
    classes: &FixtureClasses,
    initial_checksum: &str,
) -> StorageResult<()> {
    model.validate()?;
    if matches!(config.scenario.as_str(), "rollback" | "create-rollback")
        && checksum(model)?.0 != initial_checksum
    {
        return Err("Timed rollback changed the initial model".into());
    }
    if config.scenario == "create-rollback" {
        for index in 0..config.edits {
            if model.class(&format!("temporary-{index:08}"))?.is_some() {
                return Err("Rolled-back object remains accessible".into());
            }
        }
    }
    let mut editions = vec![1; config.size];
    let mut titles = vec![None; config.size];
    if matches!(
        config.scenario.as_str(),
        "basic-scalar-edit" | "transaction-scalar-edit" | "set-many"
    ) {
        for index in 0..config.edits {
            editions[config.target(index)] = index + 2;
            if config.scenario == "set-many" {
                titles[config.target(index)] = Some(format!("title-{index:08}"));
            }
        }
    }
    for index in 0..config.size {
        let id = copy_id(index);
        if model.class(&id)?.as_deref() != Some(classes.book_copy.as_str()) {
            return Err(format!("Missing copy: {id}"));
        }
        require_value(model, &id, "name", json!(id))?;
        require_value(model, &id, "edition", json!(editions[index]))?;
        if let Some(title) = &titles[index] {
            require_value(model, &id, "title", json!(title))?;
        }
    }
    if !config.creates() {
        let owner = if config.scenario == "relationship-edit" && config.edits % 2 == 1 {
            "library-b"
        } else {
            "library-a"
        };
        let other = if owner == "library-a" {
            "library-b"
        } else {
            "library-a"
        };
        require_value(model, &copy_id(0), "library", json!(owner))?;
        require_value(model, owner, "copies", json!([copy_id(0)]))?;
        require_value(model, other, "copies", json!([]))?;
    }
    if checksum(model)?.1 != config.size + 2 {
        return Err("Final object count mismatch".into());
    }
    Ok(())
}

fn verify_rejections(mut model: BasicModel, classes: &FixtureClasses) -> StorageResult<BasicModel> {
    let before = checksum(&model)?;
    let id = copy_id(0);
    if model.set(&id, "edition", json!("wrong type")).is_ok() || checksum(&model)? != before {
        return Err("Invalid scalar edit was accepted or changed model".into());
    }
    if model
        .set_many(&[
            ("library-a", "copies", json!([])),
            ("library-b", "copies", json!([id])),
            (&id, "edition", json!(false)),
        ])
        .is_ok()
        || checksum(&model)? != before
    {
        return Err("Invalid relationship batch was accepted or partially retained".into());
    }
    let mut managed = TransactionalModel::new(model);
    let outcome: StorageResult<()> = managed.transaction(|edit| {
        edit.set(&id, "edition", json!(123))?;
        edit.create("rollback-probe", &classes.book_copy, RESOURCE)?;
        edit.set("rollback-probe", "name", json!("probe"))?;
        Err(CANCELLED.into())
    });
    if outcome.as_ref().map_err(String::as_str) != Err(CANCELLED) || checksum(&managed)? != before {
        return Err("Explicit transaction rollback changed model".into());
    }
    if managed
        .transaction(|edit| edit.create("incomplete-probe", &classes.loan, RESOURCE))
        .is_ok()
        || checksum(&managed)? != before
    {
        return Err("Incomplete transaction was accepted or rollback changed model".into());
    }
    managed.validate()?;
    Ok(managed.into_basic())
}

fn ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn latency_summary(latencies: &mut [u64]) -> Value {
    latencies.sort_unstable();
    if latencies.len() < 20 {
        return json!({"samples":latencies.len(), "p50":null, "p95":null, "max":latencies.last()});
    }
    json!({
        "samples":latencies.len(),
        "p50":latencies[latencies.len().div_ceil(2) - 1],
        "p95":latencies[(latencies.len() * 95).div_ceil(100) - 1],
        "max":latencies.last(),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse()?;
    let allocation_startup = allocation::snapshot();
    handshake(&config, "startup")?;
    let setup_started = Instant::now();
    let (model, classes) = fixture(&config)?;
    let schema_revision = model.schema().revision.clone();
    let mut digest = Fingerprint::new();
    serde_json::to_writer(
        &mut digest,
        &[include_str!(
            "../assets/fixtures/library/library-bindings.rcore"
        )],
    )?;
    let initial_checksum = checksum(&model)?.0;
    let actions = actions(&config);
    let mut latencies = Vec::with_capacity(config.units());
    let setup = setup_started.elapsed();
    handshake(&config, "setup")?;
    allocation::reset_peak();
    let allocation_setup = allocation::snapshot();
    let (model, operation, boundary) =
        run(&config, model, &classes.book_copy, &actions, &mut latencies)?;
    let allocation_workload = allocation::snapshot();
    handshake(&config, "workload")?;
    let verification_started = Instant::now();
    verify_values(&config, &model, &classes, &initial_checksum)?;
    let model = verify_rejections(model, &classes)?;
    let (checksum, object_count) = checksum(&model)?;
    let verification = verification_started.elapsed();
    let allocation_verified = allocation::snapshot();
    handshake(&config, "verified")?;
    let latency_summary = latency_summary(&mut latencies);
    drop(model);
    drop(actions);
    drop(latencies);
    let allocation_teardown = allocation::snapshot();
    handshake(&config, "teardown")?;
    println!(
        "{}",
        json!({
            "format_version":1,
            "workload_id":"model-edit-v1",
            "scenario":config.scenario,
            "size":config.size,
        "initial_objects":if config.creates() { 2 } else { config.size + 2 },
        "initial_containment_edges":if config.creates() { 0 } else { 1 },
            "edits":config.edits,
            "pattern":config.pattern,
            "workload_units":config.units(),
            "edit_operations":if config.creates() { config.size * 2 } else if config.scenario == "create-rollback" { config.edits * 2 } else if config.scenario == "validate" { 0 } else { config.edits },
            "explicit_assignments":if config.creates() { config.size } else if matches!(config.scenario.as_str(), "set-many" | "relationship-edit") { config.edits * 2 } else if config.scenario == "validate" { 0 } else { config.edits },
            "full_validation_calls":if config.scenario == "validate" { config.edits } else { 0 },
            "schema_revision":schema_revision,
            "schema_digest":digest.hex(),
            "schema_digest_algorithm":"fnv1a64-json-fixture-sources",
            "correct":true,
            "checksum":checksum,
            "checksum_algorithm":"fnv1a64-json-revision-and-elements-in-insertion-order",
            "final_objects":object_count,
            "phases_ns":{"setup":ns(setup),"operation":ns(operation),"transaction_boundary":ns(boundary),"verification":ns(verification)},
            "operation_latency_ns":latency_summary,
            "allocator_instrumented":cfg!(feature = "model-edit-allocation-metrics"),
            "allocation_peak_scope":"peak reset after setup; workload and later peaks are cumulative since reset",
        "allocation_teardown_scope":"model, action payloads and latency samples dropped; host config, result metadata and IO remain",
            "allocation_snapshots":{"startup":allocation_startup,"setup":allocation_setup,"workload":allocation_workload,"verified":allocation_verified,"teardown":allocation_teardown},
        })
    );
    Ok(())
}
