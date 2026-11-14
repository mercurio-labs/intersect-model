use super::*;
use crate::definitions::runtime_schema_test_inputs as inputs;
fn caches(models: &ModelTypes<'_>) -> serde_json::Value {
    serde_json::json!({"generic_checks":models.generic_checks.iter().collect::<Vec<_>>(),"schemas":models.runtime_schemas.iter().map(|(key, schema)| (key, schema.schema())).collect::<Vec<_>>()})
}
#[test]
fn runtime_schema_capture_complete_projection_caches_and_errors() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("runtime_schema_baseline.json")).unwrap();
    for case in 0..12 {
        let (bundle, requested, name) = inputs::fixture(case);
        let mut models = ModelTypes::new(&bundle).unwrap();
        let schema = models
            .runtime_schema(&requested)
            .map(|schema| serde_json::to_value(schema.schema()).unwrap());
        let after_schema = caches(&models);
        let projection = models.feature_projection(&requested[0], name);
        let after_projection = caches(&models);
        let mut cold = ModelTypes::new(&bundle).unwrap();
        let cold_projection = cold.feature_projection(&requested[0], name);
        let row = serde_json::json!({"case":case,"schema":schema,"after_schema":after_schema,"projection":projection,"after_projection":after_projection,"cold_projection":cold_projection,"cold_caches":caches(&cold)});
        assert_eq!(row, baseline["model"][case]);
    }
}
