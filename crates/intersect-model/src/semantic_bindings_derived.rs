//! Explicit registry adapter for source-checked derived properties. Shared by
//! native hosts, model queries and the browser; no script-specific model rules.
use super::{Budget, NativeValue, Registry};
use crate::{
    model_reference::ReferenceDomain,
    query::ModelDerived,
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
};
use intersect_model::core::{
    query::{DerivedValues, QueryBudget},
    storage::{self, ModelRead},
};
use intersect_model::definitions::{
    bundle::DefinitionBundle,
    compiler::materialize_runtime_bundle,
    conversion::{ConversionRegistry, ReferenceContext},
    rcore_behavior::{
        ir_v2::{DataWire, TypeNode, ValueWire},
        types::Scalar,
    },
    schema::{DerivedArgument, Expression},
    semantic_bindings::{Contract, SourceContracts},
};
use serde_json::Value;

pub struct BoundDerived<'a> {
    package: crate::definitions::model_definition::ModelDefinitionView<'a>,
    sources: &'a SourceContracts,
    registry: &'a Registry,
    budget: Budget,
}
impl<'a> BoundDerived<'a> {
    pub fn new(
        package: impl Into<crate::definitions::model_definition::ModelPackageView<'a>>,
        sources: &'a SourceContracts,
        registry: &'a Registry,
        budget: Budget,
    ) -> Result<Self, String> {
        let retained = package.into();
        let package = retained.definition;
        if retained.structure_text != Some(sources.model_source.text.as_str())
            || retained.behavior_text != Some(sources.model_behavior.as_str())
            // The retained compiler selection must identify the same authored
            // source, including its URI and digest, as the binding contracts.
            || !matches!(retained.version, 2 | 3)
            || retained.structure != Some(&sources.model_source.id)
        {
            return Err(
                "Derived implementations belong to different model or behavior sources".into(),
            );
        }
        for rule in &package.semantics.derived {
            if let Expression::External { function, .. } = &rule.expression {
                let contract = external(sources, function)?;
                registry.pin(contract).map_err(|e| e.to_string())?;
            }
        }
        Ok(Self {
            package,
            sources,
            registry,
            budget,
        })
    }
}
fn external<'a>(sources: &'a SourceContracts, name: &str) -> Result<&'a Contract, String> {
    let mut matching = sources
        .contracts
        .iter()
        .filter(|c| c.descriptor().external && c.descriptor().name == name);
    let contract = matching
        .next()
        .ok_or_else(|| format!("Missing external contract {name}"))?;
    if matching.next().is_some() {
        return Err("External derived overloads are unsupported".into());
    }
    Ok(contract)
}
impl DerivedValues for BoundDerived<'_> {
    fn schema_revision(&self) -> Option<&str> {
        Some(&self.package.schema.revision)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        ModelDerived(self.package).supports(class, feature)
    }
    fn explicit_set_state(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<bool, String> {
        ModelDerived(self.package).explicit_set_state(store, id, feature, budget)
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        budget.charge()?;
        if store.revision()? != self.package.schema.revision {
            return Err("Derived model belongs to another schema revision".into());
        }
        let class = store.class(id)?.ok_or("Missing derived object")?;
        let rule = self
            .package
            .semantics
            .derived
            .iter()
            .find(|r| r.feature == feature && self.package.schema.conforms(&class, &r.class))
            .ok_or("Missing derivation")?;
        let Expression::External {
            function,
            arguments,
        } = &rule.expression
        else {
            return ModelDerived(self.package).read(store, id, feature, budget);
        };
        let contract = external(self.sources, function)?;
        let inputs = arguments
            .iter()
            .map(|argument| {
                budget.charge()?;
                match argument {
                    DerivedArgument::Receiver => Ok(Value::String(id.into())),
                    DerivedArgument::Stored { feature } => {
                        let value = store
                            .value(id, feature)?
                            .ok_or("Missing external argument value")?;
                        // The scalar wire profile represents exact Int64 inputs as strings.
                        Ok(
                            if value.get("kind").and_then(Value::as_str) == Some("int64") {
                                value["value"].clone()
                            } else {
                                value
                            },
                        )
                    }
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let result = invoke(
            self.sources,
            self.registry,
            contract,
            &inputs,
            Some(store),
            self.budget.clone(),
        )?;
        let DataWire::Scalar(value) = result.evaluation.value.data else {
            return Err("External derived result must be scalar".into());
        };
        Ok(value.payload)
    }
}

/// Admit a model snapshot and scalar inputs through the same checked registry
/// boundary used for explicit calls. The stored model remains authoritative;
/// only class keys are mapped into its source-identical runtime schema.
pub fn invoke(
    sources: &SourceContracts,
    registry: &Registry,
    contract: &Contract,
    inputs: &[Value],
    store: Option<&dyn ModelRead>,
    budget: Budget,
) -> Result<super::InvocationResult, String> {
    if inputs.len() != contract.descriptor().parameters.len() {
        return Err("Wrong argument count".into());
    }
    let needs_model = contract.descriptor().parameters.iter().any(|ty| {
        matches!(
            contract.descriptor().types.get(ty.0 as usize),
            Some(TypeNode::Model(_))
        )
    });
    let references = if needs_model {
        let store = store.ok_or("External function requires a model")?;
        let source = &sources.model_source;
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let schema = materialize_runtime_bundle(
            &bundle,
            &source.id,
            &sources.namespace,
            &sources.receiver_types,
            None,
        )
        .map_err(|e| e.to_string())?;
        let mut model = intersect_model::core::model::Model::default();
        let mut edits = Vec::new();
        storage::visit_views(store, |element| {
            budget.charge(1).map_err(|e| e.to_string())?;
            if model.elements.len() >= 4096 {
                return Err("Derived model exceeds 4096 elements".into());
            }
            let ty = sources
                .model_classes
                .get(element.class())
                .ok_or("Unknown model class")?;
            model.create(
                &schema,
                element.id(),
                schema.class_key_for_type(ty)?,
                element.resource(),
                element.start(),
                element.end(),
            )?;
            // Use the store's canonical projection, retaining nominal values.
            Ok(())
        })?;
        let projected = storage::export(store)?;
        for element in projected.elements {
            for (feature, value) in element.values {
                budget.charge(1).map_err(|e| e.to_string())?;
                edits.push((element.id.clone(), feature, value));
            }
        }
        model.edit(&schema, &edits)?;
        model.validate(&schema)?;
        vec![ReferenceContext {
            bundle,
            entry: source.id.clone(),
            namespace: sources.namespace.clone(),
            conversions: vec![],
            materialized_types: sources.receiver_types.clone(),
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::to_value(model).map_err(|e| e.to_string())?,
        }]
    } else {
        vec![]
    };
    let arguments = inputs
        .iter()
        .zip(&contract.descriptor().parameters)
        .map(|(input, ty)| {
            budget.charge(1).map_err(|e| e.to_string())?;
            match contract.descriptor().types.get(ty.0 as usize) {
                Some(TypeNode::Scalar(Scalar::Int64)) => input
                    .as_str()
                    .ok_or("Int64 input must be a decimal string")?
                    .parse::<i64>()
                    .map_err(|e| e.to_string())?
                    .into_wire(*ty)
                    .map_err(|e| e.to_string()),
                Some(TypeNode::Scalar(Scalar::Bool)) => input
                    .as_bool()
                    .ok_or("Expected Bool")?
                    .into_wire(*ty)
                    .map_err(|e| e.to_string()),
                Some(TypeNode::Scalar(Scalar::String)) => input
                    .as_str()
                    .ok_or("Expected String")?
                    .to_owned()
                    .into_wire(*ty)
                    .map_err(|e| e.to_string()),
                Some(TypeNode::Model(receiver)) => {
                    let reference = references.first().ok_or("Missing model context")?;
                    let payload = ReferenceDomain::new_runtime(
                        receiver.clone(),
                        reference.clone(),
                        &ConversionRegistry::default(),
                    )?
                    .bind(input.as_str().ok_or("Expected model element id")?)?;
                    Ok(ValueWire {
                        ty: *ty,
                        data: DataWire::Model(payload),
                    })
                }
                _ => Err("Derived bindings support Int64, Bool, String and model arguments".into()),
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    registry
        .pin(contract)
        .map_err(|e| e.to_string())?
        .invoke(
            &arguments,
            &ExecutionContext {
                bundle: &sources.bundle,
                conversions: None,
                references: &references,
            },
            &ExecutionLimits::default(),
            budget,
        )
        .map_err(|e| e.to_string())
}
