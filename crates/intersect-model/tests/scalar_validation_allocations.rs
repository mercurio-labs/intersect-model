//! Native host regressions for borrowed validation and temporary-free value sizing.
#![cfg(not(target_arch = "wasm32"))]

use intersect_model::definitions::scalars::Builtin;
use serde_json::{Value, json};
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

#[test]
fn estring_validation_borrows_while_formatting_preserves_owned_text() {
    let texts = [
        String::new(),
        "a\"\\\n\0\u{00e9}\u{1f600}".to_owned(),
        "x".repeat(16),
        "x".repeat(4096),
        "x".repeat(131_072),
        "x".repeat(240_000),
        "a\"\\\n\0\u{00e9}\u{1f600}".repeat(24_000),
    ];
    for text in texts {
        let value = Value::String(text);
        let (validated, validation) = measured(|| Builtin::EString.validate(&value));
        assert_eq!(validated, Ok(()));
        assert_eq!(
            validation,
            Requests::default(),
            "validating a String copied it"
        );
        let (formatted, formatting) = measured(|| Builtin::EString.format(&value));
        let formatted = formatted.unwrap().unwrap();
        let input = value.as_str().unwrap();
        assert_eq!(formatted, input);
        if !input.is_empty() {
            assert_ne!(formatted.as_ptr(), input.as_ptr());
            assert!(formatting.calls > 0);
            assert!(formatting.bytes >= input.len());
        }
    }
    let (validated, validation) = measured(|| Builtin::EString.validate(&Value::Null));
    let (formatted, formatting) = measured(|| Builtin::EString.format(&Value::Null));
    assert_eq!(validated, Ok(()));
    assert_eq!(formatted, Ok(None));
    assert_eq!(validation, Requests::default());
    assert_eq!(formatting, Requests::default());
}

#[test]
fn estring_malformed_kinds_preserve_format_error_and_allocation_path() {
    for value in [
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!(u64::MAX),
        json!(1.25),
        json!([]),
        json!(["a"]),
        json!({}),
        json!({"kind":"int64","value":"1"}),
    ] {
        let (validated, validation) = measured(|| Builtin::EString.validate(&value));
        let (formatted, formatting) = measured(|| Builtin::EString.format(&value));
        assert_eq!(validated, Err("Invalid EString wire value".to_owned()));
        assert_eq!(formatted, Err("Invalid EString wire value".to_owned()));
        assert_eq!(validation, formatting);
    }
}

#[test]
fn runtime_string_sizing_does_not_allocate_an_encoded_payload_buffer() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore_behavior::{
            self,
            function_lowering::compile_function,
            ir,
            ir_v2::{DataWire, EntrySelection, Limits, ValueWire},
            modules::SymbolId,
            types::Scalar,
        },
    };
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionContext, ExecutionLimits, SessionProgress,
    };
    use std::num::NonZeroUsize;

    let definition = DefinitionSource::new(
        "memory:runtime-string-allocation-regression.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package payload version \"1\"; fn echo(value:String)->String=value;",
    );
    let document = rcore_behavior::parse(definition.clone());
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    let declaration = document
        .declarations
        .iter()
        .position(|declaration| {
            matches!(
                declaration.value,
                rcore_behavior::ast::Declaration::Function { .. }
            )
        })
        .unwrap();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![definition.clone()],
        namespaces: vec![],
    };
    let selection = EntrySelection {
        symbol: SymbolId {
            source: definition.id.clone(),
            declaration,
        },
        types: vec![],
        arguments: vec![],
    };
    let image = compile_function(&bundle, &definition.id, &selection, &Limits::default()).unwrap();
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    for text in [
        "x".repeat(240_000),
        "a\"\\\n\0\u{00e9}\u{1f600}".repeat(12_000),
    ] {
        let arguments = [ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::String),
                payload: Value::String(text.clone()),
            }),
        }];
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        let mut complete = false;
        // Retained input and output copies each advance at most 256 UTF-8 bytes.
        // A full chunk consumes at least 253 bytes after a possible three-byte
        // UTF-8 boundary adjustment. Keep the original 1000-unit preparation
        // allowance, then budget both copies without changing runtime limits.
        let poll_bound = 1_000 + 2 * text.len().div_ceil(253);
        for _ in 0..poll_bound {
            let (progress, requests) = measured(|| session.resume(NonZeroUsize::MIN));
            // Owning the input/output leaf is still permitted. A second encoded
            // buffer grows beyond the input length and fails this bound. Setup
            // and assertions are deliberately outside allocator observation.
            assert!(
                requests.largest <= text.len(),
                "runtime sizing allocated a payload-sized serialization buffer: {requests:?}"
            );
            match progress {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => {
                    let result = result.unwrap();
                    let DataWire::Scalar(value) = result.value.data else {
                        panic!("expected the exact String result");
                    };
                    assert_eq!(value.ty, ir::ValueType::Scalar(Scalar::String));
                    assert_eq!(value.payload, Value::String(text.clone()));
                    complete = true;
                    break;
                }
                SessionProgress::Finished => panic!("runtime finished before returning its result"),
            }
        }
        assert!(complete, "runtime did not complete its bounded fixture");
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Finished
        ));
    }
}
