//! Source-owned derived-feature contracts, compiled by the existing Rcore pipeline.
use super::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    ecore::ParseLimits,
    generic_types::RuntimeType,
    rcore_behavior::{
        self, SyntaxDiagnostic,
        ast::{Declaration, Expr},
        bindings::Target,
        ir_v2::{self, EntrySelection, Limits, VerifiedExecutable},
        model_types::ModelTypes,
        types::TypeTable,
    },
    schema::MemberIdentity,
};

/// Shared storage projection profile. This validates a member shape; only fresh
/// source/function checking installs executable getter authority.
pub(crate) fn check_storage_feature(
    schema: &super::schema::Schema,
    feature: &super::schema::Feature,
) -> Result<(), String> {
    let admitted = if feature.reference {
        schema.classes.contains_key(&feature.target)
            && schema.declarations.contains_key(&feature.target)
    } else {
        feature.upper == 1 && matches!(feature.target.as_str(), "EString" | "EBoolean" | "ELong")
    };
    if !feature.derived || feature.datatype.is_some() || feature.unsettable || !admitted {
        return Err("Source getter requires an admitted scalar or source-declared reference feature contract".into());
    }
    Ok(())
}

/// Reconstruct source getter selector data from the exact authored entry.
/// This is inspection only; it installs no executable or read capability.
pub fn source_getter_rules(
    source: &super::bundle::DefinitionSource,
    schema: &super::schema::Schema,
    limits: ParseLimits,
) -> Result<Vec<super::schema::Derivation>, Vec<SyntaxDiagnostic>> {
    let span = SourceSpan {
        source: source.id.clone(),
        start: 0,
        end: 0,
    };
    let error = |message: String| {
        vec![SyntaxDiagnostic {
            span: span.clone(),
            message,
        }]
    };
    if !matches!(
        source.kind,
        super::bundle::SourceKind::Rcore | super::bundle::SourceKind::RcoreBehavior
    ) {
        return Err(error("Getter selectors require authored Rcore".into()));
    }
    if source.text.len() > limits.source_bytes {
        return Err(error(
            "Getter source exceeds the host source ceiling".into(),
        ));
    }
    source.validate().map_err(error)?;
    let document = rcore_behavior::parse_with_limits(source.clone(), limits);
    if !document.diagnostics.is_empty() {
        return Err(document.diagnostics);
    }
    let rules = super::rcore_behavior_rules::lower_getter_selectors(&document, schema).map_err(
        |errors| {
            errors
                .into_iter()
                .map(|e| SyntaxDiagnostic {
                    span: SourceSpan {
                        source: source.id.clone(),
                        start: e.start,
                        end: e.end,
                    },
                    message: e.message,
                })
                .collect::<Vec<_>>()
        },
    )?;
    Ok(rules
        .derived
        .into_iter()
        .filter(|rule| matches!(rule.expression, super::schema::Expression::Source { .. }))
        .collect())
}

/// Only fresh source checking can construct this contract. Cached selectors,
/// serialized IR and imported derived flags do not install executable getters.
#[derive(Clone, Debug)]
pub struct VerifiedGetter {
    receiver: RuntimeType,
    member: MemberIdentity,
    span: SourceSpan,
    executable: VerifiedExecutable,
}
impl VerifiedGetter {
    pub fn receiver(&self) -> &RuntimeType {
        &self.receiver
    }
    pub fn member(&self) -> &MemberIdentity {
        &self.member
    }
    pub fn span(&self) -> &SourceSpan {
        &self.span
    }
    pub fn executable(&self) -> &VerifiedExecutable {
        &self.executable
    }
}

pub(crate) struct SelectedGetter<'a> {
    pub function: &'a super::rcore_behavior::signatures::FunctionSignature,
    pub provider: &'a super::rcore_behavior::checking::ProviderType,
    pub feature: &'a super::rcore_behavior::model_types::FeatureType,
    pub span: SourceSpan,
}

