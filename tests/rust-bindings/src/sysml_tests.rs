use super::sysml::*;
use intersect_model::{
    core::frozen::{ById, FeatureId, Metamodel, Model, ModelBuilder, ObjId, ValueRef},
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle_with_limits,
        ecore::ParseLimits,
        rcore::feature_redefinitions,
    },
};
use std::sync::Arc;

struct DerivedName {
    receiver: ObjId,
    derived_name: FeatureId,
    declared_name: FeatureId,
}

impl DerivedValues for DerivedName {
    fn read(
        &self,
        model: &Model,
        receiver: ObjId,
        feature: FeatureId,
    ) -> Result<serde_json::Value, String> {
        assert_eq!(receiver, self.receiver);
        assert_eq!(feature, self.derived_name);
        let object = model.obj(receiver).ok_or("Missing receiver")?;
        Ok(serde_json::json!(object.str(self.declared_name)))
    }
}

fn check_declaration_coverage(schema: &intersect_model::definitions::schema::Schema) {
    let generated = include_str!(concat!(env!("OUT_DIR"), "/sysml.rs"));
    for class in schema.classes.values() {
        let (_, implementation) = generated
            .split_once(&format!("impl<'m> {}<'m> {{", class.name))
            .unwrap_or_else(|| panic!("No generated implementation for {}", class.name));
        let (implementation, _) = implementation
            .split_once("\n}\n")
            .unwrap_or_else(|| panic!("Unclosed implementation for {}", class.name));
        for feature in class.features.values() {
            assert!(
                implementation
                    .contains(&format!(".feature({:?}, {:?})?", class.name, feature.name)),
                "No generated accessor for {}.{}",
                class.name,
                feature.name,
            );
        }
        assert_eq!(
            implementation.matches("pub fn operation_").count(),
            class.operations.len(),
            "Operation coverage for {}",
            class.name,
        );
    }
    for enumeration in schema.enumerations.values() {
        let (_, implementation) = generated
            .split_once(&format!("impl {} {{", enumeration.name()))
            .unwrap_or_else(|| {
                panic!(
                    "No generated enum implementation for {}",
                    enumeration.name()
                )
            });
        let (implementation, _) = implementation
            .split_once("\n}\n")
            .unwrap_or_else(|| panic!("Unclosed enum implementation for {}", enumeration.name()));
        for literal in enumeration.literals() {
            assert!(
                implementation.contains(&format!("{:?} => Ok(Self::", literal.name)),
                "No generated enum variant for {}.{}",
                enumeration.name(),
                literal.name,
            );
        }
    }
}

