//! Handwritten source-correspondence checks for neutral model preparation.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    declarations::DeclarationCollection,
    ecore::ParseLimits,
    model_definition::{
        CheckedStructure, CompiledStructure, PhysicalInputCapture, StructureSelection,
    },
};
use std::{cell::Cell, rc::Rc};

fn structural_inputs() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:compiled-structure.rcore",
            SourceKind::Rcore,
            "rcore 1;package model version \"1\";metamodel \"urn:compiled-structure\" prefix m {class Base {attr name:String [0,1];}class Child extends Base {}}",
        )],
        namespaces: vec![],
    }
}

fn structural_selection(bundle: &DefinitionBundle) -> StructureSelection<'_> {
    StructureSelection {
        bundle,
        entry: &bundle.sources[0].id,
        namespace: "urn:compiled-structure",
        materialized_types: &[],
    }
}

fn compilation_inputs(behavior: &str) -> std::sync::Arc<DefinitionBundle> {
    let structure = DefinitionSource::new(
        "memory:model-owner.rcore",
        SourceKind::Rcore,
        "rcore 1;package owned version \"1\";metamodel \"urn:model-owner\" prefix p {class Node {attr values:Int64 [0,*];attr count:Int64 [0,1] derived;}}",
    );
    let mut behavior = DefinitionSource::new(
        "memory:model-owner-behavior.rcore",
        SourceKind::RcoreBehavior,
        behavior,
    );
    behavior.dependencies = vec![structure.id.clone()];
    std::sync::Arc::new(DefinitionBundle {
        version: 1,
        sources: vec![structure, behavior],
        namespaces: vec![],
    })
}
const OWNER_GOOD: &str = "rcore 1;package rules version \"1\";import model \"urn:model-owner\" as p;derive p.Node.count using count(values);";
fn compilation_structure(bundle: &DefinitionBundle) -> Result<CompiledStructure<'_>, String> {
    CompiledStructure::prepare(
        StructureSelection {
            bundle,
            entry: &bundle.sources[0].id,
            namespace: "urn:model-owner",
            materialized_types: &[],
        },
        DeclarationCollection::Classifiers,
        PhysicalInputCapture::Retain,
        None,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())
}

#[test]
fn checked_model_output_owns_exact_sources_and_keeps_schema_stable_through_moves()
-> Result<(), Box<dyn std::error::Error>> {
    use intersect_model::{
        api::{BasicModel, ModelEdit},
        core::query::{DerivedValues, QueryBudget},
        definitions::{
            model_definition::{ModelPreparationMode, StructureContext},
            preparation::SemanticRequirement,
        },
        query::ModelDerived,
    };
    use std::sync::Arc;
    let bundle = compilation_inputs(OWNER_GOOD);
    let source_address = std::ptr::from_ref(bundle.as_ref());
    let mut checked = compilation_structure(&bundle)?
        .check_behavior(&bundle.sources[1].id, None, ParseLimits::default())
        .map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(checked.behavior_source(), &bundle.sources[1].id);
    let original = checked.schema().clone();
    {
        let mut reference_context = checked.reference_context();
        reference_context.enable_eobject_references()?;
    }
    assert_eq!(checked.schema(), &original);
    {
        let mut reference_context = checked.reference_context();
        reference_context.enable_eobject_references()?;
        reference_context.commit();
    }
    let model = checked
        .finish(
            ParseLimits::default(),
            false,
            ModelPreparationMode::Strict,
            &[],
        )
        .map_err(|errors| format!("{errors:?}"))?;
    assert!(model.has_physical_inputs());
    assert_eq!(model.semantics().derived.len(), 1);
    assert_eq!(model.semantics().derived[0].feature, "count");
    let owned = model.into_owned(Arc::clone(&bundle))?;
    let schema_address = std::ptr::from_ref(owned.schema());
    let owner = Arc::new(owned);
    assert!(std::ptr::eq(schema_address, owner.schema()));
    assert!(std::ptr::eq(source_address, owner.sources()));
    assert!(owner.has_physical_inputs());
    drop(bundle);
    let prepared = owner.prepared_definition();
    assert!(std::ptr::eq(prepared.schema(), owner.schema()));
    assert!(std::ptr::eq(prepared.sources(), owner.sources()));
    assert_eq!(prepared.behavior_source(), owner.behavior_source());
    assert!(prepared.requirements().any(|row| matches!(
        row,
        Ok(SemanticRequirement::ClassBehaviorUnverified {
            receiver: "Node",
            ..
        })
    )));
    let observation = InvocationContext::new(StructureGrant::new(2_000_000, 64_000_000));
    let physical = prepared.prepare_physical_metadata(&observation)?;
    assert!(std::ptr::eq(physical.definition(), &prepared));
    let mut store = BasicModel::new(owner.schema().clone())?;
    store.create("node", "Node", "memory:model")?;
    store.set(
        "node",
        "values",
        serde_json::json!([{"kind":"int64","value":"2"},{"kind":"int64","value":"3"}]),
    )?;
    assert_eq!(
        ModelDerived(owner.model()).read(&store, "node", "count", &mut QueryBudget::new(1000))?,
        serde_json::json!({"kind":"int64","value":"2"})
    );
    Ok(())
}

