//! Compatibility policy over the common frame machine, not an interpreter.
use super::{
    machine::{ExecutionImage, ExecutionState, State},
    values::{Data, Id, Node},
    *,
};
use crate::rcore_behavior_execution as legacy;
use intersect_model::definitions::rcore_behavior::{
    ir::{self, Value, ValueType},
    ir_v2::{self, TypeId},
    types::Scalar,
};
use std::collections::BTreeMap;

fn legacy_error(
    verified: &ir::VerifiedProgram,
    kind: legacy::ExecutionErrorKind,
    message: impl Into<String>,
    span: Option<&SourceSpan>,
) -> legacy::ExecutionError {
    legacy::ExecutionError {
        kind,
        message: message.into(),
        arithmetic_kind: None,
        symbol: verified.program().symbol.clone(),
        span: span.cloned().map(Box::new),
    }
}
/// Exact v1 validation and charge order, shared by preparation and machine hooks.
fn account(
    verified: &ir::VerifiedProgram,
    value: &Value,
    limits: &legacy::ExecutionLimits,
    bytes: &mut usize,
    span: Option<&SourceSpan>,
) -> std::result::Result<usize, legacy::ExecutionError> {
    verified
        .validate_value_checked(value, &limits.values)
        .map_err(|e| {
            legacy_error(
                verified,
                if e.kind == ir::IrErrorKind::ResourceExhausted {
                    legacy::ExecutionErrorKind::ResourceExhausted
                } else {
                    legacy::ExecutionErrorKind::Arguments
                },
                e.message,
                span,
            )
        })?;
    let size = serde_json::to_vec(value)
        .map_err(|e| {
            legacy_error(
                verified,
                legacy::ExecutionErrorKind::InvalidProgram,
                e.to_string(),
                span,
            )
        })?
        .len();
    if size > limits.values.max_value_bytes {
        return Err(legacy_error(
            verified,
            legacy::ExecutionErrorKind::ResourceExhausted,
            "Scalar value byte budget exceeded",
            span,
        ));
    }
    *bytes = bytes
        .checked_add(size)
        .filter(|n| *n <= limits.max_value_bytes)
        .ok_or_else(|| {
            legacy_error(
                verified,
                legacy::ExecutionErrorKind::ResourceExhausted,
                "Cumulative scalar value byte budget exceeded",
                span,
            )
        })?;
    Ok(size)
}
fn node_value(nodes: &[Node], id: Id) -> Option<&Value> {
    match &nodes.get(id)?.data {
        Data::Scalar(value) => Some(value),
        _ => None,
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    fn scalar_image(&self) -> Result<&ir_v2::VerifiedScalarExecutable<'_>> {
        match self.image {
            ExecutionImage::Scalar(image) => Ok(image),
            _ => Err(self.invalid("Expected scalar execution authority", None)),
        }
    }
    fn scalar_error(&self, error: legacy::ExecutionError) -> ExecutionError {
        let kind = match error.kind {
            legacy::ExecutionErrorKind::Arguments => ExecutionErrorKind::Arguments,
            legacy::ExecutionErrorKind::ResourceExhausted => ExecutionErrorKind::ResourceExhausted,
            legacy::ExecutionErrorKind::Arithmetic => ExecutionErrorKind::Arithmetic,
            legacy::ExecutionErrorKind::InvalidProgram => ExecutionErrorKind::InvalidProgram,
        };
        let mut result = self.error(kind, error.message, error.span.as_deref());
        result.arithmetic_kind = error.arithmetic_kind;
        result
    }
    fn scalar_value(&self, id: Id) -> Result<&Value> {
        match &self.node(id)?.data {
            Data::Scalar(value) => Ok(value),
            _ => Err(self.invalid("Scalar capability contains non-scalar runtime data", None)),
        }
    }
    pub fn scalar_transfer(&mut self, id: Id, span: Option<&SourceSpan>) -> Result<()> {
        let mut bytes = self.data.consumed.value_bytes;
        account(
            self.scalar_image()?.original(),
            self.scalar_value(id)?,
            &self.limits.scalar,
            &mut bytes,
            span,
        )
        .map_err(|e| self.scalar_error(e))?;
        self.data.consumed.value_bytes = bytes;
        Ok(())
    }
    fn scalar_store(&mut self, value: &Value, span: Option<&SourceSpan>) -> Result<Id> {
        let mut bytes = self.data.consumed.value_bytes;
        let size = account(
            self.scalar_image()?.original(),
            value,
            &self.limits.scalar,
            &mut bytes,
            span,
        )
        .map_err(|e| self.scalar_error(e))?;
        let ty = self
            .scalar_image()?
            .type_id(&value.ty)
            .map_err(|e| self.invalid(&e.message, span))?;
        self.data.consumed.value_bytes = bytes;
        let id = self.data.nodes.len();
        self.data.nodes.push(Node {
            ty,
            data: Data::Scalar(value.clone()),
            bytes: size,
            depth: 1,
        });
        self.data.consumed.value_nodes += 1;
        Ok(id)
    }
    pub fn scalar_constant(
        &mut self,
        block: usize,
        instruction: usize,
        span: &SourceSpan,
    ) -> Result<Id> {
        let ExecutionImage::Scalar(image) = self.image else {
            return Err(self.invalid("Expected scalar constant authority", Some(span)));
        };
        let value = image
            .original()
            .program()
            .blocks
            .get(block)
            .and_then(|b| b.instructions.get(instruction))
            .ok_or_else(|| self.invalid("Verified instruction is unavailable", Some(span)))?;
        let ir::InstructionKind::Constant(value) = &value.kind else {
            return Err(self.invalid("Expected original scalar constant", Some(span)));
        };
        self.scalar_store(value, Some(span))
    }
    pub fn scalar_instruction_result(
        &self,
        id: Id,
        expected: TypeId,
        span: &SourceSpan,
    ) -> Result<()> {
        if self.node(id)?.ty != expected {
            return Err(self.invalid("Scalar operation returned the wrong type", Some(span)));
        }
        self.scalar_image()?
            .validate_value(self.scalar_value(id)?, &self.limits.scalar.values)
            .map_err(|message| self.invalid(&message, Some(span)))
    }
    pub fn scalar_unary(&mut self, op: ir::UnaryOp, id: Id, span: &SourceSpan) -> Result<Id> {
        let value = node_value(&self.data.nodes, id)
            .ok_or_else(|| self.invalid("Verified scalar operand unavailable", Some(span)))?;
        let result = crate::rcore_behavior_scalar::unary(op, value, &mut self.data.numeric)
            .map_err(|e| self.arithmetic(e, span))?;
        self.scalar_store(&result, Some(span))
    }
    pub fn scalar_binary(
        &mut self,
        op: ir::BinaryOp,
        a: Id,
        b: Id,
        span: &SourceSpan,
    ) -> Result<Id> {
        let lhs = node_value(&self.data.nodes, a)
            .ok_or_else(|| self.invalid("Verified scalar operand unavailable", Some(span)))?;
        let rhs = node_value(&self.data.nodes, b)
            .ok_or_else(|| self.invalid("Verified scalar operand unavailable", Some(span)))?;
        let result = if matches!(lhs.ty, ValueType::Enum(_)) {
            if lhs.ty != rhs.ty || !matches!(op, ir::BinaryOp::Equal | ir::BinaryOp::NotEqual) {
                return Err(self.invalid("Invalid verified enum operation", Some(span)));
            }
            let cost = serde_json::to_vec(lhs)
                .and_then(|left| {
                    serde_json::to_vec(rhs).map(|right| left.len().saturating_add(right.len()))
                })
                .map_err(|e| self.invalid(&e.to_string(), Some(span)))?;
            self.data.numeric.charge(cost as u64).map(|()| Value {
                ty: ValueType::Scalar(Scalar::Bool),
                payload: serde_json::json!(
                    (lhs.payload == rhs.payload) == (op == ir::BinaryOp::Equal)
                ),
            })
        } else {
            crate::rcore_behavior_scalar::binary(op, lhs, rhs, &mut self.data.numeric)
        }
        .map_err(|e| self.arithmetic(e, span))?;
        self.scalar_store(&result, Some(span))
    }
}

