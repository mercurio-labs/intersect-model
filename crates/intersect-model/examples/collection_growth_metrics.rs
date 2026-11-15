//! Host-only diagnostic for the exact ModelStore append used by text construction.
//! Allocation instrumentation changes timing; this is not a Java parity benchmark.
use intersect_model::core::frozen::{FrozenStore, Metamodel};
use intersect_model::core::storage::{self, ModelRead, ModelStore};
use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);

fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
// SAFETY: All pointers and layouts are forwarded unchanged to System. Atomics
// do not allocate. Failed allocation/reallocation leaves live counts unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies a valid layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            add(layout.size());
            REQUESTED.fetch_add(layout.size(), Ordering::Relaxed);
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies a valid layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            add(layout.size());
            REQUESTED.fetch_add(layout.size(), Ordering::Relaxed);
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: Caller supplies a live System allocation and its layout.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: Caller satisfies GlobalAlloc's pointer/layout/size contract.
        let next = unsafe { System.realloc(pointer, layout, new_size) };
        if !next.is_null() {
            if new_size >= layout.size() {
                add(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
            REQUESTED.fetch_add(new_size, Ordering::Relaxed);
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn run(n: usize, mode: &str) -> Result<Value, String> {
    let package = intersect_model::compile_rcore(
        "rcore 1; package growth version \"1\"; metamodel \"urn:collection-growth\" prefix g { class Bucket { attr labels: String [0,*]; } }",
    ).map_err(|e|e.to_string())?;
    let schema = package.schema();
    let class = schema
        .declarations
        .iter()
        .find(|(key, declaration)| {
            declaration.name == "Bucket" && schema.classes.contains_key(*key)
        })
        .map(|(k, _)| k.as_str())
        .ok_or("Missing Bucket class")?;
    let metamodel = Arc::new(Metamodel::freeze(schema).map_err(|e| e.to_string())?);
    let mut store = FrozenStore::new(metamodel);
    storage::atomic(&mut store, |store| {
        intersect_model::core::resources::create_resource(store, "memory:growth")?;
        store.create_element(schema, "bucket", class, "memory:growth", 0, 0)
    })?;
    let inputs: Vec<String> = (0..n).map(|i| format!("v{i:08}")).collect();
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    REQUESTED.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    let clock = Instant::now();
    storage::atomic(&mut store, |store| {
        match mode {
            "append" => {
                for value in &inputs {
                    store.append_value("bucket", "labels", Value::String(value.clone()))?;
                }
            }
            "bulk" => store.set_value(
                "bucket",
                "labels",
                Some(Value::Array(
                    inputs.iter().cloned().map(Value::String).collect(),
                )),
            )?,
            _ => return Err("Mode must be append or bulk".into()),
        }
        Ok(())
    })?;
    let elapsed_ms = clock.elapsed().as_secs_f64() * 1e3;
    let live = LIVE.load(Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed);
    let requested = REQUESTED.load(Ordering::Relaxed);
    let calls = CALLS.load(Ordering::Relaxed);
    let value = store.value("bucket", "labels")?.ok_or("Missing list")?;
    let values = value.as_array().ok_or("Expected list")?;
    if values.len() != n
        || values
            .iter()
            .zip(&inputs)
            .any(|(v, s)| v.as_str() != Some(s))
    {
        return Err("List contents/order differ".into());
    }
    storage::validate(&store, schema)?;
    let checkpoint = store.begin()?;
    store.append_value("bucket", "labels", json!("rollback-sentinel"))?;
    store.rollback(&checkpoint)?;
    if store.value("bucket", "labels")?.as_ref() != Some(&value) {
        return Err("Rollback changed the original list".into());
    }
    drop(value);
    drop(store);
    let after_drop = LIVE.load(Ordering::Relaxed);
    Ok(json!({
        "schema":"intersect.collection-growth.v1", "mode":mode, "items":n,
        "instrumented_ms":elapsed_ms, "operation_retained_requested_bytes":live.saturating_sub(before),
        "operation_peak_requested_bytes":peak.saturating_sub(before),
        "operation_cumulative_requested_bytes":requested,"allocation_calls":calls,
        "process_live_requested_bytes_after_store_drop":after_drop,
        "verified_values":n,"rollback_verified":true,
        "scope":"One scalar collection in one object; raw shared store operations, not a checked facade or language performance claim. Requested allocator bytes exclude allocator metadata, fragmentation and OS pages."
    }))
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    let n = args
        .get(1)
        .ok_or("Usage: collection_growth_metrics N append|bulk")?
        .parse()
        .map_err(|e| format!("Invalid count: {e}"))?;
    let mode = args.get(2).ok_or("Missing mode")?;
    println!("{}", run(n, mode)?);
    Ok(())
}