#[test]
fn checked_model_output_rejects_equal_foreign_source_allocations_and_stale_checks()
-> Result<(), String> {
    use intersect_model::definitions::model_definition::ModelPreparationMode;
    use std::sync::Arc;
    let bundle = compilation_inputs(OWNER_GOOD);
    let finish = || {
        compilation_structure(&bundle)?
            .check_behavior(&bundle.sources[1].id, None, ParseLimits::default())
            .map_err(|errors| format!("{errors:?}"))?
            .finish(
                ParseLimits::default(),
                false,
                ModelPreparationMode::Strict,
                &[],
            )
            .map_err(|errors| format!("{errors:?}"))
    };
    let equal_copy = Arc::new(bundle.as_ref().clone());
    assert_eq!(bundle.as_ref(), equal_copy.as_ref());
    assert!(finish()?.into_owned(equal_copy).is_err());
    let mut stale = bundle.as_ref().clone();
    stale.sources[1].text.push(' ');
    assert!(compilation_structure(&stale).is_err());
    let foreign =
        intersect_model::definitions::bundle::SourceId::for_text("memory:foreign", OWNER_GOOD);
    assert!(
        compilation_structure(&bundle)?
            .check_behavior(&foreign, None, ParseLimits::default())
            .is_err()
    );
    assert!(finish()?.into_owned(Arc::clone(&bundle)).is_ok());
    Ok(())
}

#[test]
fn checked_model_output_retains_missing_requirements_without_ignoring_invalid_behavior()
-> Result<(), String> {
    use intersect_model::definitions::model_definition::ModelPreparationMode;
    let empty = compilation_inputs("rcore 1;package rules version \"1\";");
    let finish = |mode| {
        compilation_structure(&empty)?
            .check_behavior(&empty.sources[1].id, None, ParseLimits::default())
            .map_err(|errors| format!("{errors:?}"))?
            .finish(ParseLimits::default(), false, mode, &[])
            .map_err(|errors| format!("{errors:?}"))
    };
    assert!(finish(ModelPreparationMode::Strict).is_err());
    let inspected = finish(ModelPreparationMode::Inspection)?;
    assert!(inspected.semantics().derived.is_empty());
    assert!(inspected.schema().features("Node")?["count"].derived);
    let metadata = inspected.into_metadata();
    assert!(metadata.schema.features("Node")?["count"].derived);
    assert!(metadata.semantics.derived.is_empty());
    let duplicate = compilation_inputs(&format!(
        "{OWNER_GOOD}derive p.Node.count using count(values);"
    ));
    let result = compilation_structure(&duplicate)?
        .check_behavior(&duplicate.sources[1].id, None, ParseLimits::default())
        .and_then(|checked| {
            checked.finish(
                ParseLimits::default(),
                false,
                ModelPreparationMode::Inspection,
                &[],
            )
        });
    assert!(result.is_err());
    Ok(())
}

