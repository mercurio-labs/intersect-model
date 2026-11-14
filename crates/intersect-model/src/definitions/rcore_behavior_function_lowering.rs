//! Fresh source compilation into finite typed function/closure images.
use super::{
    SyntaxDiagnostic,
    ast::{Declaration, Expr, ExprId, Pattern, PatternId},
    bindings::Target,
    checking::{self, TypeReport},
    inference, ir,
    ir_v2::{self as image, *},
    model_types::{ModelTypes, RuntimeSchemaStore},
    modules::SymbolId,
    types::{Scalar, Type, TypeRef, TypeTable},
};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    conversion::ConversionRegistry,
};
use std::collections::{BTreeMap, BTreeSet};
#[path = "rcore_behavior_function_lowering_context.rs"]
mod context;
pub(crate) use context::{instantiate_signature, lambda_evidence};
#[path = "rcore_behavior_function_lowering_body.rs"]
mod body;
#[path = "rcore_behavior_function_getters.rs"]
mod getters;

type Result<T> = std::result::Result<T, SyntaxDiagnostic>;
type LambdaSelection = (FunctionId, TypeRef, Vec<(SourceSpan, TypeRef)>);
fn diagnostic(span: &SourceSpan, message: impl Into<String>) -> SyntaxDiagnostic {
    SyntaxDiagnostic {
        span: span.clone(),
        message: message.into(),
    }
}

pub fn compile_function(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selection: &EntrySelection,
    limits: &Limits,
) -> std::result::Result<VerifiedExecutable, Vec<SyntaxDiagnostic>> {
    compile(
        bundle,
        entry,
        selection,
        None,
        limits,
        false,
        Default::default(),
    )
}
/// Compile with an explicit caller-owned per-source parser ceiling. The normal
/// function entry point retains its original default admission policy.
pub fn compile_function_with_source_limits(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selection: &EntrySelection,
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> std::result::Result<VerifiedExecutable, Vec<SyntaxDiagnostic>> {
    compile(bundle, entry, selection, None, limits, false, source_limits)
}
/// Compile a source-authorized provider boundary. Only its entry may be external.
pub(crate) fn compile_contract(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selection: &EntrySelection,
    limits: &Limits,
) -> std::result::Result<VerifiedExecutable, Vec<SyntaxDiagnostic>> {
    compile(
        bundle,
        entry,
        selection,
        None,
        limits,
        true,
        Default::default(),
    )
}
pub fn compile_function_with_conversions(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selection: &EntrySelection,
    registry: &ConversionRegistry<'_>,
    limits: &Limits,
) -> std::result::Result<VerifiedExecutable, Vec<SyntaxDiagnostic>> {
    compile(
        bundle,
        entry,
        selection,
        Some(registry),
        limits,
        false,
        Default::default(),
    )
}
fn compile(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selection: &EntrySelection,
    registry: Option<&ConversionRegistry<'_>>,
    limits: &Limits,
    binding: bool,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> std::result::Result<VerifiedExecutable, Vec<SyntaxDiagnostic>> {
    let bounded = |id: &SourceId| {
        id.uri
            .len()
            .checked_add(id.sha256.len())
            .is_some_and(|n| n <= limits.scalar.max_source_bytes)
    };
    let fallback = SourceSpan {
        source: if bounded(entry) {
            entry.clone()
        } else {
            SourceId {
                uri: String::new(),
                sha256: String::new(),
            }
        },
        start: 0,
        end: 0,
    };
    if !bounded(entry) || !bounded(&selection.symbol.source) {
        return Err(vec![diagnostic(
            &fallback,
            "Function selector metadata limit exceeded",
        )]);
    }
    ir::preflight_bundle(bundle, &limits.scalar)
        .map_err(|e| vec![diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message)])?;
    image::preflight_selection(selection, limits)
        .map_err(|e| vec![diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message)])?;
    // Source diagnostics remain complete; single-error lowering begins only after
    // the authoritative checker has accepted every reachable declaration.
    let (report, resources) =
        checking::check_with_resources(bundle, entry, registry, source_limits)?;
    let models = lowering_models(bundle, registry, source_limits, &report, resources, None)?;
    lower_and_verify(
        bundle,
        entry,
        selection,
        registry,
        limits,
        binding,
        source_limits,
        report,
        models,
        None,
    )
    .map(|(verified, _)| verified)
}

