fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report =
        intersect_semantic_bindings_tests::transaction_qualification::qualify_transactions()?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
