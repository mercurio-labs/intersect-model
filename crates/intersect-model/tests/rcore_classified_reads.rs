#[path = "../../../fixtures/rcore-model-rule-service/classified_scan.rs"]
mod portable;
#[test]
fn source_selector_keeps_first_match_order_errors_and_original_work() -> Result<(), String> {
    portable::verify()
}
#[test]
fn source_selector_cannot_fabricate_a_matching_observation() -> Result<(), String> {
    use intersect_model::{core::query::QueryBudget, semantic_reads::ClassifiedValue};
    let body = "rcore 1; package fake.selection version \"1\"; import model \"urn:intersect:classified-read-protocol\" as c; fn fake(cursor:c.Cursor)->Option<Int64> = Some(0);";
    let program = portable::compile(body)?;
    let mut called = false;
    let error = program
        .invoke(
            |_, _| {
                called = true;
                Ok(ClassifiedValue::Match(serde_json::json!("fabricated")))
            },
            &mut QueryBudget::new(1_000_000),
        )
        .err()
        .ok_or("Unobserved result accepted")?;
    assert!(!called);
    assert!(error.contains("unobserved"));
    let body = body.replace("Some(0)", "{ let head = cursor.head; Some(0) }");
    let program = portable::compile(&body)?;
    assert!(
        program
            .invoke(
                |_, budget| {
                    budget.charge()?;
                    Ok(ClassifiedValue::NonMatch)
                },
                &mut QueryBudget::new(1_000_000)
            )
            .is_err()
    );
    Ok(())
}
#[test]
fn compatibility_read_callback_cannot_replenish_the_caller_allowance() -> Result<(), String> {
    use intersect_model::{core::query::QueryBudget, semantic_reads::ClassifiedValue};
    let program = portable::compile(portable::BODY)?;
    let mut budget = QueryBudget::new(1_000_000);
    let error = program
        .invoke(
            |_, work| {
                *work = QueryBudget::new(2_000_000);
                Ok(ClassifiedValue::Absent)
            },
            &mut budget,
        )
        .err()
        .ok_or("Callback changed its original allowance")?;
    assert!(error.contains("original allowance"));
    assert_eq!(budget.remaining(), 0);
    assert!(budget.reads > 0);
    Ok(())
}
