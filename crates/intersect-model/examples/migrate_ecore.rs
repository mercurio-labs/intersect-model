//! Handwritten filesystem adapter. Migration itself is portable and performs no I/O.
use intersect_model::{
    definitions::bundle::{DefinitionSource, SourceKind},
    migration::migrate_ecore,
};
use std::{env, fs, path::PathBuf};

fn file_uri(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    let path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .replace('\\', "/");
    let encoded: String = path
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    if encoded.starts_with("//") {
        format!("file:{encoded}")
    } else if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).map(PathBuf::from).collect();
    if args.len() != 3 {
        return Err("Usage: migrate_ecore <input.ecore> <output.rcore> <report.json>".into());
    }
    let input = fs::canonicalize(&args[0])?;
    for output in &args[1..] {
        if output.exists() && fs::canonicalize(output)? == input {
            return Err("Output must not overwrite the input source".into());
        }
    }
    if args[1] == args[2]
        || (args[1].exists()
            && args[2].exists()
            && fs::canonicalize(&args[1])? == fs::canonicalize(&args[2])?)
    {
        return Err("Rcore and report outputs must be distinct".into());
    }
    let original = DefinitionSource::new(
        file_uri(&input),
        SourceKind::Ecore,
        fs::read_to_string(&input)?,
    );
    let output = env::current_dir()?.join(&args[1]);
    let parent = output.parent().ok_or("Missing output parent")?;
    let output =
        fs::canonicalize(parent)?.join(output.file_name().ok_or("Missing output filename")?);
    let report = migrate_ecore(original, &file_uri(&output))?;
    fs::write(&args[2], serde_json::to_vec_pretty(&report)?)?;
    let converted = report
        .converted
        .ok_or("Manual migration required; inspect the report and its editable draft")?;
    fs::write(&args[1], converted.text)?;
    println!(
        "Converted {}; report {}",
        args[1].display(),
        args[2].display()
    );
    Ok(())
}