fn lowering_models<'m>(
    bundle: &'m DefinitionBundle,
    registry: Option<&'m ConversionRegistry<'m>>,
    source_limits: crate::definitions::ecore::ParseLimits,
    report: &TypeReport,
    resources: crate::definitions::resources::ResourceSet,
    store: Option<&RuntimeSchemaStore>,
) -> std::result::Result<ModelTypes<'m>, Vec<SyntaxDiagnostic>> {
    let mut models = ModelTypes::with_source_resources(bundle, registry, source_limits, resources);
    models.attach_schema_store(store);
    models
        .configure_symbolic(&report.signatures)
        .map_err(|e| vec![diagnostic(&e.span, e.message)])?;
    Ok(models)
}

/// Runtime-schema stores for compiling functions: one shared by the lowering
/// side (source check, lowering, binding contracts) and one shared by the
/// verification side (its independent check and image verification).
/// Verification never reads what lowering prepared, so it stays independent.
///
/// Reusing one pair across many compiles of the same bundle and source limits
/// (every job of a language-pack binding build) prepares each metamodel schema
/// once per side instead of once per compile. Results are the same either way.
#[derive(Clone, Debug, Default)]
pub struct SchemaStores {
    pub lowering: RuntimeSchemaStore,
    pub verification: RuntimeSchemaStore,
}

/// Compile several entries of one source. The source is checked once for
/// lowering, and once more, independently, to verify every image; a single
/// compile checks twice per entry. Results are in `selections` order.
pub fn compile_functions_with_source_limits(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selections: &[EntrySelection],
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> std::result::Result<Vec<VerifiedExecutable>, Vec<SyntaxDiagnostic>> {
    compile_functions_with_schema_stores(
        bundle,
        entry,
        selections,
        limits,
        source_limits,
        &SchemaStores::default(),
    )
}

/// `compile_functions_with_source_limits` sharing runtime-schema preparations
/// through `stores` (see `SchemaStores`).
pub fn compile_functions_with_schema_stores(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selections: &[EntrySelection],
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
    stores: &SchemaStores,
) -> std::result::Result<Vec<VerifiedExecutable>, Vec<SyntaxDiagnostic>> {
    let fallback = SourceSpan {
        source: entry.clone(),
        start: 0,
        end: 0,
    };
    let bounded = |id: &SourceId| {
        id.uri
            .len()
            .checked_add(id.sha256.len())
            .is_some_and(|n| n <= limits.scalar.max_source_bytes)
    };
    if !bounded(entry) {
        return Err(vec![diagnostic(
            &fallback,
            "Function selector metadata limit exceeded",
        )]);
    }
    ir::preflight_bundle(bundle, &limits.scalar)
        .map_err(|e| vec![diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message)])?;
    for selection in selections {
        if !bounded(&selection.symbol.source) {
            return Err(vec![diagnostic(
                &fallback,
                "Function selector metadata limit exceeded",
            )]);
        }
        image::preflight_selection(selection, limits)
            .map_err(|e| vec![diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message)])?;
    }
    if selections.is_empty() {
        return Ok(vec![]);
    }
    let failed = |e: IrError| vec![diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message)];
    // Each side keeps one model-type instance for the whole batch, and shares
    // completed schema preparations only through its own side's store.
    let (report, resources) =
        checking::check_with_store(bundle, entry, None, source_limits, Some(&stores.lowering))?;
    let mut models = Some(lowering_models(
        bundle,
        None,
        source_limits,
        &report,
        resources,
        Some(&stores.lowering),
    )?);
    let (verification_report, resources) = image::verification_check(
        bundle,
        entry,
        None,
        source_limits,
        Some(&stores.verification),
    )
    .map_err(failed)?;
    let mut verification_models = image::verification_models(
        bundle,
        None,
        source_limits,
        &verification_report,
        resources,
        Some(&stores.verification),
    )
    .map_err(failed)?;
    let mut verified = Vec::with_capacity(selections.len());
    for selection in selections {
        let lowering = models
            .take()
            .ok_or_else(|| vec![diagnostic(&fallback, "Missing lowering model types")])?;
        let (image, lowering) = lower_and_verify(
            bundle,
            entry,
            selection,
            None,
            limits,
            false,
            source_limits,
            report.clone(),
            lowering,
            Some((&verification_report, &mut verification_models)),
        )?;
        models = Some(lowering);
        verified.push(image);
    }
    Ok(verified)
}

