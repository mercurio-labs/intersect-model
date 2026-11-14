//! Handwritten unit tests of computed setting contract validation.
use super::*;
use crate::definitions::bundle::DefinitionSource;

const URI: &str = "memory:/setting-contract.rcore";
const SOURCE: &str = r#"rcore 1;
package settings version "1";
metamodel "urn:intersect:setting-contract" prefix s {
  class Node {
    attr name: String [1,1] default "";
    ref first: Node [0,1] derived;
  }
}"#;

/// Validation against an already validated bundle accepts and refuses
/// exactly what the validating entry point does.
#[test]
fn contracts_validate_alike_against_a_validated_bundle() -> Result<(), String> {
    let package = crate::compile_rcore_at(URI, SOURCE).map_err(|error| error.to_string())?;
    let schema = package.schema();
    let source = DefinitionSource::new(URI, SourceKind::Rcore, SOURCE);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: Vec::new(),
    };
    let class = schema.classes.keys().next().ok_or("No class")?.to_owned();
    let contract = |feature: &str| -> Result<ComputedSettingContract, String> {
        Ok(ComputedSettingContract {
            version: 1,
            schema_revision: schema.revision.clone(),
            class: class.clone(),
            class_identity: schema.identity(&class),
            feature: feature.into(),
            member: schema.effective_feature(&class, feature)?.identity,
            provider: ProviderId {
                name: "test.provider".into(),
                version: "1".into(),
            },
            sources: vec![source.id.clone()],
            operations: vec![SettingOperation::Set],
        })
    };
    let validated = bundle.validated()?;
    let accepted = contract("first")?;
    assert_eq!(accepted.validate(schema, &bundle), Ok(()));
    assert_eq!(accepted.validate_in(schema, &validated), Ok(()));
    let mut stale = accepted.clone();
    stale.schema_revision.push('x');
    let mut unsourced = accepted.clone();
    unsourced.sources.clear();
    for refused in [contract("name")?, stale, unsourced] {
        let expected = refused.validate(schema, &bundle);
        assert!(expected.is_err());
        assert_eq!(refused.validate_in(schema, &validated), expected);
    }
    // A bundle whose text no longer matches its identity yields no proof, and
    // the validating entry point refuses it with the same error.
    let mut corrupt = bundle.clone();
    corrupt.sources[0].text.push(' ');
    let error = corrupt
        .validated()
        .err()
        .ok_or("corrupt bundle validated")?;
    assert_eq!(accepted.validate(schema, &corrupt), Err(error));
    Ok(())
}
