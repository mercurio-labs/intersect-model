//! Default retained numeric phases over shared scalar semantics.
//! The same arena allocation is moved into a safe owner during input work.
//! No callback/registry/image/context lease survives an advance.
use super::{
    machine::State,
    values::{Data, Id, Node},
    *,
};
use crate::definitions::rcore_behavior::ir::scalar_validation::ScalarSource;
use crate::definitions::rcore_behavior::ir_v2::TypeId;
use crate::definitions::rcore_behavior::{
    ir::{self, Value, ValueType},
    types::Scalar,
};
use crate::rcore_behavior_scalar::{self as scalar, validation::NumericValidation};

#[derive(Clone, Copy)]
pub(super) enum Command {
    Unary(ir::UnaryOp),
    Binary(ir::BinaryOp),
}
#[derive(Clone, Copy)]
enum InputPhase {
    Left,
    Right,
    Kernel,
}
enum InputWork<'a> {
    Validation(NumericValidation<'a>),
    Binary(scalar::binary_job::BinaryJob<'a>),
}
#[cfg(feature = "allocation-profile")]
impl InputWork<'_> {
    fn progress(&self) -> Option<(&'static str, usize, usize)> {
        match self {
            Self::Validation(value) => value.progress(),
            Self::Binary(value) => value.progress(),
        }
    }
}
struct InputJob<'a> {
    left: &'a Value,
    right: Option<&'a Value>,
    work: InputWork<'a>,
    phase: InputPhase,
}
self_cell::self_cell!(
    struct InputCell {
        owner: Vec<Node>,
        #[covariant]
        dependent: InputJob,
    }
);
struct ResultOwner {
    kind: Scalar,
    payload: serde_json::Value,
}
self_cell::self_cell!(
    struct ResultCell {
        owner: ResultOwner,
        #[covariant]
        dependent: NumericValidation,
    }
);
enum Stage {
    Input(InputCell),
    Result(ResultCell),
    Failed,
}
pub(super) struct Numeric {
    stage: Stage,
    command: Command,
}

/// Freshly measured, never serialized or imported; publication remains late.
pub(super) struct MeasuredScalar {
    value: Value,
    bytes: usize,
}
impl MeasuredScalar {
    pub(super) fn into_value(self) -> Value {
        self.value
    }
    pub(super) fn publish<I: super::inputs::InputFamily>(
        self,
        state: &mut State<'_, '_, '_, I>,
        ty: TypeId,
        span: &SourceSpan,
    ) -> Result<Id> {
        // Keep original node-before-runtime-admission order AFTER all original
        // numeric input/kernel/result work. Do not move this before the kernel.
        state.ensure_leaf_node(Some(span))?;
        let ticket = state.admit_measured_leaf(ty, self.bytes, Some(span))?;
        state.publish_scalar(ticket, self.value, Some(span))
    }
}
impl Numeric {
    #[cfg(feature = "allocation-profile")]
    fn progress(&self) -> Option<super::RetainedNumericPhase> {
        match &self.stage {
            Stage::Input(cell) => {
                let job = cell.borrow_dependent();
                Some((
                    match job.phase {
                        InputPhase::Left => "left",
                        InputPhase::Right => "right",
                        InputPhase::Kernel => "kernel",
                    },
                    job.work.progress(),
                    cell.borrow_owner().len(),
                    cell.borrow_owner().capacity(),
                ))
            }
            Stage::Result(cell) => Some(("result", cell.borrow_dependent().progress(), 0, 0)),
            Stage::Failed => None,
        }
    }

