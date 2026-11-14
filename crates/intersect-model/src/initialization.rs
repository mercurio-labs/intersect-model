//! Language-package constructor selection over the shared core service.
use intersect_model::core::{
    api::ModelEdit,
    query::QueryBudget,
    storage::{ClassInitializer, Construction, InitializationBinding, StorageResult},
};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource},
    conversion::ConversionRegistry,
    initialization::{ClassInitializationContract, InitializationReceipt},
    schema::Package,
};
use std::collections::BTreeMap;

/// Prepared from a compiled or source-verified package. Untrusted serialized IR
/// must pass the normal package replay boundary before preparation.
pub struct PackageInitializers {
    sources: Vec<DefinitionSource>,
    contracts: BTreeMap<String, ClassInitializationContract>,
    constants: BTreeMap<String, Vec<serde_json::Value>>,
}
impl PackageInitializers {
    pub fn new<'a>(
        package: impl Into<crate::definitions::model_definition::ModelPackageView<'a>>,
    ) -> StorageResult<Self> {
        let package = package.into();
        let bundle = package
            .bundle
            .ok_or("Missing retained initializer compilation inputs")?;
        Self::with_model_definition(package.definition, bundle)
    }
    /// Internal exact-source context for a compiler facade that already owns
    /// the authoritative bundle. This prepares the same existing contracts;
    /// it does not recompile or trust a serialized package as executable input.
    pub(crate) fn with_source_context(
        package: &Package,
        bundle: &DefinitionBundle,
    ) -> StorageResult<Self> {
        Self::with_model_definition(package.model_definition(), bundle)
    }
    /// Prepare the same existing contracts over exact model metadata and sources.
    /// This installs no provider and does not admit untrusted serialized output.
    pub fn with_model_definition(
        package: crate::definitions::model_definition::ModelDefinitionView<'_>,
        bundle: &DefinitionBundle,
    ) -> StorageResult<Self> {
        bundle.validate()?;
        let sources = bundle.sources.clone();
        let mut contracts = BTreeMap::new();
        let mut constants = BTreeMap::new();
        for rule in &package.semantics.initializers {
            if rule.provider.name == "intersect.constant-attributes" && rule.provider.version == "1"
            {
                let values = rule
                    .values
                    .as_ref()
                    .ok_or("Missing constant initializer values")?;
                constants.insert(
                    rule.class.clone(),
                    rule.members
                        .iter()
                        .map(|name| {
                            values
                                .get(name)
                                .cloned()
                                .ok_or_else(|| "Missing constant initializer member".into())
                        })
                        .collect::<StorageResult<Vec<_>>>()?,
                );
            }
            let members = rule
                .members
                .iter()
                .map(|name| {
                    Ok(package
                        .schema
                        .effective_feature(&rule.class, name)?
                        .identity)
                })
                .collect::<StorageResult<Vec<_>>>()?;
            let contract = ClassInitializationContract {
                version: 1,
                schema_revision: package.schema.revision.clone(),
                class: rule.class.clone(),
                class_identity: package.schema.identity(&rule.class),
                provider: rule.provider.clone(),
                sources: sources.iter().map(|source| source.id.clone()).collect(),
                members,
            };
            if contracts.insert(rule.class.clone(), contract).is_some() {
                return Err("Duplicate class initializer".into());
            }
        }
        Ok(Self {
            sources,
            contracts,
            constants,
        })
    }
    pub fn create_installed(
        &self,
        store: &mut impl intersect_model::core::storage::ModelStore,
        schema: &intersect_model::definitions::schema::Schema,
        class: &str,
        request: Construction<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut QueryBudget,
    ) -> StorageResult<InitializationReceipt> {
        self.create_installed_with_policy(
            store,
            schema,
            class,
            (
                request,
                intersect_model::core::storage::DraftReferencePolicy::Resolved,
            ),
            conversions,
            budget,
        )
    }
    /// Adapter hook for text construction inside a caller-owned linking transaction.
    /// The caller must resolve pending references and validate before publication.
    #[doc(hidden)]
    pub fn create_for_text(
        &self,
        store: &mut impl intersect_model::core::storage::ModelStore,
        schema: &intersect_model::definitions::schema::Schema,
        class: &str,
        request: Construction<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut QueryBudget,
    ) -> StorageResult<InitializationReceipt> {
        self.create_installed_with_policy(
            store,
            schema,
            class,
            (
                request,
                intersect_model::core::storage::DraftReferencePolicy::PendingNonContainment,
            ),
            conversions,
            budget,
        )
    }
    fn create_installed_with_policy(
        &self,
        store: &mut impl intersect_model::core::storage::ModelStore,
        schema: &intersect_model::definitions::schema::Schema,
        class: &str,
        construction: (
            Construction<'_>,
            intersect_model::core::storage::DraftReferencePolicy,
        ),
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut QueryBudget,
    ) -> StorageResult<InitializationReceipt> {
        let (request, references) = construction;
        let contract = self.contract(class)?;
        let values = self
            .constants
            .get(class)
            .ok_or_else(|| format!("Class {class} requires initializer provider dispatch"))?;
        let provider = ConstantAttributes { contract, values };
        intersect_model::core::storage::InitializationContext {
            schema,
            expected: contract,
            sources: &self.sources,
            provider: Some(&provider),
            conversions,
        }
        .create_with_reference_policy(store, request, budget, references)
    }
    /// Existing constant provider through the checked facade's atomic authority.
    /// No registry is inferred for a nonconstant provider contract.
    pub(crate) fn create_installed_for_edit(
        &self,
        edit: &mut impl ModelEdit,
        class: &str,
        request: Construction<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<InitializationReceipt> {
        let contract = self.contract(class)?;
        let values = self
            .constants
            .get(class)
            .ok_or_else(|| format!("Class {class} requires initializer provider dispatch"))?;
        let provider = ConstantAttributes { contract, values };
        self.create(edit, class, request, Some(&provider), None, budget)
    }
    pub fn contract(&self, class: &str) -> StorageResult<&ClassInitializationContract> {
        self.contracts
            .get(class)
            .ok_or_else(|| "No class initializer declaration".into())
    }
    /// Resolve the exact concrete-class declaration. Missing or stale providers
    /// fail; this entry point never substitutes Ecore literals for required code.
    pub fn create(
        &self,
        edit: &mut impl ModelEdit,
        class: &str,
        request: Construction<'_>,
        provider: Option<&dyn ClassInitializer>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut QueryBudget,
    ) -> StorageResult<InitializationReceipt> {
        edit.create_initialized(
            request,
            InitializationBinding {
                expected: self.contract(class)?,
                sources: &self.sources,
                provider,
                conversions,
            },
            budget,
        )
    }
}

