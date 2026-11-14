//! Native-only fork retention experiment. Handwritten fixture, not a language benchmark.
//!
//! The allocator instrumentation below is the only unsafe code in this example.
//! It forwards all operations unchanged to System and counts requested live bytes;
//! allocator metadata, fragmentation, stacks and OS pages are measured separately.

use intersect_model::core::api::{BasicModel, ModelEdit, ModelFork, VersionedModel};
use intersect_model::core::frozen::{FrozenStore, Metamodel};
use intersect_model::core::storage::{self, IndexPolicy, ModelRead, ModelStore, StorageResult};
use intersect_model::definitions::schema::{Class, Feature, Schema};
use serde::Serialize;
use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn account_add(bytes: usize) {
    let current = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(current, Ordering::Relaxed);
}

// SAFETY: Every pointer and Layout is passed unchanged to System. Counters use
// allocation-free atomic operations, and failed allocations leave LIVE unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account_add(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            account_add(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The caller supplies a live System pointer and original layout.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller meets GlobalAlloc's pointer, layout and size contract.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            if new_size >= layout.size() {
                account_add(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Empty,
    Scalar,
    Text,
}

#[derive(Serialize)]
struct Config {
    forks: usize,
    elements: usize,
    baseline_payload_bytes: usize,
    edit_bytes: usize,
    mode: Mode,
    handshake: bool,
}

impl Config {
    fn parse() -> StorageResult<Self> {
        let mut config = Self {
            forks: 256,
            elements: 1,
            baseline_payload_bytes: 1024,
            edit_bytes: 128,
            mode: Mode::Scalar,
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
                "--mode" => {
                    config.mode = match value.as_str() {
                        "empty" => Mode::Empty,
                        "scalar" => Mode::Scalar,
                        "text" => Mode::Text,
                        _ => return Err("Mode must be empty, scalar, or text".into()),
                    };
                }
                "--forks" | "--elements" | "--baseline-payload-bytes" | "--edit-bytes" => {
                    let count = value.parse().map_err(|_| format!("Invalid count: {arg}"))?;
                    match arg.as_str() {
                        "--forks" => config.forks = count,
                        "--elements" => config.elements = count,
                        "--baseline-payload-bytes" => config.baseline_payload_bytes = count,
                        _ => config.edit_bytes = count,
                    }
                }
                _ => return Err(format!("Unknown argument: {arg}")),
            }
        }
        if config.forks == 0 || config.elements == 0 || config.forks > i32::MAX as usize {
            return Err("Forks and elements must be positive; forks must fit EInt".into());
        }
        if config.mode == Mode::Text && config.edit_bytes < 16 {
            return Err("Text edits need at least 16 bytes for the unique sibling marker".into());
        }
        Ok(config)
    }
}

fn schema() -> Schema {
    let features = [
        ("counter", "EInt", json!(0)),
        ("ballast", "EString", json!("")),
        ("text", "EString", json!("")),
    ]
    .into_iter()
    .map(|(name, target, default)| {
        (
            name.into(),
            Feature {
                datatype: None,
                name: name.into(),
                target: target.into(),
                reference: false,
                containment: false,
                opposite: None,
                lower: 1,
                upper: 1,
                unique: true,
                ordered: true,
                derived: false,
                unsettable: false,
                changeable: true,
                transient: false,
                volatile: false,
                default,
            },
        )
    })
    .collect();
    Schema {
        runtime_feature_domains: BTreeMap::new(),
        reference_top: None,
        raw_reference_bounds: BTreeMap::new(),
        runtime_warnings: Vec::new(),
        declared_operation_selections: None,
        instantiations: BTreeMap::new(),
        generic_signatures: None,
        conversion_contracts: Vec::new(),
        applied_conversion_contracts: Vec::new(),
        constructor_contracts: Vec::new(),
        enumerations: BTreeMap::new(),
        datatypes: BTreeMap::new(),
        declarations: BTreeMap::new(),
        revision: "synthetic-fork-memory-v1".into(),
        uri: "urn:intersect:benchmark:forks".into(),
        name: "ForkMemory".into(),
        classes: BTreeMap::from([(
            "Item".into(),
            Class {
                interface_type: false,
                parents: Vec::new(),
                feature_order: Vec::new(),
                feature_declarations: BTreeMap::new(),
                operations: Vec::new(),
                name: "Item".into(),
                abstract_type: false,
                features,
            },
        )]),
    }
}

