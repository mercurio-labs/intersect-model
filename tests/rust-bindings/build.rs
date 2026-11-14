//! Handwritten build host. Every build compiles freshly generated Rust, including WASM.
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let output = PathBuf::from(env::var("OUT_DIR")?);
    for (name, source) in [
        (
            "library",
            root.join("../../fixtures/library/library-bindings.rcore"),
        ),
        ("shapes", root.join("fixtures/shapes.rcore")),
        ("state", root.join("fixtures/state.rcore")),
    ] {
        println!("cargo:rerun-if-changed={}", source.display());
        let source = fs::read_to_string(source)?;
        fs::write(
            output.join(format!("{name}.rs")),
            intersect_model::codegen::generate_rcore(&source)?,
        )?;
    }
    let mut builder = intersect_model::definitions::dynamic::SchemaBuilder::new(
        "programmatic",
        "urn:test:programmatic",
    );
    let record = builder.class("record")?;
    record
        .attribute(
            "message",
            intersect_model::definitions::dynamic::ScalarType::String,
        )
        .default("quote\"\n\\λ\0\u{2028}")?;
    record
        .attribute(
            "async",
            intersect_model::definitions::dynamic::ScalarType::Boolean,
        )
        .optional()?;
    fs::write(
        output.join("programmatic.rs"),
        intersect_model::codegen::generate(builder.build()?.as_schema())?,
    )?;
    Ok(())
}
