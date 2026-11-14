//! Direct executor for the side-effect-free subset of verified Rcore IR,
//! reading a frozen model. It runs the same verified program as the
//! resumable VM, with model names compiled to dense IDs and typed values.
//!
//! The VM stays the authority. A program outside the subset is refused when it
//! is prepared; at run time anything the executor cannot decide with
//! certainty (a missing value, a multiplicity or uniqueness question, deep
//! recursion, an exhausted step budget) returns `Fallback`, and the caller runs
//! the VM instead. The executor never produces an error of its own for a
//! program the VM would accept.
use crate::{
    core::frozen::{ClassId, FeatureId, LinkError, LinkValue, LinkView, Metamodel, ObjId},
    definitions::{
        rcore_behavior::{
            ir::{BinaryOp, UnaryOp},
            ir_v2::{
                AlgebraOp, Block, CollectionOp, DataWire, Function, InstructionKind, MembershipOp,
                Operand, OwnershipOp, Terminator, TextOp, TypeId, TypeNode, ValueWire,
                VerifiedExecutable,
            },
            types::{CollectionKind, Scalar},
        },
        schema::Schema,
    },
};
use serde_json::Value;
use std::rc::Rc;

/// Why a run did not produce a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FastError {
    /// Run the resumable VM instead; it decides this case.
    Fallback(&'static str),
    /// A model read depends on a reference that cannot be resolved here.
    Pending,
}

/// Model reads the executor needs. Implemented for the frozen link view.
pub trait FastRead {
    fn metamodel(&self) -> &Metamodel;
    fn class(&self, obj: ObjId) -> Option<ClassId>;
    fn value(&self, obj: ObjId, feature: FeatureId) -> Result<Option<LinkValue<'_>>, LinkError>;
    fn item(
        &self,
        obj: ObjId,
        feature: FeatureId,
        index: usize,
    ) -> Result<Option<LinkValue<'_>>, LinkError>;
    fn container(&self, obj: ObjId) -> Option<ObjId>;
    fn children(&self, obj: ObjId) -> &[ObjId];
    fn roots(&self) -> &[ObjId];
    fn object_count(&self) -> usize;
    fn identity(&self, obj: ObjId) -> Option<&str>;
    fn resource(&self, obj: ObjId) -> Option<&str>;
}
impl FastRead for LinkView<'_> {
    fn metamodel(&self) -> &Metamodel {
        LinkView::metamodel(self)
    }
    fn class(&self, obj: ObjId) -> Option<ClassId> {
        LinkView::class(self, obj)
    }
    fn value(&self, obj: ObjId, feature: FeatureId) -> Result<Option<LinkValue<'_>>, LinkError> {
        LinkView::value(self, obj, feature)
    }
    fn item(
        &self,
        obj: ObjId,
        feature: FeatureId,
        index: usize,
    ) -> Result<Option<LinkValue<'_>>, LinkError> {
        LinkView::item(self, obj, feature, index)
    }
    fn container(&self, obj: ObjId) -> Option<ObjId> {
        LinkView::container(self, obj).map(|(parent, _)| parent)
    }
    fn children(&self, obj: ObjId) -> &[ObjId] {
        LinkView::children(self, obj)
    }
    fn roots(&self) -> &[ObjId] {
        LinkView::roots(self)
    }
    fn object_count(&self) -> usize {
        self.len()
    }
    fn identity(&self, obj: ObjId) -> Option<&str> {
        LinkView::identity(self, obj)
    }
    fn resource(&self, obj: ObjId) -> Option<&str> {
        LinkView::resource(self, obj)
    }
}

/// Runtime value. Collections share their storage; their kind is static.
#[derive(Clone, Debug)]
pub enum V {
    Bool(bool),
    Int(i64),
    Str(Rc<str>),
    Obj(ObjId),
    /// An enumeration literal in its canonical stored form (`EnumValue` JSON).
    Enum(Rc<Value>),
    /// A model object `new` describes; the engine adds it to the model.
    New(Rc<NewObject>),
    None,
    Some(Rc<V>),
    Seq(Rc<Vec<V>>),
    Fun(Rc<Closure>),
    /// A record value, its fields in name order (as the IR's record type).
    Record(Rc<[(Rc<str>, V)]>),
}
impl V {
    /// The field `name` of a record value.
    pub fn field(&self, name: &str) -> Option<&V> {
        let V::Record(fields) = self else {
            return None;
        };
        fields
            .iter()
            .find(|(field, _)| field.as_ref() == name)
            .map(|(_, value)| value)
    }
}
/// The class and the stored feature values of an object `new` describes. Values
/// are scalars, enumerations, references (`V::Obj`), nested `V::New` objects,
/// options and collections of these.
#[derive(Debug)]
pub struct NewObject {
    pub class: ClassId,
    pub values: Vec<(FeatureId, V)>,
}
#[derive(Debug)]
pub struct Closure {
    function: usize,
    captures: Rc<[V]>,
}

