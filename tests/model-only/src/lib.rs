//! External consumer of the cohesive Rcore API, without Xtext or generated Ecore.
//!
//! Application entry points remain available to an independent crate:
//! ```
//! use intersect_model::{compile_rcore, dynamic::DynamicModel, frozen::ArenaStore};
//! ```
//! Host contracts have an explicit advanced entry point:
//! ```
//! use intersect_model::extensions::{SourceReadCallbacks, LOGICAL_WORD};
//! assert_eq!(LOGICAL_WORD, 8);
//! ```
//! Implementation modules and partial validation helpers are private:
//! ```compile_fail
//! use intersect_model::source_read_callbacks::SourceReadCallbacks;
//! ```
//! ```compile_fail
//! use intersect_model::logical_size::LOGICAL_WORD;
//! ```
//! ```compile_fail
//! use intersect_model::storage::validate_draft_with_conversions;
//! ```
//! ```compile_fail
//! use intersect_model::package_load::validate_lock_resources;
//! ```
//! ```compile_fail
//! use intersect_model::definitions::rcore_behavior::ir::unary_type;
//! ```
//! Execution optimization and JSON traversal are not application contracts:
//! ```compile_fail
//! use intersect_model::rcore_fast::FastProgram;
//! ```
//! ```compile_fail
//! use intersect_model::definitions::json_exact::matches_with_work;
//! ```
//! ```compile_fail
//! use intersect_model::definitions::retained_json::Cursor;
//! ```
//! Resource mutation must use checked editing or the store adapter contract:
//! ```compile_fail
//! use intersect_model::resources::change_root;
//! ```
//! Arena slot selection is an implementation detail:
//! ```compile_fail
//! fn raw_slot(mm: &intersect_model::frozen::Metamodel,
//!     class: intersect_model::frozen::ClassId,
//!     feature: intersect_model::frozen::FeatureId) {
//!     let _ = mm.read_slot(class, feature);
//! }
//! ```
//! Serialized data cannot construct a checked executable package:
//! ```compile_fail
//! let _ = serde_json::from_str::<intersect_model::rcore::RcorePackage>("{}");
//! ```
use intersect_model::{
    api::ModelEdit,
    compile_rcore_at,
    rcore::{DataWire, ExecutionErrorKind, ExecutionLimits, RcoreError, RcorePackage},
};
fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn diagnostic(error: impl std::fmt::Display) -> String {
    error.to_string()
}
/// Shared native/WASM lifecycle with independent expected results and failures.
pub fn verify_boundary() -> Result<(), String> {
    verify_dynamic_wire()?;
    let source = include_str!("../../../fixtures/rcore/api/organization.rcore");
    let mut package = compile_rcore_at("memory:organization.rcore", source).map_err(diagnostic)?;
    require(
        package.sources().sources.len() == 1,
        "Unexpected generated definition source",
    )?;
    require(
        package
            .schema()
            .declarations
            .values()
            .all(|d| d.source == *package.entry()),
        "Lost authored source identity",
    )?;
    let mut model = package.new_model().map_err(diagnostic)?;
    {
        use intersect_model::definitions::{
            bundle::{DefinitionBundle, DefinitionSource, SourceKind},
            declarations::DeclarationCollection,
            ecore::ParseLimits,
            model_definition::{
                CompiledStructure, ModelPreparationMode, PhysicalInputCapture, StructureSelection,
            },
        };
        let source = DefinitionSource::new(
            "memory:owned-model.rcore",
            SourceKind::Rcore,
            "rcore 1;package owned version \"1\";metamodel \"urn:owned-model\" prefix o {class Leaf {}}",
        );
        let entry = source.id.clone();
        let sources = std::sync::Arc::new(DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        });
        let compiled = CompiledStructure::prepare(
            StructureSelection {
                bundle: &sources,
                entry: &entry,
                namespace: "urn:owned-model",
                materialized_types: &[],
            },
            DeclarationCollection::Classifiers,
            PhysicalInputCapture::Retain,
            None,
            ParseLimits::default(),
        )
        .map_err(diagnostic)?;
        let checked = compiled
            .check_behavior(&entry, None, ParseLimits::default())
            .map_err(|errors| format!("{errors:?}"))?;
        let compiled = checked
            .finish(
                ParseLimits::default(),
                false,
                ModelPreparationMode::Strict,
                &[],
            )
            .map_err(|errors| format!("{errors:?}"))?;
        let owned = compiled.into_owned(std::sync::Arc::clone(&sources))?;
        require(
            std::ptr::eq(owned.sources(), sources.as_ref()),
            "Owned model copied its source graph",
        )?;
        require(
            owned.has_physical_inputs(),
            "Owned model lost completed structural data",
        )?;
        let prepared = owned.prepared_definition();
        require(
            std::ptr::eq(prepared.schema(), owned.schema()),
            "Owned preparation changed schema allocation",
        )?;
        let observation = intersect_model::definitions::conversion::InvocationContext::default();
        let physical = prepared
            .prepare_physical_metadata(&observation)
            .map_err(diagnostic)?;
        require(
            std::ptr::eq(physical.definition(), &prepared),
            "Owned physical metadata detached from its source owner",
        )?;
        let mut structural = intersect_model::api::BasicModel::new(owned.schema().clone())?;
        structural.create("leaf", "Leaf", "memory:owned")?;
    }
    {
        let prepared = package.prepared_definition();
        require(
            std::ptr::eq(prepared.schema(), package.schema()),
            "Prepared model copied its schema",
        )?;
        require(
            std::ptr::eq(prepared.sources(), package.sources()),
            "Prepared model copied its source graph",
        )?;
        require(
            prepared.structure_selection().entry == package.entry(),
            "Prepared model lost its entry source",
        )?;
        require(
            prepared.behavior_source() == package.entry(),
            "Prepared model lost its behavior source",
        )?;
        require(
            prepared.migration().is_none(),
            "Authored Rcore claimed migration provenance",
        )?;
        let observation = intersect_model::definitions::conversion::InvocationContext::default();
        let metadata = prepared
            .prepare_physical_metadata(&observation)
            .map_err(diagnostic)?;
        require(
            std::ptr::eq(metadata.definition(), &prepared),
            "Physical metadata detached from its prepared model",
        )?;
        require(
            prepared.requirements().next().transpose()?.is_some(),
            "Prepared model lost requirement inspection",
        )?;
    }
    let declarations = intersect_model::definitions::compiler::inspect_declarations_with_limits(
        package.sources(),
        package.entry(),
        "urn:intersect:rcore:organization",
        true,
        &intersect_model::definitions::conversion::InvocationContext::default(),
        intersect_model::definitions::ecore::ParseLimits::default(),
    )
    .map_err(diagnostic)?;
    let node = declarations
        .classifiers()
        .get("urn:intersect:rcore:organization")
        .and_then(|classifiers| classifiers.get("Node"))
        .ok_or("Missing exact inspected Node declaration")?;
    require(
        node.source == *package.entry(),
        "Declaration inventory lost authored identity",
    )?;
    {
        use intersect_model::definitions::{
            declarations::DeclarationCollection,
            ecore::ParseLimits,
            model_definition::{
                CompiledStructure, PhysicalInputCapture, StructureContext, StructureSelection,
            },
        };
        let mut structure = CompiledStructure::prepare(
            StructureSelection {
                bundle: package.sources(),
                entry: package.entry(),
                namespace: "urn:intersect:rcore:organization",
                materialized_types: &[],
            },
            DeclarationCollection::Classifiers,
            PhysicalInputCapture::Retain,
            None,
            ParseLimits::default(),
        )
        .map_err(diagnostic)?;
        require(
            structure.has_physical_inputs(),
            "Missing captured structural data",
        )?;
        require(
            std::ptr::eq(structure.sources(), package.sources()),
            "Structural compilation copied its source owner",
        )?;
        let declarations = structure.take_declarations();
        let node = declarations
            .classifiers()
            .get("urn:intersect:rcore:organization")
            .and_then(|classifiers| classifiers.get("Node"))
            .ok_or("Missing compiled Node declaration")?;
        require(
            node.source == *package.entry() && node.path == [0],
            "Compiled structure changed Node source identity",
        )?;
        require(
            structure.schema().features("Node")?["links"].reference,
            "Compiled structure lost reference metadata",
        )?;
        let revision = structure.schema().revision.clone();
        {
            let mut context = structure.reference_context();
            context.enable_eobject_references()?;
            require(
                context.reference_top.is_some(),
                "Missing canonical reference context",
            )?;
        }
        require(
            structure.schema().reference_top.is_none() && structure.schema().revision == revision,
            "Unfinished reference-context transaction changed checked metadata",
        )?;
        {
            let mut context = structure.reference_context();
            context.enable_eobject_references()?;
            context.commit();
        }
        require(
            structure.schema().reference_top.is_some() && structure.schema().revision != revision,
            "Committed reference-context transaction lost canonical metadata",
        )?;
        let mut structural_model = intersect_model::api::BasicModel::new(structure.into_schema())?;
        structural_model.create("structural", "Node", "memory:structural")?;
        require(
            intersect_model::core::storage::ModelRead::value(
                &structural_model,
                "structural",
                "name",
            )? == Some(serde_json::json!("unnamed")),
            "Owned structural metadata lost its independent default",
        )?;
    }
    for id in ["A", "B", "C"] {
        model.create(id, "Node", "memory:model")?;
    }
    model.set("A", "links", serde_json::json!(["B"]))?;
    model.set("B", "links", serde_json::json!(["C"]))?;
    model.create_resource("memory:empty")?;
    require(
        model.create_resource("memory:empty").is_err(),
        "Duplicate resource URI admitted",
    )?;
    model.reorder_roots("memory:model", &["C".into(), "B".into(), "A".into()])?;
    let resource_state = intersect_model::storage::export(&*model)?;
    require(
        resource_state.resources["memory:empty"].roots.is_empty(),
        "Empty resource missing",
    )?;
    require(
        resource_state.resources["memory:model"].roots.as_slice() == ["C", "B", "A"],
        "Root order missing",
    )?;
    require(
        model
            .reorder_roots("memory:model", &["A".into(), "A".into()])
            .is_err(),
        "Invalid root order admitted",
    )?;
    require(
        intersect_model::storage::export(&*model)? == resource_state,
        "Failed root edit changed state",
    )?;
    {
        let view = intersect_model::resources::resource_view(&*model, "memory:model")?
            .ok_or("Missing resource view")?;
        require(
            view.roots() == ["C", "B", "A"],
            "Resource view lost ordered roots",
        )?;
        require(
            view.members()? == ["A", "B", "C"],
            "Resource view changed member identity order",
        )?;
    }
    require(
        model.create("A", "Node", "memory:empty").is_err(),
        "Resource-local duplicate global ID admitted",
    )?;
    require(
        intersect_model::storage::export(&*model)? == resource_state,
        "Rejected identity collision changed resource state",
    )?;
    model.validate()?;
    require(
        matches!(
            package.invoke(&model, "A", "reachable", &[], &ExecutionLimits::default()),
            Err(RcoreError::MissingImplementation { .. })
        ),
        "Undeclared implementation executed",
    )?;
    package
        .bind_operation("Node", "reachable", "reachable")
        .map_err(diagnostic)?;
    require(
        package
            .bind_operation("Node", "reachable", "reachable")
            .is_err(),
        "Duplicate implementation admitted",
    )?;
    let baseline = package.save(&model).map_err(diagnostic)?;
    let envelope: serde_json::Value = serde_json::from_slice(&baseline).map_err(diagnostic)?;
    require(envelope["version"] == 2, "Rcore envelope version changed")?;
    for version in [0, 1, 3, u32::MAX] {
        let mut future = envelope.clone();
        future["version"] = version.into();
        require(
            RcorePackage::reload(&serde_json::to_vec(&future).map_err(diagnostic)?).is_err(),
            "Unsupported Rcore envelope version admitted",
        )?;
    }
    let mut unknown = envelope.clone();
    unknown["executable"] = serde_json::json!({"trusted": true});
    require(
        RcorePackage::reload(&serde_json::to_vec(&unknown).map_err(diagnostic)?).is_err(),
        "Unknown Rcore envelope field admitted",
    )?;
    let text = std::str::from_utf8(&baseline).map_err(diagnostic)?;
    let duplicate = format!("{{\"version\":2,{}", &text[1..]);
    require(
        RcorePackage::reload(duplicate.as_bytes()).is_err(),
        "Duplicate Rcore version admitted",
    )?;
    require(
        model.set("A", "links", serde_json::json!(42)).is_err(),
        "Invalid reference edit succeeded",
    )?;
    require(
        package.save(&model).map_err(diagnostic)? == baseline,
        "Rejected edit changed model",
    )?;
    let evaluate = |package: &RcorePackage,
                    model: &intersect_model::api::BasicModel|
     -> Result<Vec<String>, String> {
        let result = package
            .invoke(model, "A", "reachable", &[], &ExecutionLimits::default())
            .map_err(diagnostic)?;
        let DataWire::Collection(values) = result.value.data else {
            return Err("Expected collection".into());
        };
        values
            .into_iter()
            .map(|v| match v.data {
                DataWire::Model(v) => v["element"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "Missing object identity".into()),
                _ => Err("Expected model reference".into()),
            })
            .collect()
    };
    require(
        evaluate(&package, &model)? == ["A", "B", "C"],
        "Incorrect reachability result",
    )?;
    require(
        package
            .invoke(
                &model,
                "missing",
                "reachable",
                &[],
                &ExecutionLimits::default(),
            )
            .is_err(),
        "Unknown receiver admitted",
    )?;
    require(
        matches!(package.invoke(&model, "A", "reachable", &[], &ExecutionLimits { max_provider_work: 0, ..Default::default() }), Err(RcoreError::Execution(e)) if e.kind == ExecutionErrorKind::ResourceExhausted),
        "Budget failure was not reported",
    )?;
    let (reloaded, restored) = RcorePackage::reload(&baseline).map_err(diagnostic)?;
    require(
        evaluate(&reloaded, &restored)? == ["A", "B", "C"],
        "Reload/recovery changed behavior",
    )?;
    require(
        reloaded.save(&restored).map_err(diagnostic)? == baseline,
        "Reload changed exact sources/model",
    )?;
    let mut corrupt: serde_json::Value = serde_json::from_slice(&baseline).map_err(diagnostic)?;
    corrupt["bundle"]["sources"][0]["text"] = serde_json::json!("tampered source");
    require(
        RcorePackage::reload(&serde_json::to_vec(&corrupt).map_err(diagnostic)?).is_err(),
        "Corrupt source identity admitted",
    )?;
    require(
        compile_rcore_at(
            "memory:invalid.rcore",
            &source.replace("n.links", "n.missing"),
        )
        .is_err(),
        "Invalid behavior compiled",
    )?;
    Ok(())
}