/// Lower one selection with configured lowering model types (returned for
/// reuse), then verify it, with shared verification model types if given.
#[allow(clippy::too_many_arguments)]
fn lower_and_verify<'m>(
    bundle: &'m DefinitionBundle,
    entry: &SourceId,
    selection: &EntrySelection,
    registry: Option<&'m ConversionRegistry<'m>>,
    limits: &'m Limits,
    binding: bool,
    source_limits: crate::definitions::ecore::ParseLimits,
    report: TypeReport,
    mut models: ModelTypes<'m>,
    verification: Option<(&TypeReport, &mut ModelTypes<'m>)>,
) -> std::result::Result<(VerifiedExecutable, ModelTypes<'m>), Vec<SyntaxDiagnostic>> {
    let fallback = SourceSpan {
        source: entry.clone(),
        start: 0,
        end: 0,
    };
    let run = || -> Result<(VerifiedExecutable, ModelTypes<'m>)> {
        let mut table = report.signatures.types.clone();
        let inputs = image::import_types(&selection.types, &mut table, &mut models, limits)
            .map_err(|e| diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message))?;
        let actuals = selection
            .arguments
            .iter()
            .map(|id| {
                inputs
                    .get(id.0 as usize)
                    .copied()
                    .ok_or_else(|| diagnostic(&fallback, "Unknown entry type argument"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut compiler = Compiler {
            external_entry: binding.then(|| selection.symbol.clone()),
            report,
            table,
            models,
            limits,
            remaining: limits.scalar.max_work,
            types: vec![],
            type_ids: BTreeMap::new(),
            tasks: vec![],
            functions: vec![],
            named: BTreeMap::new(),
            lambdas: BTreeMap::new(),
        };
        let (entry_function, _) =
            compiler.named_instance(&selection.symbol, &actuals, &fallback)?;
        let mut next = 0;
        while next < compiler.tasks.len() {
            let task = compiler.tasks[next].clone();
            let function = body::lower(&mut compiler, &task)?;
            compiler.functions[next] = Some(function);
            next += 1;
        }
        let functions = compiler
            .functions
            .into_iter()
            .map(|f| f.ok_or_else(|| diagnostic(&fallback, "Missing compiled function body")))
            .collect::<Result<Vec<_>>>()?;
        let mut sources: Vec<_> = bundle
            .sources
            .iter()
            .map(|s| ir::SourceManifestEntry {
                source: s.id.clone(),
            })
            .collect();
        sources.sort_by(|a, b| a.source.cmp(&b.source));
        let policies = image::policy_manifest(registry)
            .map_err(|e| diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message))?;
        let program = Program {
            version: image::VERSION,
            entry_source: entry.clone(),
            sources,
            policies,
            types: compiler.types,
            functions,
            entry: entry_function,
        };
        let models = compiler.models;
        let verified = if let Some(verification) = verification {
            image::context_checked(
                program,
                bundle,
                registry,
                limits,
                binding,
                source_limits,
                Some(verification),
            )
        } else if source_limits.source_bytes
            == crate::definitions::ecore::ParseLimits::default().source_bytes
        {
            match registry {
                Some(registry) => image::verify_with_conversions(program, bundle, registry, limits),
                None if binding => image::verify_contract(program, bundle, limits),
                None => image::verify(program, bundle, limits),
            }
        } else {
            image::verify_context_with_source_limits(
                program,
                bundle,
                registry,
                limits,
                binding,
                source_limits,
            )
        };
        verified
            .map(|verified| (verified, models))
            .map_err(|e| diagnostic(e.span.as_ref().unwrap_or(&fallback), e.message))
    };
    run().map_err(|e| vec![e])
}

#[derive(Clone)]
struct Task {
    external: bool,
    id: FunctionId,
    identity: FunctionIdentity,
    module: usize,
    body: ExprId,
    span: SourceSpan,
    parameters: Vec<(SourceSpan, TypeRef)>,
    result: TypeRef,
    captures: Vec<(SourceSpan, TypeRef)>,
    substitution: inference::Substitution,
}
struct Compiler<'a> {
    external_entry: Option<SymbolId>,
    report: TypeReport,
    table: TypeTable,
    models: ModelTypes<'a>,
    limits: &'a Limits,
    remaining: usize,
    types: Vec<TypeNode>,
    type_ids: BTreeMap<TypeRef, TypeId>,
    tasks: Vec<Task>,
    functions: Vec<Option<Function>>,
    named: BTreeMap<(SymbolId, Vec<TypeRef>), FunctionId>,
    lambdas: BTreeMap<(FunctionId, usize), FunctionId>,
}
impl Compiler<'_> {
    fn spend(&mut self, n: usize, span: &SourceSpan) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(n)
            .ok_or_else(|| diagnostic(span, "Function lowering work limit exceeded"))?;
        Ok(())
    }
    fn ty(&mut self, ty: TypeRef, span: &SourceSpan) -> Result<TypeId> {
        self.spend(1, span)?;
        image::export_type(
            ty,
            &self.table,
            &mut self.types,
            &mut self.type_ids,
            self.limits,
        )
        .map_err(|e| diagnostic(e.span.as_ref().unwrap_or(span), e.message))
    }
    fn substitute(
        &mut self,
        ty: TypeRef,
        values: &inference::Substitution,
        span: &SourceSpan,
    ) -> Result<TypeRef> {
        self.spend(1, span)?;
        inference::substitute(&mut self.table, &mut self.models, ty, values)
            .map_err(|e| diagnostic(span, e))
    }
    fn reserve(&mut self, span: &SourceSpan) -> Result<FunctionId> {
        self.spend(1, span)?;
        if self.tasks.len() >= self.limits.max_functions {
            return Err(diagnostic(span, "Function instance limit exceeded"));
        }
        let id = FunctionId(
            u32::try_from(self.tasks.len())
                .map_err(|_| diagnostic(span, "Function index exceeds IR range"))?,
        );
        self.functions.push(None);
        Ok(id)
    }
    fn named_instance(
        &mut self,
        symbol: &SymbolId,
        actuals: &[TypeRef],
        span: &SourceSpan,
    ) -> Result<(FunctionId, TypeRef)> {
        let signature = self
            .report
            .signatures
            .functions
            .iter()
            .find(|f| &f.symbol == symbol)
            .cloned()
            .ok_or_else(|| {
                diagnostic(
                    span,
                    "Selected function is not in the checked reachable module graph",
                )
            })?;
        let function_type =
            instantiate_signature(&signature, actuals, &mut self.table, &mut self.models)
                .map_err(|e| diagnostic(span, e))?;
        let key = (symbol.clone(), actuals.to_vec());
        if let Some(id) = self.named.get(&key) {
            return Ok((*id, function_type));
        }
        let module = self
            .report
            .signatures
            .bound
            .index
            .modules
            .iter()
            .position(|m| m.document.source.id == symbol.source)
            .ok_or_else(|| diagnostic(span, "Missing function source"))?;
        let declaration = self.report.signatures.bound.index.modules[module]
            .document
            .declarations
            .get(symbol.declaration)
            .cloned()
            .ok_or_else(|| diagnostic(span, "Missing function declaration"))?;
        let Declaration::Function {
            parameters,
            body,
            external,
            ..
        } = declaration.value
        else {
            return Err(diagnostic(span, "Selected declaration is not a function"));
        };
        if external && self.external_entry.as_ref() != Some(symbol) {
            return Err(diagnostic(
                span,
                "External functions require a registered implementation; nested provider calls are unsupported",
            ));
        }
        let Type::Function {
            parameters: parameter_types,
            result,
        } = self
            .table
            .get(function_type)
            .map_err(|e| diagnostic(span, e))?
            .clone()
        else {
            return Err(diagnostic(span, "Missing instantiated function type"));
        };
        let type_arguments = actuals
            .iter()
            .map(|ty| self.ty(*ty, span))
            .collect::<Result<Vec<_>>>()?;
        let id = self.reserve(span)?;
        self.named.insert(key, id);
        self.tasks.push(Task {
            external,
            id,
            identity: FunctionIdentity::Named {
                symbol: symbol.clone(),
                type_arguments,
            },
            module,
            body,
            span: declaration.span,
            parameters: parameters
                .iter()
                .zip(parameter_types)
                .map(|(p, ty)| (p.name.span.clone(), ty))
                .collect(),
            result,
            captures: vec![],
            substitution: signature
                .generics
                .iter()
                .zip(actuals)
                .map(|(g, ty)| (g.ty, *ty))
                .collect(),
        });
        Ok((id, function_type))
    }
    fn lambda_instance(
        &mut self,
        enclosing: &Task,
        expression: usize,
        span: &SourceSpan,
    ) -> Result<LambdaSelection> {
        if let Some(id) = self.lambdas.get(&(enclosing.id, expression)).copied() {
            let task = &self.tasks[id.0 as usize];
            let ty = self
                .table
                .intern(Type::Function {
                    parameters: task.parameters.iter().map(|(_, ty)| *ty).collect(),
                    result: task.result,
                })
                .map_err(|e| diagnostic(span, e))?;
            return Ok((id, ty, task.captures.clone()));
        }
        let source = self.report.signatures.bound.index.modules[enclosing.module]
            .document
            .source
            .id
            .clone();
        let evidence = lambda_evidence(&self.report, &source, expression, &mut self.remaining)
            .map_err(|e| diagnostic(span, e))?;
        let expected_parent = match &enclosing.identity {
            FunctionIdentity::Named { .. } => None,
            FunctionIdentity::Lambda { expression, .. } => Some(*expression as usize),
        };
        if evidence.enclosing_lambda != expected_parent {
            return Err(diagnostic(
                span,
                "Lambda does not belong to its lexical enclosing function",
            ));
        }
        let parameters = evidence
            .parameters
            .iter()
            .map(|(name, ty)| {
                Ok((
                    name.clone(),
                    self.substitute(*ty, &enclosing.substitution, span)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let result = self.substitute(evidence.result, &enclosing.substitution, span)?;
        let captures = evidence
            .captures
            .iter()
            .map(|(name, ty)| {
                Ok((
                    name.clone(),
                    self.substitute(*ty, &enclosing.substitution, span)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        if captures.len() > self.limits.max_captures {
            return Err(diagnostic(span, "Lambda capture limit exceeded"));
        }
        let function_type = self
            .table
            .intern(Type::Function {
                parameters: parameters.iter().map(|(_, ty)| *ty).collect(),
                result,
            })
            .map_err(|e| diagnostic(span, e))?;
        let id = self.reserve(span)?;
        self.lambdas.insert((enclosing.id, expression), id);
        self.tasks.push(Task {
            external: false,
            id,
            identity: FunctionIdentity::Lambda {
                enclosing: enclosing.id,
                expression: u32::try_from(expression)
                    .map_err(|_| diagnostic(span, "Lambda expression index exceeds IR range"))?,
            },
            module: enclosing.module,
            body: evidence.body,
            span: evidence.span,
            parameters,
            result,
            captures: captures.clone(),
            substitution: enclosing.substitution.clone(),
        });
        Ok((id, function_type, captures))
    }
}
