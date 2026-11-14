#[path = "../../../fixtures/rcore-getter-service/composition.rs"]
mod composition;
#[test]
fn composed_source_getters_share_the_existing_vm_and_caller_ledger() -> Result<(), String> {
    composition::verify()
}

#[test]
fn missing_ambiguous_and_mismatched_composed_bindings_reject_at_loading() {
    let missing = composition::SOURCE.replace(
        "  derive m.Node.label(self) -> Option<String> = label(self);",
        "",
    );
    let ambiguous=composition::SOURCE.replace("  derive m.Node.label(self)","  derive m.Child.label(self) -> Option<String> = child_label(self);\n  fn child_label(self:m.Child) -> Option<String> = self.raw;\n  derive m.Node.label(self)");
    let wrong = composition::SOURCE.replace("fn label(self:m.Node)", "fn label(self:m.Child)");
    let hidden = missing.replace(
        "match self.label { Some(name) => Some(\"~\" + name), None => None }",
        "if false { self.label } else { None }",
    );
    for source in [missing, ambiguous, wrong, hidden] {
        assert!(intersect_model::compile_rcore_at("memory:composition.rcore", &source).is_err());
    }
}

#[test]
fn recursive_source_getters_use_existing_vm_depth_and_original_work_limits() -> Result<(), String> {
    use intersect_model::{core::query::QueryBudget, rcore::ExecutionLimits};
    let source = composition::SOURCE.replace(
        "fn label(self:m.Node) -> Option<String> = self.raw;",
        "fn label(self:m.Node) -> Option<String> = self.decorated;",
    );
    let package = intersect_model::compile_rcore_at("memory:composition.rcore", &source)
        .map_err(|error| error.to_string())?;
    let mut model = package.new_model().map_err(|error| error.to_string())?;
    model.create("a", "Node", "memory:objects")?;
    let limits = ExecutionLimits {
        max_frames: 8,
        ..ExecutionLimits::default()
    };
    let mut work = QueryBudget::new(1_000_000);
    let error = package
        .read_derived(&model, "a", "label", &mut work, &limits)
        .err()
        .ok_or("Recursive getters returned a value")?;
    assert!(
        error.to_string().contains("frame") || error.to_string().contains("depth"),
        "{error}"
    );
    let mut short = QueryBudget::new(1);
    assert!(
        package
            .read_derived(
                &model,
                "a",
                "label",
                &mut short,
                &ExecutionLimits::default()
            )
            .is_err()
    );
    assert_eq!(short.remaining(), 0);
    assert!(
        package
            .read_derived(
                &model,
                "a",
                "label",
                &mut short,
                &ExecutionLimits::default()
            )
            .is_err()
    );
    Ok(())
}