#[derive(Clone, Copy, Debug)]
enum Op {
    P(u32),
    R(u32),
    C(u32),
}
#[derive(Clone, Copy, Debug)]
enum Elem {
    Str,
    Bool,
    Int,
    /// `Int32`, held as a `V::Int` within the `i32` range.
    Int32,
    Obj(ClassId),
    Enum,
    /// Only produced by Option/collection nesting; reads reject it.
    Other,
}
/// The width of a signed integer type: every value and arithmetic result
/// stays within it, as the VM's checked integer results do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Width {
    I32,
    I64,
}
impl Width {
    fn of(elem: Elem) -> Option<Self> {
        match elem {
            Elem::Int => Some(Self::I64),
            Elem::Int32 => Some(Self::I32),
            _ => None,
        }
    }
    /// `value` when it is in range; `None` otherwise.
    fn fit(self, value: i64) -> Option<i64> {
        match self {
            Self::I64 => Some(value),
            Self::I32 => i32::try_from(value).ok().map(i64::from),
        }
    }
}
#[derive(Clone, Copy, Debug)]
struct Shape {
    unique: bool,
    ordered: bool,
    lower: usize,
    upper: Option<usize>,
}
#[derive(Clone, Copy, Debug)]
enum Read {
    Optional(Elem),
    Required(Elem),
    Many(Elem, Shape),
}
#[derive(Debug)]
enum Ins {
    Const(V),
    Not(Op),
    Neg(Op, Width),
    Eq(Op, Op, bool),
    Int(BinaryOp, Op, Op, Width),
    Closure(usize, Vec<Op>),
    Call(Op, Vec<Op>),
    None,
    Some(Op),
    View(Op),
    Collection(Vec<Op>, Shape),
    Convert(Op, Shape),
    Each(CollectionOp, Op, Op, Shape, bool),
    Flatten(Op, Shape),
    Size(Op),
    IsEmpty(Op),
    Split(Op, Op),
    Text(TextOp, Op),
    /// String `+`.
    StrAdd(Op, Op),
    Index(Op, Op),
    Contains(Op, Op, bool),
    Concat(Op, Op, Shape),
    /// First occurrences, in order, as the VM's `distinct`.
    Distinct(Op, Shape),
    Owner(OwnershipOp, Op, ClassId),
    Stored(Op, FeatureId, Read),
    New(ClassId, Vec<(FeatureId, Op)>),
    /// A record literal, fields in name order.
    Record(Vec<(Rc<str>, Op)>),
    /// A record field read.
    Field(Op, Rc<str>),
}
#[derive(Debug)]
struct Edge(usize, Vec<Op>);
#[derive(Debug)]
enum Term {
    Return(Op),
    Jump(Edge),
    Branch(Op, Edge, Edge),
    Option(Op, Edge, Edge),
}
#[derive(Debug)]
struct FBlock {
    instructions: Vec<Ins>,
    terminator: Term,
}
#[derive(Debug)]
struct FFunction {
    blocks: Vec<FBlock>,
}

/// Resource bounds of one direct run. Each nested call uses native stack, so
/// `call_depth` bounds the stack a run needs as well as its recursion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FastLimits {
    pub steps: usize,
    pub call_depth: usize,
}

/// A verified program compiled against one frozen metamodel.
#[derive(Debug)]
pub struct FastProgram {
    functions: Vec<FFunction>,
    entry: usize,
}

impl FastProgram {
    /// Compile `image` for `metamodel`, reusing the class of each model type that `classes` already
    /// looked up for the same `metamodel` and `schema` (one schema lookup per
    /// distinct type for many programs, as `RcoreScopes::new` prepares them).
    pub fn prepare_with(
        image: &VerifiedExecutable,
        metamodel: &Metamodel,
        schema: &Schema,
        classes: &FastClasses,
    ) -> Result<Self, &'static str> {
        let program = image.program();
        let cx = Prepare {
            image,
            metamodel,
            schema,
            classes,
        };
        let functions = program
            .functions
            .iter()
            .map(|f| cx.function(f))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            functions,
            entry: program.entry.0 as usize,
        })
    }

    /// Run the entry function. Each executed instruction, callback and item
    /// read is one step; a run that needs more than `limits.steps` steps or
    /// more than `limits.call_depth` nested calls defers to the VM.
    pub fn run(
        &self,
        read: &dyn FastRead,
        arguments: Vec<V>,
        limits: FastLimits,
    ) -> Result<V, FastError> {
        self.run_memo(read, arguments, limits, &FastMemo::default())
    }
    /// `run`, keeping the results of pure calls in `memo` so that later runs
    /// over the same unchanging reads reuse them. A memoised result costs no
    /// steps. The caller owns the memo's lifetime: it must not outlive the
    /// model state the results were read from (see `LinkView::phase_value`).
    pub fn run_memo(
        &self,
        read: &dyn FastRead,
        arguments: Vec<V>,
        limits: FastLimits,
        memo: &FastMemo,
    ) -> Result<V, FastError> {
        let mut exec = Exec {
            program: self,
            read,
            steps: 0,
            max_steps: limits.steps,
            depth: 0,
            max_depth: limits.call_depth,
            memo,
        };
        exec.call(self.entry, arguments, Rc::from(Vec::new()))
    }
}

/// The metamodel class of each model type looked up while preparing
/// programs: a schema class-key lookup validates the type and searches the
/// prepared classes, so programs prepared together share the answers. Valid
/// only for the one metamodel and schema it was filled with.
#[derive(Debug, Default)]
pub struct FastClasses {
    classes: std::cell::RefCell<
        std::collections::BTreeMap<
            crate::definitions::generic_types::Type,
            Result<ClassId, &'static str>,
        >,
    >,
}
impl FastClasses {
    /// The class of `runtime` in `metamodel`, looked up once per type.
    pub fn class(
        &self,
        runtime: &impl AsRef<crate::definitions::generic_types::Type>,
        metamodel: &Metamodel,
        schema: &Schema,
    ) -> Result<ClassId, &'static str> {
        let ty = runtime.as_ref();
        if let Some(found) = self.classes.borrow().get(ty) {
            return *found;
        }
        // The metamodel's own schema was validated when it was frozen; any
        // other schema is validated by its lookup, as before.
        let key = if std::ptr::eq(schema, metamodel.schema()) {
            metamodel.class_key_for_type(runtime)
        } else {
            schema.class_key_for_type(runtime)
        };
        let found = key
            .map_err(|_| "model type has no class key")
            .and_then(|key| {
                metamodel
                    .class(key)
                    .ok_or("model type is not in the metamodel")
            });
        self.classes.borrow_mut().insert(ty.clone(), found);
        found
    }
}

