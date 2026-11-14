//! Handwritten unit tests of the frozen metamodel's class-key lookup.
use super::*;
use intersect_model::definitions::{
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};

const SOURCE: &str = r#"rcore 1;
package keys version "1";
metamodel "urn:intersect:class-keys" prefix k {
  abstract class Named { attr name: String [1,1] default ""; }
  class Node extends Named { ref children: Named [0,*] containment; }
  class Leaf extends Named { attr note: String [0,1]; }
}"#;

/// The lookup that skips the repeated schema validation answers every class
/// declaration, and an unknown one, as `Schema::class_key_for_type` does.
#[test]
fn class_keys_match_the_validating_schema_lookup() -> Result<(), String> {
    let package = intersect_model::compile_rcore_at("memory:/class-keys.rcore", SOURCE)
        .map_err(|error| error.to_string())?;
    let metamodel = Metamodel::freeze(package.schema()).map_err(|error| error.to_string())?;
    let schema = metamodel.schema();
    let classifier = |path: Vec<usize>, source| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId { source, path },
            arguments: vec![],
            raw: false,
        })
    };
    let mut found = 0;
    for (key, declaration) in &schema.declarations {
        let ty = classifier(declaration.path.clone(), declaration.source.clone())?;
        let expected = schema.class_key_for_type(&ty);
        assert_eq!(metamodel.class_key_for_type(&ty), expected, "{key}");
        if schema.classes.contains_key(key) {
            assert_eq!(expected.as_deref(), Ok(key.as_str()));
            found += 1;
        }
    }
    assert_eq!(found, 3);
    let source = schema
        .declarations
        .values()
        .next()
        .map(|declaration| declaration.source.clone())
        .ok_or("No declarations")?;
    let unknown = classifier(vec![usize::MAX], source)?;
    let expected = schema.class_key_for_type(&unknown);
    assert!(expected.is_err());
    assert_eq!(metamodel.class_key_for_type(&unknown), expected);
    Ok(())
}

/// The lexical target selected from a class's effective features is the one
/// the per-attribute lookup selects, including its refusals.
#[test]
fn lexical_targets_among_effective_features_match_the_lookup() -> Result<(), String> {
    let package = intersect_model::compile_rcore_at("memory:/class-keys.rcore", SOURCE)
        .map_err(|error| error.to_string())?;
    let schema = package.schema();
    let mut compared = 0;
    for class in schema.classes.keys() {
        let effective = schema.effective_features(class)?;
        let names = effective
            .iter()
            .map(|member| member.feature.name.as_str())
            .chain(["missing"]);
        for name in names {
            assert_eq!(
                schema.runtime_attribute_lexical_target_among(class, name, &effective),
                schema.runtime_attribute_lexical_target(class, name),
                "{class}.{name}"
            );
            compared += 1;
        }
    }
    assert!(compared >= 7);
    Ok(())
}
