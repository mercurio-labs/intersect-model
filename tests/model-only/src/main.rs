fn main() -> Result<(), String> {
    intersect_model_only_consumer::verify_boundary()?;
    println!("Model-only Rcore construction, execution, rejection and recovery passed.");
    Ok(())
}
