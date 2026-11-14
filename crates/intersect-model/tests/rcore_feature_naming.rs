#[path = "../../../fixtures/rcore-getter-service/feature_naming.rs"]
mod portable;

#[test]
fn authored_feature_policy_uses_checked_scalar_contracts_and_original_work() -> Result<(), String> {
    portable::verify()
}
