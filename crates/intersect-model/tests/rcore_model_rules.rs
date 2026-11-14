//! Direct source activation remains bounded; no Xtext feature/dependency required.
#[path = "../../../fixtures/rcore-model-rule-service/direct.rs"]
mod direct;
use intersect_model::compile_rcore_at;

#[test]
fn authored_builtin_rules_execute_reload_and_recover_through_the_direct_api() -> Result<(), String>
{
    direct::verify()
}

#[test]
fn opaque_derived_implementations_and_missing_providers_remain_explicit() {
    let source = r#"rcore 1; package boolRules version "1";
metamodel "urn:bool-rules" prefix b {
 class Item { attr input: Bool [1,1]; attr output: Bool [1,1] derived; }
}
behavior {
 import model "urn:bool-rules" as m;
 extern fn custom(value: Bool) -> Bool;
 pub derive m.Item.output(self) -> Bool = custom(self.input);
}"#;
    assert!(compile_rcore_at("memory:opaque-rule.rcore", source).is_err());
    let structural = source.split("behavior {").next().unwrap();
    let package = compile_rcore_at("memory:unimplemented-rule.rcore", structural).unwrap();
    let model = package.new_model().unwrap();
    assert!(package.evaluate_model_rules(&model).is_err());
}

#[test]
fn lowering_errors_preserve_selected_identity_offsets_and_detail() -> Result<(), String> {
    direct::verify_diagnostics()
}