const TARGET: &str = "item-00000000";

fn model(config: &Config) -> StorageResult<VersionedModel> {
    let schema = schema();
    let policy = IndexPolicy::selected(["counter"]);
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(|error| error.to_string())?);
    let mut store = FrozenStore::with_index_policy(mm, policy);
    // One bulk storage transaction avoids quadratic per-create validation. The
    // checked facade validates the completed fixture when adopting the store.
    storage::atomic(&mut store, |store| {
        store.configure_schema(&schema)?;
        for index in 0..config.elements {
            store.create_element(
                &schema,
                &format!("item-{index:08}"),
                "Item",
                "memory:/forks",
                0,
                0,
            )?;
        }
        store.set_value(
            TARGET,
            "ballast",
            Some(Value::String("b".repeat(config.baseline_payload_bytes))),
        )?;
        Ok(())
    })?;
    VersionedModel::new(BasicModel::from_store(schema, store)?)
}

fn text_value(index: usize, bytes: usize) -> Value {
    let mut text = format!("{index:016x}");
    text.extend(std::iter::repeat_n('x', bytes - text.len()));
    Value::String(text)
}

fn verify_baseline(model: &impl ModelRead, config: &Config) -> StorageResult<()> {
    if model.value(TARGET, "counter")? != Some(json!(0))
        || model.value(TARGET, "text")? != Some(json!(""))
    {
        return Err("Baseline changed".into());
    }
    let mut checked = false;
    model.inspect(TARGET, &mut |item| {
        let ballast = item
            .value("ballast")
            .and_then(|value| value.as_str())
            .ok_or("Missing ballast")?;
        if ballast.len() != config.baseline_payload_bytes
            || !ballast.bytes().all(|byte| byte == b'b')
        {
            return Err("Baseline payload changed".into());
        }
        checked = true;
        Ok(())
    })?;
    if !checked {
        return Err("Baseline target missing".into());
    }
    Ok(())
}

