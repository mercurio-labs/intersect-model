use intersect_model::core::{
    interchange::ModelFormat,
    model::{Element, Model},
};
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::ProviderId,
    initialization::{ClassInitializationContract, InitializationReceipt},
};
use intersect_model::xmi::XmiFormat;
use serde_json::json;
#[test]
fn xmi_provenance_retains_receipts_without_bypassing_validation() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-settings/settings.ecore"),
    )
    .unwrap();
    let source = DefinitionSource::new(
        "test:constructor",
        SourceKind::RcoreBehavior,
        "retained < & provenance",
    );
    let receipt = InitializationReceipt {
        object: "a".into(),
        authored_members: vec![],
        contract: ClassInitializationContract {
            version: 1,
            schema_revision: schema.revision.clone(),
            class: "Item".into(),
            class_identity: schema.identity("Item"),
            provider: ProviderId {
                name: "test<&".into(),
                version: "1".into(),
            },
            sources: vec![source.id],
            members: vec![schema.effective_feature("Item", "title").unwrap().identity],
        },
    };
    let mut element = Element::new(&schema, "a", "Item", "example.xmi", 0, 0).unwrap();
    element.values.insert("name".into(), json!("a"));
    element.initialization = Some(receipt.clone());
    let mut model = Model {
        schema_revision: schema.revision.clone(),
        resources: Default::default(),
        elements: vec![element],
    };
    model.resources =
        intersect_model::core::resources::records_for_elements(&schema, &model.elements).unwrap();
    let resources = XmiFormat.encode(&schema, &model).unwrap();
    assert_eq!(XmiFormat.decode(&schema, &resources).unwrap(), model);
    let mut forged = model.clone();
    forged.elements[0].initialization.as_mut().unwrap().object = "foreign".into();
    assert!(XmiFormat.encode(&schema, &forged).is_err());
    let mut malformed = resources;
    let text = String::from_utf8(malformed[0].bytes.clone()).unwrap();
    malformed[0].bytes = text
        .replace(
            "&quot;object&quot;:&quot;a&quot;",
            "&quot;object&quot;:&quot;foreign&quot;",
        )
        .into_bytes();
    assert!(XmiFormat.decode(&schema, &malformed).is_err());
}
