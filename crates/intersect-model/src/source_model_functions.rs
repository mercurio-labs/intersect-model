//! Explicit supplemental read functions over the same source-bound borrowed VM.
//! This is a runtime adapter, not a language policy or a getter/operation installer.
use crate::{
    core::{query::QueryBudget, storage::ModelRead},
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        ecore::ParseLimits,
        preparation::PreparedModelDefinition,
        rcore_behavior::{
            function_lowering::compile_function_with_source_limits,
            ir,
            ir_v2::{
                DataWire, EntrySelection, Limits, TypeId, TypeNode, ValueWire, VerifiedExecutable,
            },
        },
        schema::Schema,
    },
    rcore::RcorePackage,
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    scope_runtime::{self, ScopeRead},
    semantic_bindings::BorrowedBindingWork,
    source_model_reads::SourceModelReads,
};

/// A fresh source image attached to a compiler-owned schema. Additional sources
/// have behavior kind only; the original structure, namespace selections and
/// behavior entry are preserved. Only the explicitly selected function executes.
pub struct SourceModelFunction<'a> {
    schema: &'a Schema,
    bundle: DefinitionBundle,
    executable: VerifiedExecutable,
    limits: ExecutionLimits,
}

fn scalar_parameter(types: &[TypeNode], id: TypeId) -> bool {
    match types.get(id.0 as usize) {
        Some(TypeNode::Scalar(_)) => true,
        Some(TypeNode::Option(inner)) => {
            matches!(types.get(inner.0 as usize), Some(TypeNode::Scalar(_)))
        }
        _ => false,
    }
}
fn check_signature(image: &VerifiedExecutable, schema: &Schema) -> Result<(), String> {
    let program = image.program();
    for ty in &program.types {
        if let TypeNode::Model(target) = ty {
            schema.class_key_for_type(target).map_err(|error| {
                format!("Source read model type is absent from its retained schema: {error}")
            })?;
        }
    }
    let entry = program
        .functions
        .get(program.entry.0 as usize)
        .ok_or("Missing source read entry")?;
    let Some(receiver) = entry.parameters.first() else {
        return Err("Source read requires its model receiver".into());
    };
    if !matches!(
        program.types.get(receiver.0 as usize),
        Some(TypeNode::Model(_))
    ) || !entry
        .parameters
        .iter()
        .skip(1)
        .all(|id| scalar_parameter(&program.types, *id))
    {
        return Err(
            "Source read requires one model receiver and scalar/optional-scalar arguments".into(),
        );
    }
    // Verification already bounded and checked this acyclic type graph. Walk
    // result types once to reject callable/custom-provider escape contracts.
    let mut pending = vec![entry.result];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        match program
            .types
            .get(id.0 as usize)
            .ok_or("Missing source read result type")?
        {
            TypeNode::Scalar(_) | TypeNode::Enum(_) | TypeNode::Model(_) => {}
            TypeNode::Option(inner)
            | TypeNode::Collection { element: inner, .. }
            | TypeNode::Many { element: inner, .. } => pending.push(*inner),
            TypeNode::Result { ok, error } => pending.extend([*ok, *error]),
            TypeNode::Tuple(items) => pending.extend(items),
            TypeNode::Record(fields) => pending.extend(fields.values()),
            _ => {
                return Err(
                    "Source read result excludes callable and custom datatype contracts".into(),
                );
            }
        }
    }
    Ok(())
}
impl<'a> SourceModelFunction<'a> {
    pub fn from_rcore(
        package: &'a RcorePackage,
        additional: Vec<DefinitionSource>,
        selection: &EntrySelection,
        verification: &Limits,
        limits: ExecutionLimits,
        source_limits: ParseLimits,
    ) -> Result<Self, String> {
        Self::compile(
            package.schema(),
            package.sources(),
            additional,
            selection,
            verification,
            limits,
            source_limits,
        )
    }
    pub fn from_prepared(
        definition: &'a PreparedModelDefinition<'_>,
        additional: Vec<DefinitionSource>,
        selection: &EntrySelection,
        verification: &Limits,
        limits: ExecutionLimits,
        source_limits: ParseLimits,
    ) -> Result<Self, String> {
        Self::compile(
            definition.schema(),
            definition.sources(),
            additional,
            selection,
            verification,
            limits,
            source_limits,
        )
    }
    fn compile(
        schema: &'a Schema,
        original: &DefinitionBundle,
        additional: Vec<DefinitionSource>,
        selection: &EntrySelection,
        verification: &Limits,
        limits: ExecutionLimits,
        source_limits: ParseLimits,
    ) -> Result<Self, String> {
        ir::preflight_bundle(original, &verification.scalar).map_err(|error| error.to_string())?;
        if additional.len() > verification.scalar.max_work {
            return Err("Supplemental source count exceeds compiler work limit".into());
        }
        let additional = DefinitionBundle {
            version: original.version,
            sources: additional,
            namespaces: vec![],
        };
        ir::preflight_bundle(&additional, &verification.scalar)
            .map_err(|error| error.to_string())?;
        {
            let mut uris = original
                .sources
                .iter()
                .map(|source| source.id.uri.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            for source in &additional.sources {
                if source.kind != SourceKind::RcoreBehavior {
                    return Err("Supplemental read sources must have RcoreBehavior kind".into());
                }
                if !uris.insert(source.id.uri.as_str()) {
                    return Err("Supplemental source cannot replace an existing source URI".into());
                }
            }
        }
        // Bound the original before this one definition copy. Additional text
        // moves from its caller; model data and model contexts are never copied.
        let mut bundle = original.clone();
        bundle
            .sources
            .try_reserve_exact(additional.sources.len())
            .map_err(|_| "Supplemental source allocation failed")?;
        bundle.sources.extend(additional.sources);
        ir::preflight_bundle(&bundle, &verification.scalar).map_err(|error| error.to_string())?;
        bundle.validate()?;
        let executable = compile_function_with_source_limits(
            &bundle,
            &selection.symbol.source,
            selection,
            verification,
            source_limits,
        )
        .map_err(|errors| format!("Source read compilation failed: {errors:?}"))?;
        check_signature(&executable, schema)?;
        Ok(Self {
            schema,
            bundle,
            executable,
            limits,
        })
    }
    pub fn schema(&self) -> &'a Schema {
        self.schema
    }
    /// Retained definitions, including unconsumed sources. Presence is not a
    /// statement that every source body participates in this selected function.
    pub fn sources(&self) -> &DefinitionBundle {
        &self.bundle
    }
    pub fn executable(&self) -> &VerifiedExecutable {
        &self.executable
    }

