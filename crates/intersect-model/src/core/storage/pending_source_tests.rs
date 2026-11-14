//! Authored source-service component proofs; never the original43B request.
use super::*;
use crate::definitions::conversion::{InvocationObserver, ProviderId};
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    ecore::ParseLimits,
    ecore_migration::BundleMigrationReport,
    model_definition::ModelCompilation,
    preparation::PreparedModelDefinition,
};
use std::{cell::RefCell, rc::Rc};

const ECORE: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="reuse" nsURI="urn:source-reuse">
<eClassifiers xsi:type="e:EClass" name="A"><eStructuralFeatures xsi:type="e:EReference" name="next" eType="#//A"/><eStructuralFeatures xsi:type="e:EAttribute" name="label" defaultValueLiteral="base" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EReference" name="children" containment="true" upperBound="-1" eType="#//A" eOpposite="#//A/parent"/><eStructuralFeatures xsi:type="e:EReference" name="parent" eType="#//A" eOpposite="#//A/children"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="B" eSuperTypes="#//A"/>
<eClassifiers xsi:type="e:EClass" name="C" eSuperTypes="#//B"/>
<eClassifiers xsi:type="e:EClass" name="X"/>
</e:EPackage>"##;

pub(in crate::core::storage) fn inputs() -> StorageResult<(ModelCompilation, BundleMigrationReport)>
{
    inputs_for(ECORE)
}
pub(in crate::core::storage) fn inputs_for(
    text: &str,
) -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let structure = DefinitionSource::new("memory:source-reuse.ecore", SourceKind::Ecore, text);
    let mut rules = DefinitionSource::new(
        "memory:source-reuse.rules",
        SourceKind::RcoreBehavior,
        "rcore 1; package reuse version \"1\";",
    );
    rules.dependencies = vec![structure.id.clone()];
    let original = ModelCompilation {
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![structure.clone(), rules.clone()],
            namespaces: vec![],
        },
        structure: structure.id.clone(),
        behavior: rules.id,
        namespace: "urn:source-reuse".into(),
        materialized_types: vec![],
    };
    let migration = crate::migration::migrate_ecore_bundle_with_limits(
        original.bundle.clone(),
        &[(structure.id, "memory:source-reuse.rcore".into())].into(),
        ParseLimits {
            source_bytes: text.len(),
        },
    )
    .map_err(|e| e.to_string())?;
    Ok((migration.remap_model_compilation(&original)?, migration))
}
pub(in crate::core::storage) fn definition<'a>(
    input: &'a ModelCompilation,
    migration: &'a BundleMigrationReport,
) -> StorageResult<PreparedModelDefinition<'a>> {
    let limits = crate::definitions::ecore::ParseLimits::default();
    crate::definitions::model_test_support::prepare(
        crate::definitions::model_definition::StructureSelection {
            bundle: &input.bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        migration,
        limits,
    )
}
pub(in crate::core::storage) struct Meter {
    pub(in crate::core::storage) budget: RefCell<QueryBudget>,
    pub(in crate::core::storage) bytes: Cell<usize>,
    pub(in crate::core::storage) copies: Cell<usize>,
    pub(in crate::core::storage) deny_retention: Cell<bool>,
}
impl Meter {
    pub(in crate::core::storage) fn new(limit: usize) -> Rc<Self> {
        Rc::new(Self {
            budget: RefCell::new(QueryBudget::new(limit)),
            bytes: Cell::new(0),
            copies: Cell::new(0),
            deny_retention: Cell::new(false),
        })
    }
    pub(in crate::core::storage) fn work(&self) -> usize {
        self.budget.borrow().reads
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        Err("No source-service provider".into())
    }
    fn work(&self, n: usize) -> StorageResult<()> {
        for _ in 0..n {
            self.budget
                .try_borrow_mut()
                .map_err(|_| "Test ledger borrowed")?
                .charge()?;
        }
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> StorageResult<()> {
        if self.deny_retention.get() {
            return Err("Source retention denied".into());
        }
        self.bytes.set(
            self.bytes
                .get()
                .checked_add(n)
                .ok_or("Test bytes overflow")?,
        );
        self.copies.set(self.copies.get() + 1);
        Ok(())
    }
}
