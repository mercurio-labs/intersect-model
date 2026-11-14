//! Handwritten host adapter: generate a Rust module from one Rcore file.
use std::{env, fs, path::PathBuf, process::ExitCode};

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let usage = "Usage: intersect-codegen <input.rcore> <output.rs>";
    let input = args.next().ok_or(usage)?;
    if input == "--help" || input == "-h" {
        println!("{usage}");
        return Ok(());
    }
    let input = PathBuf::from(input);
    let output = PathBuf::from(args.next().ok_or(usage)?);
    if args.next().is_some() {
        return Err(usage.into());
    }
    let source = fs::read_to_string(&input).map_err(|e| format!("{}: {e}", input.display()))?;
    // Generate and validate completely before opening the output file.
    let generated = intersect_model::codegen::generate_rcore(&source)?;
    if output.exists()
        && fs::canonicalize(&input).map_err(|e| e.to_string())?
            == fs::canonicalize(&output).map_err(|e| e.to_string())?
    {
        return Err("Input and output must be different files".into());
    }
    fs::write(&output, generated).map_err(|e| format!("{}: {e}", output.display()))?;
    println!("Generated {}", output.display());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
