//! Native physical allocation regression; does not qualify host/WASM/cleanup.
#![cfg(not(target_arch = "wasm32"))]
use intersect_model::definitions::scalars::Builtin;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
struct Requests;
static ENABLED: AtomicBool = AtomicBool::new(false);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
fn record(size: usize) {
    if ENABLED.load(Relaxed) {
        LARGEST.fetch_max(size, Relaxed);
    }
}
unsafe impl GlobalAlloc for Requests {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            record(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            record(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, l, n) };
        if !p.is_null() {
            record(n);
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Requests = Requests;
fn measured<T>(f: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.store(0, Relaxed);
    ENABLED.store(true, Relaxed);
    let value = f();
    ENABLED.store(false, Relaxed);
    (value, LARGEST.load(Relaxed))
}
#[test]
fn byte_validation_borrows_large_payload_while_formatting_keeps_owned_output() {
    let source = "AB".repeat(120_000);
    let wire = serde_json::json!({"kind":"bytes","hex":source});
    let text = wire["hex"].as_str().unwrap();
    let (valid, largest) = measured(|| Builtin::EByteArray.validate(&wire));
    assert_eq!(valid, Ok(()));
    assert!(
        largest < text.len(),
        "validation made a payload-sized request: {largest}"
    );
    let (formatted, largest) = measured(|| Builtin::EByteArray.format(&wire));
    assert_eq!(formatted.as_ref().unwrap().as_deref(), Some(text));
    assert!(
        largest >= text.len(),
        "format retains its owned lexical output"
    );
    let invalid = serde_json::json!({"kind":"bytes","hex":format!("{}AG", "AB".repeat(119_999))});
    let (result, largest) = measured(|| Builtin::EByteArray.validate(&invalid));
    assert_eq!(
        result,
        Err("Expected canonical uppercase even-length byte hex".into())
    );
    assert!(
        largest < invalid["hex"].as_str().unwrap().len(),
        "invalid validation cloned the payload: {largest}"
    );
}
