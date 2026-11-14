//! Source-derived function and lambda evidence shared with the v2 verifier.
use super::*;
use crate::definitions::rcore_behavior::{ast::Document, inference, signatures::FunctionSignature};

pub(crate) fn instantiate_signature(
    signature: &FunctionSignature,
    actuals: &[TypeRef],
    table: &mut TypeTable,
    models: &mut ModelTypes<'_>,
) -> std::result::Result<TypeRef, String> {
    if signature.generics.len() != actuals.len() {
        return Err("Function instance requires the complete ordered type argument list".into());
    }
    let mut inference = inference::Inference::default();
    for (formal, actual) in signature.generics.iter().zip(actuals) {
        inference.fixed.insert(formal.ty, *actual);
    }
    let values = inference.solve(signature, table, models, &BTreeMap::new(), true)?;
    inference::substitute(table, models, signature.function_type, &values)
}

#[derive(Clone)]
pub(crate) struct LambdaEvidence {
    pub span: SourceSpan,
    pub body: ExprId,
    pub parameters: Vec<(SourceSpan, TypeRef)>,
    pub result: TypeRef,
    pub captures: Vec<(SourceSpan, TypeRef)>,
    pub enclosing_lambda: Option<usize>,
    pub enclosing_symbol: SymbolId,
}

pub(crate) fn lambda_evidence(
    report: &TypeReport,
    source: &SourceId,
    expression: usize,
    remaining: &mut usize,
) -> std::result::Result<LambdaEvidence, String> {
    let module = report
        .signatures
        .bound
        .index
        .modules
        .iter()
        .position(|m| &m.document.source.id == source)
        .ok_or("Unknown lambda source")?;
    let doc = &report.signatures.bound.index.modules[module].document;
    let typed = report
        .modules
        .iter()
        .find(|m| &m.source == source)
        .ok_or("Missing lambda type report")?;
    let bindings = &report.signatures.bound.bindings[module];
    let node = doc
        .expressions
        .get(expression)
        .ok_or("Unknown lambda expression")?;
    let Expr::Lambda { parameters, body } = &node.value else {
        return Err("Lambda identity does not refer to source lambda".into());
    };
    let ty = typed
        .expressions
        .get(&expression)
        .ok_or("Missing checked lambda type")?;
    let Type::Function {
        parameters: types,
        result,
    } = report.signatures.types.get(*ty)?
    else {
        return Err("Source lambda is not function typed".into());
    };
    if parameters.len() != types.len() {
        return Err("Lambda parameter inventory differs from checked signature".into());
    }
    let mut captures = BTreeMap::new();
    let mut pending = vec![*body];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        *remaining = remaining
            .checked_sub(1)
            .ok_or("Lambda capture work limit exceeded")?;
        if !seen.insert(id.0) {
            continue;
        }
        if let Some(Target::Local { declaration, kind }) = bindings.expressions.get(&id.0)
            && !(matches!(
                kind,
                crate::definitions::rcore_behavior::bindings::LocalKind::Generic
            ) || (declaration.source == node.span.source
                && declaration.start >= node.span.start
                && declaration.end <= node.span.end))
        {
            let ty = *typed
                .expressions
                .get(&id.0)
                .ok_or("Missing checked capture type")?;
            let key = (
                declaration.source.clone(),
                declaration.start,
                declaration.end,
            );
            if let Some((_, previous)) = captures.insert(key, (declaration.clone(), ty))
                && previous != ty
            {
                return Err("Captured declaration has inconsistent checked types".into());
            }
        }
        pending.extend(expression_children(doc, id)?);
    }
    let mut enclosing_lambda = None;
    let mut enclosing_width = usize::MAX;
    for (id, parent) in doc.expressions.iter().enumerate() {
        *remaining = remaining
            .checked_sub(1)
            .ok_or("Lambda source ownership work limit exceeded")?;
        if id != expression
            && matches!(parent.value, Expr::Lambda { .. })
            && parent.span.start <= node.span.start
            && parent.span.end >= node.span.end
        {
            // Generated comprehension lambdas have a source range that includes
            // their generator source. That source executes outside the new
            // lambda: actual expression ancestry, not range overlap, owns it.
            let Expr::Lambda {
                body: parent_body, ..
            } = parent.value
            else {
                continue;
            };
            let mut ancestry = vec![parent_body];
            let mut contains = false;
            while let Some(child) = ancestry.pop() {
                *remaining = remaining
                    .checked_sub(1)
                    .ok_or("Lambda ancestry work limit exceeded")?;
                if child.0 == expression {
                    contains = true;
                    break;
                }
                ancestry.extend(expression_children(doc, child)?);
            }
            if !contains {
                continue;
            }
            let width = parent.span.end - parent.span.start;
            if width < enclosing_width {
                enclosing_lambda = Some(id);
                enclosing_width = width;
            }
        }
    }
    let declaration = doc
        .declarations
        .iter()
        .position(|d| {
            matches!(d.value, Declaration::Function { .. })
                && d.span.start <= node.span.start
                && d.span.end >= node.span.end
        })
        .ok_or("Lambda lacks a named source function owner")?;
    Ok(LambdaEvidence {
        span: node.span.clone(),
        body: *body,
        parameters: parameters
            .iter()
            .zip(types)
            .map(|(p, ty)| (p.name.span.clone(), *ty))
            .collect(),
        result: *result,
        captures: captures.into_values().collect(),
        enclosing_lambda,
        enclosing_symbol: SymbolId {
            source: source.clone(),
            declaration,
        },
    })
}

pub(super) fn expression_children(
    doc: &Document,
    id: ExprId,
) -> std::result::Result<Vec<ExprId>, String> {
    Ok(
        match &doc
            .expressions
            .get(id.0)
            .ok_or("Missing checked source expression")?
            .value
        {
            Expr::Group(a) | Expr::Unary { operand: a, .. } => vec![*a],
            Expr::List(items) | Expr::Tuple(items) => items.clone(),
            Expr::Record(fields) => fields.iter().map(|(_, e)| *e).collect(),
            Expr::Binary { left, right, .. } => vec![*left, *right],
            Expr::Member { receiver, .. } => vec![*receiver],
            Expr::Call {
                callee, arguments, ..
            } => std::iter::once(*callee)
                .chain(arguments.iter().copied())
                .collect(),
            Expr::Index { receiver, index } => vec![*receiver, *index],
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => vec![*condition, *then_branch, *else_branch],
            Expr::Block { bindings, result } => bindings
                .iter()
                .map(|b| b.value.value)
                .chain(std::iter::once(*result))
                .collect(),
            Expr::Lambda { body, .. } => vec![*body],
            Expr::Match { value, arms } => std::iter::once(*value)
                .chain(arms.iter().map(|(_, body)| *body))
                .collect(),
            _ => vec![],
        },
    )
}
