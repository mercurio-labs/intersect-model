//! Source-derived declared types and function interfaces, before body checking.
use super::{
    SyntaxDiagnostic,
    ast::{self, Declaration},
    bindings::{BoundModules, LocalKind, Target, bind_with_resources},
    modules::SymbolId,
    types::{CollectionKind, Scalar, Type, TypeRef, TypeTable},
};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    continuation::{complete, pause},
    resources::ResourceSet,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize)]
pub struct GenericParameter {
    pub name: String,
    pub ty: TypeRef,
    pub bound: Option<TypeRef>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FunctionSignature {
    pub symbol: SymbolId,
    pub generics: Vec<GenericParameter>,
    pub parameters: Vec<TypeRef>,
    pub result: TypeRef,
    pub function_type: TypeRef,
}
#[derive(Clone, Debug, Serialize)]
pub struct ModuleTypes {
    pub source: SourceId,
    /// Syntax arena type ordinal to canonical type in the shared table.
    pub annotations: BTreeMap<usize, TypeRef>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Signatures {
    pub bound: BoundModules,
    pub types: TypeTable,
    pub modules: Vec<ModuleTypes>,
    pub functions: Vec<FunctionSignature>,
}

/// Checks declarations and annotations only. Bodies are bound, not type checked.
/// Always derives its own AST/bindings from validated exact source inputs.
pub fn index(
    bundle: &DefinitionBundle,
    entry: &SourceId,
) -> Result<Signatures, Vec<SyntaxDiagnostic>> {
    index_with_limits(bundle, entry, Default::default())
}

pub(crate) fn index_with_limits(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Signatures, Vec<SyntaxDiagnostic>> {
    index_with_resources(bundle, entry, limits).map(|(signatures, _)| signatures)
}

pub(crate) fn index_with_resources(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<(Signatures, ResourceSet), Vec<SyntaxDiagnostic>> {
    let (bound, resources) = bind_with_resources(bundle, entry, limits)?;
    index_prepared(bound, resources)
}

pub(super) fn index_prepared(
    bound: BoundModules,
    resources: ResourceSet,
) -> Result<(Signatures, ResourceSet), Vec<SyntaxDiagnostic>> {
    complete(index_prepared_retained(bound, resources))
}

/// Pure metadata traversal over completed source/resource inputs. No observer,
/// source import, conversion registry or provider can be retained by this future.
pub(super) async fn index_prepared_retained(
    bound: BoundModules,
    resources: ResourceSet,
) -> Result<(Signatures, ResourceSet), Vec<SyntaxDiagnostic>> {
    let mut types = TypeTable::default();
    let mut modules = Vec::new();
    let mut functions = Vec::new();
    let mut errors = Vec::new();
    for (module, bindings) in bound.index.modules.iter().zip(&bound.bindings) {
        pause().await;
        let doc = &module.document;
        let mut annotations = BTreeMap::new();
        let mut attempted = BTreeSet::new();
        // Bind by exact declaration span, never a spelling shared across functions.
        let mut generic_types = Vec::new();
        for (declaration, item) in doc.declarations.iter().enumerate() {
            pause().await;
            if let Declaration::Function { generics, .. } = &item.value {
                for (ordinal, generic) in generics.iter().enumerate() {
                    pause().await;
                    let parameter = Type::Parameter {
                        symbol: SymbolId {
                            source: doc.source.id.clone(),
                            declaration,
                        },
                        ordinal,
                    };
                    match types.intern(parameter) {
                        Ok(ty) => generic_types.push((generic.name.span.clone(), ty)),
                        Err(message) => errors.push(diagnostic(&generic.name.span, message)),
                    }
                    if let Some(root) = generic.constraint {
                        let mut pending = vec![root];
                        while let Some(id) = pending.pop() {
                            pause().await;
                            if let Some(Target::Local {
                                declaration: span,
                                kind: LocalKind::Generic,
                            }) = bindings.types.get(&id.0)
                            {
                                let mut later = false;
                                for generic in generics.iter().skip(ordinal) {
                                    pause().await;
                                    if &generic.name.span == span {
                                        later = true;
                                        break;
                                    }
                                }
                                if later {
                                    errors.push(diagnostic(
                                        &doc.types[id.0].span,
                                        "Generic bounds may reference only earlier type parameters",
                                    ));
                                }
                            }
                            pending.extend(children(&doc.types[id.0].value));
                        }
                    }
                }
            }
        }
        // Explicit postorder frames make this independent of arena allocation order.
        for root in 0..doc.types.len() {
            pause().await;
            let mut pending = vec![(root, false)];
            while let Some((ordinal, exiting)) = pending.pop() {
                pause().await;
                if attempted.contains(&ordinal) {
                    continue;
                }
                let node = &doc.types[ordinal];
                if matches!(node.value, ast::Type::Inferred) {
                    attempted.insert(ordinal);
                    continue;
                }
                if !exiting {
                    pending.push((ordinal, true));
                    pending.extend(
                        children(&node.value)
                            .into_iter()
                            .rev()
                            .map(|id| (id.0, false)),
                    );
                    continue;
                }
                attempted.insert(ordinal);
                // A child already carries a precise error; do not add cascading errors.
                if children(&node.value)
                    .iter()
                    .any(|id| !annotations.contains_key(&id.0))
                {
                    continue;
                }
                match lower(
                    &node.value,
                    bindings.types.get(&ordinal),
                    &annotations,
                    &resources,
                    &types,
                    &generic_types,
                )
                .await
                .and_then(|ty| types.intern(ty))
                {
                    Ok(ty) => {
                        annotations.insert(ordinal, ty);
                    }
                    Err(message) => errors.push(diagnostic(&node.span, message)),
                }
            }
        }
        for (ordinal, declaration) in doc.declarations.iter().enumerate() {
            pause().await;
            if let Declaration::Function {
                generics,
                parameters,
                result,
                ..
            } = &declaration.value
            {
                let generic_parameters: Option<Vec<_>> = async {
                    let mut result = Vec::new();
                    for generic in generics {
                        pause().await;
                        let ty = generic_type(&generic_types, &generic.name.span).await?;
                        let bound = match generic.constraint {
                            Some(id) => Some(*annotations.get(&id.0)?),
                            None => None,
                        };
                        result.push(GenericParameter {
                            name: generic.name.value.clone(),
                            ty,
                            bound,
                        });
                    }
                    Some(result)
                }
                .await;
                let Some(generic_parameters) = generic_parameters else {
                    continue;
                };
                let parameters: Option<Vec<_>> = async {
                    let mut result = Vec::new();
                    for parameter in parameters {
                        pause().await;
                        result.push(*annotations.get(&parameter.ty.0)?);
                    }
                    Some(result)
                }
                .await;
                if let (Some(parameters), Some(&result)) = (parameters, annotations.get(&result.0))
                {
                    match types.intern(Type::Function {
                        parameters: parameters.clone(),
                        result,
                    }) {
                        Ok(function_type) => functions.push(FunctionSignature {
                            symbol: SymbolId {
                                source: doc.source.id.clone(),
                                declaration: ordinal,
                            },
                            generics: generic_parameters,
                            parameters,
                            result,
                            function_type,
                        }),
                        Err(message) => errors.push(diagnostic(&declaration.span, message)),
                    }
                }
            }
        }
        modules.push(ModuleTypes {
            source: doc.source.id.clone(),
            annotations,
        });
    }
    if errors.is_empty() {
        errors.extend(duplicate_signatures(&bound, &types, &functions).await);
    }
    errors.sort_by(|a, b| {
        (&a.span.source, a.span.start, a.span.end, &a.message).cmp(&(
            &b.span.source,
            b.span.start,
            b.span.end,
            &b.message,
        ))
    });
    if errors.is_empty() {
        let signatures = Signatures {
            bound,
            types,
            modules,
            functions,
        };
        super::symbolic::validate_signatures(&signatures).await?;
        Ok((signatures, resources))
    } else {
        Err(errors)
    }
}

// Canonical duplicate keys are identity-only metadata, never TypeTable entries or
// substitute runtime types. Formal spelling/owner is normalized to source ordinal;
// every actual nominal identity and all structural flags remain exact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum AlphaType {
    Formal(usize),
    Node(Type),
}
async fn alpha_type(
    root: TypeRef,
    signature: &FunctionSignature,
    table: &TypeTable,
    canonical: &mut BTreeMap<AlphaType, usize>,
    done: &mut BTreeMap<TypeRef, usize>,
    remaining: &mut usize,
) -> Result<usize, String> {
    let mut pending = vec![(root, false)];
    while let Some((id, exit)) = pending.pop() {
        pause().await;
        *remaining = remaining
            .checked_sub(1)
            .ok_or("Overload signature work limit exceeded")?;
        if done.contains_key(&id) {
            continue;
        }
        let source = table.get(id)?;
        let children = super::inference::children(source);
        if !exit {
            *remaining = remaining
                .checked_sub(children.len())
                .ok_or("Overload signature work limit exceeded")?;
            pending.push((id, true));
            pending.extend(children.into_iter().map(|child| (child, false)));
            continue;
        }
        let child = |id: &mut TypeRef| -> Result<(), String> {
            *id = TypeRef(
                *done
                    .get(id)
                    .ok_or("Missing alpha-normalized signature child")?,
            );
            Ok(())
        };
        let key = if let Type::Parameter { symbol, ordinal } = source {
            if symbol == &signature.symbol {
                AlphaType::Formal(*ordinal)
            } else {
                AlphaType::Node(source.clone())
            }
        } else {
            let mut node = source.clone();
            match &mut node {
                Type::Collection { element, .. }
                | Type::Many { element, .. }
                | Type::Option(element) => child(element)?,
                Type::Tuple(items)
                | Type::SymbolicModelApplication {
                    arguments: items, ..
                }
                | Type::SymbolicDatatypeApplication {
                    arguments: items, ..
                } => {
                    for item in items {
                        pause().await;
                        child(item)?;
                    }
                }
                Type::Record(fields) => {
                    for item in fields.values_mut() {
                        pause().await;
                        child(item)?;
                    }
                }
                Type::Result { ok, error } => {
                    child(ok)?;
                    child(error)?;
                }
                Type::Function { parameters, result } => {
                    for item in parameters {
                        pause().await;
                        child(item)?;
                    }
                    child(result)?;
                }
                _ => {}
            }
            AlphaType::Node(node)
        };
        let next = canonical.len();
        let value = *canonical.entry(key).or_insert(next);
        done.insert(id, value);
    }
    done.get(&root)
        .copied()
        .ok_or_else(|| "Missing alpha-normalized signature".into())
}
async fn duplicate_signatures(
    bound: &BoundModules,
    table: &TypeTable,
    functions: &[FunctionSignature],
) -> Vec<SyntaxDiagnostic> {
    let mut canonical = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut remaining = 1_048_576usize;
    let mut errors = Vec::new();
    let mut symbols = BTreeMap::new();
    for module in &bound.index.modules {
        pause().await;
        for symbol in &module.symbols {
            pause().await;
            symbols.insert(&symbol.id, symbol);
        }
    }
    let mut counts = BTreeMap::new();
    for function in functions {
        pause().await;
        if let Some(symbol) = symbols.get(&function.symbol) {
            *counts
                .entry((&function.symbol.source, &symbol.name))
                .or_insert(0usize) += 1;
        }
    }
    for function in functions {
        pause().await;
        let Some(symbol) = symbols.get(&function.symbol) else {
            continue;
        };
        if counts
            .get(&(&function.symbol.source, &symbol.name))
            .copied()
            .unwrap_or(0)
            < 2
        {
            continue;
        }
        let mut done = BTreeMap::new();
        let key = async {
            let root = alpha_type(
                function.function_type,
                function,
                table,
                &mut canonical,
                &mut done,
                &mut remaining,
            )
            .await?;
            let mut bounds = Vec::new();
            for generic in &function.generics {
                pause().await;
                bounds.push(match generic.bound {
                    Some(ty) => Some(
                        alpha_type(
                            ty,
                            function,
                            table,
                            &mut canonical,
                            &mut done,
                            &mut remaining,
                        )
                        .await?,
                    ),
                    None => None,
                });
            }
            Ok::<_, String>((
                function.symbol.source.clone(),
                symbol.name.clone(),
                bounds,
                root,
            ))
        }
        .await;
        match key {
            Ok(key) => {
                if !seen.insert(key) {
                    errors.push(diagnostic(
                        &symbol.span,
                        format!("Duplicate overload signature {}", symbol.name.join(".")),
                    ));
                }
            }
            Err(message) => {
                errors.push(diagnostic(&symbol.span, message));
                break;
            }
        }
    }
    errors
}

fn diagnostic(span: &SourceSpan, message: impl Into<String>) -> SyntaxDiagnostic {
    SyntaxDiagnostic {
        span: span.clone(),
        message: message.into(),
    }
}
fn children(ty: &ast::Type) -> Vec<ast::TypeId> {
    match ty {
        ast::Type::Inferred => vec![],
        ast::Type::Named { arguments, .. } | ast::Type::Tuple(arguments) => arguments.clone(),
        ast::Type::Optional(inner) | ast::Type::Multiplicity { element: inner, .. } => vec![*inner],
        ast::Type::Record(fields) => fields.iter().map(|(_, id)| *id).collect(),
        ast::Type::Function { parameters, result } => {
            parameters.iter().copied().chain([*result]).collect()
        }
    }
}
async fn lower(
    ty: &ast::Type,
    target: Option<&Target>,
    annotations: &BTreeMap<usize, TypeRef>,
    resources: &ResourceSet,
    table: &TypeTable,
    generic_types: &[(SourceSpan, TypeRef)],
) -> Result<Type, String> {
    let child = |id: &ast::TypeId| {
        annotations
            .get(&id.0)
            .copied()
            .ok_or_else(|| "Missing checked child type".to_owned())
    };
    match ty {
        ast::Type::Inferred => Err("Inferred parameter requires a checked lambda context".into()),
        ast::Type::Optional(inner) => Ok(Type::Option(child(inner)?)),
        ast::Type::Multiplicity {
            element,
            lower,
            upper,
            ordered,
            unique,
        } => {
            if *upper < -1 || (*upper >= 0 && *lower > *upper as usize) {
                return Err("Invalid behavior type multiplicity".into());
            }
            let element = child(element)?;
            if *upper == 1 {
                if *lower == 0 {
                    Ok(Type::Option(element))
                } else {
                    Ok(table.get(element)?.clone())
                }
            } else {
                Ok(Type::Many {
                    element,
                    lower: *lower,
                    upper: *upper,
                    ordered: *ordered,
                    unique: *unique,
                })
            }
        }
        ast::Type::Function { parameters, result } => Ok(Type::Function {
            parameters: collect_checked(parameters, |id| child(id)).await?,
            result: child(result)?,
        }),
        ast::Type::Tuple(items) if items.is_empty() => Ok(Type::Scalar(Scalar::Unit)),
        ast::Type::Tuple(items) => Ok(Type::Tuple(collect_checked(items, |id| child(id)).await?)),
        ast::Type::Record(fields) => {
            let mut lowered = BTreeMap::new();
            for (name, ty) in fields {
                pause().await;
                if lowered.insert(name.value.clone(), child(ty)?).is_some() {
                    return Err(format!("Duplicate record field {}", name.value));
                }
            }
            Ok(Type::Record(lowered))
        }
        ast::Type::Named { arguments, .. } => match target {
            Some(Target::Builtin { version: 2, name }) => {
                if let Some(scalar) = Scalar::named(name) {
                    arity(name, arguments, 0)?;
                    return Ok(Type::Scalar(scalar));
                }
                match name.as_str() {
                    "Option" => {
                        arity(name, arguments, 1)?;
                        Ok(Type::Option(child(&arguments[0])?))
                    }
                    "Result" => {
                        arity(name, arguments, 2)?;
                        Ok(Type::Result {
                            ok: child(&arguments[0])?,
                            error: child(&arguments[1])?,
                        })
                    }
                    "Sequence" | "Set" | "OrderedSet" | "Bag" => {
                        arity(name, arguments, 1)?;
                        let kind = match name.as_str() {
                            "Sequence" => CollectionKind::Sequence,
                            "Set" => CollectionKind::Set,
                            "OrderedSet" => CollectionKind::OrderedSet,
                            _ => CollectionKind::Bag,
                        };
                        Ok(Type::Collection {
                            kind,
                            element: child(&arguments[0])?,
                        })
                    }
                    _ => Err(format!("Unsupported builtin type {name}")),
                }
            }
            Some(Target::Classifier(id)) => {
                let node = resources.node(id)?;
                let is_enum = node.attributes.iter().any(|attribute| {
                    attribute.name.namespace.as_deref() == Some(crate::definitions::ecore::XSI)
                        && attribute.name.local == "type"
                        && attribute.type_name.as_ref().is_some_and(|name| {
                            name.namespace.as_deref() == Some(crate::definitions::ecore::ECORE)
                                && name.local == "EEnum"
                        })
                });
                if is_enum {
                    arity("Enumeration", arguments, 0)?;
                    return Ok(Type::Enumeration(id.clone()));
                }
                let is_datatype = node.attributes.iter().any(|attribute| {
                    attribute.name.namespace.as_deref() == Some(crate::definitions::ecore::XSI)
                        && attribute.name.local == "type"
                        && attribute.type_name.as_ref().is_some_and(|name| {
                            name.namespace.as_deref() == Some(crate::definitions::ecore::ECORE)
                                && name.local == "EDataType"
                        })
                });
                if is_datatype {
                    let generic = node.children.iter().any(|child| {
                        child.name.namespace.is_none() && child.name.local == "eTypeParameters"
                    });
                    if !generic {
                        arity("Datatype", arguments, 0)?;
                        return Ok(Type::Datatype(id.clone()));
                    }
                    if arguments.is_empty() {
                        return Err(
                            "Generic datatype annotations require explicit closed arguments".into(),
                        );
                    }
                    let argument_types = collect_checked(arguments, |id| child(id)).await?;
                    if collect_checked(&argument_types, |ty| {
                        super::symbolic::is_symbolic(table, *ty)
                    })
                    .await?
                    .into_iter()
                    .any(|symbolic| symbolic)
                    {
                        return Ok(Type::SymbolicDatatypeApplication {
                            declaration: id.clone(),
                            arguments: argument_types,
                        });
                    }
                    let arguments = collect_checked(arguments, |argument| {
                        super::model_types::generic_argument(table.get(child(argument)?)?)
                    })
                    .await?;
                    let expression = crate::definitions::generic_types::Type::Classifier {
                        declaration: id.clone(),
                        arguments,
                        raw: false,
                    };
                    crate::definitions::generic_types::ClosedType::new(expression.clone())?;
                    return Ok(Type::DatatypeApplication(expression));
                }
                let is_class = node.attributes.iter().any(|attribute| {
                    attribute.name.namespace.as_deref() == Some(crate::definitions::ecore::XSI)
                        && attribute.name.local == "type"
                        && attribute.type_name.as_ref().is_some_and(|name| {
                            name.namespace.as_deref() == Some(crate::definitions::ecore::ECORE)
                                && name.local == "EClass"
                        })
                });
                if !is_class {
                    return Err("Enum and datatype typing require E05/E06".into());
                }
                let generic = node.children.iter().any(|child| {
                    child.name.namespace.is_none() && child.name.local == "eTypeParameters"
                });
                if !generic {
                    arity("Model classifier", arguments, 0)?;
                }
                if generic {
                    let argument_types = collect_checked(arguments, |id| child(id)).await?;
                    if collect_checked(&argument_types, |ty| {
                        super::symbolic::is_symbolic(table, *ty)
                    })
                    .await?
                    .into_iter()
                    .any(|symbolic| symbolic)
                    {
                        return Ok(Type::SymbolicModelApplication {
                            declaration: id.clone(),
                            arguments: argument_types,
                        });
                    }
                    let arguments = collect_checked(arguments, |argument| {
                        super::model_types::generic_argument(table.get(child(argument)?)?)
                    })
                    .await?;
                    let expression = crate::definitions::generic_types::Type::Classifier {
                        declaration: id.clone(),
                        raw: generic && arguments.is_empty(),
                        arguments,
                    };
                    crate::definitions::generic_types::RuntimeType::new(expression.clone())?;
                    Ok(Type::ModelApplication(expression))
                } else {
                    Ok(Type::Model(id.clone()))
                }
            }
            Some(Target::Local {
                declaration,
                kind: LocalKind::Generic,
            }) => {
                arity("Generic parameter", arguments, 0)?;
                let ty = generic_type(generic_types, declaration)
                    .await
                    .ok_or("Missing scoped generic parameter identity")?;
                Ok(table.get(ty)?.clone())
            }
            _ => Err("Type annotation has no supported resolved type identity".into()),
        },
    }
}
fn arity(name: &str, arguments: &[ast::TypeId], expected: usize) -> Result<(), String> {
    if arguments.len() == expected {
        Ok(())
    } else {
        Err(format!(
            "{name} requires {expected} type arguments, got {}",
            arguments.len()
        ))
    }
}

async fn collect_checked<T, U, E>(
    items: &[T],
    mut convert: impl FnMut(&T) -> Result<U, E>,
) -> Result<Vec<U>, E> {
    let mut result = Vec::new();
    for item in items {
        pause().await;
        result.push(convert(item)?);
    }
    Ok(result)
}
async fn generic_type(types: &[(SourceSpan, TypeRef)], target: &SourceSpan) -> Option<TypeRef> {
    for (span, ty) in types {
        pause().await;
        if span == target {
            return Some(*ty);
        }
    }
    None
}

#[cfg(test)]
mod preparation_tests {
    use super::*;
    use crate::definitions::bundle::{DefinitionSource, SourceKind};
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    type Output = Result<(Signatures, ResourceSet), Vec<SyntaxDiagnostic>>;
    fn drive(
        future: impl Future<Output = Output> + Send,
        quantum: usize,
    ) -> Result<(Output, usize), String> {
        let mut future = Box::pin(future);
        let mut polls = 0;
        loop {
            for _ in 0..quantum {
                polls += 1;
                if polls > 1_000_000 {
                    return Err("Signature future did not finish".into());
                }
                if let Poll::Ready(result) = future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    return Ok((result, polls));
                }
            }
        }
    }
    #[test]
    fn signature_frontier_groups_recovery_errors_and_owned_resources_are_preserved()
    -> Result<(), String> {
        let cases = [
            (
                SourceKind::Rcore,
                "rcore 1;package p version \"1\";metamodel \"urn:sig\" prefix p {class Node {attr label:String [0,1];}}behavior {import model \"urn:sig\" as m;fn keep<T>(x:T)->T=x;fn keep<T>(x:Sequence<T>)->Sequence<T> =x;fn shape(x:Option<Sequence<Int8>>)->Option<Sequence<Int8>> =x;fn run(n:m.Node)->String?=n.label;}",
            ),
            (
                SourceKind::RcoreBehavior,
                "rcore 1;package p version \"1\";fn bad(x:Option<Int8,Bool>)->Int8=1;",
            ),
            (
                SourceKind::RcoreBehavior,
                "rcore 1;package p version \"1\";fn same<T>(x:T)->T=x;fn same<U>(x:U)->U=x;",
            ),
        ];
        for (case, (kind, text)) in cases.into_iter().enumerate() {
            let source = DefinitionSource::new("memory:signature-preparation.rcore", kind, text);
            let id = source.id.clone();
            let original = source.text.as_ptr();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            };
            let expected = index(&bundle, &id);
            match (&expected, case) {
                (Ok(signatures), 0) => assert_eq!(signatures.functions.len(), 4),
                (Err(errors), 1) => assert!(errors.iter().any(|e| e.message.contains("Option"))),
                (Err(errors), 2) => assert!(
                    errors
                        .iter()
                        .any(|e| e.message == "Duplicate overload signature same")
                ),
                _ => return Err(format!("Unexpected signature fixture {case}: {expected:?}")),
            }
            let expected = serde_json::to_value(&expected).map_err(|e| e.to_string())?;
            let mut expected_polls = None;
            for quantum in [1, 3, 256] {
                let (bound, resources) = bind_with_resources(&bundle, &id, Default::default())
                    .map_err(|e| format!("{e:?}"))?;
                let allocation = resources
                    .document(&id)
                    .ok()
                    .map(|d| (d.source.text.as_ptr(), d.root.children.as_ptr()));
                let (output, polls) = drive(index_prepared_retained(bound, resources), quantum)?;
                assert!(polls > 10);
                if let Some(previous) = expected_polls {
                    assert_eq!(polls, previous);
                } else {
                    expected_polls = Some(polls);
                }
                let observed = match output {
                    Ok((signatures, resources)) => {
                        if let Some(allocation) = allocation {
                            let d = resources.document(&id)?;
                            assert_eq!(
                                (d.source.text.as_ptr(), d.root.children.as_ptr()),
                                allocation
                            );
                        }
                        Ok(signatures)
                    }
                    Err(errors) => Err(errors),
                };
                assert_eq!(
                    serde_json::to_value(&observed).map_err(|e| e.to_string())?,
                    expected
                );
                assert_eq!(bundle.sources[0].text.as_ptr(), original);
            }
            for prefix in 0..expected_polls.ok_or("Missing signature poll count")? {
                let (bound, resources) = bind_with_resources(&bundle, &id, Default::default())
                    .map_err(|e| format!("{e:?}"))?;
                let mut future = Box::pin(index_prepared_retained(bound, resources));
                for _ in 0..prefix {
                    assert!(
                        future
                            .as_mut()
                            .poll(&mut Context::from_waker(Waker::noop()))
                            .is_pending()
                    );
                }
                drop(future);
                assert_eq!(bundle.sources[0].text.as_ptr(), original);
                assert_eq!(
                    serde_json::to_value(index(&bundle, &id)).map_err(|e| e.to_string())?,
                    expected
                );
            }
        }
        Ok(())
    }
}
