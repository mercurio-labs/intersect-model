//! First raw A/B/C public-host observations. Never overwrites an existing file.
#[path = "../tests/support/source_inventory_host_consumers.rs"]
mod inventory;
#[path = "../tests/support/legacy_agreement_host_consumers.rs"]
mod legacy;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Provide a NEW raw output path")?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    for mode in [
        inventory::Mode::Compatibility,
        inventory::Mode::GenericAgreement,
        inventory::Mode::SourceInventoryAgreement,
    ] {
        for quantum in [1, 3, 256] {
            let row = inventory::capture_raw(mode, quantum)?;
            serde_json::to_writer(&mut output, &row)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    assert_eq!(legacy::verify()?, 5);
    assert_eq!(inventory::verify()?, 5);
    eprintln!("legacy_groups=5 inventory_groups=5 raw_rows=9");
    Ok(())
}
