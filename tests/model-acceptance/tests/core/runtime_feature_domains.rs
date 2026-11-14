use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{import_datatype_bundle, materialize_runtime_types},
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
use serde_json::json;
struct Fixture {
    bundle: DefinitionBundle,
    declarations: std::collections::BTreeMap<Vec<usize>, DeclarationId>,
}
impl Fixture {
    fn new(extra: &str) -> Self {
        let text = include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore")
            .replace("</e:EPackage>", &format!("{extra}</e:EPackage>"));
        let source = DefinitionSource::new("memory:runtime-domains", SourceKind::Ecore, text);
        let (source, migration) =
            intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
        Self {
            declarations: migration,
            bundle: DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
        }
    }
    fn ty(&self, n: usize, arguments: Vec<Type>, raw: bool) -> RuntimeType {
        RuntimeType::new(Type::Classifier {
            declaration: self.declarations[&vec![n]].clone(),
            arguments,
            raw,
        })
        .unwrap()
    }
    fn atom(&self, n: usize) -> RuntimeType {
        self.ty(n, vec![], false)
    }
    fn pair(&self, a: &RuntimeType, b: &RuntimeType) -> RuntimeType {
        self.ty(
            4,
            vec![a.expression().clone(), b.expression().clone()],
            false,
        )
    }
    fn value(&self, n: usize, text: &str) -> serde_json::Value {
        json!(
            import_datatype_bundle(&self.bundle, &self.declarations[&vec![n]].clone())
                .unwrap()
                .parse(text)
                .unwrap()
        )
    }
}
#[test]
fn raw_nested_reference_domains_preserve_independent_bounds_and_rollback() {
    let fixture = Fixture::new("");
    let a = fixture.atom(1);
    let b = fixture.atom(2);
    let other = fixture.atom(3);
    let aa = fixture.pair(&a, &a);
    let ab = fixture.pair(&a, &b);
    let bad = fixture.pair(&a, &other);
    let raw = fixture.ty(5, vec![], true);
    let closed = fixture.ty(5, vec![a.expression().clone()], false);
    let schema = materialize_runtime_types(
        &fixture.bundle,
        &[
            fixture.atom(0),
            a,
            b,
            other,
            aa.clone(),
            ab.clone(),
            bad.clone(),
            raw.clone(),
            closed.clone(),
        ],
        None,
    )
    .unwrap();
    let mut model = Model::default();
    for (id, ty) in [
        ("aa", &aa),
        ("ab", &ab),
        ("bad", &bad),
        ("raw", &raw),
        ("closed", &closed),
    ] {
        model
            .create(
                &schema,
                id,
                schema.class_key_for_type(ty).unwrap(),
                "memory:model",
                0,
                0,
            )
            .unwrap();
    }
    model
        .edit(&schema, &[("raw".into(), "pair".into(), json!("ab"))])
        .unwrap();
    let before = model.clone();
    assert!(
        model
            .edit(
                &schema,
                &[
                    ("closed".into(), "pair".into(), json!("aa")),
                    ("raw".into(), "pair".into(), json!("bad"))
                ]
            )
            .is_err()
    );
    assert_eq!(model, before);
    assert!(
        model
            .edit(&schema, &[("closed".into(), "pair".into(), json!("ab"))])
            .is_err()
    );
    model.validate(&schema).unwrap();
    let replay = materialize_runtime_types(
        &fixture.bundle,
        &[
            fixture.atom(0),
            fixture.atom(1),
            fixture.atom(2),
            fixture.atom(3),
            aa,
            ab,
            bad,
            raw,
            closed,
        ],
        None,
    )
    .unwrap();
    assert_eq!(schema, replay);
    model.validate(&replay).unwrap();
}
#[test]
fn inherited_raw_attributes_validate_actual_nominal_tags_without_retagging() {
    let fixture = Fixture::new("");
    let child = fixture.ty(9, vec![], true);
    let schema =
        materialize_runtime_types(&fixture.bundle, std::slice::from_ref(&child), None).unwrap();
    let key = schema.class_key_for_type(&child).unwrap();
    let domains = schema.feature_domains(key, "value");
    assert_eq!(domains.len(), 1);
    assert_ne!(
        domains[0].source.projection.source.owner,
        *domains[0].source.projection.storage_owner.expression()
    );
    let mut model = Model::default();
    model
        .create(&schema, "c", key, "memory:model", 0, 0)
        .unwrap();
    let value = fixture.value(6, "accepted");
    assert_eq!(
        schema
            .parse_runtime_attribute(key, "value", "accepted", None)
            .unwrap(),
        value
    );
    model
        .edit(&schema, &[("c".into(), "value".into(), value.clone())])
        .unwrap();
    assert_eq!(model.elements[0].values["value"], value);
    let before = model.clone();
    assert!(
        model
            .edit(
                &schema,
                &[("c".into(), "value".into(), fixture.value(7, "7"))]
            )
            .is_err()
    );
    assert_eq!(model, before);
    assert!(
        schema
            .validate_attribute(&schema.features(key).unwrap()["value"], &value)
            .is_err()
    );
    let field = schema.features(key).unwrap()["value"].clone();
    assert!(
        schema
            .validate_runtime_attribute("missing", &field, &value, None)
            .is_err()
    );
    assert!(
        schema
            .require_runtime_attribute_conversion("missing", &field, None)
            .is_err()
    );
    let mut fabricated = field.clone();
    fabricated.upper = -1;
    assert!(
        schema
            .validate_runtime_attribute(key, &fabricated, &value, None)
            .is_err()
    );
    assert!(
        schema
            .require_runtime_attribute_conversion(key, &fabricated, None)
            .is_err()
    );
    model.validate(&schema).unwrap();
}
#[test]
fn attribute_intersections_keep_alias_tags_and_optional_empty_domains() {
    let extra = r##"<eClassifiers x:type="e:EDataType" name="TextAlias" instanceClassName="java.lang.String"/>
    <eClassifiers x:type="e:EClass" name="Aliases"><eTypeParameters name="T"><eBounds eClassifier="#//Text"/><eBounds eClassifier="#//TextAlias"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Aliases/T"/></eStructuralFeatures></eClassifiers>
    <eClassifiers x:type="e:EClass" name="Empty"><eTypeParameters name="T"><eBounds eClassifier="#//Text"/><eBounds eClassifier="#//Number"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Empty/T"/></eStructuralFeatures></eClassifiers>"##;
    let fixture = Fixture::new(extra);
    let aliases = fixture.ty(19, vec![], true);
    let empty = fixture.ty(20, vec![], true);
    let schema =
        materialize_runtime_types(&fixture.bundle, &[aliases.clone(), empty.clone()], None)
            .unwrap();
    let mut model = Model::default();
    for (id, ty) in [("a", &aliases), ("e", &empty)] {
        model
            .create(
                &schema,
                id,
                schema.class_key_for_type(ty).unwrap(),
                "memory:model",
                0,
                0,
            )
            .unwrap();
    }
    for n in [6, 18] {
        let value = fixture.value(n, "text");
        model
            .edit(&schema, &[("a".into(), "value".into(), value.clone())])
            .unwrap();
        assert_eq!(model.elements[0].values["value"], value);
    }
    assert!(
        schema.feature_domains(schema.class_key_for_type(&empty).unwrap(), "value")[0]
            .alternatives
            .is_empty()
    );
    let before = model.clone();
    assert!(
        model
            .edit(
                &schema,
                &[
                    ("a".into(), "value".into(), fixture.value(6, "rollback")),
                    ("e".into(), "value".into(), fixture.value(6, "no"))
                ]
            )
            .is_err()
    );
    assert_eq!(model, before);
    model.validate(&schema).unwrap();
    assert!(
        schema
            .parse_runtime_attribute(
                schema.class_key_for_type(&empty).unwrap(),
                "value",
                "no",
                None
            )
            .is_err()
    );
    let bad_extra = extra.replace(
        "name=\"value\"><eGenericType eTypeParameter=\"#//Empty/T\"",
        "name=\"value\" defaultValueLiteral=\"source\"><eGenericType eTypeParameter=\"#//Empty/T\"",
    );
    let bad = Fixture::new(&bad_extra);
    assert!(materialize_runtime_types(&bad.bundle, &[bad.ty(20, vec![], true)], None).is_err());
}