#[test]
fn checked_model_output_keeps_independent_check_and_lower_source_limits() -> Result<(), String> {
    use intersect_model::definitions::model_definition::ModelPreparationMode;
    // The lowering parser's structural-byte cap applies to unified Rcore;
    // legacy RcoreBehavior size admission remains the worker's responsibility.
    let inputs = compilation_inputs(OWNER_GOOD);
    let source = DefinitionSource::new(
        "memory:unified-model-owner.rcore",
        SourceKind::Rcore,
        format!(
            "{}behavior {{import model \"urn:model-owner\" as p;derive p.Node.count using count(values);}}",
            inputs.sources[0].text
        ),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let short = ParseLimits { source_bytes: 1 };
    assert!(
        compilation_structure(&bundle)?
            .check_behavior(&bundle.sources[0].id, None, short)
            .is_err()
    );
    assert!(
        compilation_structure(&bundle)?
            .check_behavior(&bundle.sources[0].id, None, ParseLimits::default())
            .map_err(|errors| format!("{errors:?}"))?
            .finish(short, false, ModelPreparationMode::Strict, &[])
            .is_err()
    );
    let recovered = compilation_structure(&bundle)?
        .check_behavior(&bundle.sources[0].id, None, ParseLimits::default())
        .map_err(|errors| format!("{errors:?}"))?
        .finish(
            ParseLimits::default(),
            false,
            ModelPreparationMode::Strict,
            &[],
        )
        .map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(recovered.semantics().derived[0].feature, "count");
    Ok(())
}

#[test]
fn checked_rcore_package_prepares_model_metadata_and_reads_without_text()
-> Result<(), Box<dyn std::error::Error>> {
    use intersect_model::{
        api::{BasicModel, ModelEdit},
        core::{
            query::QueryBudget,
            storage::{PhysicalPersistenceLimits, RetainedModelWire},
        },
        definitions::{
            bundle::SourceId,
            preparation::{PreparedModelDefinition, SemanticRequirement},
        },
        semantic_reads::{
            ReadDependencies, ReadMember, ReadPlan, SemanticReadPolicy, SemanticReadSession,
        },
    };
    use serde_json::{Value, json};
    struct StoredPolicy(SourceId);
    impl SemanticReadPolicy for StoredPolicy {
        fn validate(&self, definition: &PreparedModelDefinition<'_>) -> Result<(), String> {
            if definition.structure_selection().entry != &self.0 {
                return Err("Model read source mismatch".into());
            }
            Ok(())
        }
        fn plan(&self, _: ReadMember<'_>) -> ReadPlan {
            ReadPlan::Stored
        }
        fn dependencies(&self, _: ReadMember<'_>) -> &[&str] {
            &[]
        }
        fn read(
            &self,
            _: ReadMember<'_>,
            _: &dyn ReadDependencies,
            _: &mut QueryBudget,
        ) -> Result<Value, String> {
            Err("Stored policy must not execute a provider".into())
        }
    }
    let source = "rcore 1;package prepared version \"1\";metamodel \"urn:model-prepared\" prefix p {class Node {attr name:String [0,1];}}";
    let package = intersect_model::compile_rcore_at("memory:model-prepared.rcore", source)?;
    let prepared = package.prepared_definition();
    assert!(std::ptr::eq(prepared.schema(), package.schema()));
    assert!(std::ptr::eq(prepared.sources(), package.sources()));
    assert!(std::ptr::eq(prepared.original_sources(), package.sources()));
    assert_eq!(prepared.structure_selection().entry, package.entry());
    assert_eq!(prepared.behavior_source(), package.entry());
    assert!(prepared.migration().is_none());
    let requirements = prepared.requirements().collect::<Result<Vec<_>, _>>()?;
    let receiver = requirements
        .iter()
        .find_map(|row| match row {
            SemanticRequirement::ClassBehaviorUnverified {
                receiver,
                converted: Some(id),
                declared_operations: 0,
                declared_initializers: 0,
                ..
            } if id.source == *package.entry() && id.path == [0] => Some(*receiver),
            _ => None,
        })
        .ok_or("Missing exact authored Node requirement")?;
    assert!(requirements.iter().any(|row| matches!(row,
        SemanticRequirement::MemberBehaviorUnverified { receiver: actual, feature, .. }
        if *actual == receiver && feature.name == "name" && !feature.derived)));
    let stopped = InvocationContext::new(StructureGrant::new(0, usize::MAX));
    assert!(prepared.prepare_physical_metadata(&stopped).is_err());
    assert!(prepared.prepare_physical_metadata(&stopped).is_err());
    let observation = InvocationContext::new(StructureGrant::new(2_000_000, 64_000_000));
    let metadata = prepared.prepare_physical_metadata(&observation)?;
    assert!(std::ptr::eq(metadata.definition(), &prepared));
    let mut model = BasicModel::new(prepared.schema().clone())?;
    model.create("node", receiver, "memory:model")?;
    model.set("node", "name", json!("named"))?;
    let policy = StoredPolicy(package.entry().clone());
    let session = SemanticReadSession::new_prepared(&metadata, &model, &policy)?;
    assert_eq!(
        session.value(receiver, "node", "name", &mut QueryBudget::new(1000))?,
        json!("named")
    );
    let foreign = StoredPolicy(SourceId::for_text("memory:foreign.rcore", source));
    assert!(SemanticReadSession::new(&prepared, &model, &foreign).is_err());
    let limits = PhysicalPersistenceLimits {
        objects: 10,
        logical_bytes: 1_048_576,
        wire_bytes: 65_536,
    };
    use intersect_model::core::{
        frozen::FrozenStore,
        storage::{ModelStore, atomic, set_with_opposite},
    };
    let mut physical =
        FrozenStore::for_schema(prepared.schema()).map_err(|error| error.to_string())?;
    atomic(&mut physical, |store| {
        store.create_element(prepared.schema(), "node", receiver, "memory:model", 0, 1)?;
        set_with_opposite(store, prepared.schema(), "node", "name", json!("named"))
    })?;
    let wire = RetainedModelWire::save(&physical, &prepared, &[], &[], limits, &observation)?;
    use sha2::{Digest, Sha256};
    let legacy_context =
        serde_json::to_vec(&(prepared.sources(), prepared.schema(), Option::<()>::None))?;
    let expected: [u8; 32] = Sha256::digest(legacy_context).into();
    assert_eq!(wire.json()["context_sha256"], json!(expected));
    let restored = wire.restore_model(&prepared, &[], &[], limits, &observation)?;
    assert!(
        restored
            .elements
            .iter()
            .any(|element| element.id == "node" && element.class == receiver)
    );
    let other = intersect_model::compile_rcore_at("memory:other-prepared.rcore", source)?;
    assert!(
        wire.restore_model(&other.prepared_definition(), &[], &[], limits, &observation)
            .is_err()
    );
    Ok(())
}

#[test]
fn canonical_reference_transaction_keeps_checked_owner_and_restores_uncommitted_context()
-> Result<(), Box<dyn std::error::Error>> {
    use intersect_model::definitions::model_definition::StructureContext;
    let bundle = structural_inputs();
    let mut compiled = CompiledStructure::prepare(
        structural_selection(&bundle),
        DeclarationCollection::Classifiers,
        PhysicalInputCapture::Retain,
        None,
        ParseLimits::default(),
    )?;
    let original = compiled.schema().clone();
    let address = std::ptr::from_ref(compiled.schema());
    {
        let mut context = compiled.reference_context();
        context.enable_eobject_references()?;
        assert!(context.reference_top.is_some());
        assert_ne!(context.revision, original.revision);
        assert!(std::ptr::eq(address, &*context));
        let contextual = context.clone();
        context.enable_eobject_references()?;
        assert_eq!(&*context, &contextual);
        // An unfinished frontend operation restores both canonical fields.
    }
    assert_eq!(compiled.schema(), &original);
    assert!(compiled.has_physical_inputs());
    assert!(std::ptr::eq(compiled.sources(), &bundle));
    let mut expected = original.clone();
    expected.prepare_eobject_reference_context()?;
    {
        let mut context = compiled.reference_context();
        context.enable_eobject_references()?;
        context.commit();
    }
    assert!(std::ptr::eq(address, compiled.schema()));
    assert_eq!(compiled.schema(), &expected);
    let mut checked = compiled.into_checked();
    checked.require_schema(&expected)?;
    {
        let mut context = checked.reference_context();
        context.enable_eobject_references()?;
        // Restoring an already selected context preserves the same revision.
    }
    checked.require_schema(&expected)?;
    let mut malformed = original;
    malformed.reference_top = Some(intersect_model::definitions::resources::DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: vec![99],
    });
    let before = malformed.clone();
    {
        let mut context = malformed.reference_context();
        assert!(context.enable_eobject_references().is_err());
    }
    assert_eq!(malformed, before);
    Ok(())
}

