//! Handwritten source-verified operation bindings. Declaration metadata alone
//! is never executable; implementations use the existing verified function image.
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    generic_resolution::Resolver,
    generic_types::RuntimeType,
    operations::{Operation, TypedElement},
    rcore_behavior::{
        SyntaxDiagnostic,
        ir_v2::{self, EntrySelection, Limits, VerifiedExecutable},
        model_types::ModelTypes,
        modules::SymbolId,
        types::{CollectionKind, Type, TypeRef, TypeTable},
    },
    resources::{DeclarationId, ResourceSet},
    schema::Schema,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
pub struct OperationBinding {
    pub receiver: RuntimeType,
    pub operation: Operation,
    pub declaration_span: SourceSpan,
    pub implementation: SymbolId,
    pub implementation_span: SourceSpan,
}

/// Signature inspection over an already verified image. This does not replay
/// the function body; use `compile_operation` to obtain a source-bound typed
/// executable. Hosts calling this checker must rebuild the image from source.
/// Inspect only an exact receiver/source declaration, never a short-name lookup.
/// The caller supplies a freshly source-compiled image; serialized descriptors
/// cannot construct an executable binding or replace its verified signature.
pub fn verify_binding(
    bundle: &DefinitionBundle,
    image: &VerifiedExecutable,
    receiver: &RuntimeType,
    declaration: &DeclarationId,
    materialized_types: &[RuntimeType],
    limits: &Limits,
) -> Result<OperationBinding, Vec<SyntaxDiagnostic>> {
    let span = SourceSpan {
        source: declaration.source.clone(),
        start: 0,
        end: 0,
    };
    let error = |message: String| {
        vec![SyntaxDiagnostic {
            span: span.clone(),
            message,
        }]
    };
    image
        .check_context(bundle, None)
        .map_err(|e| error(e.to_string()))?;
    let resources = ResourceSet::new_diagnostic(bundle).map_err(|e| {
        vec![SyntaxDiagnostic {
            span: e.span,
            message: e.message,
        }]
    })?;
    let declaration_span = resources.node(declaration).map_err(error)?.span.clone();
    let schema =
        crate::definitions::compiler::materialize_runtime_types(bundle, materialized_types, None)
            .map_err(|e| {
            vec![SyntaxDiagnostic {
                span: e.span,
                message: e.message,
            }]
        })?;
    let class = schema.class_key_for_type(receiver).map_err(error)?;
    let selected = schema.operation_by_id(class, declaration).map_err(error)?;
    if selected.operation.generic {
        return Err(error(
            "Generic operation bindings require explicit method specialization; unsupported in R04"
                .into(),
        ));
    }
    let program = image.program();
    let function = program
        .functions
        .get(program.entry.0 as usize)
        .ok_or_else(|| error("Missing verified entry function".into()))?;
    let mismatch = |message: &str| {
        vec![SyntaxDiagnostic {
            span: function.span.clone(),
            message: message.into(),
        }]
    };
    let ir_v2::FunctionIdentity::Named { symbol, .. } = &function.identity else {
        return Err(mismatch(
            "Operation implementation must be a named source function",
        ));
    };
    if function.parameters.len() != selected.operation.parameters.len() + 1 {
        return Err(mismatch(
            "Operation implementation requires receiver first, followed by the declared parameters",
        ));
    }
    let mut models = ModelTypes::new(bundle).map_err(error)?;
    let mut table = TypeTable::default();
    let types = ir_v2::import_types(&program.types, &mut table, &mut models, limits)
        .map_err(|e| error(e.to_string()))?;
    let receiver_type = models
        .type_from_expression(&mut table, receiver.expression())
        .map_err(error)?;
    let first = function
        .parameters
        .first()
        .ok_or_else(|| mismatch("Missing receiver parameter"))?;
    let first_type = types
        .get(first.0 as usize)
        .copied()
        .ok_or_else(|| mismatch("Unknown receiver type"))?;
    if !same_type(&table, first_type, receiver_type, limits).map_err(error)? {
        return Err(mismatch(
            "Operation receiver type differs from the exact bound receiver",
        ));
    }
    let resolver = Resolver::new(bundle).map_err(|e| error(e.to_string()))?;
    for (parameter, actual) in selected
        .operation
        .parameters
        .iter()
        .zip(function.parameters.iter().skip(1))
    {
        let expected = signature_type(
            &schema,
            &resolver,
            &mut models,
            &mut table,
            &parameter.value,
        )
        .map_err(error)?;
        let actual = types
            .get(actual.0 as usize)
            .copied()
            .ok_or_else(|| mismatch("Unknown parameter type"))?;
        if !same_type(&table, actual, expected, limits).map_err(error)? {
            return Err(mismatch(
                "Operation parameter type or multiplicity differs from its implementation",
            ));
        }
    }
    let expected = signature_type(
        &schema,
        &resolver,
        &mut models,
        &mut table,
        &selected.operation.result,
    )
    .map_err(error)?;
    let actual = types
        .get(function.result.0 as usize)
        .copied()
        .ok_or_else(|| mismatch("Unknown result type"))?;
    if !same_type(&table, actual, expected, limits).map_err(error)? {
        return Err(mismatch(
            "Operation result type, ordering, uniqueness or multiplicity differs from its implementation",
        ));
    }
    Ok(OperationBinding {
        receiver: receiver.clone(),
        operation: selected.operation.clone(),
        declaration_span,
        implementation: symbol.clone(),
        implementation_span: function.span.clone(),
    })
}