struct Prepare<'a> {
    image: &'a VerifiedExecutable,
    metamodel: &'a Metamodel,
    schema: &'a Schema,
    classes: &'a FastClasses,
}
impl Prepare<'_> {
    fn node(&self, ty: TypeId) -> Result<&TypeNode, &'static str> {
        self.image.type_node(ty).map_err(|_| "unknown type")
    }
    fn elem(&self, ty: TypeId) -> Result<Elem, &'static str> {
        Ok(match self.node(ty)? {
            TypeNode::Scalar(Scalar::String) => Elem::Str,
            TypeNode::Scalar(Scalar::Bool) => Elem::Bool,
            TypeNode::Scalar(Scalar::Int64) => Elem::Int,
            TypeNode::Scalar(Scalar::Int32) => Elem::Int32,
            TypeNode::Model(runtime) => Elem::Obj(self.class(runtime)?),
            TypeNode::Enum(_) => Elem::Enum,
            TypeNode::Option(_) | TypeNode::Collection { .. } | TypeNode::Function { .. } => {
                Elem::Other
            }
            TypeNode::Many { .. } | TypeNode::Record(_) => Elem::Other,
            _ => return Err("scalar, enum or datatype outside String/Bool/Int64/Int32"),
        })
    }
    fn class(
        &self,
        runtime: &crate::definitions::generic_types::RuntimeType,
    ) -> Result<ClassId, &'static str> {
        self.classes.class(runtime, self.metamodel, self.schema)
    }
    fn shape(&self, ty: TypeId) -> Result<Shape, &'static str> {
        Ok(match self.node(ty)? {
            TypeNode::Collection { kind, .. } => Shape {
                unique: matches!(kind, CollectionKind::Set | CollectionKind::OrderedSet),
                ordered: matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet),
                lower: 0,
                upper: None,
            },
            TypeNode::Many {
                lower,
                upper,
                ordered,
                unique,
                ..
            } => Shape {
                unique: *unique,
                ordered: *ordered,
                lower: *lower,
                upper: usize::try_from(*upper).ok(),
            },
            _ => return Err("expected a collection type"),
        })
    }
    fn element(&self, ty: TypeId) -> Result<TypeId, &'static str> {
        match self.node(ty)? {
            TypeNode::Collection { element, .. } | TypeNode::Many { element, .. } => Ok(*element),
            _ => Err("expected a collection type"),
        }
    }
    fn operand_ty(
        &self,
        function: &Function,
        block: &Block,
        op: Operand,
    ) -> Result<TypeId, &'static str> {
        match op {
            Operand::Parameter(i) => block.parameters.get(i as usize).copied(),
            Operand::Register(i) => block.instructions.get(i as usize).map(|i| i.ty),
            Operand::Capture(i) => function.captures.get(i as usize).map(|c| c.ty),
        }
        .ok_or("unknown operand")
    }
    fn function(&self, function: &Function) -> Result<FFunction, &'static str> {
        let blocks = function
            .blocks
            .iter()
            .map(|block| {
                let instructions = block
                    .instructions
                    .iter()
                    .map(|ins| self.instruction(function, block, ins.ty, &ins.kind))
                    .collect::<Result<_, _>>()?;
                let terminator = match &block.terminator {
                    Terminator::Return(op) => Term::Return(op_of(*op)),
                    Terminator::Jump(edge) => Term::Jump(edge_of(edge.block, &edge.arguments)),
                    Terminator::Branch {
                        condition,
                        then_edge,
                        else_edge,
                    } => Term::Branch(
                        op_of(*condition),
                        edge_of(then_edge.block, &then_edge.arguments),
                        edge_of(else_edge.block, &else_edge.arguments),
                    ),
                    Terminator::Option {
                        subject,
                        none,
                        some,
                    } => Term::Option(
                        op_of(*subject),
                        edge_of(none.block, &none.arguments),
                        edge_of(some.block, &some.arguments),
                    ),
                    Terminator::Result { .. } | Terminator::Absurd { .. } => {
                        return Err("Result and Absurd terminators");
                    }
                };
                Ok(FBlock {
                    instructions,
                    terminator,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(FFunction { blocks })
    }
    fn instruction(
        &self,
        function: &Function,
        block: &Block,
        ty: TypeId,
        kind: &InstructionKind,
    ) -> Result<Ins, &'static str> {
        Ok(match kind {
            InstructionKind::Constant(value) => Ins::Const(self.constant(value)?),
            InstructionKind::Unary { op, value } => {
                let operand = self.elem(self.operand_ty(function, block, *value)?)?;
                match (op, operand, Width::of(operand)) {
                    (UnaryOp::Not, Elem::Bool, _) => Ins::Not(op_of(*value)),
                    (UnaryOp::Negate, _, Some(width)) => Ins::Neg(op_of(*value), width),
                    (UnaryOp::Plus, _, Some(_)) => Ins::View(op_of(*value)),
                    _ => return Err("unary operator outside Bool/Int64/Int32"),
                }
            }
            InstructionKind::Binary { op, left, right } => {
                let operand = self.node(self.operand_ty(function, block, *left)?)?;
                match op {
                    BinaryOp::Equal | BinaryOp::NotEqual => {
                        if matches!(
                            operand,
                            TypeNode::Collection { .. }
                                | TypeNode::Many { .. }
                                | TypeNode::Function { .. }
                        ) {
                            return Err("collection or function equality");
                        }
                        self.elem(self.operand_ty(function, block, *left)?)?;
                        Ins::Eq(op_of(*left), op_of(*right), *op == BinaryOp::NotEqual)
                    }
                    _ if matches!(operand, TypeNode::Scalar(Scalar::Int64)) => {
                        Ins::Int(*op, op_of(*left), op_of(*right), Width::I64)
                    }
                    _ if matches!(operand, TypeNode::Scalar(Scalar::Int32)) => {
                        Ins::Int(*op, op_of(*left), op_of(*right), Width::I32)
                    }
                    BinaryOp::Add if matches!(operand, TypeNode::Scalar(Scalar::String)) => {
                        Ins::StrAdd(op_of(*left), op_of(*right))
                    }
                    _ => return Err("arithmetic or ordering outside Int64/Int32"),
                }
            }
            InstructionKind::MakeClosure { function, captures } => Ins::Closure(
                function.0 as usize,
                captures.iter().copied().map(op_of).collect(),
            ),
            InstructionKind::Call { callee, arguments } => Ins::Call(
                op_of(*callee),
                arguments.iter().copied().map(op_of).collect(),
            ),
            InstructionKind::View { value } => Ins::View(op_of(*value)),
            InstructionKind::None => Ins::None,
            InstructionKind::Some(value) => Ins::Some(op_of(*value)),
            InstructionKind::Collection(items) => {
                Ins::Collection(items.iter().copied().map(op_of).collect(), self.shape(ty)?)
            }
            InstructionKind::ConvertCollection { value } => {
                let source = self.shape(self.operand_ty(function, block, *value)?)?;
                let target = self.shape(ty)?;
                if !source.ordered && target.ordered {
                    return Err("unordered to ordered conversion");
                }
                Ins::Convert(op_of(*value), target)
            }
            InstructionKind::CollectionCall {
                operation,
                value,
                callback,
                seed: None,
            } if matches!(
                operation,
                CollectionOp::Map
                    | CollectionOp::Filter
                    | CollectionOp::FlatMap
                    | CollectionOp::Find
                    | CollectionOp::Exists
                    | CollectionOp::ForAll
                    | CollectionOp::One
            ) =>
            {
                let input = self.shape(self.operand_ty(function, block, *value)?)?;
                let output = match operation {
                    CollectionOp::Map | CollectionOp::Filter | CollectionOp::FlatMap => {
                        self.shape(ty)?
                    }
                    _ => input,
                };
                Ins::Each(
                    *operation,
                    op_of(*value),
                    op_of(*callback),
                    output,
                    input.unique,
                )
            }
            InstructionKind::CollectionFlatten { value } => {
                Ins::Flatten(op_of(*value), self.shape(ty)?)
            }
            InstructionKind::CollectionSize { value } => Ins::Size(op_of(*value)),
            InstructionKind::CollectionIsEmpty { value } => Ins::IsEmpty(op_of(*value)),
            InstructionKind::Split { value, separator } => {
                Ins::Split(op_of(*value), op_of(*separator))
            }
            InstructionKind::Text { op, value } => Ins::Text(*op, op_of(*value)),
            InstructionKind::Index { value, index } => {
                if !matches!(
                    self.node(self.operand_ty(function, block, *index)?)?,
                    TypeNode::Scalar(Scalar::Int64)
                ) {
                    return Err("index outside Int64");
                }
                Ins::Index(op_of(*value), op_of(*index))
            }
            InstructionKind::CollectionMembership {
                operation: operation @ (MembershipOp::Contains | MembershipOp::Excludes),
                value,
                other,
                ..
            } => Ins::Contains(
                op_of(*value),
                op_of(*other),
                *operation == MembershipOp::Excludes,
            ),
            InstructionKind::CollectionAlgebra {
                operation: AlgebraOp::Concat,
                value,
                other: Some(other),
            } => Ins::Concat(op_of(*value), op_of(*other), self.shape(ty)?),
            InstructionKind::CollectionAlgebra {
                operation: AlgebraOp::Distinct,
                value,
                other: None,
            } => Ins::Distinct(op_of(*value), self.shape(ty)?),
            InstructionKind::Ownership {
                operation,
                value,
                selected,
            } => {
                let TypeNode::Model(runtime) = self.node(*selected)? else {
                    return Err("ownership selection is not a model type");
                };
                Ins::Owner(*operation, op_of(*value), self.class(runtime)?)
            }
            InstructionKind::New(fields) => {
                let TypeNode::Model(runtime) = self.node(ty)? else {
                    return Err("`new` result is not a model class");
                };
                let class = self.class(runtime)?;
                let mut values = Vec::with_capacity(fields.len());
                for field in fields.values() {
                    let feature = self
                        .metamodel
                        .feature_by_declaration(&field.declaration)
                        .ok_or("constructed feature is not in the metamodel")?;
                    values.push((feature, op_of(field.value)));
                }
                Ins::New(class, values)
            }
            InstructionKind::StoredFeature {
                value, declaration, ..
            } => {
                let feature = self
                    .metamodel
                    .feature_by_declaration(declaration)
                    .ok_or("stored feature is not in the metamodel")?;
                let info = self
                    .metamodel
                    .feature_info(feature)
                    .ok_or("stored feature is not in the metamodel")?;
                let volatile = self
                    .metamodel
                    .definition(feature)
                    .is_some_and(|d| d.volatile);
                if info.derived || volatile {
                    return Err("derived or volatile stored feature");
                }
                let read = match self.node(ty)? {
                    TypeNode::Option(inner) => Read::Optional(self.elem(*inner)?),
                    TypeNode::Collection { .. } | TypeNode::Many { .. } => {
                        Read::Many(self.elem(self.element(ty)?)?, self.shape(ty)?)
                    }
                    _ => Read::Required(self.elem(ty)?),
                };
                if matches!(
                    read,
                    Read::Optional(Elem::Other)
                        | Read::Required(Elem::Other)
                        | Read::Many(Elem::Other, _)
                ) {
                    return Err("nested stored feature value");
                }
                Ins::Stored(op_of(*value), feature, read)
            }
            InstructionKind::Record(fields) => Ins::Record(
                fields
                    .iter()
                    .map(|(name, op)| (Rc::from(name.as_str()), op_of(*op)))
                    .collect(),
            ),
            InstructionKind::RecordField { value, field } => {
                Ins::Field(op_of(*value), Rc::from(field.as_str()))
            }
            _ => return Err("instruction outside the fast subset"),
        })
    }
    fn constant(&self, value: &ValueWire) -> Result<V, &'static str> {
        Ok(match &value.data {
            DataWire::Scalar(scalar) => match (self.elem(value.ty)?, &scalar.payload) {
                (Elem::Str, Value::String(s)) => V::Str(Rc::from(s.as_str())),
                (Elem::Bool, Value::Bool(b)) => V::Bool(*b),
                (Elem::Int, payload) => V::Int(int64(payload).ok_or("Int64 constant payload")?),
                (Elem::Int32, payload) => V::Int(int32(payload).ok_or("Int32 constant payload")?),
                _ => return Err("scalar constant outside String/Bool/Int64/Int32"),
            },
            DataWire::Enum(value) => V::Enum(Rc::new(
                serde_json::to_value(value).map_err(|_| "enum constant encoding")?,
            )),
            DataWire::None => V::None,
            DataWire::Some(inner) => V::Some(Rc::new(self.constant(inner)?)),
            DataWire::Collection(items) => V::Seq(Rc::new(
                items
                    .iter()
                    .map(|i| self.constant(i))
                    .collect::<Result<_, _>>()?,
            )),
            _ => return Err("constant outside the fast subset"),
        })
    }
}
fn op_of(op: Operand) -> Op {
    match op {
        Operand::Parameter(i) => Op::P(i),
        Operand::Register(i) => Op::R(i),
        Operand::Capture(i) => Op::C(i),
    }
}
fn edge_of(block: u32, arguments: &[Operand]) -> Edge {
    Edge(
        block as usize,
        arguments.iter().copied().map(op_of).collect(),
    )
}
/// Int64 payloads are `{"kind":"int64","value":"…"}`; plain numbers are accepted.
fn int64(payload: &Value) -> Option<i64> {
    match payload {
        Value::Number(n) => n.as_i64(),
        Value::Object(o) => o.get("value")?.as_str()?.parse().ok(),
        _ => None,
    }
}
/// Int32 payloads are plain JSON integers within the `i32` range.
fn int32(payload: &Value) -> Option<i64> {
    Width::I32.fit(payload.as_i64()?)
}