#[test]
fn structural_outputs_keep_exact_declarations_without_granting_cached_schema_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let bundle = structural_inputs();
    let mut compiled = CompiledStructure::prepare(
        structural_selection(&bundle),
        DeclarationCollection::Classifiers,
        PhysicalInputCapture::Retain,
        None,
        ParseLimits::default(),
    )?;
    assert!(std::ptr::eq(compiled.sources(), &bundle));
    assert!(compiled.has_physical_inputs());
    assert!(compiled.schema().reference_top.is_none());
    let inventory = compiled.take_declarations();
    assert_eq!(inventory.packages().len(), 1);
    assert_eq!(
        inventory.packages()["urn:compiled-structure"].source,
        bundle.sources[0].id
    );
    for (name, path) in [("Base", vec![0]), ("Child", vec![1])] {
        let declaration = &inventory.classifiers()["urn:compiled-structure"][name];
        assert_eq!(declaration.source, bundle.sources[0].id);
        assert_eq!(declaration.path, path);
        assert_eq!(compiled.schema().declarations[name].path, declaration.path);
    }
    assert!(compiled.take_declarations().packages().is_empty());
    assert_eq!(
        compiled.schema().features("Child")?["name"].target,
        "EString"
    );
    let plain = CompiledStructure::prepare(
        structural_selection(&bundle),
        DeclarationCollection::None,
        PhysicalInputCapture::None,
        None,
        ParseLimits::default(),
    )?;
    assert_eq!(plain.schema(), compiled.schema());
    assert!(!plain.has_physical_inputs());
    assert!(plain.declarations().packages().is_empty());
    let metadata = plain.into_schema();
    assert_eq!(&metadata, compiled.schema());
    let packages = CompiledStructure::prepare(
        structural_selection(&bundle),
        DeclarationCollection::Packages,
        PhysicalInputCapture::None,
        None,
        ParseLimits::default(),
    )?;
    assert_eq!(packages.declarations().packages(), inventory.packages());
    assert!(packages.declarations().classifiers().is_empty());
    let original = compiled.schema().clone();
    compiled.enable_eobject_references()?;
    let contextual = compiled.schema().clone();
    compiled.enable_eobject_references()?;
    assert_eq!(compiled.schema(), &contextual);
    let checked = compiled.into_checked();
    checked.require_schema(&contextual)?;
    assert!(checked.require_schema(&original).is_err());
    let mut forged = contextual;
    forged
        .classes
        .get_mut("Child")
        .ok_or("Missing Child")?
        .abstract_type = true;
    assert!(checked.require_schema(&forged).is_err());
    Ok(())
}

