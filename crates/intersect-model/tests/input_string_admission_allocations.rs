//! Native allocation request regression for the shared public session decoder.
#![cfg(not(target_arch = "wasm32"))]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Requests {
    calls: usize,
    bytes: usize,
    largest: usize,
}
thread_local! {
    static REQUESTS: Cell<Option<Requests>> = const { Cell::new(None) };
}
struct ObservedSystem;
fn record(bytes: usize) {
    let _ = REQUESTS.try_with(|slot| {
        if let Some(mut requests) = slot.get() {
            requests.calls += 1;
            requests.bytes += bytes;
            requests.largest = requests.largest.max(bytes);
            slot.set(Some(requests));
        }
    });
}
unsafe impl GlobalAlloc for ObservedSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            record(size);
        }
        pointer
    }
}
#[global_allocator]
static ALLOCATOR: ObservedSystem = ObservedSystem;

struct Scope;
impl Drop for Scope {
    fn drop(&mut self) {
        REQUESTS.with(|slot| slot.set(None));
    }
}
fn measured<T>(operation: impl FnOnce() -> T) -> (T, Requests) {
    REQUESTS.with(|slot| {
        assert!(slot.get().is_none());
        slot.set(Some(Requests::default()));
    });
    let scope = Scope;
    let result = std::hint::black_box(operation());
    let requests = REQUESTS.with(|slot| slot.take()).unwrap_or_default();
    drop(scope);
    (result, requests)
}

use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::{DataWire, EntrySelection, Limits, ValueWire},
        modules::SymbolId,
        types::Scalar,
    },
};
use intersect_model::rcore_behavior_functions::{
    EvaluationSession, ExecutionContext, ExecutionErrorKind, ExecutionLimits, SessionProgress,
};
use std::num::NonZeroUsize;

#[test]
fn string_decode_refusal_precedes_owned_payload_allocation() {
    let definition = DefinitionSource::new(
        "memory:input-string-admission-allocation.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package leafTransport version \"1\";fn echo(value:String)->String=value;",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![definition.clone()],
        namespaces: vec![],
    };
    let image = compile_function(
        &bundle,
        &definition.id,
        &EntrySelection {
            symbol: SymbolId {
                source: definition.id.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap();
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let args = vec![ValueWire {
        ty,
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::String),
            payload: serde_json::Value::String("x".repeat(240_000)),
        }),
    }];
    // These are the independently frozen exact one-short boundaries: scalar
    // compact wire bytes240039 and576 existing continuation bytes.
    for owned in [false, true] {
        for per_value in [false, true] {
            let mut limits = ExecutionLimits::default();
            if per_value {
                limits.scalar.values.max_value_bytes = 240_038;
            } else {
                limits.scalar.max_value_bytes = 240_614;
            }
            let context = ExecutionContext::new(&bundle);
            // Fixture clones and source compilation occur outside measurement.
            let mut session = if owned {
                EvaluationSession::new_owned(
                    image.clone(),
                    args.clone(),
                    bundle.clone(),
                    vec![],
                    None,
                    limits.clone(),
                )
                .unwrap()
            } else {
                EvaluationSession::new(&image, &args, &context, &limits).unwrap()
            };
            let mut largest = 0;
            let error = loop {
                let (result, requests) = measured(|| session.resume(NonZeroUsize::MIN));
                largest = largest.max(requests.largest);
                match result {
                    SessionProgress::Pending => {}
                    SessionProgress::Complete(result) => break result.unwrap_err(),
                    SessionProgress::Finished => panic!("Missing terminal refusal"),
                }
            };
            assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
            assert_eq!(
                &*error.message,
                if per_value {
                    "Runtime value depth or byte limit exceeded"
                } else {
                    "Cumulative function value byte budget exceeded"
                }
            );
            assert_eq!(error.consumed.value_nodes, 0);
            assert_eq!(error.consumed.value_bytes, 576);
            assert!(
                largest < 240_000,
                "String payload was allocated before failed arena admission: largest={largest}, owned={owned}, per_value={per_value}"
            );
            assert!(matches!(
                session.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
    }
}