/// Results of calls without captures, by program, function and arguments.
/// While the model does not change, each such call is pure. Only successful
/// results are kept: a run that read a pending or failed reference keeps
/// nothing.
#[derive(Default)]
pub struct FastMemo {
    results: std::cell::RefCell<rustc_hash::FxHashMap<MemoKey, V>>,
}

/// Program address, function index and argument keys of one memoised call.
type MemoKey = (usize, usize, Vec<Key>);

struct Exec<'p> {
    program: &'p FastProgram,
    read: &'p dyn FastRead,
    steps: usize,
    max_steps: usize,
    depth: usize,
    max_depth: usize,
    memo: &'p FastMemo,
}

/// A hashable argument value; closures have no key and are not memoized.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Bool(bool),
    Int(i64),
    Str(Rc<str>),
    Obj(ObjId),
    Enum(String),
    None,
    Some(Box<Key>),
    Seq(Vec<Key>),
    Record(Vec<(Rc<str>, Key)>),
}
fn key(value: &V) -> Option<Key> {
    Some(match value {
        V::Bool(b) => Key::Bool(*b),
        V::Int(i) => Key::Int(*i),
        V::Str(s) => Key::Str(s.clone()),
        V::Obj(o) => Key::Obj(*o),
        V::Enum(value) => Key::Enum(value.to_string()),
        V::None => Key::None,
        V::Some(v) => Key::Some(Box::new(key(v)?)),
        V::Seq(items) => Key::Seq(items.iter().map(key).collect::<Option<_>>()?),
        V::Record(fields) => Key::Record(
            fields
                .iter()
                .map(|(name, value)| Some((name.clone(), key(value)?)))
                .collect::<Option<_>>()?,
        ),
        V::Fun(_) | V::New(_) => return None,
    })
}
impl Exec<'_> {
    fn step(&mut self) -> Result<(), FastError> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(FastError::Fallback("step budget"));
        }
        Ok(())
    }
    fn call(
        &mut self,
        function: usize,
        arguments: Vec<V>,
        captures: Rc<[V]>,
    ) -> Result<V, FastError> {
        if self.depth >= self.max_depth {
            return Err(FastError::Fallback("call depth"));
        }
        let program = self.program as *const FastProgram as usize;
        let memo = if captures.is_empty() {
            arguments
                .iter()
                .map(key)
                .collect::<Option<Vec<_>>>()
                .map(|args| (program, function, args))
        } else {
            None
        };
        if let Some(found) = memo
            .as_ref()
            .and_then(|k| self.memo.results.borrow().get(k).cloned())
        {
            return Ok(found);
        }
        self.depth += 1;
        let result = self.body(function, arguments, &captures);
        self.depth -= 1;
        if let (Some(k), Ok(value)) = (memo, &result) {
            self.memo.results.borrow_mut().insert(k, value.clone());
        }
        result
    }
    fn body(
        &mut self,
        function: usize,
        mut parameters: Vec<V>,
        captures: &[V],
    ) -> Result<V, FastError> {
        let program = self.program;
        let blocks = &program
            .functions
            .get(function)
            .ok_or(FastError::Fallback("missing function"))?
            .blocks;
        let mut block = 0usize;
        let mut registers: Vec<V> = Vec::new();
        loop {
            let code = blocks
                .get(block)
                .ok_or(FastError::Fallback("missing block"))?;
            registers.clear();
            for ins in &code.instructions {
                self.step()?;
                let value = self.instruction(ins, &parameters, &registers, captures)?;
                registers.push(value);
            }
            let get = |op: Op| -> Result<V, FastError> {
                match op {
                    Op::P(i) => parameters.get(i as usize),
                    Op::R(i) => registers.get(i as usize),
                    Op::C(i) => captures.get(i as usize),
                }
                .cloned()
                .ok_or(FastError::Fallback("operand"))
            };
            let take = |edge: &Edge, payload: Option<V>| -> Result<(usize, Vec<V>), FastError> {
                let mut next = Vec::with_capacity(edge.1.len() + 1);
                next.extend(payload);
                for op in &edge.1 {
                    next.push(get(*op)?);
                }
                Ok((edge.0, next))
            };
            let (target, next) = match &code.terminator {
                Term::Return(op) => return get(*op),
                Term::Jump(edge) => take(edge, None)?,
                Term::Branch(condition, then_edge, else_edge) => match get(*condition)? {
                    V::Bool(true) => take(then_edge, None)?,
                    V::Bool(false) => take(else_edge, None)?,
                    _ => return Err(FastError::Fallback("branch condition")),
                },
                Term::Option(subject, none, some) => match get(*subject)? {
                    V::None => take(none, None)?,
                    V::Some(value) => take(some, Some((*value).clone()))?,
                    _ => return Err(FastError::Fallback("option subject")),
                },
            };
            block = target;
            parameters = next;
        }
    }
    fn instruction(
        &mut self,
        ins: &Ins,
        parameters: &[V],
        registers: &[V],
        captures: &[V],
    ) -> Result<V, FastError> {
        let get = |op: Op| -> Result<&V, FastError> {
            match op {
                Op::P(i) => parameters.get(i as usize),
                Op::R(i) => registers.get(i as usize),
                Op::C(i) => captures.get(i as usize),
            }
            .ok_or(FastError::Fallback("operand"))
        };
        Ok(match ins {
            Ins::Const(value) => value.clone(),
            Ins::View(op) => get(*op)?.clone(),
            Ins::Not(op) => match get(*op)? {
                V::Bool(b) => V::Bool(!b),
                _ => return Err(FastError::Fallback("not")),
            },
            Ins::Neg(op, width) => match get(*op)? {
                V::Int(i) => V::Int(
                    i.checked_neg()
                        .and_then(|n| width.fit(n))
                        .ok_or(FastError::Fallback("overflow"))?,
                ),
                _ => return Err(FastError::Fallback("negate")),
            },
            Ins::Eq(left, right, negate) => V::Bool(equal(get(*left)?, get(*right)?)? != *negate),
            Ins::Int(op, left, right, width) => {
                let (V::Int(a), V::Int(b)) = (get(*left)?, get(*right)?) else {
                    return Err(FastError::Fallback("integer operands"));
                };
                let overflow = FastError::Fallback("integer overflow or division");
                // A result outside the width is the VM's overflow error.
                let fit = |n: Option<i64>| n.and_then(|n| width.fit(n)).ok_or(overflow.clone());
                match op {
                    BinaryOp::Add => V::Int(fit(a.checked_add(*b))?),
                    BinaryOp::Subtract => V::Int(fit(a.checked_sub(*b))?),
                    BinaryOp::Multiply => V::Int(fit(a.checked_mul(*b))?),
                    BinaryOp::Less => V::Bool(a < b),
                    BinaryOp::LessEqual => V::Bool(a <= b),
                    BinaryOp::Greater => V::Bool(a > b),
                    BinaryOp::GreaterEqual => V::Bool(a >= b),
                    // Division and remainder semantics stay with the VM.
                    _ => return Err(overflow),
                }
            }
            Ins::Closure(function, ops) => {
                let captured = ops
                    .iter()
                    .map(|op| get(*op).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                V::Fun(Rc::new(Closure {
                    function: *function,
                    captures: Rc::from(captured),
                }))
            }
            Ins::Call(callee, ops) => {
                let V::Fun(closure) = get(*callee)?.clone() else {
                    return Err(FastError::Fallback("callee"));
                };
                let arguments = ops
                    .iter()
                    .map(|op| get(*op).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                self.call(closure.function, arguments, closure.captures.clone())?
            }
            Ins::None => V::None,
            Ins::Some(op) => V::Some(Rc::new(get(*op)?.clone())),
            Ins::New(class, fields) => V::New(Rc::new(NewObject {
                class: *class,
                values: fields
                    .iter()
                    .map(|(feature, op)| Ok((*feature, get(*op)?.clone())))
                    .collect::<Result<_, FastError>>()?,
            })),
            Ins::Record(fields) => V::Record(
                fields
                    .iter()
                    .map(|(name, op)| Ok((name.clone(), get(*op)?.clone())))
                    .collect::<Result<Vec<_>, FastError>>()?
                    .into(),
            ),
            Ins::Field(op, name) => get(*op)?
                .field(name)
                .cloned()
                .ok_or(FastError::Fallback("record field"))?,
            Ins::Collection(ops, shape) => {
                let items = ops
                    .iter()
                    .map(|op| get(*op).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                collection(items, *shape, true)?
            }
            Ins::Convert(op, shape) => {
                let items = seq(get(*op)?)?.to_vec();
                collection(items, *shape, true)?
            }
            Ins::Flatten(op, shape) => {
                let mut items = Vec::new();
                for inner in seq(get(*op)?)?.iter() {
                    items.extend(seq(inner)?.iter().cloned());
                }
                collection(items, *shape, false)?
            }
            Ins::Size(op) => V::Int(
                i64::try_from(seq(get(*op)?)?.len()).map_err(|_| FastError::Fallback("size"))?,
            ),
            Ins::IsEmpty(op) => V::Bool(seq(get(*op)?)?.is_empty()),
            Ins::Contains(collection, item, negate) => {
                let item = get(*item)?;
                let mut found = false;
                for candidate in seq(get(*collection)?)?.iter() {
                    if equal(candidate, item)? {
                        found = true;
                        break;
                    }
                }
                V::Bool(found != *negate)
            }
            Ins::Concat(left, right, shape) => {
                let mut items = seq(get(*left)?)?.to_vec();
                items.extend(seq(get(*right)?)?.iter().cloned());
                collection(items, *shape, false)?
            }
            Ins::Distinct(value, shape) => {
                let mut kept: Vec<V> = Vec::new();
                for item in seq(get(*value)?)?.iter() {
                    let mut seen = false;
                    for prior in &kept {
                        if equal(prior, item)? {
                            seen = true;
                            break;
                        }
                    }
                    if !seen {
                        kept.push(item.clone());
                    }
                }
                collection(kept, *shape, false)?
            }
            Ins::Index(collection, index) => {
                let V::Int(index) = get(*index)? else {
                    return Err(FastError::Fallback("index operand"));
                };
                // Out of range, including negative, is None, as in the VM.
                match usize::try_from(*index)
                    .ok()
                    .and_then(|i| seq(get(*collection).ok()?).ok()?.get(i).cloned())
                {
                    Some(item) => V::Some(Rc::new(item)),
                    None => {
                        seq(get(*collection)?)?;
                        V::None
                    }
                }
            }
            Ins::StrAdd(left, right) => {
                let (V::Str(a), V::Str(b)) = (get(*left)?, get(*right)?) else {
                    return Err(FastError::Fallback("string operands"));
                };
                V::Str(Rc::from(format!("{a}{b}")))
            }
            Ins::Text(op, value) => {
                let text: Rc<str> = match (op, get(*value)?) {
                    (TextOp::Hex, V::Str(text)) => {
                        let mut hex = String::with_capacity(text.len() * 2);
                        for byte in text.bytes() {
                            hex.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
                            hex.push(char::from(HEX_DIGITS[usize::from(byte & 15)]));
                        }
                        Rc::from(hex)
                    }
                    (TextOp::Identity, V::Obj(obj)) => Rc::from(
                        self.read
                            .identity(*obj)
                            .ok_or(FastError::Fallback("object identity"))?,
                    ),
                    (TextOp::Resource, V::Obj(obj)) => Rc::from(
                        self.read
                            .resource(*obj)
                            .ok_or(FastError::Fallback("object resource"))?,
                    ),
                    _ => return Err(FastError::Fallback("text primitive operand")),
                };
                V::Str(text)
            }
            Ins::Split(text, separator) => {
                let (V::Str(text), V::Str(separator)) = (get(*text)?, get(*separator)?) else {
                    return Err(FastError::Fallback("split operands"));
                };
                if separator.is_empty() {
                    // The VM reports this argument error.
                    return Err(FastError::Fallback("empty split separator"));
                }
                V::Seq(Rc::new(
                    text.split(&**separator)
                        .map(|piece| V::Str(Rc::from(piece)))
                        .collect(),
                ))
            }
            Ins::Each(operation, value, callback, shape, input_unique) => {
                let items = match get(*value)? {
                    V::Seq(items) => items.clone(),
                    _ => return Err(FastError::Fallback("collection operand")),
                };
                let V::Fun(closure) = get(*callback)?.clone() else {
                    return Err(FastError::Fallback("callback"));
                };
                self.each(*operation, &items, &closure, *shape, *input_unique)?
            }
            Ins::Owner(operation, op, class) => {
                let V::Obj(obj) = get(*op)? else {
                    return Err(FastError::Fallback("ownership receiver"));
                };
                self.ownership(*operation, *obj, *class)?
            }
            Ins::Stored(op, feature, read) => {
                let V::Obj(obj) = get(*op)? else {
                    return Err(FastError::Fallback("stored receiver"));
                };
                self.stored(*obj, *feature, *read)?
            }
        })
    }
    fn invoke(&mut self, closure: &Closure, item: &V) -> Result<V, FastError> {
        self.step()?;
        self.call(
            closure.function,
            vec![item.clone()],
            closure.captures.clone(),
        )
    }
    fn predicate(&mut self, closure: &Closure, item: &V) -> Result<bool, FastError> {
        match self.invoke(closure, item)? {
            V::Bool(b) => Ok(b),
            _ => Err(FastError::Fallback("predicate result")),
        }
    }
    fn each(
        &mut self,
        operation: CollectionOp,
        items: &[V],
        closure: &Closure,
        shape: Shape,
        input_unique: bool,
    ) -> Result<V, FastError> {
        Ok(match operation {
            CollectionOp::Filter => {
                let mut kept = Vec::new();
                for item in items {
                    if self.predicate(closure, item)? {
                        kept.push(item.clone());
                    }
                }
                // A subset of a unique input is unique; otherwise let the VM decide.
                collection(kept, shape, input_unique)?
            }
            CollectionOp::Map => {
                let mut mapped = Vec::with_capacity(items.len());
                for item in items {
                    mapped.push(self.invoke(closure, item)?);
                }
                collection(mapped, shape, false)?
            }
            CollectionOp::FlatMap => {
                let mut flat = Vec::new();
                for item in items {
                    flat.extend(seq(&self.invoke(closure, item)?)?.iter().cloned());
                }
                collection(flat, shape, false)?
            }
            CollectionOp::Find => {
                for item in items {
                    if self.predicate(closure, item)? {
                        return Ok(V::Some(Rc::new(item.clone())));
                    }
                }
                V::None
            }
            CollectionOp::Exists => {
                for item in items {
                    if self.predicate(closure, item)? {
                        return Ok(V::Bool(true));
                    }
                }
                V::Bool(false)
            }
            CollectionOp::ForAll => {
                for item in items {
                    if !self.predicate(closure, item)? {
                        return Ok(V::Bool(false));
                    }
                }
                V::Bool(true)
            }
            CollectionOp::One => {
                let mut count = 0usize;
                for item in items {
                    if self.predicate(closure, item)? {
                        count += 1;
                    }
                }
                V::Bool(count == 1)
            }
            _ => return Err(FastError::Fallback("collection operation")),
        })
    }
    fn ownership(
        &mut self,
        operation: OwnershipOp,
        obj: ObjId,
        class: ClassId,
    ) -> Result<V, FastError> {
        let read = self.read;
        let is = |candidate: ObjId| -> Result<bool, FastError> {
            let actual = read
                .class(candidate)
                .ok_or(FastError::Fallback("ownership class"))?;
            Ok(read.metamodel().is_a(actual, class))
        };
        Ok(match operation {
            OwnershipOp::AsType => {
                if is(obj)? {
                    V::Some(Rc::new(V::Obj(obj)))
                } else {
                    V::None
                }
            }
            OwnershipOp::Container => match read.container(obj) {
                Some(parent) if is(parent)? => V::Some(Rc::new(V::Obj(parent))),
                _ => V::None,
            },
            OwnershipOp::Ancestors => {
                let mut found = Vec::new();
                let mut current = obj;
                let mut steps = 0usize;
                while let Some(parent) = read.container(current) {
                    steps += 1;
                    if steps > read.object_count() {
                        return Err(FastError::Fallback("containment cycle"));
                    }
                    self.step()?;
                    if is(parent)? {
                        found.push(V::Obj(parent));
                    }
                    current = parent;
                }
                V::Seq(Rc::new(found))
            }
            OwnershipOp::Roots => {
                let mut found = Vec::new();
                for root in read.roots() {
                    self.step()?;
                    if is(*root)? {
                        found.push(V::Obj(*root));
                    }
                }
                V::Seq(Rc::new(found))
            }
            OwnershipOp::Contents => {
                let mut found = Vec::new();
                for child in read.children(obj) {
                    self.step()?;
                    if is(*child)? {
                        found.push(V::Obj(*child));
                    }
                }
                V::Seq(Rc::new(found))
            }
        })
    }
    fn stored(&mut self, obj: ObjId, feature: FeatureId, read: Read) -> Result<V, FastError> {
        let model = self.read;
        let value = model
            .value(obj, feature)
            .map_err(|_| FastError::Pending)?
            .ok_or(FastError::Fallback("feature not stored on receiver"))?;
        Ok(match read {
            Read::Optional(elem) => match value {
                LinkValue::Absent | LinkValue::Null | LinkValue::Default(Value::Null) => V::None,
                value => V::Some(Rc::new(self.scalar(elem, value)?)),
            },
            Read::Required(elem) => match value {
                LinkValue::Absent | LinkValue::Null | LinkValue::Default(Value::Null) => {
                    return Err(FastError::Fallback("required value absent"));
                }
                value => self.scalar(elem, value)?,
            },
            Read::Many(elem, shape) => {
                let count = match value {
                    LinkValue::Many(count) => count,
                    LinkValue::Default(Value::Array(items)) => items.len(),
                    _ => return Err(FastError::Fallback("collection value")),
                };
                let mut items = Vec::with_capacity(count);
                for index in 0..count {
                    self.step()?;
                    let item = model
                        .item(obj, feature, index)
                        .map_err(|_| FastError::Pending)?
                        .ok_or(FastError::Fallback("collection item"))?;
                    items.push(self.scalar(elem, item)?);
                }
                collection(items, shape, false)?
            }
        })
    }
    fn scalar(&self, elem: Elem, value: LinkValue<'_>) -> Result<V, FastError> {
        let mismatch = FastError::Fallback("stored value kind");
        Ok(match (elem, value) {
            (Elem::Str, LinkValue::Str(s)) => V::Str(Rc::from(s)),
            (
                Elem::Str,
                LinkValue::Default(Value::String(s)) | LinkValue::Json(Value::String(s)),
            ) => V::Str(Rc::from(s.as_str())),
            (Elem::Bool, LinkValue::Bool(b)) => V::Bool(b),
            (Elem::Bool, LinkValue::Default(Value::Bool(b)) | LinkValue::Json(Value::Bool(b))) => {
                V::Bool(*b)
            }
            (Elem::Int, LinkValue::Int(i)) => V::Int(i),
            (Elem::Int, LinkValue::Default(v) | LinkValue::Json(v)) => {
                V::Int(int64(v).ok_or(mismatch)?)
            }
            (Elem::Int32, LinkValue::Int(i)) => V::Int(Width::I32.fit(i).ok_or(mismatch)?),
            (Elem::Int32, LinkValue::Default(v) | LinkValue::Json(v)) => {
                V::Int(int32(v).ok_or(mismatch)?)
            }
            (Elem::Enum, LinkValue::Default(v) | LinkValue::Json(v)) if v.is_object() => {
                V::Enum(Rc::new(v.clone()))
            }
            (Elem::Obj(class), LinkValue::Ref(obj)) => {
                let actual = self
                    .read
                    .class(obj)
                    .ok_or(FastError::Fallback("target class"))?;
                if !self.program_metamodel_is_a(actual, class) {
                    return Err(FastError::Fallback("target class"));
                }
                V::Obj(obj)
            }
            _ => return Err(mismatch),
        })
    }
    fn program_metamodel_is_a(&self, actual: ClassId, expected: ClassId) -> bool {
        self.read.metamodel().is_a(actual, expected)
    }
}

/// Build a collection result. When the result kind is unique and the items
/// are not already known to be unique, duplicates are left to the VM. The
/// multiplicity of a bounded result is checked the same way.
fn collection(items: Vec<V>, shape: Shape, deduplicate: bool) -> Result<V, FastError> {
    let items = if shape.unique {
        if deduplicate {
            let mut kept: Vec<V> = Vec::with_capacity(items.len());
            for item in items {
                let mut seen = false;
                for prior in &kept {
                    if equal(prior, &item)? {
                        seen = true;
                        break;
                    }
                }
                if !seen {
                    kept.push(item);
                }
            }
            kept
        } else {
            if has_duplicate(&items)? {
                return Err(FastError::Fallback("duplicates in a unique result"));
            }
            items
        }
    } else {
        items
    };
    if items.len() < shape.lower || shape.upper.is_some_and(|u| items.len() > u) {
        return Err(FastError::Fallback("result multiplicity"));
    }
    Ok(V::Seq(Rc::new(items)))
}
fn has_duplicate(items: &[V]) -> Result<bool, FastError> {
    if items.iter().all(|v| matches!(v, V::Obj(_))) {
        let mut ids: Vec<ObjId> = items
            .iter()
            .filter_map(|v| match v {
                V::Obj(o) => Some(*o),
                _ => None,
            })
            .collect();
        ids.sort_unstable();
        return Ok(ids.windows(2).any(|w| w[0] == w[1]));
    }
    for (i, a) in items.iter().enumerate() {
        for b in &items[i + 1..] {
            if equal(a, b)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn seq(value: &V) -> Result<&Rc<Vec<V>>, FastError> {
    match value {
        V::Seq(items) => Ok(items),
        _ => Err(FastError::Fallback("collection value")),
    }
}
/// Structural equality on scalars, model identities and Options.
fn equal(a: &V, b: &V) -> Result<bool, FastError> {
    Ok(match (a, b) {
        (V::Bool(x), V::Bool(y)) => x == y,
        (V::Int(x), V::Int(y)) => x == y,
        (V::Str(x), V::Str(y)) => x == y,
        (V::Obj(x), V::Obj(y)) => x == y,
        (V::Enum(x), V::Enum(y)) => x == y,
        (V::None, V::None) => true,
        (V::None, V::Some(_)) | (V::Some(_), V::None) => false,
        (V::Some(x), V::Some(y)) => equal(x, y)?,
        (V::Record(x), V::Record(y)) if x.len() == y.len() => {
            for ((a, x), (b, y)) in x.iter().zip(y.iter()) {
                if a != b {
                    return Err(FastError::Fallback("equality operands"));
                }
                if !equal(x, y)? {
                    return Ok(false);
                }
            }
            true
        }
        _ => return Err(FastError::Fallback("equality operands")),
    })
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
