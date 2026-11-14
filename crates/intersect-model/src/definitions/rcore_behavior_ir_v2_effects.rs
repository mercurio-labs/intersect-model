//! Handwritten, bounded effect inference over a freshly verified closed image.
use super::*;

fn known_target(
    block: &Block,
    mut operand: Operand,
    remaining: &mut usize,
) -> Result<Option<FunctionId>, IrError> {
    loop {
        spend(remaining, 1)?;
        let Operand::Register(index) = operand else {
            return Ok(None);
        };
        let instruction = block
            .instructions
            .get(index as usize)
            .ok_or_else(|| invalid("Missing callable effect operand", Some(&block.span)))?;
        match &instruction.kind {
            InstructionKind::MakeClosure { function, .. } => return Ok(Some(*function)),
            InstructionKind::View { value } => operand = *value,
            _ => return Ok(None),
        }
    }
}

pub(super) fn infer(
    image: &VerifiedExecutable,
    remaining: &mut usize,
) -> Result<Vec<Effect>, IrError> {
    let program = image.program();
    let mut bytes = image.limits.scalar.max_source_bytes;
    spend(&mut bytes, program.functions.len().saturating_mul(64))?;
    let mut effects = vec![Effect::Pure; program.functions.len()];
    let mut callers = vec![BTreeSet::new(); program.functions.len()];
    let mut closures = BTreeSet::new();
    for function in &program.functions {
        spend(remaining, 1)?;
        for block in &function.blocks {
            spend(remaining, 1)?;
            for instruction in &block.instructions {
                spend(remaining, 1)?;
                if let InstructionKind::MakeClosure { function, .. } = instruction.kind
                    && !closures.contains(&(instruction.ty, function))
                {
                    spend(&mut bytes, 32)?;
                    closures.insert((instruction.ty, function));
                }
            }
        }
    }
    for (caller, function) in program.functions.iter().enumerate() {
        spend(remaining, 1)?;
        // Only the private contract verifier admits a bodyless external entry.
        // Model parameters grant the bounded read-only provider capability.
        if function.blocks.is_empty()
            && function
                .parameters
                .iter()
                .any(|id| matches!(program.types.get(id.0 as usize), Some(TypeNode::Model(_))))
        {
            effects[caller] = Effect::Query;
        }
        for block in &function.blocks {
            spend(remaining, 1)?;
            for (position, instruction) in block.instructions.iter().enumerate() {
                spend(remaining, 1)?;
                let callee = match &instruction.kind {
                    InstructionKind::New(_) => {
                        effects[caller] = Effect::Construct;
                        continue;
                    }
                    InstructionKind::StoredFeature { .. }
                    | InstructionKind::Incoming { .. }
                    | InstructionKind::Ownership { .. } => {
                        effects[caller] = effects[caller].max(Effect::Query);
                        continue;
                    }
                    InstructionKind::Text { op, .. } => {
                        if op.reads_model() {
                            effects[caller] = effects[caller].max(Effect::Query);
                        }
                        continue;
                    }
                    InstructionKind::Call { callee, .. } => *callee,
                    InstructionKind::CollectionCall {
                        operation,
                        callback,
                        ..
                    } => {
                        if *operation == CollectionOp::Closure {
                            effects[caller] = effects[caller].max(Effect::Query);
                        }
                        *callback
                    }
                    // Exhaustive classification makes every future opcode choose
                    // its effect instead of silently inheriting a pure default.
                    InstructionKind::Constant(_)
                    | InstructionKind::Unary { .. }
                    | InstructionKind::Binary { .. }
                    | InstructionKind::MakeClosure { .. }
                    | InstructionKind::View { .. }
                    | InstructionKind::None
                    | InstructionKind::Some(_)
                    | InstructionKind::Ok(_)
                    | InstructionKind::Err(_)
                    | InstructionKind::Tuple(_)
                    | InstructionKind::Record(_)
                    | InstructionKind::Collection(_)
                    | InstructionKind::ConvertCollection { .. }
                    | InstructionKind::CollectionAlgebra { .. }
                    | InstructionKind::CollectionMembership { .. }
                    | InstructionKind::CollectionFlatten { .. }
                    | InstructionKind::CollectionSize { .. }
                    | InstructionKind::CollectionIsEmpty { .. }
                    | InstructionKind::Split { .. }
                    | InstructionKind::TupleField { .. }
                    | InstructionKind::RecordField { .. }
                    | InstructionKind::Index { .. } => continue,
                };
                // Direct closure construction and transparent views retain an
                // exact target. Other value flow is conservatively type-matched
                // against all closures constructible in this closed image.
                let target = known_target(block, callee, remaining)?;
                if let Some(target) = target {
                    add_edge(&mut callers, caller, target, remaining, &mut bytes)?;
                } else {
                    let ty = super::verification::operand(function, block, position, callee)?;
                    for (candidate, target) in &closures {
                        spend(remaining, 1)?;
                        if image.permits(*candidate, ty)? {
                            add_edge(&mut callers, caller, *target, remaining, &mut bytes)?;
                        }
                    }
                }
            }
        }
    }
    // Reverse propagation reaches a fixed point even for recursive SCCs. Every
    // function enters the queue at most once; no native recursive traversal.
    // A caller takes the strongest effect of its callees; each function is
    // raised at most twice (Pure to Query to Construct).
    let mut pending: Vec<_> = effects
        .iter()
        .enumerate()
        .filter_map(|(n, effect)| (*effect != Effect::Pure).then_some(n))
        .collect();
    let mut cursor = 0;
    while let Some(target) = pending.get(cursor).copied() {
        spend(remaining, 1)?;
        cursor += 1;
        for caller in &callers[target] {
            spend(remaining, 1)?;
            if effects[*caller] < effects[target] {
                effects[*caller] = effects[target];
                pending.push(*caller);
            }
        }
    }
    Ok(effects)
}

fn add_edge(
    callers: &mut [BTreeSet<usize>],
    caller: usize,
    target: FunctionId,
    remaining: &mut usize,
    bytes: &mut usize,
) -> Result<(), IrError> {
    spend(remaining, 1)?;
    let incoming = callers
        .get_mut(target.0 as usize)
        .ok_or_else(|| invalid("Unknown callable effect target", None))?;
    if !incoming.contains(&caller) {
        spend(bytes, 32)?;
        incoming.insert(caller);
    }
    Ok(())
}
