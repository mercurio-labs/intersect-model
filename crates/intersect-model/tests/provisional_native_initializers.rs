//! Frozen actual-source initializer controls, not Pilot admission.
use intersect_model::{
    core::{frozen::FrozenStore, query::QueryBudget},
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::{ConversionRegistry, InvocationContext, InvocationObserver, ProviderId},
        ecore_migration::BundleMigrationReport,
        initialization::ClassInitializationContract,
        model_definition::ModelCompilation,
        preparation::PreparedModelDefinition,
    },
    storage::{
        self, ClassInitializer, Construction, DraftClassContract, DraftConstructionContext,
        DraftDisposition, DraftMemberDisposition, DraftPhysicalDefaults, InitialValue,
        InitializationBinding, ModelRead, PendingSourceInputs, StorageResult,
    },
};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};
const XML: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="batch" nsURI="urn:batch-init">
<eClassifiers xsi:type="e:EClass" name="A">
 <eStructuralFeatures xsi:type="e:EAttribute" name="identity" lowerBound="1" changeable="false" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
 <eStructuralFeatures xsi:type="e:EAttribute" name="flag" defaultValueLiteral="false" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EBoolean"/>
 <eStructuralFeatures xsi:type="e:EReference" name="children" upperBound="-1" containment="true" eType="#//A"/>
