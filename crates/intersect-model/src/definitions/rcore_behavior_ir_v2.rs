//! Version 2 function IR. Wire metadata never grants executable authority.
use super::{
    ir,
    modules::SymbolId,
    types::{CollectionKind, Scalar, TypeRef, TypeTable},
};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    conversion::{
        AppliedConversionContract, ConstructorContract, ConversionContract, ConversionRegistry,
    },
    generic_types::{ClosedType, RuntimeType},
    resources::DeclarationId,
};
pub use ir::{IrError, IrErrorKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[path = "rcore_behavior_ir_v2_types.rs"]
mod type_graph;
pub use type_graph::{export_type, import_types, policy_manifest};
#[path = "rcore_behavior_ir_v2_values.rs"]
mod value_graph;
pub(crate) use value_graph::RetainedValueShapeCursor;
pub use value_graph::{ValueBudget, ValueShapeCursor, ValueVisitor, inspect_value_shape};
#[path = "rcore_behavior_ir_v2_verify.rs"]
mod verification;
pub(crate) use verification::SavedContext;
pub(crate) use verification::context as verify_context_with_source_limits;
pub(crate) use verification::verify_contract;
pub(crate) use verification::{context_checked, verification_check, verification_models};
pub use verification::{
    preflight_selection, verify, verify_with_conversions, verify_with_source_limits,
};
#[path = "rcore_behavior_ir_v2_scalar.rs"]
mod scalar_lift;
pub use scalar_lift::{
    ScalarBlock, ScalarFunction, ScalarInstruction, ScalarInstructionKind, ScalarProgram,
    VerifiedScalarExecutable, lift_scalar,
};

pub const VERSION: u32 = 2;
/// Verified upper bound on explicit model navigation in a callable body in this
/// closed image. Query may be conservative for indirect calls. Admission and
/// conversion-provider capabilities are separate from this query classification.
/// Construct marks a function that builds model objects with `new`, directly or
/// through a call; only implicit-element construction admits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Pure,
    Query,
    Construct,
}
#[path = "rcore_behavior_ir_v2_effects.rs"]
mod effects;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnershipOp {
    Container,
    Contents,
    Ancestors,
    /// Uncontained objects of the receiver's model, in identity order.
    Roots,
    /// Checked self view. Shares typed model navigation admission/continuations,
    /// but does not traverse containment or infer a type from a failed read.
    AsType,
}
impl OwnershipOp {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "container" => Some(Self::Container),
            "contents" => Some(Self::Contents),
            "ancestors" => Some(Self::Ancestors),
            "roots" => Some(Self::Roots),
            "asType" => Some(Self::AsType),
            _ => None,
        }
    }
    pub fn singular(self) -> bool {
        matches!(self, Self::Container | Self::AsType)
    }
}
/// String-valued primitives of one operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextOp {
    /// `hex(text)`: lowercase hexadecimal of the UTF-8 bytes of `text`.
    Hex,
    /// `identity(object)`: the object's model identity.
    Identity,
    /// `resource(object)`: the URI of the resource that holds the object.
    Resource,
}
impl TextOp {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "hex" => Some(Self::Hex),
            "identity" => Some(Self::Identity),
            "resource" => Some(Self::Resource),
            _ => None,
        }
    }
    /// Whether the operand is a model object (otherwise a String).
    pub fn reads_model(self) -> bool {
        !matches!(self, Self::Hex)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlgebraOp {
    Union,
    Intersection,
    Difference,
    Concat,
    Distinct,
}
impl AlgebraOp {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "union" => Self::Union,
            "intersection" => Self::Intersection,
            "difference" => Self::Difference,
            "concat" => Self::Concat,
            "distinct" => Self::Distinct,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipOp {
    Contains,
    Excludes,
    Count,
    IncludesAll,
    IncludesAny,
}
impl MembershipOp {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "contains" => Self::Contains,
            "excludes" => Self::Excludes,
            "count" => Self::Count,
            "includesAll" => Self::IncludesAll,
            "includesAny" => Self::IncludesAny,
            _ => return None,
        })
    }
    pub fn multiple(self) -> bool {
        matches!(self, Self::IncludesAll | Self::IncludesAny)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionOp {
    Closure,
    SortedWith,
    SortedBy,
    GroupBy,
    Map,
    Filter,
    FlatMap,
    Fold,
    Exists,
    ForAll,
    One,
    Find,
}
impl CollectionOp {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "closure" => Self::Closure,
            "sortedWith" => Self::SortedWith,
            "sortedBy" => Self::SortedBy,
            "groupBy" => Self::GroupBy,
            "map" => Self::Map,
            "filter" => Self::Filter,
            "flatMap" => Self::FlatMap,
            "fold" => Self::Fold,
            "exists" => Self::Exists,
            "forAll" => Self::ForAll,
            "one" => Self::One,
            "find" => Self::Find,
            _ => return None,
        })
    }
    pub fn predicate(self) -> bool {
        matches!(
            self,
            Self::Filter | Self::Exists | Self::ForAll | Self::One | Self::Find
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TypeId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FunctionId(pub u32);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TypeNode {
    Scalar(Scalar),
    Enum(#[serde(deserialize_with = "canonical_wire")] DeclarationId),
    Datatype(#[serde(deserialize_with = "canonical_wire")] DeclarationId),
    AppliedDatatype(#[serde(deserialize_with = "canonical_wire")] ClosedType),
    Model(#[serde(deserialize_with = "canonical_wire")] RuntimeType),
    Option(TypeId),
    Result {
        ok: TypeId,
        error: TypeId,
    },
    Collection {
        #[serde(deserialize_with = "collection_kind")]
        kind: CollectionKind,
        element: TypeId,
    },
    Many {
        element: TypeId,
        lower: usize,
        upper: i32,
        ordered: bool,
        unique: bool,
    },
    Tuple(Vec<TypeId>),
    Record(BTreeMap<String, TypeId>),
    Function {
        parameters: Vec<TypeId>,
        result: TypeId,
    },
}
fn collection_kind<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<CollectionKind, D::Error> {
    let name = String::deserialize(deserializer)?;
    match name.as_str() {
        "Sequence" => Ok(CollectionKind::Sequence),
        "Set" => Ok(CollectionKind::Set),
        "OrderedSet" => Ok(CollectionKind::OrderedSet),
        "Bag" => Ok(CollectionKind::Bag),
        _ => Err(serde::de::Error::custom("Unknown collection kind")),
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntrySelection {
    #[serde(deserialize_with = "canonical_wire")]
    pub symbol: SymbolId,
    pub types: Vec<TypeNode>,
    pub arguments: Vec<TypeId>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyManifest {
    #[serde(deserialize_with = "canonical_wire")]
    pub conversions: Vec<ConversionContract>,
    #[serde(deserialize_with = "canonical_wire")]
    pub applied: Vec<AppliedConversionContract>,
    #[serde(deserialize_with = "canonical_wire")]
    pub constructors: Vec<ConstructorContract>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum FunctionIdentity {
    Named {
        #[serde(deserialize_with = "canonical_wire")]
        symbol: SymbolId,
        type_arguments: Vec<TypeId>,
    },
    Lambda {
        enclosing: FunctionId,
        expression: u32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub declaration: SourceSpan,
    pub ty: TypeId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operand {
    Parameter(u32),
    Register(u32),
    Capture(u32),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueWire {
    pub ty: TypeId,
    pub data: DataWire,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum DataWire {
    Scalar(ir::Value),
    Enum(#[serde(deserialize_with = "canonical_wire")] crate::definitions::enums::EnumValue),
    Datatype(serde_json::Value),
    Model(serde_json::Value),
    None,
    Some(Box<ValueWire>),
    Ok(Box<ValueWire>),
    Err(Box<ValueWire>),
    Collection(Vec<ValueWire>),
    Tuple(Vec<ValueWire>),
    Record(BTreeMap<String, ValueWire>),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewField {
    #[serde(deserialize_with = "canonical_wire")]
    pub declaration: DeclarationId,
    pub value: Operand,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum InstructionKind {
    Constant(ValueWire),
    Unary {
        op: ir::UnaryOp,
        value: Operand,
    },
    Binary {
        op: ir::BinaryOp,
        left: Operand,
        right: Operand,
    },
    MakeClosure {
        function: FunctionId,
        captures: Vec<Operand>,
    },
    Call {
        callee: Operand,
        arguments: Vec<Operand>,
    },
    View {
        value: Operand,
    },
    None,
    Some(Operand),
    Ok(Operand),
    Err(Operand),
    Tuple(Vec<Operand>),
    Record(BTreeMap<String, Operand>),
    Collection(Vec<Operand>),
    ConvertCollection {
        value: Operand,
    },
    CollectionCall {
        operation: CollectionOp,
        value: Operand,
        callback: Operand,
        seed: Option<Operand>,
    },
    CollectionAlgebra {
        operation: AlgebraOp,
        value: Operand,
        other: Option<Operand>,
    },
    CollectionMembership {
        operation: MembershipOp,
        value: Operand,
        other: Operand,
        element: TypeId,
    },
    CollectionFlatten {
        value: Operand,
    },
    CollectionSize {
        value: Operand,
    },
    CollectionIsEmpty {
        value: Operand,
    },
    /// `split(text, separator)`: the pieces of `text` between occurrences of a
    /// nonempty `separator`, in order (`Sequence<String>`).
    Split {
        value: Operand,
        separator: Operand,
    },
    /// `hex`, `identity` or `resource` (`String`).
    Text {
        op: TextOp,
        value: Operand,
    },
    TupleField {
        value: Operand,
        index: u32,
    },
    Ownership {
        operation: OwnershipOp,
        value: Operand,
        selected: TypeId,
    },
    Incoming {
        value: Operand,
        source: TypeId,
        field: String,
        #[serde(deserialize_with = "canonical_wire")]
        declaration: DeclarationId,
    },
    StoredFeature {
        value: Operand,
        field: String,
        #[serde(deserialize_with = "canonical_wire")]
        declaration: DeclarationId,
    },
    /// `new`: a detached object of the result's model class with these stored
    /// features set, by name. The engine adds it to the model; executing it
    /// changes nothing.
    New(BTreeMap<String, NewField>),
    RecordField {
        value: Operand,
        field: String,
    },
    Index {
        value: Operand,
        index: Operand,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    pub ty: TypeId,
    pub kind: InstructionKind,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub block: u32,
    pub arguments: Vec<Operand>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadEdge {
    pub block: u32,
    pub arguments: Vec<Operand>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Terminator {
    Return(Operand),
    Absurd {
        value: Operand,
    },
    Jump(Edge),
    Branch {
        condition: Operand,
        then_edge: Edge,
        else_edge: Edge,
    },
    Option {
        subject: Operand,
        none: Edge,
        some: PayloadEdge,
    },
    Result {
        subject: Operand,
        ok: PayloadEdge,
        error: PayloadEdge,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub parameters: Vec<TypeId>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub identity: FunctionIdentity,
    pub parameters: Vec<TypeId>,
    pub result: TypeId,
    pub captures: Vec<Capture>,
    pub blocks: Vec<Block>,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub version: u32,
    pub entry_source: SourceId,
    pub sources: Vec<ir::SourceManifestEntry>,
    pub policies: PolicyManifest,
    pub types: Vec<TypeNode>,
    pub functions: Vec<Function>,
    pub entry: FunctionId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub scalar: ir::VerificationLimits,
    pub max_types: usize,
    pub max_functions: usize,
    pub max_captures: usize,
    pub max_type_depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            scalar: ir::VerificationLimits::default(),
            max_types: 4096,
            max_functions: 1024,
            max_captures: 4096,
            max_type_depth: 64,
        }
    }
}

/// Index into the verified image's exact source or policy manifest. No caller
/// metadata is copied into the failure; the verified image retains the identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ContextDependency {
    SourceRevision { manifest_index: usize },
    ConversionPolicy { manifest_index: usize },
    AppliedConversionPolicy { manifest_index: usize },
    ConstructorPolicy { manifest_index: usize },
}
#[derive(Clone, Debug)]
pub enum ContextValidationError {
    Dependency(ContextDependency),
    Invalid(IrError),
}

impl std::fmt::Display for ContextValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dependency(cause) => write!(
                f,
                "Required executable dependency is unavailable: {cause:?}"
            ),
            Self::Invalid(error) => std::fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for ContextValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Dependency(_) => None,
        }
    }
}

/// A verified executable as saved by a host image cache: the program and
/// the facts its verification derived.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedExecutable {
    program: Program,
    effects: Vec<Effect>,
    relations: Vec<(TypeId, TypeId)>,
    enums: Vec<(DeclarationId, crate::definitions::enums::Enumeration)>,
    context: String,
    limits: Limits,
}

/// Private construction after source, type and control-flow verification.
#[derive(Clone, Debug)]
pub struct VerifiedExecutable {
    program: Program,
    effects: Vec<Effect>,
    relations: BTreeSet<(TypeId, TypeId)>,
    enums: BTreeMap<DeclarationId, crate::definitions::enums::Enumeration>,
    context: String,
    limits: Limits,
}
impl VerifiedExecutable {
    /// The image and its verification facts, for a host image cache.
    pub fn save(&self) -> SavedExecutable {
        SavedExecutable {
            program: self.program.clone(),
            effects: self.effects.clone(),
            relations: self.relations.iter().copied().collect(),
            enums: self
                .enums
                .iter()
                .map(|(id, e)| (id.clone(), e.clone()))
                .collect(),
            context: self.context.clone(),
            limits: self.limits.clone(),
        }
    }
    /// Accept an image that this compiler verified and a host saved, without
    /// verifying it again. The caller vouches for its origin: only a trusted
    /// host cache of this compiler's own output may be loaded, never model or
    /// network input. The image must name exactly `bundle` (its context hash
    /// is recomputed from the bundle), and the program and derived facts must
    /// be structurally consistent; types and control flow are not rechecked.
    pub fn load_trusted(
        saved: SavedExecutable,
        bundle: &DefinitionBundle,
    ) -> Result<Self, IrError> {
        Self::load_trusted_in(saved, &mut SavedContext::new(bundle))
    }
    /// `load_trusted` against a bundle context shared by many images (as
    /// `rcore_scope::load_bindings` loads them): the same checks and errors,
    /// but the bundle's context hash is computed once per distinct `Limits`
    /// for the whole batch rather than once per image.
    pub(crate) fn load_trusted_in(
        saved: SavedExecutable,
        context: &mut SavedContext<'_>,
    ) -> Result<Self, IrError> {
        verification::check_saved_with_context(
            &saved.program,
            &saved.effects,
            &saved.context,
            &saved.limits,
            context,
        )?;
        Ok(Self {
            program: saved.program,
            effects: saved.effects,
            relations: saved.relations.into_iter().collect(),
            enums: saved.enums.into_iter().collect(),
            context: saved.context,
            limits: saved.limits,
        })
    }
    /// Derived from verified instructions; never accepted from serialized claims
    /// except through `load_trusted`.
    /// Entries follow the program's function order.
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }
    pub fn function_effect(&self, function: FunctionId) -> Result<Effect, IrError> {
        self.effects
            .get(function.0 as usize)
            .copied()
            .ok_or_else(|| invalid("Unknown function effect", None))
    }
    pub fn program(&self) -> &Program {
        &self.program
    }
    pub fn type_node(&self, id: TypeId) -> Result<&TypeNode, IrError> {
        self.program
            .types
            .get(id.0 as usize)
            .ok_or_else(|| invalid("Unknown executable type", None))
    }
    /// Validate an enum leaf against the exact domain retained by verification.
    pub fn validate_enum(
        &self,
        ty: TypeId,
        value: &crate::definitions::enums::EnumValue,
    ) -> Result<(), IrError> {
        let TypeNode::Enum(id) = self.type_node(ty)? else {
            return Err(invalid("Enum value requires an enum type", None));
        };
        self.enums
            .get(id)
            .ok_or_else(|| invalid("Missing checked enum domain", None))?
            .validate_value(value)
            .map(|_| ())
            .map_err(|e| invalid(e, None))
    }
    pub fn permits(&self, actual: TypeId, expected: TypeId) -> Result<bool, IrError> {
        self.type_node(actual)?;
        self.type_node(expected)?;
        Ok(actual == expected || self.relations.contains(&(actual, expected)))
    }
    pub fn check_context(
        &self,
        bundle: &DefinitionBundle,
        registry: Option<&ConversionRegistry<'_>>,
    ) -> Result<(), IrError> {
        if verification::context_hash(bundle, registry, &self.limits)? != self.context {
            return Err(invalid(
                "Runtime source or policy context differs from verified image",
                None,
            ));
        }
        Ok(())
    }
    /// Typed dependency diagnosis followed by the same exact context validation.
    /// `check_context` remains available for compatibility with IrError callers.
    pub fn check_context_detailed(
        &self,
        bundle: &DefinitionBundle,
        registry: Option<&ConversionRegistry<'_>>,
    ) -> Result<(), ContextValidationError> {
        use ContextValidationError::{Dependency, Invalid};
        ir::preflight_bundle(bundle, &self.limits.scalar).map_err(Invalid)?;
        let sources: BTreeSet<_> = bundle.sources.iter().map(|source| &source.id).collect();
        for (manifest_index, required) in self.program.sources.iter().enumerate() {
            if !sources.contains(&required.source) {
                return Err(Dependency(ContextDependency::SourceRevision {
                    manifest_index,
                }));
            }
        }
        let policies = policy_manifest(registry).map_err(Invalid)?;
        verification::serialized_size(&policies, self.limits.scalar.max_source_bytes)
            .map_err(Invalid)?;
        let mut work = self.limits.scalar.max_work;
        macro_rules! required {
            ($field:ident, $cause:ident) => {
                for (manifest_index, required) in self.program.policies.$field.iter().enumerate() {
                    spend(&mut work, policies.$field.len().saturating_add(1)).map_err(Invalid)?;
                    if !policies.$field.contains(required) {
                        return Err(Dependency(ContextDependency::$cause { manifest_index }));
                    }
                }
            };
        }
        required!(conversions, ConversionPolicy);
        required!(applied, AppliedConversionPolicy);
        required!(constructors, ConstructorPolicy);
        self.check_context(bundle, registry).map_err(Invalid)
    }
    pub fn inhabited(&self, root: TypeId) -> Result<bool, IrError> {
        self.type_node(root)?;
        let mut values = Vec::new();
        for node in self.program.types.iter().take(root.0 as usize + 1) {
            let child = |id: TypeId| {
                values
                    .get(id.0 as usize)
                    .copied()
                    .ok_or_else(|| invalid("Unverified type dependency", None))
            };
            let inhabited = match node {
                TypeNode::Enum(id) => !self
                    .enums
                    .get(id)
                    .ok_or_else(|| invalid("Missing enum domain", None))?
                    .literals()
                    .is_empty(),
                TypeNode::Tuple(items) => {
                    let mut yes = true;
                    for t in items {
                        yes &= child(*t)?;
                    }
                    yes
                }
                TypeNode::Record(fields) => {
                    let mut yes = true;
                    for t in fields.values() {
                        yes &= child(*t)?;
                    }
                    yes
                }
                TypeNode::Result { ok, error } => child(*ok)? || child(*error)?,
                TypeNode::Many { element, lower, .. } => *lower == 0 || child(*element)?,
                _ => true,
            };
            values.push(inhabited);
        }
        values
            .get(root.0 as usize)
            .copied()
            .ok_or_else(|| invalid("Missing inhabitance result", None))
    }
}
pub(crate) fn invalid(message: impl Into<String>, span: Option<&SourceSpan>) -> IrError {
    IrError {
        kind: IrErrorKind::Invalid,
        message: message.into(),
        span: span.cloned(),
    }
}
pub(crate) fn spend(left: &mut usize, count: usize) -> Result<(), IrError> {
    *left = left.checked_sub(count).ok_or_else(|| IrError {
        kind: IrErrorKind::ResourceExhausted,
        message: "Function IR work limit exceeded".into(),
        span: None,
    })?;
    Ok(())
}
fn canonical_wire<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Serialize,
{
    let wire = serde_json::Value::deserialize(deserializer)?;
    let value: T = serde_json::from_value(wire.clone()).map_err(serde::de::Error::custom)?;
    if serde_json::to_value(&value).map_err(serde::de::Error::custom)? != wire {
        return Err(serde::de::Error::custom(
            "Noncanonical or unknown nested identity metadata",
        ));
    }
    Ok(value)
}
