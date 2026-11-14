//! Handwritten adapter for source-checked, pure nullable-string read functions.
//! This executes the existing Rcore machine and never grants model/store access.
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
    rcore_behavior_functions::{
        EvaluationSession, ExecutionContext, ExecutionLimits, SessionProgress,
    },
    semantic_bindings::BorrowedBindingWork,
};
use serde_json::Value;
use std::num::NonZeroUsize;

/// An explicit source selection, compiled through the shared binding checker.
/// The bounded profile is `(Option<String>) -> Option<String>` with a Pure body.
/// Callers supply admitted scalar dependencies; this is not a semantic policy,
/// a model serializer, an external-provider registry or automatic activation.
pub struct SourceScalarRead {
    bundle: DefinitionBundle,
    contract: Contract,
    argument_string: TypeId,
    result_string: TypeId,
}

fn optional_string(types: &[TypeNode], id: TypeId) -> Result<TypeId, String> {
    let Some(TypeNode::Option(inner)) = types.get(id.0 as usize) else {
        return Err("Source scalar read requires Option<String>".into());
    };
    if types.get(inner.0 as usize) != Some(&TypeNode::Scalar(Scalar::String)) {
        return Err("Source scalar read requires Option<String>".into());
    }
    Ok(*inner)
}

fn charge_string(work: &BorrowedBindingWork<'_>, bytes: usize) -> Result<(), String> {
    // Admit each chunk on the same sticky authority so a short allowance keeps
    // its consumed prefix, including a final partially affordable string fee.
    for _ in 0..=bytes.div_ceil(64) {
        work.charge(1).map_err(|error| error.to_string())?;
    }
    Ok(())
}

impl SourceScalarRead {
    pub fn compile(
        bundle: DefinitionBundle,
        entry: &SourceId,
        selection: &EntrySelection,
    ) -> Result<Self, String> {
        bundle.validate()?;
        let contract = Contract::function(&bundle, entry, selection)?;
        let descriptor = contract.descriptor();
        if descriptor.external
            || descriptor.effect != Effect::Pure
            || descriptor.parameters.len() != 1
        {
            return Err(
                "Source scalar read requires one pure, implemented nullable-string function".into(),
            );
        }
        let argument_string = optional_string(&descriptor.types, descriptor.parameters[0])?;
        let result_string = optional_string(&descriptor.types, descriptor.result)?;
        Ok(Self {
            bundle,
            contract,
            argument_string,
            result_string,
        })
    }

    /// Exact authored declaration source, including its content digest.
    pub fn source(&self) -> &SourceId {
        &self.contract.descriptor().symbol.source
    }

    /// Includes checked signature, source/dependency revisions and policies.
    pub fn contract_id(&self) -> &str {
        self.contract.id()
    }

    /// Execute the selected body over one already-admitted dependency. Every
    /// machine slice debits the caller's original allowance, including failures.
    /// The borrowed sticky work authority is never replaced by a copied budget.
    pub fn invoke(&self, input: Value, budget: &mut QueryBudget) -> Result<Value, String> {
        let work = BorrowedBindingWork::bind(budget);
        work.charge(1).map_err(|error| error.to_string())?;
        let descriptor = self.contract.descriptor();
        let data = match input {
            Value::Null => DataWire::None,
            Value::String(value) => {
                charge_string(&work, value.len())?;
                DataWire::Some(Box::new(ValueWire {
                    ty: self.argument_string,
                    data: DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::String),
                        payload: Value::String(value),
                    }),
                }))
            }
            _ => return Err("Source scalar read dependency is neither String nor null".into()),
        };
        let arguments = [ValueWire {
            ty: descriptor.parameters[0],
            data,
        }];
        let result = run_body(&self.bundle, &self.contract, &arguments, &work)?;
        work.charge(1).map_err(|error| error.to_string())?;
        if result.ty != descriptor.result {
            return Err("Source scalar read returned a foreign result type".into());
        }
        match result.data {
            DataWire::None => Ok(Value::Null),
            DataWire::Some(value) if value.ty == self.result_string => match value.data {
                DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::String),
                    payload: Value::String(value),
                }) => {
                    charge_string(&work, value.len())?;
                    Ok(Value::String(value))
                }
                _ => Err("Source scalar read returned an invalid String".into()),
            },
            _ => Err("Source scalar read returned an invalid nullable String".into()),
        }
    }
}