fn verify_dynamic_wire() -> Result<(), String> {
    use intersect_model::{
        definitions::dynamic::{ScalarType, SchemaBuilder},
        dynamic::DynamicModel,
    };
    let mut builder = SchemaBuilder::new("wire", "urn:wire");
    let item = builder.class("Item")?;
    let name = item.attribute("name", ScalarType::String).required()?;
    let mut model = DynamicModel::new(builder.build()?)?;
    model.transaction(|tx| {
        let mut resource = tx.create_resource("memory:wire")?;
        let object = resource.create(&item, "item")?;
        resource.set(&object, &name, "Unicode 😀\r\ntext")?;
        Ok(())
    })?;
    let saved = model.to_json()?;
    require(
        DynamicModel::from_json(&saved)?.to_json()? == saved,
        "Dynamic snapshot round trip changed data",
    )?;
    let envelope: serde_json::Value = serde_json::from_str(&saved).map_err(diagnostic)?;
    require(envelope["version"] == 1, "Dynamic envelope version changed")?;
    for version in [0, 2, u32::MAX] {
        let mut future = envelope.clone();
        future["version"] = version.into();
        require(
            DynamicModel::from_json(&future.to_string()).is_err(),
            "Unsupported dynamic version admitted",
        )?;
    }
    let mut unknown = envelope;
    unknown["executable"] = true.into();
    require(
        DynamicModel::from_json(&unknown.to_string()).is_err(),
        "Unknown dynamic envelope field admitted",
    )?;
    let duplicate = format!("{{\"version\":1,{}", &saved[1..]);
    require(
        DynamicModel::from_json(&duplicate).is_err(),
        "Duplicate dynamic version admitted",
    )?;
    Ok(())
}
#[path = "../../../fixtures/rcore-model-rule-service/classified_scan.rs"]
mod classified_scan;
#[path = "../../../fixtures/rcore-getter-service/composition.rs"]
mod composed_getters;
#[path = "../../../fixtures/rcore-getter-service/conjugated_name.rs"]
mod conjugated_name;
#[path = "../../../fixtures/rcore-getter-service/direct.rs"]
mod direct_getters;
#[path = "../../../fixtures/rcore-model-rule-service/initializers.rs"]
mod direct_initializers;
#[path = "../../../fixtures/rcore-model-rule-service/direct.rs"]
mod direct_rules;
#[path = "../../../fixtures/rcore-getter-service/feature_naming.rs"]
mod feature_naming;
#[path = "../../../fixtures/rcore-model-rule-service/read_function.rs"]
mod read_function;
#[path = "../../../fixtures/rcore-getter-service/references.rs"]
mod reference_getters;
#[path = "../../../fixtures/rcore-getter-service/type_views.rs"]
mod type_views;
#[unsafe(no_mangle)]
pub extern "C" fn verify_model_boundary() -> u32 {
    u32::from(
        verify_boundary()
            .and_then(|()| direct_rules::verify())
            .and_then(|()| direct_initializers::verify())
            .and_then(|()| direct_getters::verify())
            .and_then(|()| conjugated_name::verify())
            .and_then(|()| feature_naming::verify())
            .and_then(|()| reference_getters::verify())
            .and_then(|()| composed_getters::verify())
            .and_then(|()| type_views::verify())
            .and_then(|()| read_function::verify())
            .and_then(|()| classified_scan::verify())
            .is_err(),
    )
}
#[cfg(test)]
mod tests {
    #[test]
    fn model_and_rcore_execute_without_xtext() -> Result<(), String> {
        super::verify_boundary()?;
        super::direct_rules::verify()?;
        super::direct_initializers::verify()?;
        super::direct_getters::verify()?;
        super::conjugated_name::verify()?;
        super::feature_naming::verify()?;
        super::reference_getters::verify()?;
        super::composed_getters::verify()?;
        super::type_views::verify()?;
        super::read_function::verify()?;
        super::classified_scan::verify()
    }
}
