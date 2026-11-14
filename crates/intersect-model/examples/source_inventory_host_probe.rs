//! Actual zero-import WASM probe over the same shared Rust public-host controls.
#[path = "../tests/support/source_inventory_host_consumers.rs"]
mod inventory;
#[path = "../tests/support/legacy_agreement_host_consumers.rs"]
mod legacy;
use std::cell::RefCell;
thread_local! { static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) }; }

fn verify() -> Result<u32, String> {
    assert_eq!(legacy::verify()?, 5);
    assert_eq!(inventory::verify()?, 5);
    Ok(10)
}
fn main() -> Result<(), String> {
    assert_eq!(verify()?, 10);
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_source_inventory_host_consumers() -> u32 {
    verify().unwrap_or(0)
}
#[unsafe(no_mangle)]
pub extern "C" fn capture_source_inventory_host_row(mode: u32, quantum: u32) -> usize {
    let selected = match mode {
        0 => Some(inventory::Mode::Compatibility),
        1 => Some(inventory::Mode::GenericAgreement),
        2 => Some(inventory::Mode::SourceInventoryAgreement),
        _ => None,
    };
    let result = selected
        .ok_or("Unsupported verification mode".to_owned())
        .and_then(|mode| inventory::capture_raw(mode, quantum as usize));
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
pub extern "C" fn source_inventory_host_output_buffer() -> *const u8 {
    OUTPUT.with(|output| output.borrow().as_ptr())
}