/// Shared borrowed runner for admitted pure scalar codecs. It never replaces
/// the caller's allowance or registers a host/native implementation.
pub(crate) fn run_body(
    bundle: &DefinitionBundle,
    contract: &Contract,
    arguments: &[ValueWire],
    work: &BorrowedBindingWork<'_>,
) -> Result<ValueWire, String> {
    let image = contract
        .reference()
        .ok_or("Source scalar read has no executable body")?;
    let context = ExecutionContext::new(bundle);
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(image, arguments, &context, &limits)
        .map_err(|error| error.to_string())?;
    loop {
        work.charge(1).map_err(|error| error.to_string())?;
        match session.resume_with_control(NonZeroUsize::MIN, work) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(result) => {
                work.check().map_err(|error| error.to_string())?;
                return Ok(result.map_err(|error| error.to_string())?.value);
            }
            SessionProgress::Finished => {
                return Err("Source scalar read finished without a result".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        rcore_behavior::modules::SymbolId,
    };

    fn compile(body: &str) -> Result<SourceScalarRead, String> {
        let source = DefinitionSource::new(
            "memory:source-scalar-read.rcore",
            SourceKind::RcoreBehavior,
            format!("rcore 1; package scalarRead version \"1\"; {body}"),
        );
        let selection = EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        };
        let entry = source.id.clone();
        SourceScalarRead::compile(
            DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
            &entry,
            &selection,
        )
    }

    #[test]
    fn checked_source_body_controls_the_result_and_identity() -> Result<(), String> {
        let identity = compile("fn read(value: Option<String>) -> Option<String> { value }")?;
        let changed = compile("fn read(value: Option<String>) -> Option<String> { None }")?;
        assert_ne!(identity.source(), changed.source());
        assert_ne!(identity.contract_id(), changed.contract_id());
        for input in [Value::Null, Value::String("P\u{1f642}".into())] {
            assert_eq!(
                identity.invoke(input.clone(), &mut QueryBudget::new(100_000))?,
                input
            );
            assert_eq!(
                changed.invoke(input, &mut QueryBudget::new(100_000))?,
                Value::Null
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_wrong_signatures_external_bodies_and_scalar_inputs() -> Result<(), String> {
        assert!(compile("fn read(value: String) -> String { value }").is_err());
        assert!(compile("extern fn read(value: Option<String>) -> Option<String>;").is_err());
        let identity = compile("fn read(value: Option<String>) -> Option<String> { value }")?;
        assert!(
            identity
                .invoke(Value::Bool(true), &mut QueryBudget::new(100_000))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn caller_work_is_exact_on_success_and_never_replenished_on_failure() -> Result<(), String> {
        let identity = compile("fn read(value: Option<String>) -> Option<String> { value }")?;
        let input = Value::String("P".into());
        let mut measured = QueryBudget::new(100_000);
        assert_eq!(identity.invoke(input.clone(), &mut measured)?, input);
        let used = measured.reads;
        assert!(used > 1);
        for limit in [0, used - 1] {
            let mut short = QueryBudget::new(limit);
            assert!(identity.invoke(input.clone(), &mut short).is_err());
            assert_eq!(short.reads, limit);
            assert_eq!(short.remaining(), 0);
        }
        let mut exact = QueryBudget::new(used);
        assert_eq!(identity.invoke(input.clone(), &mut exact)?, input);
        assert_eq!(exact.remaining(), 0);
        Ok(())
    }

    #[test]
    fn preconsumed_caller_work_retains_exact_and_short_string_chunk_ledgers() -> Result<(), String>
    {
        let identity = compile("fn read(value: Option<String>) -> Option<String> { value }")?;
        let input = Value::String("P".repeat(129));
        let mut measured = QueryBudget::new(100_000);
        assert_eq!(identity.invoke(input.clone(), &mut measured)?, input);
        let used = measured.reads;
        let prior = 7;

        let mut exact = QueryBudget::new(prior + used);
        exact.charge_many(prior)?;
        assert_eq!(identity.invoke(input.clone(), &mut exact)?, input);
        assert_eq!(exact.reads, prior + used);
        assert_eq!(exact.remaining(), 0);

        let mut short = QueryBudget::new(prior + used - 1);
        short.charge_many(prior)?;
        assert!(identity.invoke(input, &mut short).is_err());
        assert_eq!(short.reads, prior + used - 1);
        assert_eq!(short.remaining(), 0);
        Ok(())
    }
}
