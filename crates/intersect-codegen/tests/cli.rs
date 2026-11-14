use std::{fs, process::Command};

#[test]
fn cli_generates_and_preserves_output_on_invalid_input() -> Result<(), Box<dyn std::error::Error>> {
    let directory =
        std::env::temp_dir().join(format!("intersect-codegen-cli-{}", std::process::id()));
    fs::create_dir_all(&directory)?;
    let input = directory.join("input.rcore");
    let output = directory.join("output.rs");
    let source = include_str!("../../../fixtures/library/library-bindings.rcore");
    fs::write(&input, source)?;
    let run = |input: &std::path::Path, output: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_intersect-codegen"))
            .arg(input)
            .arg(output)
            .output()
    };
    let result = run(&input, &output)?;
    assert!(result.status.success(), "{:?}", result.stderr);
    let generated = fs::read_to_string(&output)?;
    assert_eq!(generated, intersect_model::codegen::generate_rcore(source)?);
    assert!(!run(&input, &input)?.status.success());
    assert_eq!(fs::read_to_string(&input)?, source);
    fs::write(&input, "not rcore")?;
    assert!(!run(&input, &output)?.status.success());
    assert_eq!(fs::read_to_string(&output)?, generated);
    fs::remove_file(&input)?;
    fs::remove_file(&output)?;
    fs::remove_dir(&directory)?;
    Ok(())
}
