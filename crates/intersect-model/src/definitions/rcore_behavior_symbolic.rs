//! Source-owned nominal bridge; no runtime or closed-type certificates are forged.
use super::{
    signatures::Signatures,
    types::{Type, TypeRef, TypeTable},
};
use crate::definitions::continuation::{complete, pause};
use crate::definitions::generic_types::{RigidParameterId, Type as GenericType};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn identity(ty: &Type) -> Result<RigidParameterId, String> {
    match ty {
        Type::Parameter { symbol, ordinal } => Ok(RigidParameterId {
            source: symbol.source.clone(),
            declaration: symbol.declaration,
            parameter: *ordinal,
        }),
        _ => Err("Expected a scoped Rcore parameter".into()),
    }
}
pub(super) fn expression(table: &TypeTable, root: TypeRef) -> Result<GenericType, String> {
    fn visit(
        table: &TypeTable,
        root: TypeRef,
        depth: usize,
        budget: &mut usize,
    ) -> Result<GenericType, String> {
        *budget = budget
            .checked_sub(1)
            .ok_or("Symbolic application work limit exceeded")?;
        if depth > 64 {
            return Err("Symbolic application nesting exceeds 64 levels".into());
        }
        match table.get(root)? {
            parameter @ Type::Parameter { .. } => Ok(GenericType::RigidParameter {
                identity: identity(parameter)?,
            }),
            Type::SymbolicModelApplication {
                declaration,
                arguments,
            }
            | Type::SymbolicDatatypeApplication {
                declaration,
                arguments,
            } => Ok(GenericType::Classifier {
                declaration: declaration.clone(),
                raw: false,
                arguments: arguments
                    .iter()
                    .map(|ty| visit(table, *ty, depth + 1, budget))
                    .collect::<Result<_, _>>()?,
            }),
            concrete => super::model_types::generic_argument(concrete),
        }
    }
    visit(table, root, 0, &mut 16384)
}
pub(super) fn is_symbolic(table: &TypeTable, ty: TypeRef) -> Result<bool, String> {
    Ok(matches!(
        table.get(ty)?,
        Type::Parameter { .. }
            | Type::SymbolicModelApplication { .. }
            | Type::SymbolicDatatypeApplication { .. }
    ))
}
fn nominal_bound(
    table: &TypeTable,
    ty: TypeRef,
    bounds: &BTreeMap<TypeRef, TypeRef>,
    seen: &mut BTreeSet<TypeRef>,
    budget: &mut usize,
) -> Result<bool, String> {
    complete(nominal_bound_retained(table, ty, bounds, seen, budget))
}
async fn nominal_bound_retained(
    table: &TypeTable,
    ty: TypeRef,
    bounds: &BTreeMap<TypeRef, TypeRef>,
    seen: &mut BTreeSet<TypeRef>,
    budget: &mut usize,
) -> Result<bool, String> {
    pause().await;
    *budget = budget
        .checked_sub(1)
        .ok_or("Nominal generic bound work limit exceeded")?;
    if seen.len() >= 64 {
        return Err("Nominal generic bound nesting exceeds 64 levels".into());
    }
    if !seen.insert(ty) {
        return Err("Cyclic nominal generic bound".into());
    }
    let result = match table.get(ty)? {
        Type::Parameter { .. } => match bounds.get(&ty) {
            Some(bound) => {
                Box::pin(nominal_bound_retained(table, *bound, bounds, seen, budget)).await?
            }
            None => false,
        },
        Type::Model(_)
        | Type::ModelApplication(_)
        | Type::Enumeration(_)
        | Type::Datatype(_)
        | Type::DatatypeApplication(_) => true,
        Type::SymbolicModelApplication { arguments, .. }
        | Type::SymbolicDatatypeApplication { arguments, .. } => {
            let mut valid = true;
            for argument in arguments {
                pause().await;
                valid &= Box::pin(nominal_bound_retained(
                    table, *argument, bounds, seen, budget,
                ))
                .await?;
            }
            valid
        }
        _ => false,
    };
    seen.remove(&ty);
    Ok(result)
}
/// This is called only with freshly indexed signatures, never deserialized reports.
pub(super) fn entries(
    signatures: &Signatures,
) -> Result<
    Vec<(
        RigidParameterId,
        Vec<GenericType>,
        crate::definitions::bundle::SourceSpan,
    )>,
    crate::definitions::ecore::ImportError,
