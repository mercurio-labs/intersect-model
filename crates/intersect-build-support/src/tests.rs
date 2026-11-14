use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "intersect-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).map_err(|error| error.to_string())?;
        Ok(Self(path))
    }
    fn write(&self, name: &str, value: &str) -> Result<(), String> {
        let path = self.0.join(name);
        let parent = path.parent().ok_or("Fixture has no parent")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        std::fs::write(path, value).map_err(|error| error.to_string())
    }
    fn identity(&self) -> Result<String, String> {
        let path = canonical(&self.0)?;
        let mut digest = Sha256::new();
        crate_inputs(&mut digest, &path, &path)?;
        Ok(format!("{:x}", digest.finalize()))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn standalone_identity_tracks_nested_embedded_bytes_without_workspace() -> Result<(), String> {
    let fixture = Fixture::new()?;
    fixture.write(
        "Cargo.toml",
        "[package]\nname='standalone'\nversion='1.0.0'\n",
    )?;
    fixture.write(
        "src/lib.rs",
        "#[path = \"../inputs/module.rs\"] mod input;\n",
    )?;
    fixture.write(
        "inputs/module.rs",
        "const DATA: &str = include_str!(\"data.txt\");\n",
    )?;
    fixture.write("inputs/data.txt", "original bytes\r\n")?;
    let original = fixture.identity()?;
    fixture.write("inputs/data.txt", "original bytes\n")?;
    assert_ne!(fixture.identity()?, original);
    fixture.write("inputs/data.txt", "original bytes\r\n")?;
    assert_eq!(fixture.identity()?, original);
    std::fs::remove_file(fixture.0.join("inputs/data.txt")).map_err(|error| error.to_string())?;
    assert!(
        fixture.identity().is_err(),
        "missing embedded input must fail closed"
    );
    Ok(())
}

#[test]
fn relocation_does_not_change_owned_identity_and_dynamic_inputs_are_refused() -> Result<(), String>
{
    let first = Fixture::new()?;
    let second = Fixture::new()?;
    for fixture in [&first, &second] {
        fixture.write(
            "Cargo.toml",
            "[package]\nname='portable'\nversion='1.0.0'\n",
        )?;
        fixture.write("src/lib.rs", "pub const VALUE: u32 = 7;\n")?;
    }
    assert_eq!(first.identity()?, second.identity()?);
    second.write(
        "src/lib.rs",
        "const DATA: &str = include_str!(concat!(\"a\", \"b\"));\n",
    )?;
    assert!(second.identity().is_err(), "dynamic input must be rejected");
    Ok(())
}
