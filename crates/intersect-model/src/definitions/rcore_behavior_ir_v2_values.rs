//! Shape inspection is not a certificate of nominal membership or uniqueness.
use super::*;

pub struct ValueBudget {
    pub remaining_nodes: usize,
    pub remaining_bytes: usize,
    pub max_depth: usize,
    pub scalar: ir::ValueLimits,
}
pub trait ValueVisitor {
    fn nominal_leaf(&mut self, ty: &TypeNode, payload: &serde_json::Value) -> Result<(), IrError>;
    fn unique_collection(&mut self, ty: TypeId, items: &[ValueWire]) -> Result<(), IrError>;
}
impl ValueBudget {
    fn bytes(&mut self, size: usize) -> Result<(), IrError> {
        spend(&mut self.remaining_bytes, size)
    }
    fn children(&self, count: usize, pending: usize) -> Result<(), IrError> {
        if count > self.remaining_nodes.saturating_sub(pending) {
            return Err(IrError {
                kind: IrErrorKind::ResourceExhausted,
                message: "Runtime value node limit exceeded before child allocation".into(),
                span: None,
            });
        }
        Ok(())
    }
    pub(crate) fn json(&mut self, value: &serde_json::Value) -> Result<(), IrError> {
        let mut cursor = crate::definitions::retained_json::ResourceCursor::new(value);
        while !advance_resource(&mut cursor, self)? {}
        Ok(())
    }
}

