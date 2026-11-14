#![cfg(feature = "xmi")]
//! Bounded native lexical-provider XMI controls; no external oracle award.
use intersect_model::{
    conversion::RgbConverter,
    core::{
        interchange::{FormatContext, FormatRegistry, ModelFormat, ReferenceClosure, Resource},
        model::Model,
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::import_datatype_bundle,
        conversion::{ConversionContract, ConversionProvider, ConversionRegistry, ProviderId},
        datatypes::Datatype,
        rcore::{compile_structure, compile_structure_with_conversions},
        resources::ResourceSet,
        schema::Schema,
    },
    xmi::XmiFormat,
};
use serde_json::{Value, json};
use std::rc::Rc;

const SOURCE: &str = r##"rcore 1; package colors version "1";
metamodel "urn:colors" prefix c {
    datatype RGB instanceClass "example.RGB" {}
    enum Mode { Day = 1; Night = 2; }
    class Item {
        attr color: RGB [0,1] default "#010203";
        attr optional: RGB [0,1];
        attr palette: RGB [0,*];
        attr note: EString [0,1];
        attr count: EInt [0,1];
        attr mode: Mode [0,1];
        ref peer: Item [0,1];
    }
}"##;
struct Fixture {
    schema: Schema,
    datatype: Datatype,
    conversions: ConversionRegistry<'static>,
}
fn fixture(serializable: bool) -> Fixture {
    let source = DefinitionSource::new(
        "memory:colors.rcore",
        SourceKind::Rcore,
        if serializable {
            SOURCE.to_owned()
        } else {
            SOURCE
                .replace(
                    "instanceClass \"example.RGB\"",
                    "instanceClass \"example.RGB\" nonserializable",
                )
                .replace(" default \"#010203\"", "")
        },
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let id = ResourceSet::new(&bundle)
        .unwrap()
        .resolve(&source.id, "#//RGB")
        .unwrap();
    let datatype = import_datatype_bundle(&bundle, &id).unwrap();
    let mut conversions = ConversionRegistry::default();
    conversions
        .register_shared(&datatype, Rc::new(provider(&datatype, "1")))
        .unwrap();
    let schema = compile_structure_with_conversions(&bundle, &source.id, None, &conversions)
        .unwrap()
        .schema;
    Fixture {
        schema,
        datatype,
        conversions,
    }
}
fn provider(datatype: &Datatype, version: &str) -> RgbConverter {
    RgbConverter::new(ConversionContract {
        datatype: datatype.declaration().clone(),
        provider: ProviderId {
            name: "example.rgb".into(),
            version: version.into(),
        },
        serializable: datatype.serializable(),
        declarations: vec![],
        members: vec![],
    })
}
fn registry() -> FormatRegistry {
    let mut registry = FormatRegistry::default();
    registry.register(XmiFormat).unwrap();
    registry
}
fn resource(uri: &str, body: &str) -> Resource {
    Resource { uri: uri.into(), bytes: format!(r#"<xmi:XMI xmlns:xmi="http://www.omg.org/XMI" xmlns:c="urn:colors" xmi:version="2.0">{body}</xmi:XMI>"#).into_bytes() }
}
fn import(f: &Fixture, resources: &[Resource]) -> Result<Model, String> {
    registry().import_with_format_context(
        "xmi",
        &f.schema,
        resources,
        None,
        &FormatContext {
            conversions: Some(&f.conversions),
            ..Default::default()
        },
    )
}
fn export(f: &Fixture, model: &Model) -> Result<Vec<Resource>, String> {
    registry().export_with_format_context(
        "xmi",
        &f.schema,
        model,
        None,
        &FormatContext {
            conversions: Some(&f.conversions),
            ..Default::default()
        },
    )
}
#[cfg_attr(test, test)]
pub fn source_bound_rgb_single_collection_default_null_and_builtin_enum_round_trip() {
    let f = fixture(true);
    let input = resource(
        "models/colors.xmi",
        r##"<c:Item xmi:id="a" color="#123abc" count="7" mode="Night" note="snow &amp; ice"><palette>#FF0000</palette><palette>#00FF00</palette></c:Item><c:Item xmi:id="b"/>"##,
    );
    let model = import(&f, std::slice::from_ref(&input)).unwrap();
    let converted = |text| {
        serde_json::to_value(f.conversions.parse(f.datatype.declaration(), text).unwrap()).unwrap()
    };
    assert_eq!(model.elements[0].values["color"], converted("#123ABC"));
    assert_eq!(
        model.elements[0].values["palette"],
        json!([converted("#FF0000"), converted("#00FF00")])
    );
    assert_eq!(model.elements[0].values["count"], json!(7));
    assert_eq!(model.elements[1].values["color"], converted("#010203"));
    assert_eq!(model.elements[1].values["optional"], Value::Null);
    assert_eq!(f.datatype.declaration().source.uri, "memory:colors.rcore");
    let output = export(&f, &model).unwrap();
    let xml = String::from_utf8(output[0].bytes.clone()).unwrap();
    assert!(xml.contains("color=\"#123ABC\""));
    assert!(xml.contains("<palette>#FF0000</palette>"));
    assert!(!xml.contains("&quot;kind&quot;"));
    assert_eq!(import(&f, &output).unwrap(), model);
    assert!(registry().import("xmi", &f.schema, &[input]).is_err());
    assert!(registry().export("xmi", &f.schema, &model).is_err());
    let mut absent = model;
    absent.elements[0]
        .values
        .insert("color".into(), Value::Null);
    let output = export(&f, &absent).unwrap();
    assert_eq!(import(&f, &output).unwrap(), absent);
}
#[cfg_attr(test, test)]
pub fn missing_wrong_version_stale_source_and_untrusted_payloads_reject() {
    let f = fixture(true);
    let input = resource("colors.xmi", r##"<c:Item color="#123ABC"/>"##);
    let model = import(&f, std::slice::from_ref(&input)).unwrap();
    let mut wrong_version = ConversionRegistry::default();
    wrong_version
        .register_shared(&f.datatype, Rc::new(provider(&f.datatype, "2")))
        .unwrap();
    let stale = fixture_with_changed_source();
    for conversions in [
        ConversionRegistry::default(),
        wrong_version,
        stale.conversions,
    ] {
        let context = FormatContext {
            conversions: Some(&conversions),
            ..Default::default()
        };
        assert!(
            registry()
                .import_with_format_context(
                    "xmi",
                    &f.schema,
                    std::slice::from_ref(&input),
                    None,
                    &context
                )
                .is_err()
        );
        assert!(
            registry()
                .export_with_format_context("xmi", &f.schema, &model, None, &context)
                .is_err()
        );
    }
    for value in [
        json!("#123ABC"),
        json!({"red":18,"green":58,"blue":188}),
        {
            let mut v = model.elements[0].values["color"].clone();
            v["provider"]["version"] = json!("2");
            v
        },
        {
            let mut v = model.elements[0].values["color"].clone();
            v["datatype"]["source"]["sha256"] = json!("0".repeat(64));
            v
        },
    ] {
        let mut invalid = model.clone();
        invalid.elements[0].values.insert("color".into(), value);
        assert!(export(&f, &invalid).is_err());
    }
}
fn fixture_with_changed_source() -> Fixture {
    let f = fixture(true);
    let source = DefinitionSource::new(
        "memory:colors.rcore",
        SourceKind::Rcore,
        format!("{SOURCE}\n"),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let id = ResourceSet::new(&bundle)
        .unwrap()
        .resolve(&source.id, "#//RGB")
        .unwrap();
    let datatype = import_datatype_bundle(&bundle, &id).unwrap();
    assert_ne!(
        datatype.declaration().source,
        f.datatype.declaration().source
    );
    let mut conversions = ConversionRegistry::default();
    conversions
        .register_shared(&datatype, Rc::new(provider(&datatype, "1")))
        .unwrap();
    Fixture {
        schema: f.schema,
        datatype,
        conversions,
    }
}
#[cfg_attr(test, test)]
pub fn malformed_lexical_nonserializable_and_namespace_reject_without_publication() {
    let f = fixture(true);
    for text in ["123ABC", "#123", "#GGGGGG", r#"{&quot;red&quot;:1}"#] {
        let input = resource("colors.xmi", &format!("<c:Item color=\"{text}\"/>"));
        assert!(import(&f, &[input]).is_err());
    }
    let input = resource("colors.xmi", r##"<c:Item color="#123ABC"/>"##);
    let mut wrong_ns = input.clone();
    wrong_ns.bytes = String::from_utf8(wrong_ns.bytes)
        .unwrap()
        .replace("urn:colors", "urn:other")
        .into_bytes();
    assert!(import(&f, &[wrong_ns]).is_err());
    let mut renamed = fixture(true);
    renamed
        .schema
        .declarations
        .get_mut("Item")
        .unwrap()
        .namespace = "urn:other".into();
    assert!(
        import(&renamed, &[input])
            .unwrap_err()
            .contains("authored class name")
    );
    let nonserializable = fixture(false);
    assert!(import(&nonserializable, &[resource("colors.xmi", "<c:Item/>")]).is_err());
}
#[cfg_attr(test, test)]
pub fn selected_resource_persistence_and_retained_reference_context_share_providers() {
    let f = fixture(true);
    let resources = [
        resource(
            "models/a.xmi",
            r##"<c:Item xmi:id="a" color="#FF0000" peer="b.xmi#b"/>"##,
        ),
        resource("models/b.xmi", r##"<c:Item xmi:id="b" color="#0000FF"/>"##),
    ];
    let model = import(&f, &resources).unwrap();
    let context = FormatContext {
        conversions: Some(&f.conversions),
        ..Default::default()
    };
    let selected = ["models/a.xmi".into()];
    let one = registry()
        .export_with_format_context(
            "xmi",
            &f.schema,
            &model,
            Some((&selected, ReferenceClosure::SelectedOnly)),
            &context,
        )
        .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].uri, "models/a.xmi");
    let retained = import(&f, &resources[1..]).unwrap();
    let restored = registry()
        .import_with_format_context("xmi", &f.schema, &one, Some(&retained), &context)
        .unwrap();
    assert_eq!(restored.elements.len(), 2);
    let a = restored
        .elements
        .iter()
        .find(|e| e.id == "models/a.xmi#a")
        .unwrap();
    assert_eq!(a.values["peer"], json!("models/b.xmi#b"));
    assert_eq!(a.values["color"], model.elements[0].values["color"]);
    assert!(
        registry()
            .import_with_format_context("xmi", &f.schema, &one, None, &context)
            .is_err()
    );
    let all = registry()
        .export_with_format_context(
            "xmi",
            &f.schema,
            &model,
            Some((&selected, ReferenceClosure::IncludeDependencies)),
            &context,
        )
        .unwrap();
    assert_eq!(
        all.iter().map(|r| r.uri.as_str()).collect::<Vec<_>>(),
        ["models/a.xmi", "models/b.xmi"]
    );
    let before = retained.clone();
    let invalid = resource("models/a.xmi", r##"<c:Item color="invalid"/>"##);
    assert!(
        registry()
            .import_with_format_context("xmi", &f.schema, &[invalid], Some(&retained), &context)
            .is_err()
    );
    assert_eq!(retained, before);
}
struct Lossy(RgbConverter);
impl ConversionProvider for Lossy {
    fn contract(&self) -> &ConversionContract {
        self.0.contract()
    }
    fn parse(&self, text: &str) -> Result<Value, String> {
        self.0.parse(text)
    }
    fn format(&self, _value: &Value) -> Result<String, String> {
        Ok("#000000".into())
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.0.validate(value)
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        self.0.default_value()
    }
}
#[cfg_attr(test, test)]
pub fn lossy_provider_format_is_rejected_by_shared_registry_round_trip_check() {
    let f = fixture(true);
    let model = import(
        &f,
        &[resource("colors.xmi", r##"<c:Item color="#123ABC"/>"##)],
    )
    .unwrap();
    let mut conversions = ConversionRegistry::default();
    conversions
        .register_shared(&f.datatype, Rc::new(Lossy(provider(&f.datatype, "1"))))
        .unwrap();
    let error = registry()
        .export_with_format_context(
            "xmi",
            &f.schema,
            &model,
            None,
            &FormatContext {
                conversions: Some(&conversions),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(error.contains("loses value information"), "{error}");
}
#[cfg_attr(test, test)]
pub fn ordinary_context_free_codec_bytes_remain_equal_with_explicit_empty_adapters() {
    let source = DefinitionSource::new(
        "memory:ordinary.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package colors version "1"; metamodel "urn:colors" prefix c { enum Mode { Day = 1; Night = 2; } class Item { attr note: EString [0,1]; attr count: EInt [0,1]; attr mode: Mode [0,1]; } }"#,
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure(&bundle, &source.id).unwrap().schema;
    let input = resource(
        "ordinary.xmi",
        r#"<c:Item note="A &amp; B" count="7" mode="Night"/>"#,
    );
    let model = XmiFormat.decode(&schema, &[input]).unwrap();
    let ordinary = XmiFormat.encode(&schema, &model).unwrap();
    let conversions = ConversionRegistry::default();
    let explicit = registry()
        .export_with_format_context(
            "xmi",
            &schema,
            &model,
            None,
            &FormatContext {
                conversions: Some(&conversions),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(ordinary, explicit);
    assert_eq!(registry().import("xmi", &schema, &ordinary).unwrap(), model);
}

#[cfg_attr(test, test)]
pub fn codecs_without_provider_adoption_do_not_silently_discard_context() {
    struct ContextFree;
    impl ModelFormat for ContextFree {
        fn id(&self) -> &str {
            "context_free"
        }
        fn extensions(&self) -> &[&str] {
            &["cf"]
        }
        fn media_type(&self) -> &str {
            "application/octet-stream"
        }
        fn decode(&self, schema: &Schema, _resources: &[Resource]) -> Result<Model, String> {
            Ok(Model {
                schema_revision: schema.revision.clone(),
                ..Model::default()
            })
        }
        fn encode(&self, _schema: &Schema, _model: &Model) -> Result<Vec<Resource>, String> {
            Ok(vec![])
        }
    }
    let f = fixture(true);
    let mut registry = FormatRegistry::default();
    registry.register(ContextFree).unwrap();
    let context = FormatContext {
        conversions: Some(&f.conversions),
        ..Default::default()
    };
    assert!(
        registry
            .import_with_format_context("context_free", &f.schema, &[], None, &context)
            .unwrap_err()
            .contains("does not support conversion providers")
    );
    let model = Model {
        schema_revision: f.schema.revision.clone(),
        ..Model::default()
    };
    assert!(
        registry
            .export_with_format_context("context_free", &f.schema, &model, None, &context)
            .unwrap_err()
            .contains("does not support conversion providers")
    );
}
