//! Manually authored manifest registration, not an Ecore/Xtext import demo.

use std::error::Error;

use intersect_model::core::PackageRegistry;
use intersect_model::definitions::{DefinitionManifest, PackageId};

fn main() -> Result<(), Box<dyn Error>> {
    let package = PackageId::new("example.library", "starter-1")?;
    let mut registry = PackageRegistry::default();
    registry.register(DefinitionManifest::new(package.clone()))?;

    println!(
        "Registered {package}; registry contains {} manifest.",
        registry.len()
    );
    println!(
        "This example registers metadata only. Run the browser workbench for the native compiler and model demo."
    );
    Ok(())
}
