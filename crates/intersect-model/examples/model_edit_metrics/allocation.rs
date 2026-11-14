//! Optional host-only requested-allocation accounting for the measurement example.
//!
//! The uninstrumented build uses Rust's default allocator directly. Instrumented
//! builds forward unchanged to `System` and count successful allocation requests.
//! `allocation_calls` counts alloc, alloc_zeroed and realloc; `requested_bytes`
//! sums each full requested size (including realloc's new size, not just growth).
//! Those cumulative counters never decrease. Live/peak requested bytes exclude
//! allocator metadata, fragmentation, and transient internal realloc copies.

use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
pub struct Snapshot {
    pub live_bytes: usize,
    pub peak_bytes: usize,
    pub allocation_calls: usize,
    pub requested_bytes: usize,
}

pub fn snapshot() -> Option<Snapshot> {
    #[cfg(feature = "model-edit-allocation-metrics")]
    {
        Some(instrumented::snapshot())
    }
    #[cfg(not(feature = "model-edit-allocation-metrics"))]
    {
        None
    }
}

pub fn reset_peak() {
    #[cfg(feature = "model-edit-allocation-metrics")]
    instrumented::reset_peak();
}

#[cfg(feature = "model-edit-allocation-metrics")]
mod instrumented {
    use super::Snapshot;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingAllocator;

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static REQUESTED: AtomicUsize = AtomicUsize::new(0);

    fn account_request(bytes: usize) {
        CALLS.fetch_add(1, Ordering::Relaxed);
        REQUESTED.fetch_add(bytes, Ordering::Relaxed);
    }

    fn account_growth(bytes: usize) {
        let current = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK.fetch_max(current, Ordering::Relaxed);
    }

    // SAFETY: All pointer, layout and size arguments pass unchanged to System.
    // Counters use allocation-free atomics, and failed requests change no counts.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                account_request(layout.size());
                account_growth(layout.size());
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            // SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                account_request(layout.size());
                account_growth(layout.size());
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: The caller supplies a live System pointer and its layout.
            unsafe { System.dealloc(pointer, layout) };
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: The caller meets GlobalAlloc's pointer/layout/size contract.
            let replacement = unsafe { System.realloc(pointer, layout, new_size) };
            if !replacement.is_null() {
                account_request(new_size);
                if new_size >= layout.size() {
                    account_growth(new_size - layout.size());
                } else {
                    LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
                }
            }
            replacement
        }
    }

    // The harness snapshots at single-threaded phase boundaries. Individual
    // atomic loads do not promise a coherent snapshot during concurrent edits.
    pub(super) fn snapshot() -> Snapshot {
        Snapshot {
            live_bytes: LIVE.load(Ordering::Relaxed),
            peak_bytes: PEAK.load(Ordering::Relaxed),
            allocation_calls: CALLS.load(Ordering::Relaxed),
            requested_bytes: REQUESTED.load(Ordering::Relaxed),
        }
    }

    pub(super) fn reset_peak() {
        PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    }
}
