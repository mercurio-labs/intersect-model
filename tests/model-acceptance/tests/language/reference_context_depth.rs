use intersect_model::conversion::with_catalog_policies;
use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::compile_structure_bundle_with_conversions,
    conversion::{ConversionContract, MemberContract, MemberType, ProviderId, ReferenceContext},
    resources::DeclarationId,
};
use serde_json::json;

// Every inner selected policy is consumed by a real attribute. This crosses
// immutable model-context boundaries, rather than merely nesting list payloads.
fn context(level: usize, inner: Option<ReferenceContext>) -> Result<ReferenceContext, String> {
    let attribute = if inner.is_some() {
        r##"<eStructuralFeatures x:type="e:EAttribute" name="items" eType="#//List"/>"##
    } else {
        ""
    };
    let datatype = if inner.is_some() {
        r#"<eClassifiers x:type="e:EDataType" name="List" instanceClassName="org.eclipse.emf.common.util.EList" serializable="false"/>"#
    } else {
        ""
    };
    let source = DefinitionSource::new(
        format!("memory:depth-{level}"),
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="depth" nsURI="urn:depth:{level}"><eClassifiers x:type="e:EClass" name="Object">{attribute}</eClassifiers>{datatype}</e:EPackage>"#
        ),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let conversions = inner
        .into_iter()
        .map(|inner| ConversionContract {
            datatype: DeclarationId {
                source: source.id.clone(),
                path: vec![1],
            },
            provider: ProviderId {
                name: "ecore.list".into(),
                version: "1".into(),
            },
            serializable: false,
            declarations: vec![],
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::Reference {
                    class: DeclarationId {
                        source: inner.entry.clone(),
                        path: vec![0],
                    },
                    context: Box::new(inner),
                },
            }],
        })
        .collect::<Vec<_>>();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let namespace = format!("urn:depth:{level}");
    let model = with_catalog_policies(&bundle, &conversions, &[], &[], |registry| {
        let schema =
            compile_structure_bundle_with_conversions(&bundle, &source.id, &namespace, registry)
                .map_err(|e| e.to_string())?;
        let mut model = Model::default();
        model.create(
            &schema,
            "object",
            schema.language_class_key("Object")?,
            "memory:depth",
            0,
            0,
        )?;
        model.validate_with_conversions(&schema, registry)?;
        Ok(json!(model))
    })?;
    Ok(ReferenceContext {
        bundle,
        entry: source.id,
        namespace,
        conversions,
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model,
    })
}

#[test]
fn nested_reference_contexts_share_the_catalog_depth_limit() {
    let mut inner = context(0, None).unwrap();
    for level in 1..=15 {
        inner = context(level, Some(inner)).unwrap();
    }
    let error = context(16, Some(inner)).unwrap_err();
    assert!(error.contains("depth budget"), "{error}");
}
