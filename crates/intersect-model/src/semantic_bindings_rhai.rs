//! Optional Rhai implementation adapter. No model semantics or cross-provider
//! dispatch live here. Script values cross the shared checked invocation boundary.
use super::*;
use ::rhai::{
    AST, CallFnOptions, Dynamic, Engine, EvalAltResult, ImmutableString, OptimizationLevel, Scope,
};
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    semantic_bindings::ModelProperty,
};

pub struct RhaiProvider {
    contract: Contract,
    ast: AST,
    identity: ImplementationId,
}
fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_optimization_level(OptimizationLevel::None);
    engine.disable_symbol("eval");
    engine.set_max_expr_depths(64, 64);
    engine.set_max_call_levels(64);
    engine.set_max_variables(256);
    engine.set_max_functions(128);
    engine.set_max_string_size(262_144);
    engine.set_max_array_size(65_536);
    engine.set_max_map_size(256);
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    engine
}
impl RhaiProvider {
    /// Parse an editable file for stub generation without executing any statements.
    pub fn functions(source: &str) -> Result<Vec<(String, usize)>> {
        if source.len() > 262_144 {
            return Err(Error::new(
                ErrorKind::ResourceExhausted,
                "Rhai source exceeds 256 KiB",
            ));
        }
        let ast = engine()
            .compile(source)
            .map_err(|e| Error::new(ErrorKind::Contract, e.to_string()))?;
        let statements: &[_] = ast.as_ref();
        if !statements.is_empty() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "Implementation files must contain functions only",
            ));
        }
        Ok(ast
            .iter_functions()
            .map(|f| (f.name.to_owned(), f.params.len()))
            .collect())
    }
    pub fn compile(contract: &Contract, uri: &str, source: &str) -> Result<Self> {
        if source.len() > 262_144 {
            return Err(Error::new(
                ErrorKind::ResourceExhausted,
                "Rhai source exceeds 256 KiB",
            ));
        }
        let descriptor = contract.descriptor();
        if descriptor.effect == Effect::Pure
            && descriptor.parameters.iter().any(|id| {
                matches!(
                    descriptor.types.get(id.0 as usize),
                    Some(TypeNode::Model(_))
                )
            })
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "Rhai model parameters require a query contract in this profile",
            ));
        }
        for id in descriptor
            .parameters
            .iter()
            .chain(std::iter::once(&descriptor.result))
        {
            supported(contract, *id, 0)?;
        }
        let retained = DefinitionSource::new(uri, SourceKind::Rcore, source);
        retained
            .validate()
            .map_err(|e| Error::new(ErrorKind::Contract, e))?;
        let mut ast = engine()
            .compile(source)
            .map_err(|e| Error::new(ErrorKind::Contract, format!("{uri}: {e}")))?;
        // Functions-only modules have no implicit per-invocation initialization.
        let statements: &[_] = ast.as_ref();
        if !statements.is_empty() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "Rhai binding files must contain functions only; top-level initialization is unsupported",
            ));
        }
        let name = contract.script_name();
        if !ast
            .iter_functions()
            .any(|f| f.name == name && f.params.len() == descriptor.parameters.len())
        {
            return Err(Error::new(
                ErrorKind::Contract,
                format!(
                    "Missing {name} with {} parameters",
                    descriptor.parameters.len()
                ),
            ));
        }
        ast.set_source(uri);
        Ok(Self {
            contract: contract.clone(),
            ast,
            identity: ImplementationId {
                backend: "rhai".into(),
                revision: format!(
                    "rhai-1.26.1/bindings-v2:{}#{}",
                    retained.id.uri, retained.id.sha256
                ),
            },
        })
    }
}
fn supported(contract: &Contract, id: TypeId, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "Rhai binding type depth exceeded",
        ));
    }
    match contract.descriptor().types.get(id.0 as usize) {
        Some(TypeNode::Scalar(Scalar::Int64 | Scalar::Bool | Scalar::String | Scalar::Unit))
        | Some(TypeNode::Model(_))
        | Some(TypeNode::Enum(_))
        | Some(TypeNode::Datatype(_)) => Ok(()),
        Some(TypeNode::Collection { element, .. } | TypeNode::Many { element, .. }) => {
            if matches!(
                contract.descriptor().types.get(element.0 as usize),
                Some(TypeNode::Model(_))
            ) {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "Collections of model handles are outside the initial Rhai binding profile",
                ));
            }
            supported(contract, *element, depth + 1)
        }
        _ => Err(Error::new(
            ErrorKind::Unsupported,
            "Rhai binding supports Int64, Bool, String, Unit, opaque nominal values, model receivers and bounded collections; no implicit numeric narrowing",
        )),
    }
}
#[derive(Clone)]
struct OpaqueValue(ValueWire);
fn script_error(error: Error) -> Box<EvalAltResult> {
    error.to_string().into()
}

