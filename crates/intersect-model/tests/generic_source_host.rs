//! Actual public A/B/C/D host controls shared with the zero-import WASM probe.
#[path = "support/generic_source_host_consumers.rs"]
mod decoder;
#[path = "support/source_inventory_host_consumers.rs"]
mod inventory;
#[path = "support/legacy_agreement_host_consumers.rs"]
mod legacy;

#[test]
fn actual_public_generic_source_decoding_host_consumers() -> Result<(), String> {
    assert_eq!(legacy::verify()?, 5);
    assert_eq!(inventory::verify()?, 5);
    assert_eq!(decoder::verify()?, 5);
    Ok(())
}