/// A resumable shape-only inspection. Supply the same immutable value and program
/// on every advance. Completion is not nominal membership or uniqueness authority;
/// visitors still own those checks. An error makes this cursor terminal.
struct ShapeCursor<R> {
    frames: Vec<ShapeFrame<R>>,
    path: Vec<ShapeRoute>,
    pending: usize,
    storage: usize,
    storage_limit: usize,
    failed: bool,
}
enum ShapeRoute {
    Wrapped,
    Index(usize),
    Field(String),
}
enum ShapeFrame<R> {
    Resource(R),
    Visit(TypeId),
    Leave,
    RecordKeys {
        last: Option<String>,
    },
    Children {
        index: usize,
        last: Option<String>,
        unique: bool,
    },
}
impl<R> ShapeCursor<R> {
    pub fn new(
        expected: TypeId,
        budget: &mut ValueBudget,
        storage_limit: usize,
    ) -> Result<Self, IrError> {
        let mut cursor = Self {
            frames: Vec::new(),
            path: Vec::new(),
            pending: 1,
            storage: 0,
            storage_limit,
            failed: false,
        };
        cursor.push(ShapeFrame::Visit(expected), budget)?;
        Ok(cursor)
    }
    /// Logical retained storage charged before cursor allocations.
    pub fn storage_bytes(&self) -> usize {
        self.storage
    }
    fn storage(&mut self, bytes: usize) -> Result<(), IrError> {
        self.storage = self
            .storage
            .checked_add(bytes)
            .filter(|n| *n <= self.storage_limit)
            .ok_or_else(|| IrError {
                kind: IrErrorKind::ResourceExhausted,
                message: "Shape cursor storage limit exceeded".into(),
                span: None,
            })?;
        Ok(())
    }
    fn push(&mut self, frame: ShapeFrame<R>, budget: &mut ValueBudget) -> Result<(), IrError> {
        if self.frames.len() >= budget.max_depth.saturating_add(2).saturating_mul(3) {
            return Err(IrError {
                kind: IrErrorKind::ResourceExhausted,
                message: "Shape cursor frame limit exceeded".into(),
                span: None,
            });
        }
        if self.frames.len() == self.frames.capacity() {
            self.storage(128)?;
            self.frames.try_reserve_exact(1).map_err(|_| IrError {
                kind: IrErrorKind::ResourceExhausted,
                message: "Shape cursor allocation failed".into(),
                span: None,
            })?;
        }
        self.frames.push(frame);
        Ok(())
    }
    fn descend(
        &mut self,
        route: ShapeRoute,
        expected: TypeId,
        budget: &mut ValueBudget,
    ) -> Result<(), IrError> {
        if self.path.len() == self.path.capacity() {
            self.storage(64)?;
            self.path.try_reserve_exact(1).map_err(|_| IrError {
                kind: IrErrorKind::ResourceExhausted,
                message: "Shape path allocation failed".into(),
                span: None,
            })?;
        }
        self.path.push(route);
        self.push(ShapeFrame::Leave, budget)?;
        self.push(ShapeFrame::Visit(expected), budget)
    }
    fn value<'a>(&self, root: &'a ValueWire) -> Result<&'a ValueWire, IrError> {
        let mut value = root;
        for route in &self.path {
            value = match (route, &value.data) {
                (ShapeRoute::Wrapped, DataWire::Some(v) | DataWire::Ok(v) | DataWire::Err(v)) => v,
                (ShapeRoute::Index(i), DataWire::Tuple(values) | DataWire::Collection(values)) => {
                    values
                        .get(*i)
                        .ok_or_else(|| invalid("Shape cursor item unavailable", None))?
                }
                (ShapeRoute::Field(key), DataWire::Record(values)) => values
                    .get(key)
                    .ok_or_else(|| invalid("Shape cursor field unavailable", None))?,
                _ => return Err(invalid("Shape cursor value changed", None)),
            };
        }
        Ok(value)
    }
    fn advance<'a, M: ResourceMode<'a, Task = R>>(
        &mut self,
        program: &VerifiedExecutable,
        root: &'a ValueWire,
        budget: &mut ValueBudget,
        visitor: &mut impl ValueVisitor,
        mode: &mut M,
    ) -> Result<bool, IrError>
    where
        R: ResourceTask<'a>,
    {
        if self.failed {
            return Err(invalid("Shape cursor already failed", None));
        }
        self.failed = true;
        let result = self.advance_inner(program, root, budget, visitor, mode);
        if result.is_ok() {
            self.failed = false;
        }
        result
    }
    fn advance_inner<'a, M: ResourceMode<'a, Task = R>>(
        &mut self,
        program: &VerifiedExecutable,
        root: &'a ValueWire,
        budget: &mut ValueBudget,
        visitor: &mut impl ValueVisitor,
        mode: &mut M,
    ) -> Result<bool, IrError>
    where
        R: ResourceTask<'a>,
    {
        use std::ops::Bound::{Excluded, Unbounded};
        let Some(frame) = self.frames.pop() else {
            return Ok(true);
        };
        let value = self.value(root)?;
        match frame {
            ShapeFrame::Resource(mut resource) => {
                if !resource.advance(program, value, budget, visitor)? {
                    self.push(ShapeFrame::Resource(resource), budget)?;
                }
            }
            ShapeFrame::Leave => {
                self.path.pop();
            }
            ShapeFrame::RecordKeys { last } => {
                let (TypeNode::Record(types), DataWire::Record(values)) =
                    (program.type_node(value.ty)?, &value.data)
                else {
                    return Err(invalid("Shape cursor record changed", None));
                };
                let next = match &last {
                    Some(last) => values
                        .range::<str, _>((Excluded(last.as_str()), Unbounded))
                        .next(),
                    None => values.first_key_value(),
                };
                if let Some((key, _)) = next {
                    if !types.contains_key(key) {
                        return Err(invalid(
                            "Runtime value shape differs from declared type",
                            None,
                        ));
                    }
                    budget.bytes(key.len())?;
                    self.storage(key.len())?;
                    self.push(
                        ShapeFrame::RecordKeys {
                            last: Some(key.clone()),
                        },
                        budget,
                    )?;
                } else {
                    self.push(
                        ShapeFrame::Children {
                            index: 0,
                            last: None,
                            unique: false,
                        },
                        budget,
                    )?;
                }
            }
            ShapeFrame::Children {
                index,
                last,
                unique,
            } => {
                let next = match (program.type_node(value.ty)?, &value.data) {
                    (TypeNode::Tuple(types), DataWire::Tuple(values)) => values
                        .get(index)
                        .map(|_| {
                            types
                                .get(index)
                                .copied()
                                .map(|ty| (ShapeRoute::Index(index), ty, None))
                                .ok_or_else(|| invalid("Shape cursor tuple changed", None))
                        })
                        .transpose()?,
                    (
                        TypeNode::Collection { element, .. } | TypeNode::Many { element, .. },
                        DataWire::Collection(values),
                    ) => values
                        .get(index)
                        .map(|_| (ShapeRoute::Index(index), *element, None)),
                    (TypeNode::Record(types), DataWire::Record(values)) => {
                        let next = match &last {
                            Some(last) => values
                                .range::<str, _>((Excluded(last.as_str()), Unbounded))
                                .next(),
                            None => values.first_key_value(),
                        };
                        if let Some((key, _)) = next {
                            self.storage(key.len().saturating_mul(2))?;
                            Some((
                                ShapeRoute::Field(key.clone()),
                                *types
                                    .get(key)
                                    .ok_or_else(|| invalid("Missing record type", None))?,
                                Some(key.clone()),
                            ))
                        } else {
                            None
                        }
                    }
                    _ => return Err(invalid("Shape cursor aggregate changed", None)),
                };
                if let Some((route, ty, last)) = next {
                    self.push(
                        ShapeFrame::Children {
                            index: index + 1,
                            last,
                            unique,
                        },
                        budget,
                    )?;
                    self.descend(route, ty, budget)?;
                } else if unique {
                    self.pending = self.pending.saturating_sub(1);
                    let DataWire::Collection(values) = &value.data else {
                        return Err(invalid("Missing unique collection", None));
                    };
                    visitor.unique_collection(value.ty, values)?;
                }
            }
            ShapeFrame::Visit(expected) => {
                self.pending = self.pending.saturating_sub(1);
                spend(&mut budget.remaining_nodes, 1)?;
                budget.bytes(8)?;
                if self.path.len() > budget.max_depth {
                    return Err(IrError {
                        kind: IrErrorKind::ResourceExhausted,
                        message: "Runtime value depth exceeded".into(),
                        span: None,
                    });
                }
                if !program.permits(value.ty, expected)? {
                    return Err(invalid("Runtime value has incompatible static type", None));
                }
                let node = program.type_node(value.ty)?;
                match (node, &value.data) {
                    (TypeNode::Scalar(_), DataWire::Scalar(v)) => {
                        if let Some(resource) = mode.start(&v.payload, budget)? {
                            self.push(ShapeFrame::Resource(resource), budget)?;
                        } else {
                            finish_resource(program, value, budget, visitor)?;
                        }
                    }
                    (TypeNode::Enum(_), DataWire::Enum(v)) => {
                        let crate::definitions::enums::EnumValue::Enum {
                            enumeration,
                            literal,
                        } = v;
                        for id in [enumeration, literal] {
                            budget.bytes(id.source.uri.len())?;
                            budget.bytes(id.source.sha256.len())?;
                            budget.bytes(id.path.len().saturating_mul(8))?;
                        }
                        program.validate_enum(value.ty, v)?;
                        let payload =
                            serde_json::to_value(v).map_err(|e| invalid(e.to_string(), None))?;
                        budget.json(&payload)?;
                    }
                    (
                        TypeNode::Datatype(_) | TypeNode::AppliedDatatype(_),
                        DataWire::Datatype(v),
                    )
                    | (TypeNode::Model(_), DataWire::Model(v)) => {
                        if let Some(resource) = mode.start(v, budget)? {
                            self.push(ShapeFrame::Resource(resource), budget)?;
                        } else {
                            visitor.nominal_leaf(node, v)?;
                        }
                    }

                    (TypeNode::Option(_), DataWire::None) => {}
                    (TypeNode::Option(child), DataWire::Some(_))
                    | (TypeNode::Result { ok: child, .. }, DataWire::Ok(_))
                    | (TypeNode::Result { error: child, .. }, DataWire::Err(_)) => {
                        self.pending += 1;
                        self.descend(ShapeRoute::Wrapped, *child, budget)?;
                    }
                    (TypeNode::Tuple(types), DataWire::Tuple(values))
                        if types.len() == values.len() =>
                    {
                        budget.children(values.len(), self.pending)?;
                        budget.bytes(values.len())?;
                        self.pending += values.len();
                        self.push(
                            ShapeFrame::Children {
                                index: 0,
                                last: None,
                                unique: false,
                            },
                            budget,
                        )?;
                    }
                    (TypeNode::Record(types), DataWire::Record(values))
                        if types.len() == values.len() =>
                    {
                        budget.children(values.len(), self.pending)?;
                        budget.bytes(values.len())?;
                        self.pending += values.len();
                        self.push(ShapeFrame::RecordKeys { last: None }, budget)?;
                    }
                    (TypeNode::Collection { kind, .. }, DataWire::Collection(values)) => {
                        budget.children(values.len(), self.pending)?;
                        budget.bytes(values.len())?;
                        let unique =
                            matches!(kind, CollectionKind::Set | CollectionKind::OrderedSet);
                        self.pending += values.len() + usize::from(unique);
                        self.push(
                            ShapeFrame::Children {
                                index: 0,
                                last: None,
                                unique,
                            },
                            budget,
                        )?;
                    }
                    (
                        TypeNode::Many {
                            lower,
                            upper,
                            unique,
                            ..
                        },
                        DataWire::Collection(values),
                    ) => {
                        budget.children(values.len(), self.pending)?;
                        if values.len() < *lower || (*upper >= 0 && values.len() > *upper as usize)
                        {
                            return Err(invalid("Runtime multiplicity outside bounds", None));
                        }
                        budget.bytes(values.len())?;
                        self.pending += values.len() + usize::from(*unique);
                        self.push(
                            ShapeFrame::Children {
                                index: 0,
                                last: None,
                                unique: *unique,
                            },
                            budget,
                        )?;
                    }
                    _ => {
                        return Err(invalid(
                            "Runtime value shape differs from declared type",
                            None,
                        ));
                    }
                }
            }
        }
        Ok(self.frames.is_empty())
    }
}

