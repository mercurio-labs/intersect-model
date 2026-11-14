//! Handwritten scalar codecs over the shared source-checked borrowed VM runner.
//! Language decisions belong in the selected Rcore body, never in this adapter.
use crate::{
    core::query::QueryBudget,
    definitions::{
        bundle::{DefinitionBundle, SourceId},
        rcore_behavior::{
            ir,
            ir_v2::{DataWire, Effect, EntrySelection, TypeId, TypeNode, ValueWire},
            types::Scalar,
        },
        semantic_bindings::Contract,
    },
    rcore_behavior_functions::ExecutionLimits,
    semantic_bindings::BorrowedBindingWork,
    source_scalar_reads::run_body,
};
use serde_json::Value;

/// Explicit pure source function with `Bool` and `Option<String>` parameters/result.
/// This bounded codec grants no model access, virtual dispatch or host imports.
pub struct SourceScalarFunction {
    bundle: DefinitionBundle,
    contract: Contract,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarType {
    Bool,
    OptionalString,
}
fn scalar_type(types: &[TypeNode], id: TypeId) -> Option<ScalarType> {
    match types.get(id.0 as usize) {
        Some(TypeNode::Scalar(Scalar::Bool)) => Some(ScalarType::Bool),
        Some(TypeNode::Option(inner))
            if types.get(inner.0 as usize) == Some(&TypeNode::Scalar(Scalar::String)) =>
        {
            Some(ScalarType::OptionalString)
        }
        _ => None,
    }
}
fn charge_string(work: &BorrowedBindingWork<'_>, bytes: usize) -> Result<(), String> {
    for _ in 0..=bytes.div_ceil(64) {
        work.charge(1).map_err(|error| error.to_string())?;
    }
    Ok(())
}
impl SourceScalarFunction {
    pub fn compile(
        bundle: DefinitionBundle,
        entry: &SourceId,
        selection: &EntrySelection,
    ) -> Result<Self, String> {
        bundle.validate()?;
        let contract = Contract::function(&bundle, entry, selection)?;
        let descriptor = contract.descriptor();
        if descriptor.external || descriptor.effect != Effect::Pure {
            return Err("Source scalar function requires a pure implemented body".into());
        }
        if !descriptor
            .parameters
            .iter()
            .chain(std::iter::once(&descriptor.result))
            .all(|id| scalar_type(&descriptor.types, *id).is_some())
        {
            return Err("Source scalar function supports Bool and Option<String> only".into());
        }
        Ok(Self { bundle, contract })
    }
    pub fn source(&self) -> &SourceId {
        &self.contract.descriptor().symbol.source
    }
    pub fn contract_id(&self) -> &str {
        self.contract.id()
    }
    /// Explicit compatibility ABI check against the retained authored signature.
    pub fn check_signature(
        &self,
        parameters: &[ScalarType],
        result: ScalarType,
    ) -> Result<(), String> {
        let descriptor = self.contract.descriptor();
        if descriptor.parameters.len() != parameters.len()
            || !descriptor
                .parameters
                .iter()
                .zip(parameters)
                .all(|(id, expected)| scalar_type(&descriptor.types, *id) == Some(*expected))
            || scalar_type(&descriptor.types, descriptor.result) != Some(result)
        {
            return Err("Source scalar function differs from the required signature".into());
        }
        Ok(())
    }
    /// Inputs use JSON bool/string/null, checked against each authored parameter.
    /// All VM slices and codec storage/work debit the original sticky allowance.
    pub fn invoke(&self, inputs: &[Value], budget: &mut QueryBudget) -> Result<Value, String> {
        let work = BorrowedBindingWork::bind(budget);
        work.charge(1).map_err(|error| error.to_string())?;
        let descriptor = self.contract.descriptor();
        if inputs.len() != descriptor.parameters.len() {
            return Err("Source scalar function argument count differs".into());
        }
        let bytes = inputs
            .len()
            .checked_mul(crate::logical_size!(ValueWire, 152))
            .ok_or("Source scalar argument storage overflow")?;
        for _ in 0..=bytes.div_ceil(64) {
            work.charge(1).map_err(|error| error.to_string())?;
        }
        let mut arguments = Vec::new();
        arguments
            .try_reserve_exact(inputs.len())
            .map_err(|_| "Source scalar argument allocation failed")?;
        let mut string_bytes = 0usize;
        for (input, ty) in inputs.iter().zip(&descriptor.parameters) {
            work.charge(1).map_err(|error| error.to_string())?;
            let data = match (descriptor.types.get(ty.0 as usize), input) {
                (Some(TypeNode::Scalar(Scalar::Bool)), Value::Bool(value)) => {
                    DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::Bool),
                        payload: Value::Bool(*value),
                    })
                }
                (Some(TypeNode::Option(_)), Value::Null) => DataWire::None,
                (Some(TypeNode::Option(inner)), Value::String(value)) => {
                    charge_string(&work, value.len())?;
                    string_bytes = string_bytes
                        .checked_add(value.len())
                        .filter(|bytes| *bytes <= ExecutionLimits::default().scalar.max_value_bytes)
                        .ok_or("Source scalar input string storage limit exceeded")?;
                    let mut admitted = String::new();
                    admitted
                        .try_reserve_exact(value.len())
                        .map_err(|_| "Source scalar input string allocation failed")?;
                    admitted.push_str(value);
                    DataWire::Some(Box::new(ValueWire {
                        ty: *inner,
                        data: DataWire::Scalar(ir::Value {
                            ty: ir::ValueType::Scalar(Scalar::String),
                            payload: Value::String(admitted),
                        }),
                    }))
                }
                _ => return Err("Source scalar function argument type differs".into()),
            };
            arguments.push(ValueWire { ty: *ty, data });
        }
        let result = run_body(&self.bundle, &self.contract, &arguments, &work)?;
        work.charge(1).map_err(|error| error.to_string())?;
        if result.ty != descriptor.result {
            return Err("Source scalar function returned a foreign result type".into());
        }
        match (descriptor.types.get(result.ty.0 as usize), result.data) {
            (
                Some(TypeNode::Scalar(Scalar::Bool)),
                DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::Bool),
                    payload: Value::Bool(value),
                }),
            ) => Ok(Value::Bool(value)),
            (Some(TypeNode::Option(_)), DataWire::None) => Ok(Value::Null),
            (Some(TypeNode::Option(inner)), DataWire::Some(value)) if *inner == value.ty => {
                match value.data {
                    DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::String),
                        payload: Value::String(value),
                    }) => {
                        charge_string(&work, value.len())?;
                        Ok(Value::String(value))
                    }
                    _ => Err("Source scalar function returned an invalid String".into()),
                }
            }
            _ => Err("Source scalar function result type differs".into()),
        }
    }
}