#[test]
fn selected_generic_outputs_use_the_shared_materializer_and_keep_source_identity()
-> Result<(), Box<dyn std::error::Error>> {
    use intersect_model::definitions::{
        compiler::materialize_runtime_bundle_with_limits,
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    let source = DefinitionSource::new(
        "memory:compiled-generics.rcore",
        SourceKind::Rcore,
        "rcore 1;package generics version \"1\";metamodel \"urn:compiled-generics\" prefix g {class Leaf {}class Box<T> {ref value:T [0,1];}}",
    );
    let declaration = |ordinal| DeclarationId {
        source: source.id.clone(),
        path: vec![ordinal],
    };
    let leaf = RuntimeType::new(Type::Classifier {
        declaration: declaration(0),
        arguments: vec![],
        raw: false,
    })?;
    let boxed = RuntimeType::new(Type::Classifier {
        declaration: declaration(1),
        arguments: vec![leaf.expression().clone()],
        raw: false,
    })?;
    let selected = [leaf.clone(), boxed.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![
            source,
            DefinitionSource::new(
                "memory:unreachable.rcore",
                SourceKind::Rcore,
                "rcore 1;package unreachable version \"1\";metamodel \"urn:unreachable\" prefix u {class Ignored {}}",
            ),
        ],
        namespaces: vec![],
    };
    let entry = &bundle.sources[0].id;
    let compiled = CompiledStructure::prepare(
        StructureSelection {
            bundle: &bundle,
            entry,
            namespace: "urn:compiled-generics",
            materialized_types: &selected,
        },
        DeclarationCollection::Classifiers,
        PhysicalInputCapture::Retain,
        None,
        ParseLimits::default(),
    )?;
    // Generic materialization deliberately retains its existing cold metadata path.
    assert!(!compiled.has_physical_inputs());
    assert!(std::ptr::eq(compiled.sources(), &bundle));
    assert!(
        !compiled
            .declarations()
            .packages()
            .contains_key("urn:unreachable")
    );
    let schema = compiled.schema();
    assert!(!schema.classes.contains_key("Ignored"));
    let leaf_key = schema.class_key_for_type(&leaf)?;
    let box_key = schema.class_key_for_type(&boxed)?;
    let fields = schema.features(box_key)?;
    assert!(fields["value"].reference);
    assert_eq!(fields["value"].target, leaf_key);
    assert_eq!(schema.declarations[box_key].source, *entry);
    assert_eq!(schema.declarations[box_key].path, vec![1]);
    assert_eq!(
        compiled.declarations().classifiers()["urn:compiled-generics"]["Box"].path,
        vec![1]
    );
    assert_eq!(
        schema,
        &materialize_runtime_bundle_with_limits(
            &bundle,
            entry,
            "urn:compiled-generics",
            &selected,
            None,
            ParseLimits::default(),
        )?
    );
    Ok(())
}

struct StructureGrant {
    work: Cell<usize>,
    storage: Cell<usize>,
    failed: Cell<bool>,
    cancelled: Cell<bool>,
    work_limit: usize,
    storage_limit: usize,
}
impl StructureGrant {
    fn new(work_limit: usize, storage_limit: usize) -> Rc<Self> {
        Rc::new(Self {
            work: Cell::new(0),
            storage: Cell::new(0),
            failed: Cell::new(false),
            cancelled: Cell::new(false),
            work_limit,
            storage_limit,
        })
    }
}
impl InvocationObserver for StructureGrant {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Structural compilation cannot invoke a provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.failed.get() || self.cancelled.get() {
            Err("structure grant stopped".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self.work.get().checked_add(units).ok_or("work overflow")?;
        if next > self.work_limit {
            self.failed.set(true);
            return Err("structure work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self
            .storage
            .get()
            .checked_add(bytes)
            .ok_or("storage overflow")?;
        if next > self.storage_limit {
            self.failed.set(true);
            return Err("structure storage denied".into());
        }
        self.storage.set(next);
        Ok(())
    }
}

#[test]
fn structural_output_observation_keeps_exact_grants_and_typed_failures()
-> Result<(), Box<dyn std::error::Error>> {
    use intersect_model::definitions::ecore::ObservedImportError;
    let bundle = structural_inputs();
    let prepare = |bundle, context: &InvocationContext<'_>| {
        CompiledStructure::prepare_observed(
            structural_selection(bundle),
            DeclarationCollection::Classifiers,
            PhysicalInputCapture::Retain,
            None,
            context,
            ParseLimits::default(),
        )
    };
    let wide = StructureGrant::new(usize::MAX, usize::MAX);
    let compiled = prepare(&bundle, &InvocationContext::new(wide.clone()))?;
    assert!(compiled.has_physical_inputs());
    let (work, storage) = (wide.work.get(), wide.storage.get());
    assert!(work > 0 && storage > 0);
    let exact = StructureGrant::new(work, storage);
    let repeated = prepare(&bundle, &InvocationContext::new(exact.clone()))?;
    assert_eq!(compiled.schema(), repeated.schema());
    assert_eq!((exact.work.get(), exact.storage.get()), (work, storage));
    for grant in [
        StructureGrant::new(work - 1, storage),
        StructureGrant::new(work, storage - 1),
    ] {
        let context = InvocationContext::new(grant.clone());
        assert!(matches!(
            prepare(&bundle, &context),
            Err(ObservedImportError::Observation(_))
        ));
        assert!(grant.failed.get());
        assert!(matches!(
            prepare(&bundle, &context),
            Err(ObservedImportError::Observation(_))
        ));
    }
    let preconsumed = StructureGrant::new(work, storage);
    let context = InvocationContext::new(preconsumed.clone());
    context.work(1)?;
    assert!(matches!(
        prepare(&bundle, &context),
        Err(ObservedImportError::Observation(_))
    ));
    assert!(preconsumed.failed.get());
    let cancelled = StructureGrant::new(work, storage);
    cancelled.cancelled.set(true);
    assert!(matches!(
        prepare(&bundle, &InvocationContext::new(cancelled)),
        Err(ObservedImportError::Observation(_))
    ));
    let mut stale = bundle.clone();
    stale.sources[0].text.push(' ');
    assert!(matches!(
        prepare(&stale, &InvocationContext::default()),
        Err(ObservedImportError::Import(_))
    ));
    let recovered = prepare(&bundle, &InvocationContext::default())?;
    assert_eq!(recovered.schema(), compiled.schema());
    Ok(())
}

#[test]
fn checked_structure_rejects_cached_mutation_and_keeps_canonical_context_idempotent()
-> Result<(), String> {
    let source = DefinitionSource::new(
        "memory:organization.rcore",
        SourceKind::Rcore,
        include_str!("../assets/fixtures/rcore/api/organization.rcore"),
    );
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let selection = StructureSelection {
        bundle: &bundle,
        entry: &entry,
        namespace: "urn:intersect:rcore:organization",
        materialized_types: &[],
    };
    let mut checked = CheckedStructure::prepare(selection, ParseLimits::default())?;
    assert_eq!(checked.schema().declarations["Node"].source, entry);
    assert!(std::ptr::eq(checked.sources(), &bundle));
    let original = checked.schema().clone();
    checked.require_schema(&original)?;
    let mut forged = original.clone();
    forged
        .classes
        .get_mut("Node")
        .ok_or("Missing Node")?
        .abstract_type = true;
    assert_eq!(forged.revision, original.revision);
    assert!(checked.require_schema(&forged).is_err());
    checked.enable_eobject_references()?;
    let contextual = checked.schema().revision.clone();
    assert_ne!(contextual, original.revision);
    assert!(checked.schema().reference_top.is_some());
    checked.enable_eobject_references()?;
    assert_eq!(checked.schema().revision, contextual);
    assert!(checked.require_schema(&original).is_err());
    let fresh = CheckedStructure::prepare(selection, ParseLimits::default())?;
    fresh.require_schema(&original)?;
    assert!(fresh.schema().reference_top.is_none());
    let mut stale = bundle.clone();
    stale.sources[0].text.push(' ');
    assert!(
        CheckedStructure::prepare(
            StructureSelection {
                bundle: &stale,
                ..selection
            },
            ParseLimits::default()
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn checked_model_preparation_replays_origin_and_rejects_foreign_allocations() -> Result<(), String>
{
    use intersect_model::definitions::{
        ecore_migration::migrate_ecore_bundle_with_limits, model_definition::ModelPreparationMode,
        preparation::CheckedModelOrigin,
    };
    let source = DefinitionSource::new(
        "memory:origin.ecore",
        SourceKind::Ecore,
        r#"<ecore:EPackage xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:ecore="http://www.eclipse.org/emf/2002/Ecore" name="owner" nsURI="urn:origin-owner" nsPrefix="o"><eClassifiers xsi:type="ecore:EClass" name="Leaf"/></ecore:EPackage>"#,
    );
    let targets = [(source.id.clone(), "memory:origin.rcore".into())].into();
    let report = migrate_ecore_bundle_with_limits(
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &targets,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    let bundle = report
        .converted
        .as_ref()
        .ok_or("Migration did not produce a bundle")?;
    let prepare = || {
        CompiledStructure::prepare(
            StructureSelection {
                bundle,
                entry: &bundle.sources[0].id,
                namespace: "urn:origin-owner",
                materialized_types: &[],
            },
            DeclarationCollection::Classifiers,
            PhysicalInputCapture::None,
            None,
            ParseLimits::default(),
        )
        .map_err(|error| error.to_string())?
        .check_behavior(&bundle.sources[0].id, None, ParseLimits::default())
        .map_err(|errors| format!("{errors:?}"))?
        .finish(
            ParseLimits::default(),
            false,
            ModelPreparationMode::Strict,
            &[],
        )
        .map_err(|errors| format!("{errors:?}"))
        .map(|model| model.into_prepared_definition())
    };
    let checked = CheckedModelOrigin::check(bundle, &report, ParseLimits::default())?;
    let prepared = prepare()?.with_origin(checked)?;
    assert!(std::ptr::eq(prepared.sources(), bundle));
    assert!(std::ptr::eq(prepared.original_sources(), &report.original));
    assert_eq!(prepared.migration(), Some(&report));
    assert!(prepared.requirements().any(|row| matches!(row,
        Ok(intersect_model::definitions::preparation::SemanticRequirement::ClassBehaviorUnverified { original: Some(_), .. }))));
    let equal = bundle.clone();
    let foreign = CheckedModelOrigin::check(&equal, &report, ParseLimits::default())?;
    assert!(prepare()?.with_origin(foreign).is_err());
    assert!(CheckedModelOrigin::check(bundle, &report, ParseLimits { source_bytes: 1 }).is_err());
    let mut stale = report.clone();
    let row = stale
        .reports
        .get_mut(0)
        .and_then(|resource| resource.correspondence.get_mut(0))
        .ok_or("Missing original declaration correspondence")?;
    row.original_span.end += 1;
    assert!(CheckedModelOrigin::check(bundle, &stale, ParseLimits::default()).is_err());
    assert!(
        prepare()?
            .with_origin(CheckedModelOrigin::check(
                bundle,
                &report,
                ParseLimits::default()
            )?)
            .is_ok()
    );
    Ok(())
}

#[test]
fn owned_model_origin_and_saved_data_preserve_identity_and_cold_inputs() -> Result<(), String> {
    use intersect_model::definitions::{
        ecore_migration::migrate_ecore_bundle_completed_with_limits,
        model_definition::{
            BoundModelOrigin, ModelPreparationMode, SavedModelDefinition, SavedModelOrigin,
        },
    };
    use std::sync::Arc;
    let source = DefinitionSource::new(
        "memory:owned-origin.ecore",
        SourceKind::Ecore,
        r#"<ecore:EPackage xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:ecore="http://www.eclipse.org/emf/2002/Ecore" name="owner" nsURI="urn:owned-origin" nsPrefix="o"><eClassifiers xsi:type="ecore:EClass" name="Leaf"/></ecore:EPackage>"#,
    );
    let targets = [(source.id.clone(), "memory:owned-origin.rcore".into())].into();
    let completed = migrate_ecore_bundle_completed_with_limits(
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &targets,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    let bundle = Arc::new(
        completed
            .report()
            .converted
            .clone()
            .ok_or("No converted bundle")?,
    );
    let owner = CompiledStructure::prepare(
        StructureSelection {
            bundle: &bundle,
            entry: &bundle.sources[0].id,
            namespace: "urn:owned-origin",
            materialized_types: &[],
        },
        DeclarationCollection::Classifiers,
        PhysicalInputCapture::Retain,
        None,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?
    .check_behavior(&bundle.sources[0].id, None, ParseLimits::default())
    .map_err(|errors| format!("{errors:?}"))?
    .finish(
        ParseLimits::default(),
        false,
        ModelPreparationMode::Strict,
        &[],
    )
    .map_err(|errors| format!("{errors:?}"))?
    .into_owned(Arc::clone(&bundle))?;
    let origin = BoundModelOrigin::from_completed(&owner, completed)?;
    let prepared = origin.prepare(&owner)?;
    assert!(owner.has_physical_inputs());
    assert!(std::ptr::eq(prepared.schema(), owner.schema()));
    assert!(std::ptr::eq(prepared.sources(), bundle.as_ref()));
    assert!(std::ptr::eq(
        prepared.original_sources(),
        &origin.report().original
    ));
    let saved = serde_json::to_value(owner.save_host_cache()).map_err(|error| error.to_string())?;
    let cold: SavedModelDefinition =
        serde_json::from_value(saved.clone()).map_err(|error| error.to_string())?;
    let cold = cold.load_host_cache()?;
    assert!(!cold.has_physical_inputs());
    assert_eq!(owner.schema(), cold.schema());
    assert!(!std::ptr::eq(owner.sources(), cold.sources()));
    assert!(origin.prepare(&cold).is_err());
    let saved_origin: SavedModelOrigin = serde_json::from_value(
        serde_json::to_value(origin.save_host_cache()).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let rebound = saved_origin.load_host_cache(&cold)?;
    assert!(std::ptr::eq(
        rebound.prepare(&cold)?.schema(),
        cold.schema()
    ));
    assert_eq!(rebound.report(), origin.report());
    let mut stale = saved.clone();
    let text = stale["sources"]["sources"][0]["text"]
        .as_str()
        .ok_or("Missing saved source text")?
        .to_owned();
    stale["sources"]["sources"][0]["text"] = serde_json::json!(format!("{text} "));
    assert!(
        serde_json::from_value::<SavedModelDefinition>(stale)
            .map_err(|error| error.to_string())?
            .load_host_cache()
            .is_err()
    );
    let mut forged = saved;
    forged
        .as_object_mut()
        .ok_or("Missing saved object")?
        .insert("approved".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<SavedModelDefinition>(forged).is_err());
    let fresh = BoundModelOrigin::replay(&owner, origin.report().clone(), ParseLimits::default())?;
    assert!(fresh.prepare(&owner).is_ok());
    Ok(())
}

fn neutral_recipe_inputs()
-> Result<intersect_model::definitions::model_definition::ModelCompilation, String> {
    use intersect_model::definitions::{
        generic_types::{RuntimeType, Type},
        model_definition::ModelCompilation,
        resources::DeclarationId,
    };
    let structure = DefinitionSource::new(
        "memory:model-recipe.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="recipe" nsURI="urn:model-recipe">
<eClassifiers x:type="e:EClass" name="Leaf"/>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures></eClassifiers>
</e:EPackage>"##,
    );
    let mut behavior = DefinitionSource::new(
        "memory:model-recipe.rules",
        SourceKind::RcoreBehavior,
        "rcore 1; package recipeRules version \"1\";",
    );
    behavior.dependencies.push(structure.id.clone());
    // Retained unsupported text is audit data, not a selected grammar or native support.
    let mut artifact = DefinitionSource::new(
        "memory:unselected.xtext",
        SourceKind::Xtext,
        "unsupported syntax \u{03b1} ??",
    );
    artifact.dependencies.push(structure.id.clone());
    let leaf = Type::Classifier {
        declaration: DeclarationId {
            source: structure.id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    };
    let selected = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: structure.id.clone(),
            path: vec![1],
        },
        arguments: vec![leaf.clone()],
        raw: false,
    })?;
    Ok(ModelCompilation {
        structure: structure.id.clone(),
        behavior: behavior.id.clone(),
        namespace: "urn:model-recipe".into(),
        materialized_types: vec![selected, RuntimeType::new(leaf)?],
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![structure, behavior, artifact],
            namespaces: vec![],
        },
    })
}

#[test]
fn neutral_model_recipe_migration_remaps_nested_types_and_retains_unselected_artifacts()
-> Result<(), String> {
    use intersect_model::{
        definitions::{
            generic_types::Type,
            model_definition::{ModelCompilation, ModelPreparationMode},
        },
        migration::migrate_ecore_bundle_completed_with_limits,
    };
    let original = neutral_recipe_inputs()?;
    let completed = migrate_ecore_bundle_completed_with_limits(
        original.bundle.clone(),
        &[(
            original.structure.clone(),
            "memory:model-recipe.rcore".into(),
        )]
        .into(),
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    let report = completed.report();
    let remapped = completed.remap_model_compilation(&original)?;
    let from_data = report.remap_model_compilation(&original)?;
    assert_eq!(
        serde_json::to_value(&remapped).map_err(|error| error.to_string())?,
        serde_json::to_value(&from_data).map_err(|error| error.to_string())?
    );
    assert_ne!(remapped.structure, original.structure);
    assert_eq!(remapped.structure.uri, "memory:model-recipe.rcore");
    assert_eq!(remapped.behavior, original.behavior);
    let artifact = remapped.bundle.source(&original.bundle.sources[2].id)?;
    assert_eq!(artifact.id, original.bundle.sources[2].id);
    assert_eq!(artifact.text, original.bundle.sources[2].text);
    assert_eq!(artifact.kind, SourceKind::Xtext);
    assert_eq!(artifact.dependencies, vec![remapped.structure.clone()]);
    assert_eq!(
        remapped.bundle.source(&remapped.behavior)?.dependencies,
        vec![remapped.structure.clone()]
    );
    let Type::Classifier {
        declaration,
        arguments,
        raw,
    } = remapped.materialized_types[0].expression()
    else {
        return Err("Missing selected classifier".into());
    };
    assert!(!raw);
    assert_eq!(declaration.source, remapped.structure);
    let [
        Type::Classifier {
            declaration: argument,
            arguments,
            raw,
        },
    ] = arguments.as_slice()
    else {
        return Err("Missing nested classifier argument".into());
    };
    assert_eq!(argument.source, remapped.structure);
    assert!(!raw && arguments.is_empty());
    let serialized = serde_json::to_value(&remapped).map_err(|error| error.to_string())?;
    for field in ["xtext", "xtext_inference", "grammar", "source_policy"] {
        assert!(serialized.get(field).is_none());
    }
    let decoded: ModelCompilation =
        serde_json::from_value(serialized).map_err(|error| error.to_string())?;
    let compiled = CompiledStructure::prepare(
        decoded.structure_selection(),
        DeclarationCollection::Classifiers,
        PhysicalInputCapture::None,
        None,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?
    .check_behavior(&decoded.behavior, None, ParseLimits::default())
    .map_err(|errors| format!("{errors:?}"))?
    .finish(
        ParseLimits::default(),
        false,
        ModelPreparationMode::Inspection,
        &[],
    )
    .map_err(|errors| format!("{errors:?}"))?;
    let key = compiled
        .schema()
        .class_key_for_type(&decoded.materialized_types[0])?;
    let leaf = compiled
        .schema()
        .class_key_for_type(&decoded.materialized_types[1])?;
    assert_eq!(compiled.schema().features(key)?["value"].target, leaf);
    assert!(std::ptr::eq(compiled.sources(), &decoded.bundle));
    Ok(())
}

#[test]
fn neutral_recipe_refuses_text_fields_foreign_worlds_and_missing_correspondence()
-> Result<(), String> {
    use intersect_model::{
        definitions::{
            bundle::SourceId, model_definition::ModelCompilation, preparation::CheckedModelOrigin,
        },
        migration::migrate_ecore_bundle_with_limits,
    };
    let original = neutral_recipe_inputs()?;
    let report = migrate_ecore_bundle_with_limits(
        original.bundle.clone(),
        &[(
            original.structure.clone(),
            "memory:model-recipe.rcore".into(),
        )]
        .into(),
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    let wire = serde_json::to_value(&original).map_err(|error| error.to_string())?;
    for field in [
        "xtext",
        "xtext_inference",
        "grammar",
        "source_policy",
        "admission",
    ] {
        let mut foreign = wire.clone();
        foreign[field] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<ModelCompilation>(foreign).is_err(),
            "{field}"
        );
    }
    let mut foreign = original.clone();
    foreign.bundle.sources[2].text.push(' ');
    assert!(
        report
            .remap_model_compilation(&foreign)
            .err()
            .ok_or("Foreign original accepted")?
            .contains("original bundle")
    );
    let mut missing = original.clone();
    missing.structure = SourceId::for_text("memory:foreign-model", "");
    assert!(report.remap_model_compilation(&missing).is_err());
    let mut unsupported = report.clone();
    unsupported.version = 99;
    assert!(unsupported.selection_remapping(&original.bundle).is_err());
    let mut incomplete = report.clone();
    incomplete.reports[0]
        .correspondence
        .retain(|row| row.original.path != vec![0]);
    assert!(
        incomplete
            .remap_model_compilation(&missing)
            .err()
            .ok_or("Missing correspondence accepted")?
            .contains("Missing exact runtime declaration correspondence")
    );
    let mut forged = report.clone();
    forged.reports[0]
        .correspondence
        .first_mut()
        .ok_or("Missing original correspondence")?
        .converted
        .path
        .push(999);
    let converted = forged.converted.as_ref().ok_or("Missing converted world")?;
    assert!(CheckedModelOrigin::check(converted, &forged, ParseLimits::default()).is_err());
    // Selector rewriting is ordinary retained data and cannot mint fresh origin provenance.
    Ok(())
}

#[test]
fn neutral_model_package_refuses_text_metadata_and_borrows_its_actual_sources() -> Result<(), String>
{
    use intersect_model::{
        definitions::{
            model_definition::{ModelCompilation, ModelPreparationMode},
            rcore_behavior::ir_v2::Limits,
            schema::Package,
        },
        rcore_behavior_functions::ExecutionLimits,
    };
    let source = DefinitionSource::new(
        "memory:neutral-package.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package neutral version "1";
metamodel "urn:neutral-package" prefix n { class Item { attr answer:Int64 [1,1] derived; } }
behavior {
 import model "urn:neutral-package" as n;
 derive n.Item.answer(self) -> Int64 = answer(self);
 fn answer(self:n.Item) -> Int64 = 42;
}"#,
    );
    let artifact = DefinitionSource::new(
        "memory:unselected.xtext",
        SourceKind::Xtext,
        "unsupported syntax ?",
    );
    let recipe = ModelCompilation {
        structure: source.id.clone(),
        behavior: source.id.clone(),
        namespace: "urn:neutral-package".into(),
        materialized_types: vec![],
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![source, artifact.clone()],
            namespaces: vec![],
        },
    };
    let compiled = CompiledStructure::prepare(
        recipe.structure_selection(),
        DeclarationCollection::Classifiers,
        PhysicalInputCapture::None,
        None,
        ParseLimits::default(),
    )
    .map_err(|error| error.to_string())?
    .check_behavior(&recipe.behavior, None, ParseLimits::default())
    .map_err(|errors| format!("{errors:?}"))?
    .finish(
        ParseLimits::default(),
        false,
        ModelPreparationMode::Inspection,
        &[],
    )
    .map_err(|errors| format!("{errors:?}"))?;
    let package = Package {
        version: 3,
        schema: compiled.schema().clone(),
        semantics: compiled.semantics().clone(),
        sources: [
            (
                "structure".into(),
                recipe.bundle.source(&recipe.structure)?.text.clone(),
            ),
            (
                "behavior".into(),
                recipe.bundle.source(&recipe.behavior)?.text.clone(),
            ),
        ]
        .into(),
        compilation: Some(recipe.clone()),
    };
    let selected = package
        .compilation
        .as_ref()
        .ok_or("Missing neutral recipe")?;
    let view = package.model_package();
    assert!(std::ptr::eq(view.definition.schema(), &package.schema));
    assert!(std::ptr::eq(
        view.definition.semantics(),
        &package.semantics
    ));
    assert!(std::ptr::eq(
        view.bundle.ok_or("Missing borrowed bundle")?,
        &selected.bundle
    ));
    assert!(std::ptr::eq(
        view.structure.ok_or("Missing structure selection")?,
        &selected.structure
    ));
    assert!(std::ptr::eq(
        view.behavior.ok_or("Missing behavior selection")?,
        &selected.behavior
    ));
    let wire = serde_json::to_value(&package).map_err(|error| error.to_string())?;
    for field in ["grammar", "xtext_inference", "source_policy", "admission"] {
        assert!(wire.get(field).is_none());
        let mut text = wire.clone();
        text[field] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Package>(text).is_err(), "{field}");
    }
    assert_eq!(wire["version"], 3);
    for version in [1, 2, 4, 99] {
        let mut unsupported = wire.clone();
        unsupported["version"] = version.into();
        assert!(serde_json::from_value::<Package>(unsupported).is_err());
    }
    let decoded: Package = serde_json::from_value(wire).map_err(|error| error.to_string())?;
    let retained = decoded
        .compilation
        .as_ref()
        .ok_or("Missing reloaded recipe")?
        .bundle
        .source(&artifact.id)?;
    assert_eq!(retained.text, artifact.text);
    assert_eq!(retained.kind, SourceKind::Xtext);
    // Unselected audit artifacts do not invoke a text frontend. Activation still
    // rechecks the exact native model/behavior sources through the shared verifier.
    let getters = intersect_model::rcore_getters::RcoreGetters::from_package(
        &decoded,
        &Limits::default(),
        ExecutionLimits::default(),
        ParseLimits::default(),
    )?;
    assert!(std::ptr::eq(getters.schema(), &decoded.schema));
    assert!(std::ptr::eq(
        getters.sources(),
        &decoded
            .compilation
            .as_ref()
            .ok_or("Missing getter recipe")?
            .bundle
    ));
    Ok(())
}