pub(crate) fn evaluate_scalar(
    verified: &ir::VerifiedProgram,
    arguments: &[Value],
    limits: &legacy::ExecutionLimits,
) -> std::result::Result<legacy::Evaluation, legacy::ExecutionError> {
    // Preserve argument failure order before normalization allocates any image.
    if arguments.len() != verified.program().parameters.len() {
        return Err(legacy_error(
            verified,
            legacy::ExecutionErrorKind::Arguments,
            "Scalar argument count differs from selected signature",
            None,
        ));
    }
    let mut bytes = 0;
    let mut sizes = Vec::new();
    for (argument, ty) in arguments.iter().zip(&verified.program().parameters) {
        if &argument.ty != ty {
            return Err(legacy_error(
                verified,
                legacy::ExecutionErrorKind::Arguments,
                "Scalar argument type differs from selected signature",
                None,
            ));
        }
        sizes.push(account(verified, argument, limits, &mut bytes, None)?);
        verified
            .validate_value(argument, &limits.values)
            .map_err(|message| {
                legacy_error(
                    verified,
                    legacy::ExecutionErrorKind::Arguments,
                    message,
                    None,
                )
            })?;
    }
    let image = ir_v2::lift_scalar(verified).map_err(|e| {
        legacy_error(
            verified,
            legacy::ExecutionErrorKind::InvalidProgram,
            e.message,
            None,
        )
    })?;
    // Only scalar limits participate. Arena nodes are arguments + executed
    // instructions; lifted forward-only blocks cannot create calls or cycles.
    let internal_limits = ExecutionLimits {
        scalar: limits.clone(),
        max_frames: 1,
        max_value_nodes: usize::MAX,
        ..Default::default()
    };
    let mut execution: ExecutionState = ExecutionState {
        observation: super::observation::Recorder::new(&internal_limits.observation),
        consumed: Consumption {
            value_bytes: bytes,
            ..Default::default()
        },
        numeric: NumericBudget::new(limits.numeric.clone()),
        nodes: vec![],
        closures: vec![],
        frames: vec![],
        constants: BTreeMap::new(),
        preparing: None,
        provider_receipts: std::rc::Rc::new(super::providers::Receipts::new(&internal_limits)),
        domains: BTreeMap::new(),
        contexts: BTreeMap::new(),
        reference_inputs: BTreeMap::new(),
        owned_reference_inputs: None,
        datatypes: BTreeMap::new(),
    };
    let mut state = State {
        image: ExecutionImage::Scalar(&image),
        context: None,
        limits: &internal_limits,
        control: None,
        data: &mut execution,
    };
    let outcome = (|| -> Result<legacy::Evaluation> {
        let mut ids = Vec::new();
        for (value, size) in arguments.iter().zip(sizes) {
            let ty = image
                .type_id(&value.ty)
                .map_err(|e| state.invalid(&e.message, None))?;
            ids.push(state.data.nodes.len());
            state.data.nodes.push(Node {
                ty,
                data: Data::Scalar(value.clone()),
                bytes: size,
                depth: 1,
            });
            state.data.consumed.value_nodes += 1;
        }
        let entry = image
            .program()
            .functions
            .first()
            .ok_or_else(|| state.invalid("Missing scalar entry function", None))?;
        state.push_scalar_entry(image.program().entry, ids, vec![], None, entry.result)?;
        let result = state.run()?;
        Ok(legacy::Evaluation {
            value: state.scalar_value(result)?.clone(),
            steps: state.data.consumed.steps,
            numeric_work: state.data.numeric.work_used(),
            value_bytes: state.data.consumed.value_bytes,
        })
    })();
    outcome.map_err(|error| legacy::ExecutionError {
        kind: match error.kind {
            ExecutionErrorKind::Arguments => legacy::ExecutionErrorKind::Arguments,
            ExecutionErrorKind::ResourceExhausted => legacy::ExecutionErrorKind::ResourceExhausted,
            ExecutionErrorKind::Arithmetic => legacy::ExecutionErrorKind::Arithmetic,
            _ => legacy::ExecutionErrorKind::InvalidProgram,
        },
        message: *error.message,
        arithmetic_kind: error.arithmetic_kind,
        symbol: verified.program().symbol.clone(),
        span: error.span,
    })
}