fn verify_forks(forks: &[ModelFork], config: &Config, edited: bool) -> StorageResult<()> {
    if forks.len() != config.forks {
        return Err("Retained fork count does not match requested count".into());
    }
    for index in [0, forks.len() / 2, forks.len() - 1] {
        let fork = &forks[index];
        let (field, expected) = if edited {
            match config.mode {
                Mode::Scalar => ("counter", json!(index + 1)),
                Mode::Text => ("text", text_value(index, config.edit_bytes)),
                Mode::Empty => ("counter", json!(0)),
            }
        } else {
            ("counter", json!(0))
        };
        if fork.value(TARGET, field)? != Some(expected) {
            return Err(format!("Unexpected sibling value at fork {index}"));
        }
        // Borrow the unchanged field rather than requesting an owned 16 MiB copy.
        let found = fork.inspect(TARGET, &mut |item| {
            if item
                .value("ballast")
                .and_then(|value| value.as_str())
                .map(str::len)
                != Some(config.baseline_payload_bytes)
            {
                return Err("Sibling lost its unchanged payload".into());
            }
            Ok(())
        })?;
        if !found {
            return Err("Sibling target missing".into());
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct Observation<'a> {
    event: &'static str,
    phase: &'static str,
    config: &'a Config,
    actual_forks: usize,
    edited_forks: usize,
    fork_size_bytes: usize,
    fork_vec_capacity: usize,
    fork_vec_requested_bytes: usize,
    elapsed_ms: f64,
    allocator_live_bytes: usize,
    allocator_phase_peak_bytes: usize,
    allocator_live_delta_from_baseline_bytes: i128,
    allocator_live_delta_from_empty_bytes: i128,
}

fn start_phase() -> Instant {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    Instant::now()
}

#[derive(Default)]
struct References {
    baseline: usize,
    empty: usize,
}

fn observe(
    phase: &'static str,
    config: &Config,
    forks: &[ModelFork],
    capacity: usize,
    edited: usize,
    elapsed: Duration,
    references: &References,
) -> Result<usize, Box<dyn std::error::Error>> {
    // Read counters before JSON serialization allocates any transient storage.
    let live = LIVE.load(Ordering::Relaxed);
    let observation = Observation {
        event: "phase",
        phase,
        config,
        actual_forks: forks.len(),
        edited_forks: edited,
        fork_size_bytes: size_of::<ModelFork>(),
        fork_vec_capacity: capacity,
        fork_vec_requested_bytes: capacity * size_of::<ModelFork>(),
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        allocator_live_bytes: live,
        allocator_phase_peak_bytes: PEAK.load(Ordering::Relaxed),
        allocator_live_delta_from_baseline_bytes: live as i128 - references.baseline as i128,
        allocator_live_delta_from_empty_bytes: live as i128 - references.empty as i128,
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &observation)?;
    writeln!(stdout)?;
    stdout.flush()?;
    drop(stdout);
    if config.handshake {
        let mut response = String::new();
        if io::stdin().read_line(&mut response)? == 0 {
            return Err("Runner closed the phase handshake".into());
        }
    }
    Ok(live)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse()?;
    // Initialize stdout/stdin buffers before baseline allocation accounting.
    println!("{{\"event\":\"start\"}}");
    io::stdout().flush()?;
    if config.handshake {
        let mut response = String::new();
        if io::stdin().read_line(&mut response)? == 0 {
            return Err("Runner closed the startup handshake".into());
        }
    }
    let mut references = References::default();
    let setup = start_phase();
    let versions = model(&config)?;
    let baseline = versions.snapshot()?;
    verify_baseline(&baseline, &config)?;
    references.baseline = LIVE.load(Ordering::Relaxed);
    references.empty = references.baseline;
    observe("baseline", &config, &[], 0, 0, setup.elapsed(), &references)?;

    let creation = start_phase();
    let mut forks = Vec::with_capacity(config.forks);
    for _ in 0..config.forks {
        forks.push(versions.fork(&baseline)?);
    }
    let creation_elapsed = creation.elapsed();
    verify_forks(&forks, &config, false)?;
    references.empty = LIVE.load(Ordering::Relaxed);
    observe(
        "empty",
        &config,
        &forks,
        forks.capacity(),
        0,
        creation_elapsed,
        &references,
    )?;

    if config.mode != Mode::Empty {
        let editing = start_phase();
        for (index, fork) in forks.iter_mut().enumerate() {
            match config.mode {
                Mode::Scalar => fork.set(TARGET, "counter", json!(index + 1))?,
                Mode::Text => fork.set(TARGET, "text", text_value(index, config.edit_bytes))?,
                Mode::Empty => unreachable!("empty mode skips editing"),
            }
        }
        let editing_elapsed = editing.elapsed();
        verify_baseline(&baseline, &config)?;
        verify_forks(&forks, &config, true)?;
        observe(
            "edited",
            &config,
            &forks,
            forks.capacity(),
            forks.len(),
            editing_elapsed,
            &references,
        )?;
    }

    let dropping = start_phase();
    drop(forks);
    observe(
        "forks_dropped",
        &config,
        &[],
        0,
        0,
        dropping.elapsed(),
        &references,
    )?;
    verify_baseline(&baseline, &config)?;
    let dropping = start_phase();
    drop(baseline);
    drop(versions);
    observe(
        "all_dropped",
        &config,
        &[],
        0,
        0,
        dropping.elapsed(),
        &references,
    )?;
    println!("{{\"event\":\"complete\",\"status\":\"ok\"}}");
    Ok(())
}