struct ConstantAttributes<'a> {
    contract: &'a ClassInitializationContract,
    values: &'a [serde_json::Value],
}
impl ClassInitializer for ConstantAttributes<'_> {
    fn contract(&self) -> &ClassInitializationContract {
        self.contract
    }
    fn plan(
        &self,
        _: &dyn intersect_model::core::storage::ModelRead,
        _: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<intersect_model::core::storage::InitialValue>> {
        self.contract
            .members
            .iter()
            .zip(self.values)
            .map(|(feature, value)| {
                budget.charge()?;
                Ok(intersect_model::core::storage::InitialValue {
                    feature: feature.clone(),
                    value: value.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod source_context_controls {
    use super::*;
    use crate::{api::BasicModel, storage};
    use serde_json::json;
    #[test]
    fn retained_facade_and_explicit_context_preserve_outcomes_order_and_budget() -> StorageResult<()>
    {
        let source = r#"rcore 1; package initParity version "1";
metamodel "urn:init-parity" prefix p {
 class Item { attr label: String [1,1] default "schema";
   attr origin: String [1,1] readonly default "schema-origin"; }
}
behavior { import model "urn:init-parity" as p;
 initialize p.Item using "intersect.constant-attributes" version "1" {
   label = "constructor"; origin = "constructed-origin";
 }
}"#;
        let source = DefinitionSource::new(
            "memory:model.rcore",
            crate::definitions::bundle::SourceKind::Rcore,
            source,
        );
        let entry = source.id.clone();
        let sources = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let mut schema = crate::definitions::compiler::compile_structure_bundle(
            &sources,
            &entry,
            "urn:init-parity",
        )
        .map_err(|error| error.to_string())?;
        let semantics = crate::definitions::compiler::compile_semantics_source(
            &mut schema,
            &source,
            crate::definitions::ecore::ParseLimits::default(),
            false,
        )
        .map_err(|errors| format!("{errors:?}"))?;
        let package = Package {
            version: 3,
            schema,
            semantics,
            sources: [
                ("structure".into(), source.text.clone()),
                ("behavior".into(), source.text),
            ]
            .into(),
            compilation: Some(crate::definitions::model_definition::ModelCompilation {
                bundle: sources,
                structure: entry.clone(),
                namespace: "urn:init-parity".into(),
                behavior: entry,
                materialized_types: vec![],
            }),
        };
        let bundle = &package
            .compilation
            .as_ref()
            .ok_or("Missing parity compilation")?
            .bundle;
        let facade = PackageInitializers::new(&package)?;
        let explicit = PackageInitializers::with_source_context(&package, bundle)?;
        let class = package.schema.language_class_key("Item")?;
        assert_eq!(facade.contract(class)?, explicit.contract(class)?);
        let mut facade_model = BasicModel::new(package.schema.clone())?;
        let mut explicit_model = BasicModel::new(package.schema.clone())?;
        let mut facade_budget = QueryBudget::new(1000);
        let mut explicit_budget = QueryBudget::new(1000);
        let authored = [("label".to_owned(), json!("authored"))];
        let a = facade.create_installed_for_edit(
            &mut facade_model,
            class,
            Construction {
                id: "a",
                resource: "memory:parity",
                authored: &authored,
            },
            &mut facade_budget,
        )?;
        let b = explicit.create_installed_for_edit(
            &mut explicit_model,
            class,
            Construction {
                id: "a",
                resource: "memory:parity",
                authored: &authored,
            },
            &mut explicit_budget,
        )?;
        assert_eq!(a, b);
        assert_eq!(facade_budget.reads, explicit_budget.reads);
        assert_eq!(
            storage::export(&facade_model)?,
            storage::export(&explicit_model)?
        );
        let baseline = storage::export(&facade_model)?;
        let invalid = [("origin".to_owned(), json!("forbidden"))];
        let a = facade.create_installed_for_edit(
            &mut facade_model,
            class,
            Construction {
                id: "bad",
                resource: "memory:parity",
                authored: &invalid,
            },
            &mut facade_budget,
        );
        let b = explicit.create_installed_for_edit(
            &mut explicit_model,
            class,
            Construction {
                id: "bad",
                resource: "memory:parity",
                authored: &invalid,
            },
            &mut explicit_budget,
        );
        assert_eq!(a, b);
        assert!(a.is_err());
        assert_eq!(facade_budget.reads, explicit_budget.reads);
        assert_eq!(storage::export(&facade_model)?, baseline);
        assert_eq!(storage::export(&explicit_model)?, baseline);
        let mut missing = package.clone();
        missing.compilation = None;
        assert_eq!(
            PackageInitializers::new(&missing)
                .err()
                .ok_or("Missing legacy rejection")?,
            "Missing retained initializer compilation inputs"
        );
        let mut stale_bundle = bundle.clone();
        stale_bundle.sources[0].text.push(' ');
        let mut stale_package = package.clone();
        stale_package
            .compilation
            .as_mut()
            .ok_or("Missing parity inputs")?
            .bundle = stale_bundle.clone();
        assert_eq!(
            PackageInitializers::new(&stale_package)
                .err()
                .ok_or("Missing stale facade rejection")?,
            PackageInitializers::with_source_context(&package, &stale_bundle)
                .err()
                .ok_or("Missing stale explicit rejection")?
        );
        Ok(())
    }
}