// Both public adapters use this one traversal and the same resource policy.
// The synchronous adapter drains only the shared JSON frontier. The retained
// adapter borrows its exact input for the complete cursor lifetime.
pub struct ValueShapeCursor(ShapeCursor<DrainedResource>);
pub(crate) struct RetainedValueShapeCursor<'a>(ShapeCursor<RetainedResource<'a>>);
impl ValueShapeCursor {
    pub fn new(
        expected: TypeId,
        budget: &mut ValueBudget,
        storage_limit: usize,
    ) -> Result<Self, IrError> {
        ShapeCursor::new(expected, budget, storage_limit).map(Self)
    }
    pub fn storage_bytes(&self) -> usize {
        self.0.storage_bytes()
    }
    pub fn advance(
        &mut self,
        program: &VerifiedExecutable,
        root: &ValueWire,
        budget: &mut ValueBudget,
        visitor: &mut impl ValueVisitor,
    ) -> Result<bool, IrError> {
        self.0
            .advance(program, root, budget, visitor, &mut DrainResource)
    }
}
impl<'a> RetainedValueShapeCursor<'a> {
    pub fn new(
        expected: TypeId,
        budget: &mut ValueBudget,
        storage_limit: usize,
    ) -> Result<Self, IrError> {
        ShapeCursor::new(expected, budget, storage_limit).map(Self)
    }
    pub fn storage_bytes(&self) -> usize {
        self.0.storage_bytes()
    }
    #[cfg(feature = "allocation-profile")]
    pub fn layouts() -> (usize, usize) {
        (
            std::mem::size_of::<Self>(),
            std::mem::size_of::<ShapeFrame<RetainedResource<'a>>>(),
        )
    }
    #[cfg(feature = "allocation-profile")]
    pub(crate) fn scalar_layouts() -> [usize; 3] {
        [
            ir::scalar_validation::ScalarValidation::ROOT_BYTES,
            std::mem::size_of::<RetainedResource<'a>>(),
            std::mem::size_of::<ShapeFrame<RetainedResource<'a>>>(),
        ]
    }
    #[cfg(feature = "allocation-profile")]
    pub(crate) fn scalar_progress(&self) -> Option<(&'static str, usize, usize)> {
        match self.0.frames.last() {
            Some(ShapeFrame::Resource(RetainedResource::Scalar(cursor))) => cursor.progress(),
            _ => None,
        }
    }
    pub(crate) fn continues_visit(&self) -> bool {
        matches!(self.0.frames.last(), Some(ShapeFrame::Resource(_)))
    }
    #[cfg(feature = "allocation-profile")]
    pub fn resource_frontier(&self) -> Option<(usize, usize)> {
        match self.0.frames.last() {
            Some(ShapeFrame::Resource(RetainedResource::Json(cursor))) => {
                Some(cursor.retained_slots())
            }
            _ => None,
        }
    }
    pub fn advance(
        &mut self,
        program: &VerifiedExecutable,
        root: &'a ValueWire,
        budget: &mut ValueBudget,
        visitor: &mut impl ValueVisitor,
    ) -> Result<bool, IrError> {
        self.0
            .advance(program, root, budget, visitor, &mut RetainResource)
    }
}
trait ResourceTask<'a> {
    fn advance(
        &mut self,
        program: &VerifiedExecutable,
        value: &'a ValueWire,
        budget: &mut ValueBudget,
        visitor: &mut impl ValueVisitor,
    ) -> Result<bool, IrError>;
}