#[test]
fn full_sysml_metamodel_generates_and_supports_typed_reads() -> Result<(), String> {
    let source = DefinitionSource::new(
        "org.omg.sysml/model/SysML.ecore.rcore",
        SourceKind::Rcore,
        include_str!(concat!(env!("OUT_DIR"), "/SysML.ecore.rcore")),
    );
    let limits = ParseLimits {
        source_bytes: 2_000_000,
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle_with_limits(&bundle, &source.id, SCHEMA_URI, limits)
        .map_err(|error| error.to_string())?;
    assert_eq!(schema.revision, SCHEMA_REVISION);
    assert_eq!(schema.classes.len(), 175);
    assert_eq!(schema.enumerations.len(), 7);
    assert_eq!(
        schema
            .classes
            .values()
            .map(|class| class.features.len())
            .sum::<usize>(),
        415
    );
    assert_eq!(
        schema
            .classes
            .values()
            .map(|class| class.operations.len())
            .sum::<usize>(),
        70
    );
    check_declaration_coverage(&schema);
    let redefinitions =
        feature_redefinitions(&source, limits).map_err(|error| error.to_string())?;
    let without_redefinitions = ModelBuilder::new(Arc::new(
        Metamodel::freeze(&schema).map_err(|error| error.to_string())?,
    ))
    .link(&ById)
    .and_then(|linked| linked.validate())
    .map_err(|error| format!("{error:?}"))?;
    assert!(ModelView::bind(&without_redefinitions).is_err());
    let metamodel = Metamodel::freeze(&schema)
        .and_then(|metamodel| metamodel.with_redefinitions(&redefinitions))
        .map_err(|error| error.to_string())?;
    let metamodel = Arc::new(metamodel);
    let class = |name| {
        metamodel
            .class(name)
            .ok_or_else(|| format!("Missing class {name}"))
    };
    let feature = |owner, name| {
        metamodel
            .feature(class(owner)?, name)
            .ok_or_else(|| format!("Missing feature {owner}.{name}"))
    };
    let mut builder = ModelBuilder::new(metamodel.clone());
    let package = builder
        .create("pkg", class("Package")?, "test.sysml")
        .map_err(|e| e.to_string())?;
    let action = builder
        .create("action", class("ActionDefinition")?, "test.sysml")
        .map_err(|e| e.to_string())?;
    let membership = builder
        .create("membership", class("Membership")?, "test.sysml")
        .map_err(|e| e.to_string())?;
    let requirement = builder
        .create("requirement", class("RequirementDefinition")?, "test.sysml")
        .map_err(|e| e.to_string())?;
    let element_id = feature("Element", "elementId")?;
    for (object, identity) in [
        (package, "pkg"),
        (action, "action"),
        (membership, "membership"),
        (requirement, "requirement"),
    ] {
        builder
            .set_str(object, element_id, identity)
            .map_err(|error| error.to_string())?;
    }
    builder
        .set_str(package, feature("Element", "declaredName")?, "ThePackage")
        .map_err(|e| e.to_string())?;
    builder
        .set_str(
            requirement,
            feature("RequirementDefinition", "reqId")?,
            "R1",
        )
        .map_err(|e| e.to_string())?;
    builder
        .add_ref(
            package,
            feature("Element", "ownedRelationship")?,
            membership,
        )
        .map_err(|e| e.to_string())?;
    builder
        .add_ref(membership, feature("Membership", "memberElement")?, action)
        .map_err(|e| e.to_string())?;
    let model = builder
        .link(&ById)
        .and_then(|linked| linked.validate())
        .map_err(|e| format!("{e:?}"))?;
    let view = ModelView::bind(&model)?;
    let package = Package::bind(view, package).ok_or("Expected Package")?;
    let operation: OperationContract<'_, (), Option<String>> =
        package.as_element().operation_effective_name()?;
    let operation = operation.declaration();
    assert_eq!(operation, &schema.classes["Element"].operations[0]);
    assert!(operation.parameters.is_empty());
    assert_eq!(operation.result.lower, 0);
    assert_eq!(operation.result.upper, 1);
    let operation: OperationContract<'_, (Element<'_>,), Vec<String>> =
        package.as_namespace().operation_names_of()?;
    assert_eq!(operation.declaration().parameters[0].name, "element");
    let name_feature = feature("Element", "declaredName")?;
    assert_eq!(
        package.declared_name()?.as_deref(),
        package.object().str(name_feature)
    );
    assert_eq!(package.declared_name()?.as_deref(), Some("ThePackage"));
    let provider = DerivedName {
        receiver: package.object().id(),
        derived_name: feature("Element", "name")?,
        declared_name: name_feature,
    };
    assert_eq!(package.name(&provider)?, package.declared_name()?);
    let relationship = package.owned_relationship()?;
    assert_eq!(relationship.len(), 1);
    let membership =
        Membership::bind(view, relationship[0].object().id()).ok_or("Expected Membership")?;
    assert_eq!(membership.member_element()?.object().id(), action);
    let action = ActionDefinition::bind(view, action).ok_or("Expected ActionDefinition")?;
    assert_eq!(
        action.as_behavior().object().id(),
        action.as_occurrence_definition().object().id()
    );
    assert!(PartUsage::bind(view, action.object().id()).is_none());
    let requirement =
        RequirementDefinition::bind(view, requirement).ok_or("Expected RequirementDefinition")?;
    assert_eq!(requirement.req_id()?.as_deref(), Some("R1"));
    assert_eq!(
        requirement.as_element().declared_short_name()?.as_deref(),
        Some("R1")
    );
    let visibility = membership
        .object()
        .get(feature("Membership", "visibility")?);
    assert!(matches!(visibility, Some(ValueRef::Default(_))));
    assert_eq!(membership.visibility()?, VisibilityKind::Public);
    Ok(())
}
