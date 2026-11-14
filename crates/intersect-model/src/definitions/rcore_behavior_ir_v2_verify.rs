//! Fresh source/type verification for function images; no runtime provider ownership.
use super::*;
use crate::definitions::rcore_behavior::{
    checking, function_lowering, inference, model_types::ModelTypes, types::Type,
};

pub fn verify(
    program: Program,
    bundle: &DefinitionBundle,
    limits: &Limits,
) -> Result<VerifiedExecutable, IrError> {
    context(program, bundle, None, limits, false, Default::default())
}
/// Verify source-backed types under an explicit finite per-source parser ceiling.
pub fn verify_with_source_limits(
    program: Program,
    bundle: &DefinitionBundle,
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<VerifiedExecutable, IrError> {
    context(program, bundle, None, limits, false, source_limits)
}
pub(crate) fn verify_contract(
    program: Program,
    bundle: &DefinitionBundle,
    limits: &Limits,
) -> Result<VerifiedExecutable, IrError> {
    context(program, bundle, None, limits, true, Default::default())
}
pub fn verify_with_conversions(
    program: Program,
    bundle: &DefinitionBundle,
    registry: &ConversionRegistry<'_>,
    limits: &Limits,
) -> Result<VerifiedExecutable, IrError> {
    context(
        program,
        bundle,
        Some(registry),
        limits,
        false,
        Default::default(),
    )
}

pub(crate) fn context(
    program: Program,
    bundle: &DefinitionBundle,
    registry: Option<&ConversionRegistry<'_>>,
    limits: &Limits,
    binding: bool,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<VerifiedExecutable, IrError> {
    context_checked(
        program,
        bundle,
        registry,
        limits,
        binding,
        source_limits,
        None,
    )
}

/// A fresh source check made for verification only, with its model types,
/// shared by the images of one batch. It is never the check or the model
/// types that lowered those images.
pub(crate) type VerificationCheck = (
    checking::TypeReport,
    crate::definitions::resources::ResourceSet,
);

/// `context` with an optional shared verification check (its report and its
/// configured model types) of the image's exact entry source; without one,
/// the source is checked here.
pub(crate) fn context_checked<'m>(
    program: Program,
    bundle: &'m DefinitionBundle,
    registry: Option<&'m ConversionRegistry<'m>>,
    limits: &Limits,
    binding: bool,
    source_limits: crate::definitions::ecore::ParseLimits,
    shared: Option<(&checking::TypeReport, &mut ModelTypes<'m>)>,
) -> Result<VerifiedExecutable, IrError> {
    ir::preflight_bundle(bundle, &limits.scalar)?;
    let mut remaining = limits.scalar.max_work;
    preflight(&program, limits, &mut remaining)?;
    if program.version != VERSION {
        return Err(IrError {
            kind: IrErrorKind::Unsupported,
            message: "Unsupported function IR version".into(),
            span: None,
        });
    }
    let mut sources = bundle
        .sources
        .iter()
        .map(|source| ir::SourceManifestEntry {
            source: source.id.clone(),
        })
        .collect::<Vec<_>>();
    sources.sort_by(|a, b| a.source.cmp(&b.source));
    if sources != program.sources {
        return Err(invalid("Function image source manifest mismatch", None));
    }
    if program.policies != policy_manifest(registry)? {
        return Err(invalid(
            "Function image provider policy manifest mismatch",
            None,
        ));
    }
    // Recheck the exact source on every verification. Only immutable parsed
    // resources move from that fresh check into a fresh model policy context.
    let mut owned = None;
    let (report, models) = match shared {
        Some(shared) => shared,
        None => {
            let (report, resources) =
                verification_check(bundle, &program.entry_source, registry, source_limits, None)?;
            let models =
                verification_models(bundle, registry, source_limits, &report, resources, None)?;
            let (report, models) = owned.insert((report, models));
            (&*report, models)
        }
    };
    let mut table = report.signatures.types.clone();
    let refs = import_types(&program.types, &mut table, models, limits)?;
    let mut relations = BTreeSet::new();
    let mut relation_bytes = limits.scalar.max_source_bytes;
    for (a, actual) in refs.iter().enumerate() {
        for (b, expected) in refs.iter().enumerate() {
            spend(&mut remaining, 1)?;
            if models
                .assignable(&table, *actual, *expected)
                .map_err(|e| invalid(e, None))?
            {
                spend(&mut relation_bytes, 8)?;
                relations.insert((TypeId(a as u32), TypeId(b as u32)));
            }
        }
    }
    let mut enums = BTreeMap::new();
    for node in &program.types {
        if let TypeNode::Enum(id) = node {
            enums.insert(
                id.clone(),
                models
                    .enumeration(id)
                    .map_err(|e| invalid(e, None))?
                    .clone(),
            );
        }
    }
    let context = context_hash(bundle, registry, limits)?;
    let mut verified = VerifiedExecutable {
        program,
        effects: Vec::new(),
        relations,
        enums,
        context,
        limits: limits.clone(),
    };
    let program = verified.program();
    let entry = program
        .functions
        .get(program.entry.0 as usize)
        .ok_or_else(|| invalid("Unknown entry function", None))?;
    if !matches!(entry.identity, FunctionIdentity::Named { .. }) || !entry.captures.is_empty() {
        return Err(invalid(
            "Entry must be a capture-free named function",
            Some(&entry.span),
        ));
    }
    let get = |id: TypeId| {
        refs.get(id.0 as usize)
            .copied()
            .ok_or_else(|| invalid("Unknown function type", None))
    };
    let mut instances = Vec::<inference::Substitution>::new();
    let mut identities = BTreeSet::new();
    let mut value_budget = ValueBudget {
        remaining_nodes: remaining,
        remaining_bytes: limits.scalar.max_total_value_bytes,
        max_depth: limits.max_type_depth,
        scalar: limits.scalar.values.clone(),
    };
    let mut exported = program.types.clone();
    let mut mapping = refs
        .iter()
        .enumerate()
        .map(|(i, t)| (*t, TypeId(i as u32)))
        .collect::<BTreeMap<_, _>>();
    for (index, function) in program.functions.iter().enumerate() {
        let mut external = false;
        spend(&mut remaining, 1)?;
        span(bundle, &function.span)?;
        let key =
            serde_json::to_string(&function.identity).map_err(|e| invalid(e.to_string(), None))?;
        if !identities.insert(key) {
            return Err(invalid("Duplicate function identity", Some(&function.span)));
        }
        let (parameters, result, captures, substitution) = match &function.identity {
            FunctionIdentity::Named {
                symbol,
                type_arguments,
            } => {
                let signature = report
                    .signatures
                    .functions
                    .iter()
                    .find(|f| &f.symbol == symbol)
                    .ok_or_else(|| {
                        invalid("Unknown source function instance", Some(&function.span))
                    })?;
                let actuals = type_arguments
                    .iter()
                    .map(|t| get(*t))
                    .collect::<Result<Vec<_>, _>>()?;
                let ty = function_lowering::instantiate_signature(
                    signature, &actuals, &mut table, models,
                )
                .map_err(|e| invalid(e, Some(&function.span)))?;
                let Type::Function { parameters, result } =
                    table.get(ty).map_err(|e| invalid(e, None))?
                else {
                    return Err(invalid("Source instance is not a function", None));
                };
                let module = report
                    .signatures
                    .bound
                    .index
                    .modules
                    .iter()
                    .find(|m| m.document.source.id == symbol.source)
                    .ok_or_else(|| invalid("Unknown source module", None))?;
                let declaration = module
                    .document
                    .declarations
                    .get(symbol.declaration)
                    .ok_or_else(|| invalid("Unknown function source declaration", None))?;
                if function.span != declaration.span {
                    return Err(invalid(
                        "Function source span differs from declaration",
                        Some(&function.span),
                    ));
                }
                external = matches!(
                    declaration.value,
                    crate::definitions::rcore_behavior::ast::Declaration::Function {
                        external: true,
                        ..
                    }
                );
                if external
                    && (!binding
                        || index != program.entry.0 as usize
                        || program.functions.len() != 1
                        || !function.blocks.is_empty())
                {
                    return Err(invalid(
                        "External signature requires an isolated provider boundary with no Rcore body",
                        Some(&function.span),
                    ));
                }
                let substitution = signature
                    .generics
                    .iter()
                    .zip(&actuals)
                    .map(|(g, a)| (g.ty, *a))
                    .collect();
                (parameters.clone(), *result, vec![], substitution)
            }
            FunctionIdentity::Lambda {
                enclosing,
                expression,
            } => {
                let parent_index = enclosing.0 as usize;
                if parent_index >= index {
                    return Err(invalid(
                        "Lambda enclosing function must precede it",
                        Some(&function.span),
                    ));
                }
                let parent = &program.functions[parent_index];
                let evidence = function_lowering::lambda_evidence(
                    report,
                    &parent.span.source,
                    *expression as usize,
                    &mut remaining,
                )
                .map_err(|e| invalid(e, Some(&function.span)))?;
                let valid = match &parent.identity {
                    FunctionIdentity::Named { symbol, .. } => {
                        evidence.enclosing_lambda.is_none() && evidence.enclosing_symbol == *symbol
                    }
                    FunctionIdentity::Lambda { expression, .. } => {
                        evidence.enclosing_lambda == Some(*expression as usize)
                    }
                };
                if !valid || function.span != evidence.span {
                    return Err(invalid(
                        "Lambda does not have the exact lexical origin",
                        Some(&function.span),
                    ));
                }
                let substitution = instances
                    .get(parent_index)
                    .cloned()
                    .ok_or_else(|| invalid("Missing enclosing specialization", None))?;
                let mut substitute = |ty| {
                    inference::substitute(&mut table, models, ty, &substitution)
                        .map_err(|e| invalid(e, Some(&function.span)))
                };
                let parameters = evidence
                    .parameters
                    .iter()
                    .map(|(_, ty)| substitute(*ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = substitute(evidence.result)?;
                let captures = evidence
                    .captures
                    .iter()
                    .map(|(span, ty)| Ok((span.clone(), substitute(*ty)?)))
                    .collect::<Result<Vec<_>, IrError>>()?;
                (parameters, result, captures, substitution)
            }
        };
        let mut export = |ty| export_type(ty, &table, &mut exported, &mut mapping, limits);
        let expected_parameters = parameters
            .into_iter()
            .map(&mut export)
            .collect::<Result<Vec<_>, _>>()?;
        let expected_result = export(result)?;
        let expected_captures = captures
            .into_iter()
            .map(|(declaration, ty)| {
                Ok(Capture {
                    declaration,
                    ty: export(ty)?,
                })
            })
            .collect::<Result<Vec<_>, IrError>>()?;
        if function.parameters != expected_parameters
            || function.result != expected_result
            || function.captures != expected_captures
        {
            return Err(invalid(
                "Function signature or captures differ from fresh source",
                Some(&function.span),
            ));
        }
        instances.push(substitution);
        if external {
            continue;
        }
        blocks(
            &verified,
            function,
            bundle,
            &mut value_budget,
            &mut remaining,
            &mut MemberTypes {
                models,
                table: &mut table,
                refs: &refs,
            },
        )?;
    }
    verified.effects = super::effects::infer(&verified, &mut remaining)?;
    Ok(verified)
}

fn span(bundle: &DefinitionBundle, span: &SourceSpan) -> Result<(), IrError> {
    let source = bundle
        .sources
        .iter()
        .find(|s| s.id == span.source)
        .ok_or_else(|| invalid("Unknown instruction source", None))?;
    if span.start > span.end
        || span.end > source.text.len()
        || !source.text.is_char_boundary(span.start)
        || !source.text.is_char_boundary(span.end)
    {
        return Err(invalid("Invalid function IR span", None));
    }
    Ok(())
}
struct ShapeOnly;
impl ValueVisitor for ShapeOnly {
    fn nominal_leaf(&mut self, _: &TypeNode, _: &serde_json::Value) -> Result<(), IrError> {
        Ok(())
    }
    fn unique_collection(&mut self, _: TypeId, _: &[ValueWire]) -> Result<(), IrError> {
        Ok(())
    }
}

pub(super) fn operand(
    function: &Function,
    block: &Block,
    before: usize,
    op: Operand,
) -> Result<TypeId, IrError> {
    match op {
        Operand::Parameter(i) => block.parameters.get(i as usize).copied(),
        Operand::Capture(i) => function.captures.get(i as usize).map(|c| c.ty),
        Operand::Register(i) if (i as usize) < before => {
            block.instructions.get(i as usize).map(|i| i.ty)
        }
        Operand::Register(_) => None,
    }
    .ok_or_else(|| {
        invalid(
            "Unknown or not-yet-defined function operand",
            Some(&block.span),
        )
    })
}
fn require(verified: &VerifiedExecutable, actual: TypeId, expected: TypeId) -> Result<(), IrError> {
    if verified.permits(actual, expected)? {
        Ok(())
    } else {
        Err(invalid("Incompatible function value types", None))
    }
}

struct MemberTypes<'a, 'b> {
    models: &'a mut ModelTypes<'b>,
    table: &'a mut crate::definitions::rcore_behavior::types::TypeTable,
    refs: &'a [crate::definitions::rcore_behavior::types::TypeRef],
}
fn blocks(
    verified: &VerifiedExecutable,
    function: &Function,
    bundle: &DefinitionBundle,
    value_budget: &mut ValueBudget,
    remaining: &mut usize,
    members: &mut MemberTypes<'_, '_>,
) -> Result<(), IrError> {
    let entry = function
        .blocks
        .first()
        .ok_or_else(|| invalid("Function requires an entry block", Some(&function.span)))?;
    if entry.parameters != function.parameters {
        return Err(invalid(
            "Function entry parameter mismatch",
            Some(&entry.span),
        ));
    }
    let mut reached = vec![false; function.blocks.len()];
    reached[0] = true;
    for (index, block) in function.blocks.iter().enumerate() {
        spend(remaining, 1)?;
        span(bundle, &block.span)?;
        if !reached[index] {
            return Err(invalid("Unreachable function block", Some(&block.span)));
        }
        for ty in &block.parameters {
            verified.type_node(*ty)?;
        }
        for (position, instruction) in block.instructions.iter().enumerate() {
            spend(remaining, 1)?;
            span(bundle, &instruction.span)?;
            value_budget.remaining_nodes = *remaining;
            instruction_type(
                verified,
                function,
                block,
                position,
                instruction,
                value_budget,
                members,
            )?;
            *remaining = value_budget.remaining_nodes;
        }
        let op = |value| operand(function, block, block.instructions.len(), value);
        let mut edge =
            |target: u32, arguments: &[Operand], payload: Option<TypeId>| -> Result<(), IrError> {
                spend(remaining, arguments.len().saturating_add(1))?;
                if target as usize <= index {
                    return Err(invalid(
                        "Function block edges must be forward",
                        Some(&block.span),
                    ));
                }
                let destination = function
                    .blocks
                    .get(target as usize)
                    .ok_or_else(|| invalid("Unknown function block", Some(&block.span)))?;
                let offset = usize::from(payload.is_some());
                if destination.parameters.len() != arguments.len() + offset {
                    return Err(invalid("Function edge arity mismatch", Some(&block.span)));
                }
                if let Some(ty) = payload {
                    require(verified, ty, destination.parameters[0])?;
                }
                for (value, ty) in arguments
                    .iter()
                    .zip(destination.parameters.iter().skip(offset))
                {
                    require(verified, op(*value)?, *ty)?;
                }
                reached[target as usize] = true;
                Ok(())
            };
        match &block.terminator {
            Terminator::Return(value) => require(verified, op(*value)?, function.result)?,
            Terminator::Absurd { value } => {
                if verified.inhabited(op(*value)?)? {
                    return Err(invalid(
                        "Absurd requires a proven uninhabited operand",
                        Some(&block.span),
                    ));
                }
            }
            Terminator::Jump(e) => edge(e.block, &e.arguments, None)?,
            Terminator::Branch {
                condition,
                then_edge,
                else_edge,
            } => {
                if verified.type_node(op(*condition)?)? != &TypeNode::Scalar(Scalar::Bool) {
                    return Err(invalid("Function branch requires Bool", None));
                }
                edge(then_edge.block, &then_edge.arguments, None)?;
                edge(else_edge.block, &else_edge.arguments, None)?;
            }
            Terminator::Option {
                subject,
                none,
                some,
            } => {
                let TypeNode::Option(payload) = verified.type_node(op(*subject)?)? else {
                    return Err(invalid("Option branch requires Option", None));
                };
                edge(none.block, &none.arguments, None)?;
                edge(some.block, &some.arguments, Some(*payload))?;
            }
            Terminator::Result { subject, ok, error } => {
                let TypeNode::Result {
                    ok: ok_type,
                    error: error_type,
                } = verified.type_node(op(*subject)?)?
                else {
                    return Err(invalid("Result branch requires Result", None));
                };
                edge(ok.block, &ok.arguments, Some(*ok_type))?;
                edge(error.block, &error.arguments, Some(*error_type))?;
            }
        }
    }
    Ok(())
}

fn collection(node: &TypeNode) -> Option<(CollectionKind, TypeId)> {
    match node {
        TypeNode::Collection { kind, element } => Some((kind.clone(), *element)),
        TypeNode::Many {
            element,
            ordered,
            unique,
            ..
        } => Some((
            match (*ordered, *unique) {
                (true, false) => CollectionKind::Sequence,
                (true, true) => CollectionKind::OrderedSet,
                (false, true) => CollectionKind::Set,
                (false, false) => CollectionKind::Bag,
            },
            *element,
        )),
        _ => None,
    }
}
fn equality_type(verified: &VerifiedExecutable, root: TypeId) -> Result<(), IrError> {
    let mut pending = vec![root];
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        match verified.type_node(id)? {
            TypeNode::Function { .. } | TypeNode::Collection { .. } | TypeNode::Many { .. } => {
                return Err(invalid(
                    "Source equality for callable or collection values is unsupported",
                    None,
                ));
            }
            TypeNode::Tuple(items) => pending.extend(items),
            TypeNode::Record(fields) => pending.extend(fields.values()),
            TypeNode::Option(t) => pending.push(*t),
            TypeNode::Result { ok, error } => pending.extend([*ok, *error]),
            _ => {}
        }
    }
    Ok(())
}
fn instruction_type(
    verified: &VerifiedExecutable,
    function: &Function,
    block: &Block,
    before: usize,
    instruction: &Instruction,
    values: &mut ValueBudget,
    members: &mut MemberTypes<'_, '_>,
) -> Result<(), IrError> {
    let target = verified.type_node(instruction.ty)?;
    let op = |value| operand(function, block, before, value);
    let check = |value, expected| require(verified, op(value)?, expected);
    match &instruction.kind {
        InstructionKind::Constant(value) => {
            inspect_value_shape(verified, value, instruction.ty, values, &mut ShapeOnly)?
        }
        InstructionKind::Unary {
            op: operator,
            value,
        } => {
            let TypeNode::Scalar(s) = verified.type_node(op(*value)?)? else {
                return Err(invalid("Unary requires scalar", None));
            };
            let result = ir::unary_type(*operator, &ir::ValueType::Scalar(*s))
                .map_err(|e| invalid(e, None))?;
            if result
                != ir::ValueType::Scalar(match target {
                    TypeNode::Scalar(s) => *s,
                    _ => return Err(invalid("Unary result requires scalar", None)),
                })
            {
                return Err(invalid("Unary result mismatch", None));
            }
        }
        InstructionKind::Binary {
            op: operator,
            left,
            right,
        } => {
            let left = op(*left)?;
            let right = op(*right)?;
            if left != right {
                return Err(invalid(
                    "Binary operands require an explicit common checked view",
                    None,
                ));
            }
            if matches!(operator, ir::BinaryOp::Equal | ir::BinaryOp::NotEqual) {
                equality_type(verified, left)?;
                if target != &TypeNode::Scalar(Scalar::Bool) {
                    return Err(invalid("Equality result requires Bool", None));
                }
            } else {
                let TypeNode::Scalar(s) = verified.type_node(left)? else {
                    return Err(invalid("Arithmetic requires scalar", None));
                };
                let result = ir::binary_type(
                    *operator,
                    &ir::ValueType::Scalar(*s),
                    &ir::ValueType::Scalar(*s),
                )
                .map_err(|e| invalid(e, None))?;
                let TypeNode::Scalar(target) = target else {
                    return Err(invalid("Arithmetic result requires scalar", None));
                };
                if result != ir::ValueType::Scalar(*target) {
                    return Err(invalid("Arithmetic result mismatch", None));
                }
            }
        }
        InstructionKind::MakeClosure {
            function: callee,
            captures,
        } => {
            let callee = verified
                .program
                .functions
                .get(callee.0 as usize)
                .ok_or_else(|| invalid("Unknown closure function", None))?;
            let expected = TypeNode::Function {
                parameters: callee.parameters.clone(),
                result: callee.result,
            };
            if target != &expected || captures.len() != callee.captures.len() {
                return Err(invalid("Closure signature or capture arity mismatch", None));
            }
            for (value, capture) in captures.iter().zip(&callee.captures) {
                check(*value, capture.ty)?;
            }
        }
        InstructionKind::Call { callee, arguments } => {
            let TypeNode::Function { parameters, result } = verified.type_node(op(*callee)?)?
            else {
                return Err(invalid("Call operand is not callable", None));
            };
            if arguments.len() != parameters.len() {
                return Err(invalid("Call argument arity mismatch", None));
            }
            for (value, ty) in arguments.iter().zip(parameters) {
                check(*value, *ty)?;
            }
            require(verified, *result, instruction.ty)?;
        }
        InstructionKind::View { value } => check(*value, instruction.ty)?,
        InstructionKind::None => {
            if !matches!(target, TypeNode::Option(_)) {
                return Err(invalid("None requires Option result", None));
            }
        }
        InstructionKind::Some(value) => {
            let TypeNode::Option(child) = target else {
                return Err(invalid("Some requires Option result", None));
            };
            check(*value, *child)?;
        }
        InstructionKind::Ok(value) => {
            let TypeNode::Result { ok, .. } = target else {
                return Err(invalid("Ok requires Result type", None));
            };
            check(*value, *ok)?;
        }
        InstructionKind::Err(value) => {
            let TypeNode::Result { error, .. } = target else {
                return Err(invalid("Err requires Result type", None));
            };
            check(*value, *error)?;
        }
        InstructionKind::Tuple(items) => {
            let TypeNode::Tuple(types) = target else {
                return Err(invalid("Tuple instruction requires tuple result", None));
            };
            if items.len() != types.len() {
                return Err(invalid("Tuple arity mismatch", None));
            }
            for (value, ty) in items.iter().zip(types) {
                check(*value, *ty)?;
            }
        }
        InstructionKind::Record(fields) => {
            let TypeNode::Record(types) = target else {
                return Err(invalid("Record instruction requires record result", None));
            };
            if !fields.keys().eq(types.keys()) {
                return Err(invalid("Record field mismatch", None));
            }
            for (name, value) in fields {
                check(
                    *value,
                    *types
                        .get(name)
                        .ok_or_else(|| invalid("Unknown record field", None))?,
                )?;
            }
        }
        InstructionKind::Collection(items) => {
            let (_, element) = collection(target)
                .ok_or_else(|| invalid("Collection instruction requires collection type", None))?;
            for value in items {
                check(*value, element)?;
            }
        }
        InstructionKind::CollectionCall {
            operation,
            value,
            callback,
            seed,
        } => {
            let (kind, element) = collection(verified.type_node(op(*value)?)?)
                .ok_or_else(|| invalid("Collection call requires collection input", None))?;
            let TypeNode::Function { parameters, result } = verified.type_node(op(*callback)?)?
            else {
                return Err(invalid("Collection callback must be callable", None));
            };
            let fold = *operation == CollectionOp::Fold;
            if seed.is_some() != fold
                || parameters.len()
                    != if fold || *operation == CollectionOp::SortedWith {
                        2
                    } else {
                        1
                    }
            {
                return Err(invalid("Collection callback or seed arity mismatch", None));
            }
            require(verified, element, parameters[usize::from(fold)])?;
            if operation.predicate()
                && verified.type_node(*result)? != &TypeNode::Scalar(Scalar::Bool)
            {
                return Err(invalid("Collection predicate must return Bool", None));
            }
            let ordered = |kind: &CollectionKind| {
                matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet)
            };
            let expected = match operation {
                CollectionOp::Closure => {
                    if !matches!(verified.type_node(element)?, TypeNode::Model(_)) {
                        return Err(invalid("Closure requires model-reference seeds", None));
                    }
                    let (_, successor) =
                        collection(verified.type_node(*result)?).ok_or_else(|| {
                            invalid("Closure callback must return a collection", None)
                        })?;
                    require(verified, successor, element)?;
                    TypeNode::Collection {
                        kind: CollectionKind::OrderedSet,
                        element,
                    }
                }
                CollectionOp::SortedBy => {
                    if !matches!(verified.type_node(*result)?, TypeNode::Scalar(_)) {
                        return Err(invalid("Sorting key must be a scalar", None));
                    }
                    TypeNode::Collection {
                        kind: CollectionKind::Sequence,
                        element,
                    }
                }
                CollectionOp::SortedWith => {
                    require(verified, element, parameters[1])?;
                    if verified.type_node(*result)? != &TypeNode::Scalar(Scalar::Int64) {
                        return Err(invalid("Sorting comparator must return Int", None));
                    }
                    TypeNode::Collection {
                        kind: CollectionKind::Sequence,
                        element,
                    }
                }
                CollectionOp::GroupBy => {
                    let TypeNode::Collection {
                        kind: outer,
                        element: pair,
                    } = target
                    else {
                        return Err(invalid(
                            "Grouping requires a concrete outer collection",
                            None,
                        ));
                    };
                    let expected_outer = if ordered(&kind) {
                        CollectionKind::Sequence
                    } else {
                        CollectionKind::Bag
                    };
                    if *outer != expected_outer {
                        return Err(invalid("Grouping outer kind mismatch", None));
                    }
                    let TypeNode::Tuple(fields) = verified.type_node(*pair)? else {
                        return Err(invalid("Grouping requires key/member pairs", None));
                    };
                    if fields.len() != 2
                        || fields[0] != *result
                        || verified.type_node(fields[1])?
                            != &(TypeNode::Collection {
                                kind: kind.clone(),
                                element,
                            })
                    {
                        return Err(invalid("Grouping key/member contract mismatch", None));
                    }
                    target.clone()
                }
                CollectionOp::Fold => {
                    let accumulator = op(seed.ok_or_else(|| invalid("Missing fold seed", None))?)?;
                    require(verified, accumulator, parameters[0])?;
                    require(verified, *result, accumulator)?;
                    verified.type_node(accumulator)?.clone()
                }
                CollectionOp::Exists | CollectionOp::ForAll | CollectionOp::One => {
                    TypeNode::Scalar(Scalar::Bool)
                }
                CollectionOp::Find => {
                    if !ordered(&kind) {
                        return Err(invalid("Find requires ordered input", None));
                    }
                    TypeNode::Option(element)
                }
                CollectionOp::Filter => TypeNode::Collection { kind, element },
                CollectionOp::Map => TypeNode::Collection {
                    kind: if ordered(&kind) {
                        CollectionKind::Sequence
                    } else {
                        CollectionKind::Bag
                    },
                    element: *result,
                },
                CollectionOp::FlatMap => {
                    let (inner, element) = collection(verified.type_node(*result)?)
                        .ok_or_else(|| invalid("FlatMap callback must return collection", None))?;
                    TypeNode::Collection {
                        kind: if ordered(&kind) && ordered(&inner) {
                            CollectionKind::Sequence
                        } else {
                            CollectionKind::Bag
                        },
                        element,
                    }
                }
            };
            if target != &expected {
                return Err(invalid(
                    "Collection operation result contract mismatch",
                    None,
                ));
            }
        }
        InstructionKind::CollectionAlgebra {
            operation,
            value,
            other,
        } => {
            let (left, element) = collection(verified.type_node(op(*value)?)?)
                .ok_or_else(|| invalid("Algebra requires a collection", None))?;
            let TypeNode::Collection {
                kind: target_kind,
                element: target_element,
            } = target
            else {
                return Err(invalid("Algebra must produce a concrete collection", None));
            };
            let ordered = |kind: &CollectionKind| {
                matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet)
            };
            let kind = if *operation == AlgebraOp::Distinct {
                if other.is_some() || element != *target_element {
                    return Err(invalid("Invalid distinct operand/result contract", None));
                }
                if ordered(&left) {
                    CollectionKind::OrderedSet
                } else {
                    CollectionKind::Set
                }
            } else {
                let other = other
                    .ok_or_else(|| invalid("Binary algebra requires second collection", None))?;
                let (right, right_element) = collection(verified.type_node(op(other)?)?)
                    .ok_or_else(|| invalid("Algebra requires second collection", None))?;
                require(verified, right_element, *target_element)?;
                if *operation == AlgebraOp::Concat {
                    if !ordered(&left) || !ordered(&right) {
                        return Err(invalid("Concat requires ordered inputs", None));
                    }
                    CollectionKind::Sequence
                } else {
                    if left != right {
                        return Err(invalid("Algebra collection kind mismatch", None));
                    }
                    left
                }
            };
            require(verified, element, *target_element)?;
            if kind != *target_kind {
                return Err(invalid("Algebra result kind mismatch", None));
            }
        }
        InstructionKind::CollectionMembership {
            operation,
            value,
            other,
            element,
        } => {
            let (_, left) = collection(verified.type_node(op(*value)?)?)
                .ok_or_else(|| invalid("Membership requires a collection", None))?;
            let right = op(*other)?;
            let right = if operation.multiple() {
                collection(verified.type_node(right)?)
                    .ok_or_else(|| invalid("Membership requires a collection right operand", None))?
                    .1
            } else {
                right
            };
            verified.type_node(*element)?;
            require(verified, left, *element)?;
            require(verified, right, *element)?;
            let scalar = if *operation == MembershipOp::Count {
                Scalar::Int64
            } else {
                Scalar::Bool
            };
            if target != &TypeNode::Scalar(scalar) {
                return Err(invalid("Membership result type mismatch", None));
            }
        }
        InstructionKind::CollectionFlatten { value } => {
            let (outer, inner) = collection(verified.type_node(op(*value)?)?)
                .ok_or_else(|| invalid("Flatten requires a collection", None))?;
            let (inner, element) = collection(verified.type_node(inner)?)
                .ok_or_else(|| invalid("Flatten requires collection elements", None))?;
            let ordered =
                |kind| matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet);
            let kind = if ordered(outer) && ordered(inner) {
                CollectionKind::Sequence
            } else {
                CollectionKind::Bag
            };
            if target != &(TypeNode::Collection { kind, element }) {
                return Err(invalid("Flatten result contract mismatch", None));
            }
        }
        InstructionKind::CollectionSize { value }
        | InstructionKind::CollectionIsEmpty { value } => {
            collection(verified.type_node(op(*value)?)?)
                .ok_or_else(|| invalid("Collection measure requires a collection operand", None))?;
            let scalar = if matches!(instruction.kind, InstructionKind::CollectionSize { .. }) {
                Scalar::Int64
            } else {
                Scalar::Bool
            };
            if target != &TypeNode::Scalar(scalar) {
                return Err(invalid(
                    "Collection measure has an invalid result type",
                    None,
                ));
            }
        }
        InstructionKind::Text { op: text, value } => {
            let operand = verified.type_node(op(*value)?)?;
            let valid = if text.reads_model() {
                matches!(operand, TypeNode::Model(_))
            } else {
                operand == &TypeNode::Scalar(Scalar::String)
            };
            if !valid || target != &TypeNode::Scalar(Scalar::String) {
                return Err(invalid(
                    "text primitive has invalid operand or result types",
                    None,
                ));
            }
        }
        InstructionKind::Split { value, separator } => {
            for operand in [value, separator] {
                if verified.type_node(op(*operand)?)? != &TypeNode::Scalar(Scalar::String) {
                    return Err(invalid("split requires String operands", None));
                }
            }
            let Some((CollectionKind::Sequence, element)) = collection(target) else {
                return Err(invalid("split has an invalid result type", None));
            };
            if verified.type_node(element)? != &TypeNode::Scalar(Scalar::String) {
                return Err(invalid("split has an invalid result type", None));
            }
        }
        InstructionKind::ConvertCollection { value } => {
            let (kind, element) = collection(verified.type_node(op(*value)?)?)
                .ok_or_else(|| invalid("Collection conversion source mismatch", None))?;
            let (target_kind, target_element) = collection(target)
                .ok_or_else(|| invalid("Collection conversion target mismatch", None))?;
            if matches!(
                target_kind,
                CollectionKind::Sequence | CollectionKind::OrderedSet
            ) && !matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet)
            {
                return Err(invalid(
                    "Unordered collection cannot acquire ordering",
                    None,
                ));
            }
            require(verified, element, target_element)?;
            if let TypeNode::Many { lower, upper, .. } = target
                && (*lower != 0 || *upper != -1)
            {
                return Err(invalid(
                    "Conversion cannot establish stronger multiplicity bounds",
                    None,
                ));
            }
        }
        InstructionKind::TupleField { value, index } => {
            let TypeNode::Tuple(items) = verified.type_node(op(*value)?)? else {
                return Err(invalid("Tuple projection requires tuple", None));
            };
            let ty = items
                .get(*index as usize)
                .ok_or_else(|| invalid("Tuple projection outside arity", None))?;
            require(verified, *ty, instruction.ty)?;
        }
        InstructionKind::Ownership {
            operation,
            value,
            selected,
        } => {
            if !matches!(verified.type_node(op(*value)?)?, TypeNode::Model(_))
                || !matches!(verified.type_node(*selected)?, TypeNode::Model(_))
            {
                return Err(invalid("Ownership navigation requires model types", None));
            }
            let expected = if operation.singular() {
                TypeNode::Option(*selected)
            } else {
                TypeNode::Collection {
                    kind: CollectionKind::OrderedSet,
                    element: *selected,
                }
            };
            if verified.type_node(instruction.ty)? != &expected {
                return Err(invalid("Ownership result type differs", None));
            }
        }
        InstructionKind::Incoming {
            value,
            source,
            field,
            declaration,
        } => {
            let source_ref = *members
                .refs
                .get(source.0 as usize)
                .ok_or_else(|| invalid("Missing inverse source type", None))?;
            let target = *members
                .refs
                .get(op(*value)?.0 as usize)
                .ok_or_else(|| invalid("Missing inverse target type", None))?;
            let evidence = members
                .models
                .incoming_feature(members.table, source_ref, target, field)
                .map_err(|e| invalid(e, None))?;
            if &evidence.declaration != declaration
                || verified.type_node(instruction.ty)?
                    != &(TypeNode::Collection {
                        kind: CollectionKind::OrderedSet,
                        element: *source,
                    })
            {
                return Err(invalid(
                    "Inverse declaration or result differs from schema",
                    None,
                ));
            }
        }
        InstructionKind::StoredFeature {
            value,
            field,
            declaration,
        } => {
            let receiver = *members
                .refs
                .get(op(*value)?.0 as usize)
                .ok_or_else(|| invalid("Missing model receiver type", None))?;
            let evidence = members
                .models
                .stored_feature(members.table, receiver, field)
                .map_err(|e| invalid(e, None))?;
            let expected = members
                .refs
                .get(instruction.ty.0 as usize)
                .ok_or_else(|| invalid("Missing stored feature result type", None))?;
            // Source model names and imported runtime applications may have
            // different TypeRefs while exporting the same exact nominal type.
            let mut types = Vec::new();
            let mut mapping = BTreeMap::new();
            let actual = export_type(
                evidence.value,
                members.table,
                &mut types,
                &mut mapping,
                &verified.limits,
            )?;
            let expected = export_type(
                *expected,
                members.table,
                &mut types,
                &mut mapping,
                &verified.limits,
            )?;
            if &evidence.declaration != declaration || actual != expected {
                return Err(invalid(
                    "Stored feature declaration or result type differs from schema",
                    None,
                ));
            }
        }
        InstructionKind::New(fields) => {
            if !matches!(target, TypeNode::Model(_)) {
                return Err(invalid("`new` requires a model class result", None));
            }
            let class = *members
                .refs
                .get(instruction.ty.0 as usize)
                .ok_or_else(|| invalid("Missing constructed class type", None))?;
            for (field, value) in fields {
                let evidence = members
                    .models
                    .stored_feature(members.table, class, field)
                    .map_err(|e| invalid(e, None))?;
                let actual = *members
                    .refs
                    .get(op(value.value)?.0 as usize)
                    .ok_or_else(|| invalid("Missing constructed feature value type", None))?;
                // A many-valued feature is written as a Sequence of its element.
                let expected_value = match members.table.get(evidence.value) {
                    Ok(Type::Many { element, .. }) => {
                        let element = *element;
                        members
                            .table
                            .intern(Type::Collection {
                                kind: CollectionKind::Sequence,
                                element,
                            })
                            .map_err(|e| invalid(e, None))?
                    }
                    _ => evidence.value,
                };
                let mut types = Vec::new();
                let mut mapping = BTreeMap::new();
                let expected = export_type(
                    expected_value,
                    members.table,
                    &mut types,
                    &mut mapping,
                    &verified.limits,
                )?;
                let actual = export_type(
                    actual,
                    members.table,
                    &mut types,
                    &mut mapping,
                    &verified.limits,
                )?;
                if evidence.declaration != value.declaration || actual != expected {
                    return Err(invalid(
                        "Constructed feature declaration or value type differs from schema",
                        None,
                    ));
                }
            }
        }
        InstructionKind::RecordField { value, field } => {
            let TypeNode::Record(fields) = verified.type_node(op(*value)?)? else {
                return Err(invalid("Record projection requires record", None));
            };
            let ty = fields
                .get(field)
                .ok_or_else(|| invalid("Unknown record projection field", None))?;
            require(verified, *ty, instruction.ty)?;
        }
        InstructionKind::Index { value, index } => {
            let (kind, element) = collection(verified.type_node(op(*value)?)?)
                .ok_or_else(|| invalid("Index requires ordered collection", None))?;
            if !matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet)
                || verified.type_node(op(*index)?)? != &TypeNode::Scalar(Scalar::Int64)
            {
                return Err(invalid("Invalid collection indexing operands", None));
            }
            if target != &TypeNode::Option(element) {
                return Err(invalid(
                    "Collection index result must be exact Option element",
                    None,
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn serialized_size<T: Serialize>(value: &T, limit: usize) -> Result<usize, IrError> {
    struct Counter {
        remaining: usize,
        used: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.remaining {
                return Err(std::io::Error::other(
                    "Function metadata byte limit exceeded",
                ));
            }
            self.remaining -= bytes.len();
            self.used += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        remaining: limit,
        used: 0,
    };
    serde_json::to_writer(&mut counter, value).map_err(|e| IrError {
        kind: IrErrorKind::ResourceExhausted,
        message: e.to_string(),
        span: None,
    })?;
    Ok(counter.used)
}
fn preflight(program: &Program, limits: &Limits, remaining: &mut usize) -> Result<(), IrError> {
    let mut functions = limits.max_functions;
    spend(&mut functions, program.functions.len())?;
    let mut types = limits.max_types;
    spend(&mut types, program.types.len())?;
    for node in &program.types {
        spend(remaining, 1)?;
        spend(
            remaining,
            match node {
                TypeNode::Tuple(items) => items.len(),
                TypeNode::Record(fields) => fields.len(),
                TypeNode::Function { parameters, .. } => parameters.len().saturating_add(1),
                TypeNode::Option(_) | TypeNode::Collection { .. } | TypeNode::Many { .. } => 1,
                TypeNode::Result { .. } => 2,
                _ => 0,
            },
        )?;
    }
    let mut captures = limits.max_captures;
    let mut blocks = limits.scalar.max_blocks;
    let mut instructions = limits.scalar.max_instructions;
    let mut params = limits.scalar.max_parameters;
    for function in &program.functions {
        spend(remaining, 1)?;
        spend(remaining, function.captures.len())?;
        spend(remaining, function.parameters.len())?;
        spend(&mut captures, function.captures.len())?;
        spend(&mut blocks, function.blocks.len())?;
        spend(&mut params, function.parameters.len())?;
        for block in &function.blocks {
            spend(&mut instructions, block.instructions.len())?;
            spend(&mut params, block.parameters.len())?;
            spend(remaining, block.parameters.len())?;
            for instruction in &block.instructions {
                let arity = match &instruction.kind {
                    InstructionKind::CollectionAlgebra { other, .. } => {
                        1 + usize::from(other.is_some())
                    }
                    InstructionKind::CollectionCall { seed, .. } => 2 + usize::from(seed.is_some()),
                    InstructionKind::MakeClosure { captures, .. } => captures.len(),
                    InstructionKind::Call { arguments, .. } => arguments.len().saturating_add(1),
                    InstructionKind::Tuple(items) | InstructionKind::Collection(items) => {
                        items.len()
                    }
                    InstructionKind::Record(fields) => fields.len(),
                    InstructionKind::Binary { .. }
                    | InstructionKind::Index { .. }
                    | InstructionKind::CollectionMembership { .. } => 2,
                    InstructionKind::Constant(_) | InstructionKind::None => 0,
                    _ => 1,
                };
                spend(remaining, arity)?;
                if let InstructionKind::Constant(value) = &instruction.kind {
                    preflight_value(value, limits, remaining)?;
                }
            }
            match &block.terminator {
                Terminator::Jump(edge) => spend(remaining, edge.arguments.len())?,
                Terminator::Branch {
                    then_edge,
                    else_edge,
                    ..
                } => {
                    spend(remaining, then_edge.arguments.len())?;
                    spend(remaining, else_edge.arguments.len())?;
                }
                Terminator::Option { none, some, .. } => {
                    spend(remaining, none.arguments.len())?;
                    spend(remaining, some.arguments.len())?;
                }
                Terminator::Result { ok, error, .. } => {
                    spend(remaining, ok.arguments.len())?;
                    spend(remaining, error.arguments.len())?;
                }
                _ => {}
            }
        }
    }
    serialized_size(program, limits.scalar.max_source_bytes)?;
    Ok(())
}
fn preflight_value(
    root: &ValueWire,
    limits: &Limits,
    remaining: &mut usize,
) -> Result<(), IrError> {
    let mut pending = vec![(root, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        spend(remaining, 1)?;
        if depth > limits.max_type_depth {
            return Err(IrError {
                kind: IrErrorKind::ResourceExhausted,
                message: "Function constant depth exceeded".into(),
                span: None,
            });
        }
        match &value.data {
            DataWire::Some(v) | DataWire::Ok(v) | DataWire::Err(v) => pending.push((v, depth + 1)),
            DataWire::Tuple(items) | DataWire::Collection(items) => {
                spend(remaining, items.len())?;
                pending.extend(items.iter().map(|v| (v, depth + 1)));
            }
            DataWire::Record(fields) => {
                spend(remaining, fields.len())?;
                pending.extend(fields.values().map(|v| (v, depth + 1)));
            }
            DataWire::Scalar(v) => ir::validate_value_resources(v, &limits.scalar.values)?,
            DataWire::Datatype(v) | DataWire::Model(v) => {
                let mut budget = ValueBudget {
                    remaining_nodes: *remaining,
                    remaining_bytes: limits.scalar.values.max_value_bytes,
                    max_depth: limits.max_type_depth,
                    scalar: limits.scalar.values.clone(),
                };
                budget.json(v)?;
                *remaining = budget.remaining_nodes;
            }
            _ => {}
        }
    }
    Ok(())
}
pub fn preflight_selection(selection: &EntrySelection, limits: &Limits) -> Result<(), IrError> {
    let mut types = limits.max_types;
    spend(&mut types, selection.types.len())?;
    let mut work = limits.scalar.max_work;
    spend(&mut work, selection.arguments.len())?;
    serialized_size(selection, limits.scalar.max_source_bytes)?;
    Ok(())
}
pub(super) fn context_hash(
    bundle: &DefinitionBundle,
    registry: Option<&ConversionRegistry<'_>>,
    limits: &Limits,
) -> Result<String, IrError> {
    use sha2::{Digest, Sha256};
    ir::preflight_bundle(bundle, &limits.scalar)?;
    bundle.validate().map_err(|e| invalid(e, None))?;
    let mut canonical = bundle.clone();
    canonical.sources.sort_by(|a, b| a.id.cmp(&b.id));
    for source in &mut canonical.sources {
        source.dependencies.sort();
    }
    canonical
        .namespaces
        .sort_by(|a, b| a.namespace.cmp(&b.namespace));
    let policies = policy_manifest(registry)?;
    serialized_size(&policies, limits.scalar.max_source_bytes)?;
    let bytes =
        serde_json::to_vec(&(canonical, policies)).map_err(|e| invalid(e.to_string(), None))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// The fresh source check that verification performs.
pub(crate) fn verification_check(
    bundle: &DefinitionBundle,
    entry: &crate::definitions::bundle::SourceId,
    registry: Option<&ConversionRegistry<'_>>,
    source_limits: crate::definitions::ecore::ParseLimits,
    store: Option<&crate::definitions::rcore_behavior::model_types::RuntimeSchemaStore>,
) -> Result<VerificationCheck, IrError> {
    checking::check_with_store(bundle, entry, registry, source_limits, store).map_err(|errors| {
        errors
            .first()
            .map(|e| invalid(e.message.clone(), Some(&e.span)))
            .unwrap_or_else(|| invalid("Invalid source context", None))
    })
}

/// Model types for verification, configured from its own fresh check.
pub(crate) fn verification_models<'m>(
    bundle: &'m DefinitionBundle,
    registry: Option<&'m ConversionRegistry<'m>>,
    source_limits: crate::definitions::ecore::ParseLimits,
    report: &checking::TypeReport,
    resources: crate::definitions::resources::ResourceSet,
    store: Option<&crate::definitions::rcore_behavior::model_types::RuntimeSchemaStore>,
) -> Result<ModelTypes<'m>, IrError> {
    let mut models = ModelTypes::with_source_resources(bundle, registry, source_limits, resources);
    models.attach_schema_store(store);
    models
        .configure_symbolic(&report.signatures)
        .map_err(|e| invalid(e.message, Some(&e.span)))?;
    Ok(models)
}

/// The saved-image context of one bundle, for checking many saved images
/// against it (`VerifiedExecutable::load_trusted_in`). It holds the context
/// hash that `check_saved_with_context` checks for every image (`context_hash`
/// with no registry), computed once per distinct `Limits` on first use, so a
/// batch of images saved under the same limits hashes the bundle once.
pub(crate) struct SavedContext<'b> {
    bundle: &'b DefinitionBundle,
    hashes: Vec<(Limits, String)>,
}
impl<'b> SavedContext<'b> {
    pub(crate) fn new(bundle: &'b DefinitionBundle) -> Self {
        Self {
            bundle,
            hashes: Vec::new(),
        }
    }
    /// `context_hash(bundle, None, limits)`, computed at most once per
    /// distinct `limits`. A failed computation is not kept: it fails again,
    /// with the same error, on the next image.
    fn hash(&mut self, limits: &Limits) -> Result<&str, IrError> {
        let index = match self.hashes.iter().position(|(seen, _)| seen == limits) {
            Some(index) => index,
            None => {
                let hash = context_hash(self.bundle, None, limits)?;
                self.hashes.push((limits.clone(), hash));
                self.hashes.len() - 1
            }
        };
        self.hashes
            .get(index)
            .map(|(_, hash)| hash.as_str())
            .ok_or_else(|| invalid("Missing saved-image context hash", None))
    }
}

/// Structural checks for a saved image (see `VerifiedExecutable::load_trusted`):
/// IR version, bounded program shape, source and policy manifests, effect
/// count, and the context hash of exactly the bundle of `saved`. The hash is
/// taken from `saved` (computed there at most once per distinct `limits`), so
/// a batch of images shares it; a single image uses a fresh `SavedContext`.
pub(super) fn check_saved_with_context(
    program: &Program,
    effects: &[Effect],
    context: &str,
    limits: &Limits,
    saved: &mut SavedContext<'_>,
) -> Result<(), IrError> {
    let bundle = saved.bundle;
    if program.version != VERSION {
        return Err(invalid("Saved image has another function IR version", None));
    }
    let mut remaining = limits.scalar.max_work;
    preflight(program, limits, &mut remaining)?;
    let mut sources = bundle
        .sources
        .iter()
        .map(|source| ir::SourceManifestEntry {
            source: source.id.clone(),
        })
        .collect::<Vec<_>>();
    sources.sort_by(|a, b| a.source.cmp(&b.source));
    if sources != program.sources {
        return Err(invalid("Saved image names other sources", None));
    }
    if program.policies != policy_manifest(None)? {
        return Err(invalid("Saved image has a provider policy manifest", None));
    }
    if effects.len() != program.functions.len() {
        return Err(invalid(
            "Saved image effects do not match its functions",
            None,
        ));
    }
    if saved.hash(limits)? != context {
        return Err(invalid(
            "Saved image was verified against another bundle",
            None,
        ));
    }
    Ok(())
}