enum DrainedResource {}
impl<'a> ResourceTask<'a> for DrainedResource {
    fn advance(
        &mut self,
        _: &VerifiedExecutable,
        _: &'a ValueWire,
        _: &mut ValueBudget,
        _: &mut impl ValueVisitor,
    ) -> Result<bool, IrError> {
        match *self {}
    }
}
enum RetainedResource<'a> {
    Json(crate::definitions::retained_json::ResourceCursor<'a>),
    Scalar(Box<ir::scalar_validation::ScalarValidation<'a>>),
    Failed,
}
impl<'a> ResourceTask<'a> for RetainedResource<'a> {
    fn advance(
        &mut self,
        program: &VerifiedExecutable,
        value: &'a ValueWire,
        budget: &mut ValueBudget,
        visitor: &mut impl ValueVisitor,
    ) -> Result<bool, IrError> {
        match std::mem::replace(self, Self::Failed) {
            Self::Json(mut cursor) => {
                if !advance_resource(&mut cursor, budget)? {
                    *self = Self::Json(cursor);
                    return Ok(false);
                }
                if let Some(scalar) = scalar_leaf(program, value)? {
                    // The fixed complete Session holder proof covers one scalar
                    // root OR one general copy owner, never both. No payload or
                    // logical fee is admitted by this layout check.
                    if ir::scalar_validation::ScalarValidation::ROOT_BYTES
                        > crate::definitions::retained_json_frontier::CopyJob::OWNER_BYTES
                    {
                        return Err(invalid(
                            "Scalar validation root exceeds fixed holder allowance",
                            None,
                        ));
                    }
                    let root = ir::scalar_validation::ScalarValidation::from_completed(
                        scalar,
                        &budget.scalar,
                        cursor,
                    )
                    .map_err(|e| invalid(e, None))?;
                    *self = Self::Scalar(Box::new(root));
                    Ok(false)
                } else {
                    finish_resource(program, value, budget, visitor)?;
                    Ok(true)
                }
            }
            Self::Scalar(mut cursor) => {
                if cursor.advance(&budget.scalar).map_err(|e| e.into_ir())? {
                    Ok(true)
                } else {
                    *self = Self::Scalar(cursor);
                    Ok(false)
                }
            }
            Self::Failed => Err(invalid("Resource validation already failed", None)),
        }
    }
}
trait ResourceMode<'a> {
    type Task: ResourceTask<'a>;
    fn start(
        &mut self,
        value: &'a serde_json::Value,
        budget: &mut ValueBudget,
    ) -> Result<Option<Self::Task>, IrError>;
}
struct DrainResource;
impl<'a> ResourceMode<'a> for DrainResource {
    type Task = DrainedResource;
    fn start(
        &mut self,
        value: &'a serde_json::Value,
        budget: &mut ValueBudget,
    ) -> Result<Option<Self::Task>, IrError> {
        budget.json(value)?;
        Ok(None)
    }
}
struct RetainResource;
impl<'a> ResourceMode<'a> for RetainResource {
    type Task = RetainedResource<'a>;
    fn start(
        &mut self,
        value: &'a serde_json::Value,
        _: &mut ValueBudget,
    ) -> Result<Option<Self::Task>, IrError> {
        Ok(Some(RetainedResource::Json(
            crate::definitions::retained_json::ResourceCursor::new(value),
        )))
    }
}
fn scalar_leaf<'a>(
    program: &VerifiedExecutable,
    value: &'a ValueWire,
) -> Result<Option<&'a ir::Value>, IrError> {
    if let (TypeNode::Scalar(s), DataWire::Scalar(v)) = (program.type_node(value.ty)?, &value.data)
    {
        if v.ty != ir::ValueType::Scalar(*s) {
            return Err(invalid("Scalar payload type differs from value type", None));
        }
        Ok(Some(v))
    } else {
        Ok(None)
    }
}
fn finish_resource(
    program: &VerifiedExecutable,
    value: &ValueWire,
    budget: &mut ValueBudget,
    visitor: &mut impl ValueVisitor,
) -> Result<(), IrError> {
    if let Some(value) = scalar_leaf(program, value)? {
        let mut cursor = ir::scalar_validation::ScalarValidation::full(value, &budget.scalar);
        while !cursor.advance(&budget.scalar).map_err(|e| e.into_ir())? {}
        return Ok(());
    }
    match (program.type_node(value.ty)?, &value.data) {
        (
            node @ (TypeNode::Datatype(_) | TypeNode::AppliedDatatype(_) | TypeNode::Model(_)),
            DataWire::Datatype(payload) | DataWire::Model(payload),
        ) => visitor.nominal_leaf(node, payload),
        _ => Err(invalid("Shape JSON continuation changed", None)),
    }
}
impl crate::definitions::retained_json::Inspection for ValueBudget {
    fn node(
        &mut self,
        value: &serde_json::Value,
        depth: usize,
        pending: usize,
    ) -> Result<(), String> {
        let check = (|| {
            spend(&mut self.remaining_nodes, 1)?;
            if depth > self.max_depth {
                return Err(IrError {
                    kind: IrErrorKind::ResourceExhausted,
                    message: "Runtime payload depth exceeded".into(),
                    span: None,
                });
            }
            match value {
                serde_json::Value::String(text) => self.bytes(text.len().saturating_add(2)),
                serde_json::Value::Array(values) => {
                    self.children(values.len(), pending)?;
                    self.bytes(values.len().saturating_add(2))
                }
                serde_json::Value::Object(values) => {
                    self.children(values.len(), pending)?;
                    self.bytes(values.len().saturating_add(2))
                }
                _ => self.bytes(32),
            }
        })();
        check.map_err(|error| error.message)
    }
    fn key(&mut self, key: &str) -> Result<(), String> {
        self.bytes(key.len().saturating_add(3))
            .map_err(|error| error.message)
    }
}
struct FrontierAdmission {
    max_slots: usize,
}
impl crate::definitions::retained_json::Admission for FrontierAdmission {
    fn allocate(
        &mut self,
        request: crate::definitions::retained_json::Allocation,
    ) -> Result<(), String> {
        match request {
            crate::definitions::retained_json::Allocation::Frames {
                new_slots,
                element_bytes,
                ..
            } if new_slots <= self.max_slots
                && element_bytes == std::mem::size_of::<(&serde_json::Value, usize)>() =>
            {
                Ok(())
            }
            _ => Err("JSON frontier exceeds original node admission".into()),
        }
    }
}
fn advance_resource(
    cursor: &mut crate::definitions::retained_json::ResourceCursor<'_>,
    budget: &mut ValueBudget,
) -> Result<bool, IrError> {
    let mut admission = FrontierAdmission {
        max_slots: budget
            .remaining_nodes
            .saturating_add(cursor.retained_slots().0),
    };
    cursor
        .advance(&mut admission, budget)
        .map_err(|message| IrError {
            kind: IrErrorKind::ResourceExhausted,
            message,
            span: None,
        })
}
const _: () = assert!(std::mem::size_of::<ShapeFrame<RetainedResource<'static>>>() <= 128);

/// Shared bounded shape walker. Synchronous callers drive the same cursor used
/// by suspended preparation. Completion alone is not semantic validation.
pub fn inspect_value_shape(
    program: &VerifiedExecutable,
    value: &ValueWire,
    expected: TypeId,
    budget: &mut ValueBudget,
    visitor: &mut impl ValueVisitor,
) -> Result<(), IrError> {
    // The wire-byte budget and temporary cursor storage are distinct limits.
    // The synchronous adapter bounds storage by admitted key bytes and depth.
    let storage_limit = budget
        .remaining_bytes
        .saturating_mul(4)
        .saturating_add(budget.max_depth.saturating_add(2).saturating_mul(512));
    let mut cursor = ValueShapeCursor::new(expected, budget, storage_limit)?;
    while !cursor.advance(program, value, budget, visitor)? {}
    Ok(())
}

const _: () = assert!(
    ir::scalar_validation::ScalarValidation::ROOT_BYTES
        <= crate::definitions::retained_json_frontier::CopyJob::OWNER_BYTES
);
