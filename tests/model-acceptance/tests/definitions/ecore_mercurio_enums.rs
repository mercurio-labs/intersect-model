use serde_json::Value;
use sha2::{Digest, Sha256};

#[test]
fn pinned_mercurio_rule_projections_preserve_literal_values_not_grammar_aliases() {
    let bytes = include_bytes!("../../../../fixtures/ecore-scalars/mercurio-enum-rules.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "0a76cbca06b55dbe9943911ac8dd497b86a83cab9c527d0488d07e90c5ce09db"
    );
    let fixture: Value = serde_json::from_slice(bytes).unwrap();
    let rules = fixture["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 14);
    let mut nonidentity_tokens = 0;
    for rule in rules {
        let literals = rule["literals"].as_array().unwrap();
        let escape = |text: &str| {
            text.replace('&', "&amp;")
                .replace('\'', "&apos;")
                .replace('<', "&lt;")
        };
        let body: String = literals
            .iter()
            .map(|literal| {
                format!(
                    "<eLiterals name='{}' value='{}'/>",
                    escape(literal["literal"].as_str().unwrap()),
                    literal["value"].as_i64().unwrap()
                )
            })
            .collect();
        let source = format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='projection' nsURI='urn:projection'><eClassifiers xsi:type='e:EEnum' name='Kind'>{body}</eClassifiers></e:EPackage>"
        );
        let schema =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source)
                .unwrap();
        let enumeration = &schema.enumerations["Kind"];
        for literal in literals {
            let spelling = literal["literal"].as_str().unwrap();
            let value = enumeration.parse(spelling).unwrap();
            let selected = enumeration.validate_value(&value).unwrap();
            assert_eq!(
                i64::from(selected.value),
                literal["value"].as_i64().unwrap()
            );
            assert_eq!(enumeration.format(&value).unwrap(), spelling);
            let token = literal["token"].as_str().unwrap();
            if token != spelling {
                nonidentity_tokens += 1;
                if !literals.iter().any(|other| other["literal"] == token) {
                    assert!(enumeration.parse(token).is_err(), "{}: {token}", rule["id"]);
                }
            }
        }
    }
    assert!(nonidentity_tokens >= 5);
}