    pub(super) const OWNER_BYTES: usize = {
        let input = std::mem::size_of::<(Vec<Node>, InputJob<'static>)>()
            + std::mem::size_of::<InputCell>();
        let output = std::mem::size_of::<(ResultOwner, NumericValidation<'static>)>()
            + std::mem::size_of::<ResultCell>();
        if input > output { input } else { output }
    };
    #[cfg(feature = "allocation-profile")]
    pub(super) fn layouts() -> serde_json::Value {
        use std::mem::size_of;
        serde_json::json!({"input_owner":size_of::<Vec<Node>>(),"input_dependent":size_of::<InputJob>(),"input_cell_header":size_of::<InputCell>(),"input_joined_storage":size_of::<(Vec<Node>,InputJob)>(),"result_owner":size_of::<ResultOwner>(),"result_dependent":size_of::<NumericValidation>(),"result_cell_header":size_of::<ResultCell>(),"result_joined_storage":size_of::<(ResultOwner,NumericValidation)>(),"controller_header":size_of::<Numeric>(),"instruction":size_of::<Instruction>(),"conservative_extra_owner":Self::OWNER_BYTES})
    }

    /// Caller has performed the exact original transfers/comparison prelude.
    pub(super) fn begin<I: super::inputs::InputFamily>(
        state: &mut State<'_, '_, '_, I>,
        left: Id,
        right: Option<Id>,
        command: Command,
        span: &SourceSpan,
    ) -> Result<Self> {
        let left_origin = state.node(left)?.origin;
        let right_origin = right
            .map(|id| state.node(id).map(|node| node.origin))
            .transpose()?;
        let is_scalar = |origin| {
            matches!(
                state.data.nodes.get(origin),
                Some(Node {
                    data: Data::Scalar(_),
                    ..
                })
            )
        };
        if !is_scalar(left_origin) || right_origin.is_some_and(|origin| !is_scalar(origin)) {
            return Err(state.invalid(
                if matches!(command, Command::Unary(_)) {
                    "Unary operation requires scalar"
                } else {
                    "Arithmetic requires scalar values"
                },
                Some(span),
            ));
        }
        let nodes = std::mem::take(&mut state.data.nodes);
        // A failed invariant must recover the SAME owner, never discard it.
        let cell = InputCell::try_new_or_recover(nodes, |nodes| {
            let select = |origin| match nodes.get(origin) {
                Some(Node {
                    data: Data::Scalar(value),
                    ..
                }) => Ok(value),
                _ => Err("Numeric input owner changed before preparation"),
            };
            let left = select(left_origin)?;
            let right = right_origin.map(select).transpose()?;
            Ok::<_, &str>(InputJob {
                left,
                right,
                work: InputWork::Validation(NumericValidation::new(
                    left,
                    &state.data.numeric.limits,
                )),
                phase: InputPhase::Left,
            })
        });
        match cell {
            Ok(cell) => Ok(Self {
                stage: Stage::Input(cell),
                command,
            }),
            Err((nodes, error)) => {
                state.data.nodes = nodes;
                Err(state.invalid(error, Some(span)))
            }
        }
    }
    pub(super) fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: &SourceSpan,
    ) -> Result<Option<MeasuredScalar>> {
        state.checkpoint(Some(span))?;
        state.check_provider_status(Some(span))?;
        match std::mem::replace(&mut self.stage, Stage::Failed) {
            Stage::Input(mut cell) => {
                let command = self.command;
                let progress = cell.with_dependent_mut(|_, job| {
                    if matches!(job.phase, InputPhase::Kernel) {
                        let control = state.control;
                        let cancelled =
                            || control.is_some_and(|control| control.cancellation_requested());
                        // The budget/control lease is fresh for this transition.
                        // Completed validation and binary work are exclusive in
                        // the existing stable arena owner, never simultaneous.
                        return match (command, job.right) {
                            (Command::Unary(op), None) => scalar::unary_prevalidated(
                                op,
                                job.left,
                                &mut state.data.numeric,
                                Some(&cancelled),
                            )
                            .map(Some),
                            (Command::Binary(op), Some(right)) => {
                                if matches!(&job.work, InputWork::Validation(_)) {
                                    job.work = InputWork::Binary(
                                        scalar::binary_job::BinaryJob::new(op, job.left, right),
                                    );
                                }
                                let InputWork::Binary(binary) = &mut job.work else {
                                    return Err(scalar::error(
                                        scalar::ArithmeticErrorKind::InvalidValue,
                                        "Missing binary kernel continuation",
                                    ));
                                };
                                binary
                                    .advance_prevalidated(&mut state.data.numeric, Some(&cancelled))
                            }
                            _ => Err(scalar::error(
                                scalar::ArithmeticErrorKind::InvalidValue,
                                "Numeric command arity changed",
                            )),
                        };
                    }
                    let InputWork::Validation(validation) = &mut job.work else {
                        return Err(scalar::error(
                            scalar::ArithmeticErrorKind::InvalidValue,
                            "Missing numeric input validation",
                        ));
                    };
                    if validation.advance(&mut state.data.numeric)?.is_some() {
                        match (job.phase, job.right) {
                            (InputPhase::Left, Some(right)) => {
                                job.work = InputWork::Validation(NumericValidation::new(
                                    right,
                                    &state.data.numeric.limits,
                                ));
                                job.phase = InputPhase::Right;
                            }
                            _ => job.phase = InputPhase::Kernel,
                        }
                    }
                    Ok(None)
                });
                match progress {
                    Ok(None) => {
                        self.stage = Stage::Input(cell);
                        Ok(None)
                    }
                    Ok(Some(value)) => {
                        // Drop all dependent borrows BEFORE restoring arena.
                        state.data.nodes = cell.into_owner();
                        let ValueType::Scalar(kind) = value.ty else {
                            return Err(state
                                .invalid("Numeric kernel returned non-scalar header", Some(span)));
                        };
                        let limits = &state.data.numeric.limits;
                        // Old frontier/cell is gone before result cell allocation.
                        self.stage = Stage::Result(ResultCell::new(
                            ResultOwner {
                                kind,
                                payload: value.payload,
                            },
                            |owner| {
                                NumericValidation::from_source(
                                    ScalarSource::scalar(owner.kind, &owner.payload),
                                    limits,
                                )
                            },
                        ));
                        Ok(None)
                    }
                    Err(error) => {
                        state.data.nodes = cell.into_owner();
                        Err(state.arithmetic(error, span))
                    }
                }
            }
            Stage::Result(mut cell) => {
                let progress = cell.with_dependent_mut(|_, validation| {
                    validation.advance(&mut state.data.numeric)
                });
                match progress {
                    Ok(None) => {
                        self.stage = Stage::Result(cell);
                        Ok(None)
                    }
                    Ok(Some(bytes)) => {
                        // Single original result-byte debit, after its exact
                        // validation/work fees and before late arena node checks.
                        state
                            .data
                            .numeric
                            .finish_measured(bytes)
                            .map_err(|error| state.arithmetic(error, span))?;
                        let owner = cell.into_owner();
                        Ok(Some(MeasuredScalar {
                            value: Value {
                                ty: ValueType::Scalar(owner.kind),
                                payload: owner.payload,
                            },
                            bytes,
                        }))
                    }
                    Err(error) => Err(state.arithmetic(error, span)),
                }
            }
            Stage::Failed => Err(state.invalid(
                "Numeric continuation already failed or completed",
                Some(span),
            )),
        }
    }
}

pub(super) struct Instruction {
    pub(super) site: super::machine::CallSite,
    ty: TypeId,
    numeric: Numeric,
    equal: Option<bool>,
    // Preserve the exact original equality admission/capacity until the same
    // logical comparison completes; destroy it before late arena publication.
    comparison: Option<super::equivalence::Equivalence>,
}
#[cfg(feature = "allocation-profile")]
impl Instruction {
    pub(super) fn progress(&self) -> Option<super::RetainedNumericPhase> {
        self.numeric.progress()
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_numeric_unary(
        &mut self,
        op: ir::UnaryOp,
        left: Id,
        ty: TypeId,
        site: super::machine::CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing unary source", None))?;
        self.transfer(left, Some(span))?;
        let numeric = Numeric::begin(self, left, None, Command::Unary(op), span)?;
        let pending = Instruction {
            site,
            ty,
            numeric,
            equal: None,
            comparison: None,
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing numeric frame", Some(span)));
        };
        frame.set_numeric(pending);
        Ok(())
    }
    pub(super) fn start_numeric_binary(
        &mut self,
        op: ir::BinaryOp,
        left: Id,
        right: Id,
        ty: TypeId,
        site: super::machine::CallSite,
    ) -> Result<Option<Id>> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing binary source", None))?;
        let equality = matches!(op, ir::BinaryOp::Equal | ir::BinaryOp::NotEqual);
        let comparison = if equality {
            let mut comparison =
                super::equivalence::Equivalence::new(self, left, right, false, 0, Some(span))?;
            let mut deferred = None;
            let result = comparison.advance_numeric(self, Some(span), Some(&mut deferred))?;
            if let Some(equal) = result {
                drop(comparison);
                return self
                    .alloc(
                        ty,
                        Data::Scalar(Value {
                            ty: ValueType::Scalar(Scalar::Bool),
                            payload: serde_json::json!(equal == (op == ir::BinaryOp::Equal)),
                        }),
                        Some(span),
                    )
                    .map(Some);
            }
            if deferred != Some((left, right)) {
                return Err(self.invalid("Missing scalar equality selection", Some(span)));
            }
            Some(comparison)
        } else {
            self.transfer(left, Some(span))?;
            self.transfer(right, Some(span))?;
            None
        };
        let command = Command::Binary(if equality { ir::BinaryOp::Equal } else { op });
        let numeric = Numeric::begin(self, left, Some(right), command, span)?;
        let pending = Instruction {
            site,
            ty,
            numeric,
            equal: equality.then_some(op == ir::BinaryOp::Equal),
            comparison,
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing numeric frame", Some(span)));
        };
        frame.set_numeric(pending);
        Ok(None)
    }
    pub(super) fn advance_numeric(&mut self) -> Result<()> {
        let mut pending = self
            .data
            .frames
            .last_mut()
            .and_then(|frame| frame.take_numeric())
            .ok_or_else(|| self.invalid("Missing numeric continuation", None))?;
        let span = self
            .image
            .call_span(pending.site)
            .ok_or_else(|| self.invalid("Missing numeric source", None))?;
        let result = pending.numeric.advance(self, span)?;
        match result {
            None => {
                let Some(frame) = self.data.frames.last_mut() else {
                    return Err(self.invalid("Missing numeric continuation frame", Some(span)));
                };
                frame.set_numeric(pending);
            }
            Some(measured) => {
                let Instruction {
                    ty,
                    equal,
                    comparison,
                    numeric,
                    ..
                } = pending;
                drop(comparison);
                drop(numeric);
                let id = if let Some(equal) = equal {
                    let mut value = measured.into_value();
                    let result = value
                        .payload
                        .as_bool()
                        .ok_or_else(|| self.invalid("Equality returned non-Boolean", Some(span)))?;
                    value.payload = serde_json::json!(result == equal);
                    // This Bool is the original outer equality result. Its tiny
                    // measurement happens after dropping the old comparison.
                    self.alloc(ty, Data::Scalar(value), Some(span))?
                } else {
                    measured.publish(self, ty, span)?
                };
                let Some(frame) = self.data.frames.last_mut() else {
                    return Err(self.invalid("Missing numeric result frame", Some(span)));
                };
                frame.registers.push(id);
            }
        }
        Ok(())
    }
}