fn signature_type(
    schema: &Schema,
    resolver: &Resolver,
    models: &mut ModelTypes<'_>,
    table: &mut TypeTable,
    value: &TypedElement,
) -> Result<TypeRef, String> {
    let Some(target) = &value.target else {
        if value.upper != 1 {
            return Err("Void collection operation signatures are unsupported".into());
        }
        return table.intern(Type::Scalar(
            crate::definitions::rcore_behavior::types::Scalar::Unit,
        ));
    };
    let expression = schema.canonical_operation_target(resolver, target)?;
    let element = models.type_from_expression(table, &expression)?;
    if value.upper == 1 {
        if value.lower == 0 {
            table.intern(Type::Option(element))
        } else {
            Ok(element)
        }
    } else if value.lower == 0 && value.upper == -1 {
        let kind = match (value.ordered, value.unique) {
            (true, true) => CollectionKind::OrderedSet,
            (false, true) => CollectionKind::Set,
            (true, false) => CollectionKind::Sequence,
            (false, false) => CollectionKind::Bag,
        };
        table.intern(Type::Collection { kind, element })
    } else {
        // Preserve bounded signatures rather than erasing them to an unbounded
        // collection. The shared type engine decides which images can satisfy it.
        table.intern(Type::Many {
            element,
            lower: value.lower,
            upper: value.upper,
            ordered: value.ordered,
            unique: value.unique,
        })
    }
}

fn same_type(table: &TypeTable, a: TypeRef, b: TypeRef, limits: &Limits) -> Result<bool, String> {
    fn canonical(
        table: &TypeTable,
        ty: TypeRef,
        limits: &Limits,
    ) -> Result<(Vec<ir_v2::TypeNode>, ir_v2::TypeId), String> {
        let mut nodes = Vec::new();
        let root = ir_v2::export_type(ty, table, &mut nodes, &mut BTreeMap::new(), limits)
            .map_err(|e| e.to_string())?;
        Ok((nodes, root))
    }
    Ok(canonical(table, a, limits)? == canonical(table, b, limits)?)
}

/// Source-bound operation authority. No deserialization or public field mutation.
#[derive(Clone, Debug)]
pub struct VerifiedOperation {
    binding: OperationBinding,
    executable: VerifiedExecutable,
}
impl VerifiedOperation {
    pub fn binding(&self) -> &OperationBinding {
        &self.binding
    }
    pub fn executable(&self) -> &VerifiedExecutable {
        &self.executable
    }
}

#[derive(Clone, Debug)]
pub struct OperationSelection {
    pub receiver: RuntimeType,
    pub declaration: DeclarationId,
    pub materialized_types: Vec<RuntimeType>,
    pub implementation: EntrySelection,
}
/// Compile the authored body and its operation contract together through shared
/// source compilation. A caller-supplied image cannot mint this authority.
pub fn compile_operation(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selection: &OperationSelection,
    limits: &Limits,
) -> Result<VerifiedOperation, Vec<SyntaxDiagnostic>> {
    let executable = crate::definitions::rcore_behavior::function_lowering::compile_function(
        bundle,
        entry,
        &selection.implementation,
        limits,
    )?;
    let binding = verify_binding(
        bundle,
        &executable,
        &selection.receiver,
        &selection.declaration,
        &selection.materialized_types,
        limits,
    )?;
    Ok(VerifiedOperation {
        binding,
        executable,
    })
}
