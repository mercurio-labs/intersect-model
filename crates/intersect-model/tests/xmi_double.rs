#![cfg(feature = "xmi")]
//! Existing generic XMI uses the SAME EDouble IEEE wire and lexical codec.
use intersect_model::{
    core::{
        interchange::{FormatContext, FormatRegistry, ReferenceClosure, Resource},
        model::Model,
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::ConversionRegistry,
        rcore::compile_structure,
        scalars::Builtin,
        schema::Schema,
    },
    xmi::XmiFormat,
};
use serde_json::{Value, json};

fn schema() -> Schema {
    // The same original Ecore declaration shape as LiteralRational.value in the
    // combined KerML/SysML metamodel, without depending on a language package.
    let input = DefinitionSource::new(
        "memory:rational.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="rational" nsURI="urn:rational" nsPrefix="r">
      <eClassifiers xsi:type="e:EClass" name="LiteralRational">
        <eStructuralFeatures xsi:type="e:EAttribute" name="value" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EDouble"/>
      </eClassifiers>
      <eClassifiers xsi:type="e:EClass" name="Other">
        <eStructuralFeatures xsi:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
      </eClassifiers>
    </e:EPackage>"##,
    );
    let migrated =
        intersect_model::migration::migrate_ecore(input, "memory:rational.rcore").unwrap();
    let source = migrated.converted.unwrap();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    compile_structure(&bundle, &source.id).unwrap().schema
}
fn registry() -> FormatRegistry {
    let mut registry = FormatRegistry::default();
    registry.register(XmiFormat).unwrap();
    registry
}
fn input(body: &str) -> Resource {
    Resource { uri: "models/rational.kerml".into(), bytes: format!(r#"<xmi:XMI xmlns:xmi="http://www.omg.org/XMI" xmlns:r="urn:rational" xmi:version="2.0">{body}</xmi:XMI>"#).into_bytes() }
}
fn rational(model: &Model) -> &Value {
    &model
        .elements
        .iter()
        .find(|row| row.class == "LiteralRational")
        .unwrap()
        .values["value"]
}

#[test]
fn finite_doubles_use_shared_bits_and_exact_canonical_lexical_round_trip() {
    let schema = schema();
    let formats = registry();
    let selected = vec!["models/rational.kerml".into()];
    for text in [
        "1.5",
        "-0.0",
        "0x1.0000000000001p0",
        "4.9e-324",
        "1.7976931348623157e308",
    ] {
        let expected = Builtin::EDouble.parse(text).unwrap();
        let lexical = Builtin::EDouble.format(&expected).unwrap().unwrap();
        let source = input(&format!(
            r#"<r:LiteralRational xmi:id="r" value="{text}"/>"#
        ));
        let model = formats.import("xmi", &schema, &[source]).unwrap();
        assert_eq!(rational(&model), &expected);
        let output = formats
            .export_selected(
                "xmi",
                &schema,
                &model,
                &selected,
                ReferenceClosure::SelectedOnly,
            )
            .unwrap();
        assert_eq!(output[0].uri, selected[0]);
        let xml = String::from_utf8(output[0].bytes.clone()).unwrap();
        assert!(xml.contains(&format!("value=\"{lexical}\"")));
        assert!(!xml.contains("float64"));
        assert_eq!(formats.import("xmi", &schema, &output).unwrap(), model);
        // Supplying an unrelated empty adapter context cannot change builtin
        // scalar dispatch into a custom conversion-provider lookup.
        let empty = ConversionRegistry::default();
        let context = FormatContext {
            conversions: Some(&empty),
            ..Default::default()
        };
        assert_eq!(
            formats
                .export_with_format_context(
                    "xmi",
                    &schema,
                    &model,
                    Some((&selected, ReferenceClosure::SelectedOnly)),
                    &context
                )
                .unwrap(),
            output
        );
    }
    // Profile validation scans the entire schema, including the unused EDouble
    // declaration when the selected resource only contains an ordinary object.
    let model = formats
        .import(
            "xmi",
            &schema,
            &[input(r#"<r:Other xmi:id="o" name="unused-double"/>"#)],
        )
        .unwrap();
    formats
        .export_selected(
            "xmi",
            &schema,
            &model,
            &selected,
            ReferenceClosure::SelectedOnly,
        )
        .unwrap();
}

#[test]
fn canonical_nonfinite_values_and_unsupported_nan_payloads_match_the_core_codec() {
    let schema = schema();
    let formats = registry();
    let selected = vec!["models/rational.kerml".into()];
    for text in ["Infinity", "-Infinity", "NaN"] {
        let expected = Builtin::EDouble.parse(text).unwrap();
        let lexical = Builtin::EDouble.format(&expected).unwrap().unwrap();
        let model = formats
            .import(
                "xmi",
                &schema,
                &[input(&format!(
                    r#"<r:LiteralRational xmi:id="r" value="{text}"/>"#
                ))],
            )
            .unwrap();
        assert_eq!(rational(&model), &expected);
        let bytes = formats
            .export_selected(
                "xmi",
                &schema,
                &model,
                &selected,
                ReferenceClosure::SelectedOnly,
            )
            .unwrap();
        assert!(
            String::from_utf8(bytes[0].bytes.clone())
                .unwrap()
                .contains(&format!("value=\"{lexical}\""))
        );
        assert_eq!(formats.import("xmi", &schema, &bytes).unwrap(), model);
    }
    let mut model = formats
        .import(
            "xmi",
            &schema,
            &[input(r#"<r:LiteralRational xmi:id="r" value="1.5"/>"#)],
        )
        .unwrap();
    let bad = json!({"kind":"float64", "bits":"7ff8000000000001"});
    let error = Builtin::EDouble.format(&bad).unwrap_err();
    assert_eq!(error, "Noncanonical NaN bits");
    model
        .elements
        .iter_mut()
        .find(|row| row.class == "LiteralRational")
        .unwrap()
        .values
        .insert("value".into(), bad);
    assert!(
        formats
            .export_selected(
                "xmi",
                &schema,
                &model,
                &selected,
                ReferenceClosure::SelectedOnly
            )
            .unwrap_err()
            .contains(&error)
    );
    for text in ["INF", "-INF", "nan", "not-a-double"] {
        let error = Builtin::EDouble.parse(text).unwrap_err();
        assert!(
            formats
                .import(
                    "xmi",
                    &schema,
                    &[input(&format!(
                        r#"<r:LiteralRational xmi:id="r" value="{text}"/>"#
                    ))]
                )
                .unwrap_err()
                .contains(&error)
        );
    }
}
