#[path = "support/source_inventory_host_consumers.rs"]
mod inventory;
#[path = "support/legacy_agreement_host_consumers.rs"]
mod legacy;

#[test]
fn actual_public_source_inventory_host_consumers() -> Result<(), String> {
    assert_eq!(legacy::verify()?, 5);
    assert_eq!(inventory::verify()?, 5);
    Ok(())
}