/// Select the one direct source function and its exact checked derive contract.
/// Shared by top-level installation and composed source property calls.
pub(crate) fn select_source_getter<'a>(
    report: &'a super::rcore_behavior::checking::TypeReport,
    entry: &SourceId,
    declaration: usize,
) -> Result<SelectedGetter<'a>, Vec<SyntaxDiagnostic>> {
    let fallback = SourceSpan {
        source: entry.clone(),
        start: 0,
        end: 0,
    };
    let diagnostic = |span: &SourceSpan, message: String| {
        vec![SyntaxDiagnostic {
            span: span.clone(),
            message,
        }]
    };
    let module = report
        .signatures
        .bound
        .index
        .module(entry)
        .map_err(|e| diagnostic(&fallback, e))?;
    let selected = module
        .document
        .declarations
        .get(declaration)
        .ok_or_else(|| diagnostic(&fallback, "Missing getter declaration".into()))?;
    let Declaration::Derive { receiver, body, .. } = &selected.value else {
        return Err(diagnostic(
            &selected.span,
            "Getter selection requires a derive declaration".into(),
        ));
    };
    let document = &module.document;
    let Some(Expr::Call {
        callee,
        types,
        arguments,
    }) = document.expressions.get(body.0).map(|node| &node.value)
    else {
        return Err(diagnostic(
            &selected.span,
            "Source getter requires a direct source function call".into(),
        ));
    };
    if !types.is_empty()
        || arguments.len() != 1
        || !matches!(document.expressions.get(arguments[0].0).map(|node| &node.value),Some(Expr::Name(name)) if name.value == receiver.value)
    {
        return Err(diagnostic(
            &selected.span,
            "Source getter call requires exactly its receiver argument".into(),
        ));
    }
    let bindings = report
        .signatures
        .bound
        .bindings
        .iter()
        .find(|module| module.source == *entry)
        .ok_or_else(|| diagnostic(&selected.span, "Missing getter bindings".into()))?;
    let Some(Target::Symbol(symbol)) = bindings.expressions.get(&callee.0) else {
        return Err(diagnostic(
            &selected.span,
            "Source getter requires one unambiguous named function".into(),
        ));
    };
    let function = report
        .signatures
        .functions
        .iter()
        .find(|function| function.symbol == *symbol)
        .ok_or_else(|| diagnostic(&selected.span, "Missing source getter signature".into()))?;
    if !function.generics.is_empty() {
        return Err(diagnostic(
            &selected.span,
            "Generic getter entries require explicit specialization; unsupported".into(),
        ));
    }
    let provider = report
        .modules
        .iter()
        .find(|module| module.source == *entry)
        .and_then(|module| module.providers.get(&declaration))
        .ok_or_else(|| diagnostic(&selected.span, "Missing checked getter feature".into()))?;
    let feature = provider.feature.as_ref().ok_or_else(|| {
        diagnostic(
            &selected.span,
            "Getter requires an exact feature contract".into(),
        )
    })?;
    if !feature.derived || !feature.assignment_exact {
        return Err(diagnostic(
            &selected.span,
            "Source getter requires an exact derived feature value contract".into(),
        ));
    }
    Ok(SelectedGetter {
        function,
        provider,
        feature,
        span: selected.span.clone(),
    })
}

/// Compile `derive m.Class.feature(self) -> T = implementation(self);`.
/// The direct nongeneric source function must have the exact receiver and
/// feature result ABI. Its body uses the ordinary expression/function language.
pub fn compile_getter(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    declaration: usize,
    limits: &Limits,
    source_limits: ParseLimits,
) -> Result<VerifiedGetter, Vec<SyntaxDiagnostic>> {
    let diagnostic = |span: &SourceSpan, message: String| {
        vec![SyntaxDiagnostic {
            span: span.clone(),
            message,
        }]
    };
    let (report, resources) =
        rcore_behavior::checking::check_with_resources(bundle, entry, None, source_limits)?;
    let selected = select_source_getter(&report, entry, declaration)?;
    let feature = selected.feature;
    let provider = selected.provider;
    let symbol = &selected.function.symbol;
    let mut expected = Vec::new();
    let mut mapped = std::collections::BTreeMap::new();
    let receiver_type = ir_v2::export_type(
        provider.receiver,
        &report.signatures.types,
        &mut expected,
        &mut mapped,
        limits,
    )
    .map_err(|e| diagnostic(&selected.span, e.to_string()))?;
    let result_type = ir_v2::export_type(
        feature.value,
        &report.signatures.types,
        &mut expected,
        &mut mapped,
        limits,
    )
    .map_err(|e| diagnostic(&selected.span, e.to_string()))?;
    let executable = rcore_behavior::function_lowering::compile_function_with_source_limits(
        bundle,
        entry,
        &EntrySelection {
            symbol: symbol.clone(),
            types: vec![],
            arguments: vec![],
        },
        limits,
        source_limits,
    )?;
    let program = executable.program();
    let callable = program
        .functions
        .get(program.entry.0 as usize)
        .ok_or_else(|| diagnostic(&selected.span, "Missing verified getter entry".into()))?;
    let mut table = TypeTable::default();
    let mut models = ModelTypes::with_source_resources(bundle, None, source_limits, resources);
    let wanted = ir_v2::import_types(&expected, &mut table, &mut models, limits)
        .map_err(|e| diagnostic(&selected.span, e.to_string()))?;
    let found = ir_v2::import_types(&program.types, &mut table, &mut models, limits)
        .map_err(|e| diagnostic(&selected.span, e.to_string()))?;
    if callable.parameters.len() != 1
        || found.get(callable.parameters[0].0 as usize) != wanted.get(receiver_type.0 as usize)
        || found.get(callable.result.0 as usize) != wanted.get(result_type.0 as usize)
    {
        return Err(diagnostic(
            &selected.span,
            "Getter function differs from the exact receiver or feature result contract".into(),
        ));
    }
    Ok(VerifiedGetter {
        receiver: feature.receiver.clone(),
        member: MemberIdentity::Source {
            declaration: feature.declaration.clone(),
        },
        span: selected.span.clone(),
        executable,
    })
}