    /// Run against an explicit read capability. Tail argument data moves into
    /// ordinary VM admission; no copied allowance or raw graph is substituted.
    pub fn invoke_with(
        &self,
        reads: &dyn ScopeRead,
        owner: &str,
        arguments: Vec<DataWire>,
        budget: &mut QueryBudget,
    ) -> Result<ValueWire, String> {
        let work = BorrowedBindingWork::bind(budget);
        self.invoke_with_work(reads, owner, arguments, &work)
    }
    pub(crate) fn invoke_with_work(
        &self,
        reads: &dyn ScopeRead,
        owner: &str,
        arguments: Vec<DataWire>,
        work: &BorrowedBindingWork<'_>,
    ) -> Result<ValueWire, String> {
        scope_runtime::evaluate_read_function(
            &self.executable,
            &ExecutionContext::new(&self.bundle),
            self.schema,
            reads,
            owner,
            arguments,
            &self.limits,
            work,
        )
        .map_err(|error| error.to_string())
    }
    pub fn invoke_model(
        &self,
        model: &dyn ModelRead,
        owner: &str,
        arguments: Vec<DataWire>,
        budget: &mut QueryBudget,
    ) -> Result<ValueWire, String> {
        self.invoke_with(
            &SourceModelReads::new(model, self.schema),
            owner,
            arguments,
            budget,
        )
    }
    /// Compatibility entry for checked leaves that borrow the original native
    /// ledger. Domain decisions remain in the selected source function.
    pub fn invoke_callbacks(
        &self,
        reads: &dyn crate::source_read_callbacks::SourceReadCallbacks,
        owner: &str,
        arguments: Vec<DataWire>,
        budget: &mut QueryBudget,
    ) -> Result<ValueWire, String> {
        let work = BorrowedBindingWork::bind(budget);
        let reads = crate::source_read_callbacks::CallbackReads {
            reads,
            work: &work,
            failure: std::cell::RefCell::new(None),
        };
        let result = self.invoke_with_work(&reads, owner, arguments, &work);
        if let Some(error) = reads.failure.borrow_mut().take() {
            return Err(error);
        }
        result
    }
}
