use super::machine::State;
use super::*;
use intersect_model::definitions::rcore_behavior::{
    ir,
    ir_v2::{self, TypeId, TypeNode},
    types::{CollectionKind, Scalar},
};
use std::collections::BTreeMap;
pub(super) type Id = usize;
#[derive(Clone)]
pub(super) enum Data {
    /// Private header-only view of an immutable, non-view arena payload.
    View(Id),
    Scalar(ir::Value),
    Enum(intersect_model::definitions::enums::EnumValue),
    Datatype(serde_json::Value),
    Model(serde_json::Value),
    None,
    Some(Id),
    Ok(Id),
    Err(Id),
    Collection(Vec<Id>),
    Tuple(Vec<Id>),
    Record(BTreeMap<String, Id>),
    Closure(usize),
}
pub(super) struct Node {
    pub ty: TypeId,
    pub data: Data,
    pub bytes: usize,
    pub depth: usize,
}
/// A resolved payload borrow with the requested header type and measurements.
pub(super) struct NodeView<'a> {
    pub ty: TypeId,
    pub data: &'a Data,
    pub bytes: usize,
    pub depth: usize,
    pub origin: Id,
}
pub(super) struct ShapeOnly;
/// A private, single-use byte reservation for one scalar arena publication.
/// It cannot be cloned/deserialized; dropping it never refunds cumulative work.
pub(super) struct ScalarAllocation {
    ty: TypeId,
    bytes: usize,
    index: usize,
    nodes: usize,
    reserved_total: usize,
}
impl ir_v2::ValueVisitor for ShapeOnly {
    fn nominal_leaf(
        &mut self,
        _: &TypeNode,
        _: &serde_json::Value,
    ) -> std::result::Result<(), ir_v2::IrError> {
        Ok(())
    }
    fn unique_collection(
        &mut self,
        _: TypeId,
        _: &[ValueWire],
    ) -> std::result::Result<(), ir_v2::IrError> {
        Ok(())
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    // Both ordinary arena allocation and retained String decoding use this exact
    // ordering. Measurement borrows the source and does not construct an owned JSON buffer.
    fn scalar_allocation_size(
        &self,
        value: &ir::Value,
        span: Option<&SourceSpan>,
    ) -> Result<usize> {
        if self.data.consumed.value_nodes >= self.limits.max_value_nodes {
            return Err(self.resource("Runtime value node limit exceeded", span));
        }
        let bytes = super::preflight::serialized_size(value, usize::MAX)
            .map_err(|e| self.invalid(&e, span))?;
        self.check_allocation(bytes, 1, span)?;
        Ok(bytes)
    }
    pub(super) fn admit_scalar_allocation(
        &mut self,
        ty: TypeId,
        value: &ir::Value,
        span: Option<&SourceSpan>,
    ) -> Result<ScalarAllocation> {
        let bytes = self.scalar_allocation_size(value, span)?;
        self.admit_measured_leaf(ty, bytes, span)
    }
    pub(super) fn admit_measured_leaf(
        &mut self,
        ty: TypeId,
        bytes: usize,
        span: Option<&SourceSpan>,
    ) -> Result<ScalarAllocation> {
        self.check_allocation(bytes, 1, span)?;
        // Debit before any owned buffer reservation. The decoder exclusively
        // holds this preparation phase; its ticket retains the original claim.
        self.data.consumed.value_bytes = self
            .data
            .consumed
            .value_bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.scalar.max_value_bytes)
            .ok_or_else(|| self.resource("Cumulative function value byte budget exceeded", span))?;
        Ok(ScalarAllocation {
            ty,
            bytes,
            index: self.data.nodes.len(),
            nodes: self.data.consumed.value_nodes,
            reserved_total: self.data.consumed.value_bytes,
        })
    }
    pub(super) fn publish_scalar(
        &mut self,
        ticket: ScalarAllocation,
        value: ir::Value,
        span: Option<&SourceSpan>,
    ) -> Result<Id> {
        // A ticket belongs to the uninterrupted private allocation phase, never
        // to an imported/cached certificate. No semantic work may interleave.
        if self.data.nodes.len() != ticket.index
            || self.data.consumed.value_nodes != ticket.nodes
            || self.data.consumed.value_bytes != ticket.reserved_total
        {
            return Err(self.invalid("Scalar allocation changed before publication", span));
        }
        self.data.consumed.value_nodes += 1;
        self.data.nodes.push(Node {
            ty: ticket.ty,
            data: Data::Scalar(value),
            bytes: ticket.bytes,
            depth: 1,
        });
        Ok(ticket.index)
    }
    pub(super) fn admitted_leaf_bytes(&self, ticket: &ScalarAllocation) -> usize {
        ticket.bytes
    }
    pub(super) fn ensure_leaf_node(&self, span: Option<&SourceSpan>) -> Result<()> {
        if self.data.consumed.value_nodes >= self.limits.max_value_nodes {
            return Err(self.resource("Runtime value node limit exceeded", span));
        }
        Ok(())
    }
    pub(super) fn scalar_header_bytes(
        &self,
        value: &ir::Value,
        span: Option<&SourceSpan>,
    ) -> Result<usize> {
        let header = ir::Value {
            ty: value.ty.clone(),
            payload: (),
        };
        super::preflight::serialized_size(&header, usize::MAX)
            .map_err(|e| self.invalid(&e, span))?
            .checked_sub(4)
            .ok_or_else(|| self.invalid("Invalid scalar payload header size", span))
    }
    pub(super) fn publish_json_leaf(
        &mut self,
        ticket: ScalarAllocation,
        data: Data,
        span: Option<&SourceSpan>,
    ) -> Result<Id> {
        if !matches!(data, Data::Datatype(_) | Data::Model(_)) {
            return Err(self.invalid("Expected JSON leaf publication", span));
        }
        if self.data.nodes.len() != ticket.index
            || self.data.consumed.value_nodes != ticket.nodes
            || self.data.consumed.value_bytes != ticket.reserved_total
        {
            return Err(self.invalid("JSON allocation changed before publication", span));
        }
        self.data.consumed.value_nodes += 1;
        self.data.nodes.push(Node {
            ty: ticket.ty,
            data,
            bytes: ticket.bytes,
            depth: 1,
        });
        Ok(ticket.index)
    }
    pub fn check_allocation(
        &self,
        bytes: usize,
        depth: usize,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        if self.data.consumed.value_nodes >= self.limits.max_value_nodes {
            return Err(self.resource("Runtime value node limit exceeded", span));
        }
        if depth > self.limits.max_value_depth || bytes > self.limits.scalar.values.max_value_bytes
        {
            return Err(self.resource("Runtime value depth or byte limit exceeded", span));
        }
        if self
            .data
            .consumed
            .value_bytes
            .checked_add(bytes)
            .is_none_or(|n| n > self.limits.scalar.max_value_bytes)
        {
            return Err(self.resource("Cumulative function value byte budget exceeded", span));
        }
        Ok(())
    }
    pub fn alloc(&mut self, ty: TypeId, data: Data, span: Option<&SourceSpan>) -> Result<Id> {
        if matches!(data, Data::View(_)) {
            return Err(self.invalid("Views require measured allocation", span));
        }
        if self.data.consumed.value_nodes >= self.limits.max_value_nodes {
            return Err(self.resource("Runtime value node limit exceeded", span));
        }
        if let Data::Scalar(value) = data {
            let ticket = self.admit_scalar_allocation(ty, &value, span)?;
            return self.publish_scalar(ticket, value, span);
        }
        let mut bytes = 32usize;
        let mut depth = 1usize;
        match &data {
            Data::Enum(value) => {
                bytes = super::preflight::serialized_size(value, usize::MAX)
                    .map_err(|e| self.invalid(&e, span))?
            }
            Data::Datatype(value) | Data::Model(value) => {
                bytes = super::preflight::serialized_size(value, usize::MAX)
                    .map_err(|e| self.invalid(&e, span))?
            }
            Data::Record(fields) => {
                for key in fields.keys() {
                    bytes = bytes
                        .checked_add(key.len())
                        .ok_or_else(|| self.resource("Value size overflow", span))?
                }
            }
            _ => {}
        }
        let mut child_size = |id: Id| -> Result<()> {
            let child = self.node(id)?;
            bytes = bytes
                .checked_add(child.bytes)
                .ok_or_else(|| self.resource("Value size overflow", span))?;
            depth = depth.max(child.depth.saturating_add(1));
            Ok(())
        };
        match &data {
            Data::Some(id) | Data::Ok(id) | Data::Err(id) => child_size(*id)?,
            Data::Tuple(items) | Data::Collection(items) => {
                for id in items {
                    child_size(*id)?;
                }
            }
            Data::Record(fields) => {
                for id in fields.values() {
                    child_size(*id)?;
                }
            }
            _ => {}
        }
        self.alloc_measured(ty, data, bytes, depth, span)
    }
    pub(super) fn alloc_measured(
        &mut self,
        ty: TypeId,
        data: Data,
        bytes: usize,
        depth: usize,
        span: Option<&SourceSpan>,
    ) -> Result<Id> {
        self.check_allocation(bytes, depth, span)?;
        self.data.consumed.value_bytes = self
            .data
            .consumed
            .value_bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.scalar.max_value_bytes)
            .ok_or_else(|| self.resource("Cumulative function value byte budget exceeded", span))?;
        self.data.consumed.value_nodes += 1;
        let id = self.data.nodes.len();
        self.data.nodes.push(Node {
            ty,
            data,
            bytes,
            depth,
        });
        Ok(id)
    }
    pub fn view(&mut self, id: Id, ty: TypeId, span: Option<&SourceSpan>) -> Result<Id> {
        self.permit(self.node(id)?.ty, ty, span)?;
        if self.node(id)?.ty == ty {
            self.transfer(id, span)?;
            return Ok(id);
        }
        self.check_allocation(self.node(id)?.bytes, self.node(id)?.depth, span)?;
        self.transfer(id, span)?;
        self.check_allocation(self.node(id)?.bytes, self.node(id)?.depth, span)?;
        let node = self.node(id)?;
        self.alloc_measured(ty, Data::View(node.origin), node.bytes, node.depth, span)
    }
    pub fn unary(&mut self, op: ir::UnaryOp, id: Id, ty: TypeId, span: &SourceSpan) -> Result<Id> {
        if self.image.is_scalar() {
            return self.scalar_unary(op, id, span);
        }
        self.transfer(id, Some(span))?;
        let origin = self.node(id)?.origin;
        let Some(Node {
            data: Data::Scalar(value),
            ..
        }) = self.data.nodes.get(origin)
        else {
            return Err(self.invalid("Unary operation requires scalar", Some(span)));
        };
        let control = self.control;
        let result = crate::rcore_behavior_scalar::unary_controlled(
            op,
            value,
            &mut self.data.numeric,
            Some(&|| control.is_some_and(|c| c.cancellation_requested())),
        )
        .map_err(|e| self.arithmetic(e, span))?;
        self.alloc(ty, Data::Scalar(result), Some(span))
    }
    pub fn binary(
        &mut self,
        op: ir::BinaryOp,
        a: Id,
        b: Id,
        ty: TypeId,
        span: &SourceSpan,
    ) -> Result<Id> {
        if self.image.is_scalar() {
            return self.scalar_binary(op, a, b, span);
        }
        if matches!(op, ir::BinaryOp::Equal | ir::BinaryOp::NotEqual) {
            let equal = self.equivalent(a, b, false, 0, Some(span))?;
            return self.alloc(
                ty,
                Data::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::Bool),
                    payload: serde_json::json!(equal == (op == ir::BinaryOp::Equal)),
                }),
                Some(span),
            );
        }
        self.transfer(a, Some(span))?;
        self.transfer(b, Some(span))?;
        // Resolve both original handles in their original order before any
        // type error. Fresh field borrows cannot outlive this kernel call.
        let a_origin = self.node(a)?.origin;
        let b_origin = self.node(b)?.origin;
        let (
            Some(Node {
                data: Data::Scalar(a),
                ..
            }),
            Some(Node {
                data: Data::Scalar(b),
                ..
            }),
        ) = (self.data.nodes.get(a_origin), self.data.nodes.get(b_origin))
        else {
            return Err(self.invalid("Arithmetic requires scalar values", Some(span)));
        };
        let control = self.control;
        let result = crate::rcore_behavior_scalar::binary_controlled(
            op,
            a,
            b,
            &mut self.data.numeric,
            Some(&|| control.is_some_and(|c| c.cancellation_requested())),
        )
        .map_err(|e| self.arithmetic(e, span))?;
        self.alloc(ty, Data::Scalar(result), Some(span))
    }
    pub(super) fn compare_charge(&mut self, span: Option<&SourceSpan>) -> Result<()> {
        self.data.consumed.collection_comparisons = self
            .data
            .consumed
            .collection_comparisons
            .checked_add(1)
            .filter(|n| *n <= self.limits.max_collection_comparisons)
            .ok_or_else(|| self.resource("Value comparison budget exceeded", span))?;
        Ok(())
    }
    pub fn equivalent(
        &mut self,
        a: Id,
        b: Id,
        total: bool,
        depth: usize,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        let mut comparison = super::equivalence::Equivalence::new(self, a, b, total, depth, span)?;
        loop {
            if let Some(result) = comparison.advance(self, span)? {
                return Ok(result);
            }
        }
    }
    pub(super) fn equivalent_leaf(
        &mut self,
        a: Id,
        b: Id,
        total: bool,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        let an = self.node(a)?;
        let aty = an.ty;
        let a_origin = an.origin;
        let bn = self.node(b)?;
        let bty = bn.ty;
        let b_origin = bn.origin;
        let an = self
            .data
            .nodes
            .get(a_origin)
            .ok_or_else(|| self.invalid("Invalid runtime value handle", None))?;
        let bn = self
            .data
            .nodes
            .get(b_origin)
            .ok_or_else(|| self.invalid("Invalid runtime value handle", None))?;
        Ok(match (&an.data, &bn.data) {
            (Data::Scalar(a), Data::Scalar(b)) => {
                if a.ty != b.ty {
                    false
                } else if total && scalar_nan(a) && scalar_nan(b) {
                    true
                } else {
                    let control = self.control;
                    let result = crate::rcore_behavior_scalar::binary_controlled(
                        ir::BinaryOp::Equal,
                        a,
                        b,
                        &mut self.data.numeric,
                        Some(&|| control.is_some_and(|c| c.cancellation_requested())),
                    )
                    .map_err(|e| match span {
                        Some(s) => self.arithmetic(e, s),
                        None => self.error(
                            if e.kind == ArithmeticErrorKind::Cancelled {
                                ExecutionErrorKind::Cancelled
                            } else if e.kind == ArithmeticErrorKind::ResourceLimit {
                                ExecutionErrorKind::ResourceExhausted
                            } else {
                                ExecutionErrorKind::Arithmetic
                            },
                            e.message,
                            None,
                        ),
                    })?;
                    result
                        .payload
                        .as_bool()
                        .ok_or_else(|| self.invalid("Equality returned non-Boolean", span))?
                }
            }
            (Data::Enum(a), Data::Enum(b)) => a == b,
            (Data::Datatype(a), Data::Datatype(b)) => aty == bty && a == b,
            (Data::Model(a), Data::Model(b)) => {
                crate::model_reference::reference_identity(a).map_err(|e| self.invalid(&e, span))?
                    == crate::model_reference::reference_identity(b)
                        .map_err(|e| self.invalid(&e, span))?
            }
            (Data::None, Data::None) => true,
            (Data::Closure(a), Data::Closure(b)) => {
                if !total {
                    return Err(self.error(
                        ExecutionErrorKind::Unsupported,
                        "Source equality for functions is unsupported",
                        span,
                    ));
                }
                a == b
            }
            _ => false,
        })
    }
    pub fn collection_info(&self, ty: TypeId) -> Result<(CollectionKind, TypeId, usize, i32)> {
        match self.type_node(ty)? {
            TypeNode::Collection { kind, element } => Ok((kind.clone(), *element, 0, -1)),
            TypeNode::Many {
                element,
                lower,
                upper,
                ordered,
                unique,
            } => Ok((
                match (*ordered, *unique) {
                    (true, true) => CollectionKind::OrderedSet,
                    (true, false) => CollectionKind::Sequence,
                    (false, true) => CollectionKind::Set,
                    (false, false) => CollectionKind::Bag,
                },
                *element,
                *lower,
                *upper,
            )),
            _ => Err(self.invalid("Expected collection type", None)),
        }
    }
    pub fn index(&mut self, id: Id, index: Id, ty: TypeId, span: &SourceSpan) -> Result<Id> {
        let Data::Scalar(index) = &self.node(index)?.data else {
            return Err(self.invalid("Index requires integer", Some(span)));
        };
        let n = match index.ty {
            ir::ValueType::Scalar(Scalar::Int64) => index
                .payload
                .get("value")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<i64>().ok()),
            _ => index.payload.as_i64(),
        }
        .ok_or_else(|| self.invalid("Invalid integer index", Some(span)))?;
        let Data::Collection(items) = &self.node(id)?.data else {
            return Err(self.invalid("Index requires collection", Some(span)));
        };
        let data = usize::try_from(n)
            .ok()
            .and_then(|n| items.get(n))
            .copied()
            .map(Data::Some)
            .unwrap_or(Data::None);
        self.alloc(ty, data, Some(span))
    }
}
fn scalar_nan(value: &ir::Value) -> bool {
    let bits = value
        .payload
        .get("bits")
        .and_then(|v| v.as_str())
        .and_then(|v| u64::from_str_radix(v, 16).ok());
    match (&value.ty, bits) {
        (ir::ValueType::Scalar(Scalar::Float32), Some(n)) => {
            u32::try_from(n).is_ok_and(|n| f32::from_bits(n).is_nan())
        }
        (ir::ValueType::Scalar(Scalar::Float64), Some(n)) => f64::from_bits(n).is_nan(),
        _ => false,
    }
}
