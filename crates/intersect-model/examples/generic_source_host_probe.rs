//! Actual zero-import WASM probe over the same shared Rust public-host controls.
#[path = "../tests/support/generic_source_host_consumers.rs"]
mod decoder;
#[path = "../tests/support/source_inventory_host_consumers.rs"]
mod inventory;
#[path = "../tests/support/legacy_agreement_host_consumers.rs"]
mod legacy;
use std::cell::RefCell;
thread_local! { static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) }; }

fn verify() -> Result<u32, String> {
    assert_eq!(legacy::verify()?, 5);
    assert_eq!(inventory::verify()?, 5);
    assert_eq!(decoder::verify()?, 5);
    Ok(15)
}
fn main() -> Result<(), String> {
    assert_eq!(verify()?, 15);
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_generic_source_host_consumers() -> u32 {
    verify().unwrap_or(0)
}
#[unsafe(no_mangle)]
pub extern "C" fn capture_generic_source_host_row(mode: u32, quantum: u32) -> usize {
    let result = if mode == 3 {
        decoder::capture_raw(decoder::Mode::GenericSourceDecoding, quantum as usize)
    } else {
        let selected = match mode {
            0 => Some(inventory::Mode::Compatibility),
            1 => Some(inventory::Mode::GenericAgreement),
            2 => Some(inventory::Mode::SourceInventoryAgreement),
            _ => None,
        };
        selected
            .ok_or("Unsupported verification mode".to_owned())
            .and_then(|mode| inventory::capture_raw(mode, quantum as usize))
    };
    let value = match result {
        Ok(row) => serde_json::json!({"ok":true,"row":row}),
        Err(error) => serde_json::json!({"ok":false,"error":error}),
    };
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return 0;
    };
    let size = bytes.len();
    OUTPUT.with(|output| *output.borrow_mut() = bytes);
    size
}
#[unsafe(no_mangle)]
pub extern "C" fn generic_source_host_output_buffer() -> *const u8 {
    OUTPUT.with(|output| output.borrow().as_ptr())
}