/// Handwritten adapter for schema-generated getters. Reads retain model identity,
/// checked storage projection and the encompassing invocation's sticky budget.
fn property_value(handle: &ModelHandle, property: &ModelProperty) -> Result<Dynamic> {
    if let Some(message) = &property.unsupported {
        return Err(Error::new(ErrorKind::Unsupported, message));
    }
    let value = handle.read(&property.name)?;
    if property.optional && value.is_null() {
        return Ok(Dynamic::UNIT);
    }
    let invalid = || {
        Error::new(
            ErrorKind::Semantic,
            format!("Property {} does not match its checked type", property.name),
        )
    };
    match property.scalar {
        Some(Scalar::String) => value
            .as_str()
            .map(|s| Dynamic::from(s.to_owned()))
            .ok_or_else(invalid),
        Some(Scalar::Bool) => value.as_bool().map(Dynamic::from_bool).ok_or_else(invalid),
        Some(Scalar::Int8 | Scalar::Int16 | Scalar::Int32 | Scalar::Int64) => value
            .as_i64()
            .or_else(|| {
                (value["kind"] == "int64")
                    .then(|| {
                        value
                            .get("value")
                            .and_then(|v| v.as_str())
                            .and_then(|v| v.parse::<i64>().ok())
                    })
                    .flatten()
            })
            .map(Dynamic::from_int)
            .ok_or_else(invalid),
        _ => Err(Error::new(
            ErrorKind::Unsupported,
            "Unsupported Rhai property type",
        )),
    }
}