</eClassifiers><eClassifiers xsi:type="e:EClass" name="B" eSuperTypes="#//A"/></e:EPackage>"##;
fn inputs() -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let model = DefinitionSource::new("memory:batch.ecore", SourceKind::Ecore, XML);
    let mut rules = DefinitionSource::new(
        "memory:batch.rules",
        SourceKind::RcoreBehavior,
        "rcore 1; package batch version \"1\";",
    );
    rules.dependencies.push(model.id.clone());
    let original = ModelCompilation {
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), rules.clone()],
            namespaces: vec![],
        },
        structure: model.id.clone(),
        behavior: rules.id,
        namespace: "urn:batch-init".into(),
        materialized_types: vec![],
    };
    let origin = intersect_model::migration::migrate_ecore_bundle(
        original.bundle.clone(),
        &[(model.id, "memory:batch.rcore".into())].into(),
    )
    .map_err(|e| e.to_string())?;
    Ok((origin.remap_model_compilation(&original)?, origin))
}
fn prepare<'a>(
    input: &'a ModelCompilation,
    origin: &'a BundleMigrationReport,
) -> StorageResult<PreparedModelDefinition<'a>> {
    crate::model_fixture::prepare(
        intersect_model::definitions::model_definition::StructureSelection {
            bundle: &input.bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        origin,
        intersect_model::definitions::ecore::ParseLimits::default(),
    )
}
struct Native(Vec<DraftClassContract>);
impl DraftPhysicalDefaults for Native {
    fn contracts(&self) -> &[DraftClassContract] {
        &self.0
    }
}
fn native(d: &PreparedModelDefinition<'_>) -> StorageResult<Native> {
    Ok(Native(vec![DraftClassContract {
        version: 1,
        schema_revision: d.schema().revision.clone(),
        class: "A".into(),
        class_identity: d.schema().identity("A"),
        sources: d.sources().sources.iter().map(|s| s.id.clone()).collect(),
        members: d
            .schema()
            .effective_features("A")?
            .into_iter()
            .map(|m| DraftMemberDisposition {
                name: m.feature.name.clone(),
                identity: m.identity,
                disposition: if m.feature.name == "children" {
                    DraftDisposition::DelegatedAbsent
                } else {
                    DraftDisposition::StoredDefault
                },
            })
            .collect(),
    }]))
}
struct Initializer {
    expected: ClassInitializationContract,
    changed: ClassInitializationContract,
    calls: Cell<usize>,
    contract_calls: Cell<usize>,
    late: bool,
    bad: bool,
}
impl Initializer {
    fn new(d: &PreparedModelDefinition<'_>, late: bool, bad: bool) -> StorageResult<Self> {
        Self::for_class(d, "A", late, bad)
    }
    fn for_class(
        d: &PreparedModelDefinition<'_>,
        class: &str,
        late: bool,
        bad: bool,
    ) -> StorageResult<Self> {
        let expected = ClassInitializationContract {
            version: 1,
            schema_revision: d.schema().revision.clone(),
            class: class.into(),
            class_identity: d.schema().identity(class),
            provider: ProviderId {
                name: "test.batch-init".into(),
                version: "1".into(),
            },
            sources: d.sources().sources.iter().map(|s| s.id.clone()).collect(),
            members: vec![
                d.schema().effective_feature(class, "identity")?.identity,
                d.schema().effective_feature(class, "flag")?.identity,
            ],
        };
        let mut changed = expected.clone();
        changed.provider.version = "drift".into();
        Ok(Self {
            expected,
            changed,
            calls: Cell::new(0),
            contract_calls: Cell::new(0),
            late,
            bad,
        })
    }
    fn binding<'a>(
        &'a self,
        d: &'a PreparedModelDefinition<'_>,
        conversions: Option<&'a ConversionRegistry<'a>>,
    ) -> InitializationBinding<'a> {
        InitializationBinding {
            expected: &self.expected,
            sources: &d.sources().sources,
            provider: Some(self),
            conversions,
        }
    }
}
impl ClassInitializer for Initializer {
    fn contract(&self) -> &ClassInitializationContract {
        let n = self.contract_calls.get() + 1;
        self.contract_calls.set(n);
        if self.late && n >= 3 {
            &self.changed
        } else {
            &self.expected
        }
    }
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<InitialValue>> {
        budget.charge()?;
        self.calls.set(self.calls.get() + 1);
        assert_eq!(store.value(id, "identity")?, Some(Value::Null));
        assert_eq!(store.value(id, "flag")?, Some(json!(false)));
        let mut fresh = false;
        store.inspect(id, &mut |row| {
            fresh = row.initialization().is_none();
            Ok(())
        })?;
        assert!(fresh);
        Ok(vec![
            InitialValue {
                feature: self.expected.members[0].clone(),
                value: if self.bad {
                    json!(7)
                } else {
                    json!(format!("id:{id}"))
                },
            },
            InitialValue {
                feature: self.expected.members[1].clone(),
                value: json!(true),
            },
        ])
    }
}
fn request<'a>(id: &'a str, resource: &'a str) -> Construction<'a> {
    Construction {
        id,
        resource,
        authored: &[],
    }
}
#[test]
fn invalid_and_late_initializer_failures_roll_back_and_poison_the_lease() -> StorageResult<()> {
    for (late, bad) in [(false, true), (true, false)] {
        let (input, origin) = inputs()?;
        let d = prepare(&input, &origin)?;
        let m = d
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
        let native = native(&d)?;
        let mut work = QueryBudget::new(5_000_000);
        let c = DraftConstructionContext::bind(d.schema(), d.sources(), &native, &mut work)?;
        let i = PendingSourceInputs::new(&m, &mut work)?;
        let l = c.source_lease(&m, &i, &mut work)?;
        let init = Initializer::new(&d, late, bad)?;
        let mut store = FrozenStore::for_schema(d.schema()).map_err(|e| e.to_string())?;
        let before = storage::export(&store)?;
        let result: StorageResult<()> = storage::atomic(&mut store, |s| {
            storage::bind_schema(s, d.schema())?;
            let error = l
                .create_pending_with_initializer(
                    s,
                    request("bad", "memory:bad"),
                    "A",
                    0,
                    1,
                    &init.binding(&d, None),
                    &mut work,
                )
                .err()
                .ok_or("Bad initialization succeeded")?;
            assert!(s.class("bad")?.is_none());
            let retry = Initializer::new(&d, false, false)?;
            assert!(
                l.create_pending_with_initializer(
                    s,
                    request("retry", "memory:bad"),
                    "A",
                    0,
                    1,
                    &retry.binding(&d, None),
                    &mut work,
                )
                .is_err()
            );
            assert_eq!(retry.calls.get(), 0);
            Err(error)
        });
        assert!(result.is_err());
        assert_eq!(storage::export(&store)?, before);
    }
    Ok(())
}
struct Observe;
impl InvocationObserver for Observe {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        Err("Binding must not invoke semantic providers".into())
    }
}
fn compiled_initializer_owner() -> StorageResult<(
    intersect_model::definitions::model_definition::OwnedModelDefinition,
    intersect_model::definitions::model_definition::BoundModelOrigin,
)> {
    let model_text = XML.replace("</e:EPackage>", r##"<eClassifiers xsi:type="e:EClass" name="Root"><eStructuralFeatures xsi:type="e:EReference" name="children" upperBound="-1" containment="true" eType="#//Target"/></eClassifiers><eClassifiers xsi:type="e:EClass" name="Target"><eStructuralFeatures xsi:type="e:EAttribute" name="tag" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers></e:EPackage>"##);
    let model = DefinitionSource::new("memory:compiled-batch.ecore", SourceKind::Ecore, model_text);
    let mut grammar = DefinitionSource::new(
        "memory:compiled-batch.xtext",
        SourceKind::Xtext,
        "grammar fixture.Batch hidden(WS)\nimport \"urn:batch-init\" as b\nRoot returns b::Root: {b::Root} (=> children+=Target)* 'x';\nTarget returns b::Target: {b::Target} tag='x';\nterminal WS: (' '|'\\n'|'\\r'|'\\t')+;\n",
    );
    grammar.dependencies.push(model.id.clone());
    // Material actual hashing work, not a simulated cost/counter. This remains
    // an authored generic fixture, not a replacement for an original stdlib.
    let mut rules = DefinitionSource::new(
        "memory:compiled-batch.rules",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package batch version \"1\";\n// {}\n",
            "p".repeat(128_000)
        ),
    );
    rules.dependencies.push(model.id.clone());
    let original = ModelCompilation {
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), grammar.clone(), rules.clone()],
            namespaces: vec![],
        },
        structure: model.id.clone(),
        behavior: rules.id,
        namespace: "urn:batch-init".into(),
        materialized_types: vec![],
    };
    let completed = intersect_model::migration::migrate_ecore_bundle_completed_with_limits(
        original.bundle.clone(),
        &[(model.id, "memory:compiled-batch.rcore".into())].into(),
        intersect_model::definitions::ecore::ParseLimits::default(),
    )
    .map_err(|e| e.to_string())?;
    let input = completed.remap_model_compilation(&original)?;
    use intersect_model::definitions::{
        ecore::ParseLimits,
        model_definition::{BoundModelOrigin, PhysicalInputCapture, StructureSelection},
    };
    let bundle = std::sync::Arc::new(input.bundle);
    let owner = crate::model_fixture::compile(
        StructureSelection {
            bundle: &bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        PhysicalInputCapture::Retain,
        ParseLimits::default(),
        true,
    )?
    .into_owned(std::sync::Arc::clone(&bundle))?;
    let origin = BoundModelOrigin::from_completed(&owner, completed)?;
    Ok((owner, origin))
}
#[test]
fn frozen_initializer_checks_source_hash_and_live_provider() -> StorageResult<()> {
    let (owner, origin) = compiled_initializer_owner()?;
    let d = origin.prepare(&owner).map_err(|e| format!("{e:?}"))?;
    let metadata = d
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let native = native(&d)?;
    let control = InvocationContext::new(Rc::new(Observe));
    let context = DraftConstructionContext::bind_frozen(&metadata, &native, &control)?;
    let mut successful = Vec::new();
    let mut work_by_mode = Vec::new();
    for mode in 0..4 {
        let mut work = QueryBudget::new(20_000_000);
        let inputs = PendingSourceInputs::new(&metadata, &mut work)?;
        let lease = context.source_lease(&metadata, &inputs, &mut work)?;
        let initializer = Initializer::new(&d, mode == 3, false)?;
        let mut detached = d.sources().sources.clone();
        if mode == 2 {
            detached[0].text.push_str("// tampered source bytes");
        }
        let mut binding = initializer.binding(&d, None);
        if mode == 1 || mode == 2 {
            binding.sources = &detached;
        }
        let mut store = FrozenStore::for_schema(d.schema()).map_err(|e| e.to_string())?;
        let before = storage::export(&store)?;
        let result = storage::atomic(&mut store, |s| {
            storage::bind_schema(s, d.schema())?;
            let start = work.reads;
            lease.create_pending_with_initializer(
                s,
                request("same", "memory:one"),
                "A",
                0,
                1,
                &binding,
                &mut work,
            )?;
            work_by_mode.push(work.reads - start);
            Ok(())
        });
        if mode <= 1 {
            result?;
            successful.push(storage::export(&store)?);
            assert_eq!(initializer.calls.get(), 1);
        } else {
            assert!(result.is_err());
            assert_eq!(storage::export(&store)?, before);
            assert_eq!(initializer.calls.get(), if mode == 2 { 0 } else { 1 });
        }
    }
    assert_eq!(
        successful[0], successful[1],
        "Canonical and detached-source fallback outputs differ"
    );
    assert_eq!(
        work_by_mode[0], work_by_mode[1],
        "Frozen initialization charged different work for equivalent source data: {work_by_mode:?}"
    );
    Ok(())
}