> {
    let bounds: BTreeMap<_, _> = signatures
        .functions
        .iter()
        .flat_map(|f| &f.generics)
        .filter_map(|g| g.bound.map(|bound| (g.ty, bound)))
        .collect();
    let mut entries = Vec::new();
    for function in &signatures.functions {
        for (ordinal, generic) in function.generics.iter().enumerate() {
            let Some(bound) = generic.bound else { continue };
            let fallback = crate::definitions::bundle::SourceSpan {
                source: function.symbol.source.clone(),
                start: 0,
                end: 0,
            };
            let span = signatures
                .bound
                .index
                .modules
                .iter()
                .find(|module| module.document.source.id == function.symbol.source)
                .and_then(|module| {
                    match &module
                        .document
                        .declarations
                        .get(function.symbol.declaration)?
                        .value
                    {
                        super::ast::Declaration::Function { generics, .. } => {
                            let generic = generics.get(ordinal)?;
                            generic
                                .constraint
                                .and_then(|ty| module.document.types.get(ty.0))
                                .map(|ty| ty.span.clone())
                                .or_else(|| Some(generic.name.span.clone()))
                        }
                        _ => None,
                    }
                })
                .unwrap_or(fallback);
            let error = |message: String| {
                crate::definitions::ecore::ImportError::new(
                    crate::definitions::ecore::ErrorKind::InvalidDefinition,
                    &span,
                    message,
                )
            };
            if nominal_bound(
                &signatures.types,
                generic.ty,
                &bounds,
                &mut BTreeSet::new(),
                &mut 16384,
            )
            .map_err(error)?
            {
                let id =
                    identity(signatures.types.get(generic.ty).map_err(error)?).map_err(error)?;
                let expression = expression(&signatures.types, bound).map_err(error)?;
                entries.push((id, vec![expression], span));
            }
        }
    }
    Ok(entries)
}
pub(super) async fn validate_signatures(
    signatures: &Signatures,
) -> Result<(), Vec<super::SyntaxDiagnostic>> {
    let mut bounds = BTreeMap::new();
    for function in &signatures.functions {
        pause().await;
        for generic in &function.generics {
            pause().await;
            if let Some(bound) = generic.bound {
                bounds.insert(generic.ty, bound);
            }
        }
    }
    let mut errors = Vec::new();
    for module in &signatures.modules {
        pause().await;
        let mut selected = None;
        for candidate in &signatures.bound.index.modules {
            pause().await;
            if candidate.document.source.id == module.source {
                selected = Some(candidate);
                break;
            }
        }
        let Some(source) = selected else {
            continue;
        };
        for (ordinal, ty) in &module.annotations {
            pause().await;
            if matches!(
                signatures.types.get(*ty),
                Ok(Type::SymbolicModelApplication { .. }
                    | Type::SymbolicDatatypeApplication { .. })
            ) {
                match nominal_bound_retained(
                    &signatures.types,
                    *ty,
                    &bounds,
                    &mut BTreeSet::new(),
                    &mut 16384,
                )
                .await
                {
                    Ok(true) => {}
                    result => errors.push(super::SyntaxDiagnostic {
                        span: source.document.types[*ordinal].span.clone(),
                        message: result.err().unwrap_or_else(|| {
                            "Ecore symbolic arguments require an explicit nominal generic bound"
                                .into()
                        }),
                    }),
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod retained_bound_tests {
    use super::*;
    use crate::definitions::{
        bundle::DefinitionSource,
        rcore_behavior::{modules::SymbolId, types::Scalar},
        resources::DeclarationId,
    };
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    #[test]
    fn nominal_bound_future_preserves_depth_budget_cycle_precedence_and_nonshortcircuiting()
    -> Result<(), String> {
        let source = DefinitionSource::new(
            "memory:bound.rcore",
            crate::definitions::bundle::SourceKind::RcoreBehavior,
            "rcore 1;",
        );
        let symbol = SymbolId {
            source: source.id.clone(),
            declaration: 0,
        };
        let mut table = TypeTable::default();
        let model = table.intern(Type::Model(DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        }))?;
        let mut bounds = BTreeMap::new();
        let mut chain = vec![model];
        for ordinal in 0..64 {
            let parameter = table.intern(Type::Parameter {
                symbol: symbol.clone(),
                ordinal,
            })?;
            bounds.insert(parameter, *chain.last().ok_or("Missing previous bound")?);
            chain.push(parameter);
        }
        let boolean = table.intern(Type::Scalar(Scalar::Bool))?;
        let cyclic = table.intern(Type::Parameter {
            symbol,
            ordinal: 64,
        })?;
        bounds.insert(cyclic, cyclic);
        let mixed = table.intern(Type::SymbolicModelApplication {
            declaration: DeclarationId {
                source: source.id,
                path: vec![1],
            },
            arguments: vec![boolean, cyclic],
        })?;
        for (root, budget, expected) in [
            (chain[63], 64, Ok(true)),
            (
                chain[63],
                63,
                Err("Nominal generic bound work limit exceeded"),
            ),
            (
                chain[64],
                16384,
                Err("Nominal generic bound nesting exceeds 64 levels"),
            ),
            (
                chain[64],
                64,
                Err("Nominal generic bound work limit exceeded"),
            ),
            (cyclic, 16384, Err("Cyclic nominal generic bound")),
            (mixed, 16384, Err("Cyclic nominal generic bound")),
        ] {
            let mut previous = None;
            for quantum in [1, 3, 256] {
                let mut seen = BTreeSet::new();
                let mut remaining = budget;
                let mut future = Box::pin(nominal_bound_retained(
                    &table,
                    root,
                    &bounds,
                    &mut seen,
                    &mut remaining,
                ));
                let mut polls = 0;
                let result = 'run: loop {
                    for _ in 0..quantum {
                        polls += 1;
                        if polls > 1024 {
                            return Err("Nominal bound future did not finish".into());
                        }
                        if let Poll::Ready(result) = future
                            .as_mut()
                            .poll(&mut Context::from_waker(Waker::noop()))
                        {
                            break 'run result;
                        }
                    }
                };
                drop(future);
                assert_eq!(
                    result.as_ref().map(|v| *v).map_err(|e| e.as_str()),
                    expected
                );
                let receipt = (polls, remaining, seen.clone());
                if let Some(ref previous) = previous {
                    assert_eq!(&receipt, previous);
                } else {
                    previous = Some(receipt);
                }
            }
        }
        Ok(())
    }
}