fn register_properties(engine: &mut Engine, contract: &Contract, budget: &Budget) -> Result<()> {
    let mut getters = BTreeMap::<String, Vec<_>>::new();
    for binding in contract.model_properties() {
        for property in &binding.properties {
            budget.charge(1)?;
            getters
                .entry(property.name.clone())
                .or_default()
                .push((binding.receiver.clone(), property.clone()));
        }
    }
    for (name, candidates) in getters {
        engine.register_get(
            &name,
            move |handle: &mut ModelHandle| -> std::result::Result<Dynamic, Box<EvalAltResult>> {
                for (receiver, property) in &candidates {
                    handle.budget.charge(1).map_err(script_error)?;
                    if handle.domain.runtime_target() == Some(receiver) {
                        return property_value(handle, property).map_err(script_error);
                    }
                }
                Err("Property is not declared on this model receiver".into())
            },
        );
    }
    Ok(())
}
impl Provider for RhaiProvider {
    fn contract_id(&self) -> &str {
        self.contract.id()
    }
    fn identity(&self) -> ImplementationId {
        self.identity.clone()
    }
    fn invoke(&self, call: &Call<'_>, arguments: &[ValueWire]) -> Result<ValueWire> {
        let mut engine = engine();
        let budget = call.budget.clone();
        engine.on_progress(move |_| budget.charge(1).err().map(|_| Dynamic::UNIT));
        engine.register_type_with_name::<ModelHandle>("ModelHandle");
        engine.register_type_with_name::<OpaqueValue>("NominalValue");
        register_properties(&mut engine, &self.contract, &call.budget)?;
        engine.register_fn("read_int", |handle: &mut ModelHandle, name: &str| {
            handle.read_i64(name).map_err(script_error)
        });
        engine.register_fn(
            "read_string",
            |handle: &mut ModelHandle,
             name: &str|
             -> std::result::Result<String, Box<EvalAltResult>> {
                handle
                    .read(name)
                    .map_err(script_error)?
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "Attribute is not a String".into())
            },
        );
        engine.register_fn(
            "read_bool",
            |handle: &mut ModelHandle,
             name: &str|
             -> std::result::Result<bool, Box<EvalAltResult>> {
                handle
                    .read(name)
                    .map_err(script_error)?
                    .as_bool()
                    .ok_or_else(|| "Attribute is not a Bool".into())
            },
        );
        let values = arguments
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if matches!(value.data, DataWire::Model(_)) {
                    call.model(index).map(Dynamic::from)
                } else {
                    to_dynamic(value, &call.budget)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let result = engine.call_fn_with_options::<Dynamic>(
            CallFnOptions::new().eval_ast(false),
            &mut Scope::new(),
            &self.ast,
            self.contract.script_name(),
            values,
        );
        call.budget.check()?;
        let result = result.map_err(|e| Error::new(ErrorKind::Semantic, e.to_string()))?;
        from_dynamic(
            result,
            self.contract.descriptor().result,
            &self.contract,
            &call.budget,
        )
    }
}
fn to_dynamic(value: &ValueWire, budget: &Budget) -> Result<Dynamic> {
    budget.charge(1)?;
    match &value.data {
        DataWire::Scalar(value) => match value.ty {
            ir::ValueType::Scalar(Scalar::Int64) => {
                let n = value
                    .payload
                    .get("value")
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<i64>().ok())
                    .ok_or_else(|| Error::new(ErrorKind::Arguments, "Invalid Int64"))?;
                Ok(Dynamic::from_int(n))
            }
            ir::ValueType::Scalar(Scalar::Bool) => value
                .payload
                .as_bool()
                .map(Dynamic::from_bool)
                .ok_or_else(|| Error::new(ErrorKind::Arguments, "Invalid Bool")),
            ir::ValueType::Scalar(Scalar::String) => value
                .payload
                .as_str()
                .map(|s| Dynamic::from(s.to_owned()))
                .ok_or_else(|| Error::new(ErrorKind::Arguments, "Invalid String")),
            ir::ValueType::Scalar(Scalar::Unit) => Ok(Dynamic::UNIT),
            _ => Err(Error::new(
                ErrorKind::Unsupported,
                "Unsupported Rhai scalar",
            )),
        },
        DataWire::Collection(items) => items
            .iter()
            .map(|v| to_dynamic(v, budget))
            .collect::<Result<Vec<_>>>()
            .map(Dynamic::from_array),
        DataWire::Enum(_) | DataWire::Datatype(_) => Ok(Dynamic::from(OpaqueValue(value.clone()))),
        _ => Err(Error::new(ErrorKind::Unsupported, "Unsupported Rhai input")),
    }
}
fn from_dynamic(
    value: Dynamic,
    ty: TypeId,
    contract: &Contract,
    budget: &Budget,
) -> Result<ValueWire> {
    budget.charge(1)?;
    let invalid = || {
        Error::new(
            ErrorKind::Result,
            "Rhai result does not match the contract type",
        )
    };
    match contract.descriptor().types.get(ty.0 as usize) {
        Some(TypeNode::Scalar(Scalar::Int64)) => {
            value.try_cast::<i64>().ok_or_else(invalid)?.into_wire(ty)
        }
        Some(TypeNode::Scalar(Scalar::Bool)) => {
            value.try_cast::<bool>().ok_or_else(invalid)?.into_wire(ty)
        }
        Some(TypeNode::Scalar(Scalar::String)) => value
            .try_cast::<ImmutableString>()
            .ok_or_else(invalid)?
            .to_string()
            .into_wire(ty),
        Some(TypeNode::Scalar(Scalar::Unit)) if value.is_unit() => Ok(ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Unit),
                payload: serde_json::json!({"kind":"unit"}),
            }),
        }),
        Some(TypeNode::Collection { element, .. } | TypeNode::Many { element, .. }) => {
            let items = value.try_cast::<::rhai::Array>().ok_or_else(invalid)?;
            let values = items
                .into_iter()
                .map(|v| from_dynamic(v, *element, contract, budget))
                .collect::<Result<Vec<_>>>()?;
            Ok(ValueWire {
                ty,
                data: DataWire::Collection(values),
            })
        }
        Some(TypeNode::Enum(_) | TypeNode::Datatype(_)) => value
            .try_cast::<OpaqueValue>()
            .map(|v| v.0)
            .ok_or_else(invalid),
        Some(TypeNode::Model(_)) => value
            .try_cast::<ModelHandle>()
            .map(|v| v.wire)
            .ok_or_else(invalid),
        _ => Err(invalid()),
    }
}
