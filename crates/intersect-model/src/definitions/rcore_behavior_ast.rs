//! Handwritten syntax-only arena: IDs are local to one retained document.
//! Flat arenas avoid recursive destruction of long, left-associated expressions.
use super::SyntaxDiagnostic;
use crate::definitions::bundle::{DefinitionSource, SourceSpan};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located<T> {
    pub value: T,
    pub span: SourceSpan,
}
pub type Name = Located<String>;
/// Parser expansions name builtins with this prefix so source bindings cannot
/// shadow them. No lexable name starts with `#`.
pub const BUILTIN_PREFIX: &str = "#builtin.";
/// Prefix of locals introduced by parser expansions.
pub const LOCAL_PREFIX: &str = "#local.";
pub type Path = Vec<Name>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExprId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternId(pub usize);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub source: DefinitionSource,
    pub syntax_version: Option<Name>,
    pub module: Option<Located<Module>>,
    pub declarations: Vec<Located<Declaration>>,
    pub expressions: Vec<Located<Expr>>,
    pub types: Vec<Located<Type>>,
    pub patterns: Vec<Located<Pattern>>,
    pub diagnostics: Vec<SyntaxDiagnostic>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub name: Path,
    pub version: Name,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: Name,
    pub ty: TypeId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generic {
    pub name: Name,
    pub constraint: Option<TypeId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub name: Name,
    pub ty: Option<TypeId>,
    pub value: ExprId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    pub condition: ExprId,
    pub code: Name,
    pub message: ExprId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Declaration {
    /// Select one source function as the name-resolution policy of a reference.
    ScopeBinding {
        target: Path,
        implementation: Path,
    },
    /// Select one source function that supplies a stored reference's value
    /// when the parsed model leaves it empty (`bind default`).
    DefaultBinding {
        target: Path,
        implementation: Path,
    },
    /// Select one constructing function whose objects the engine adds to a
    /// containment reference as implicit elements (`bind implicit`).
    ImplicitBinding {
        target: Path,
        implementation: Path,
    },
    /// Select one read-only function that checks every object of a class and
    /// returns the issues it finds (`bind validate`); `target` names the class.
    ValidateBinding {
        target: Path,
        implementation: Path,
    },
    /// Explicit host initializer contract; values are constant expressions only.
    Initialize {
        target: Path,
        provider: Name,
        version: Name,
        members: Vec<(Name, Option<ExprId>)>,
    },
    /// A derived feature backed by a stored feature with the same value contract.
    Projection {
        target: Path,
        input: Name,
        writable: bool,
    },
    /// Exact grammar declaration identity and installed conversion contract.
    RuleConversion(crate::definitions::rule_conversion::RuleConversion),
    /// Built-in model-rule contract, checked against the feature's declared type.
    DerivedContract {
        public: bool,
        target: Path,
        contract: DerivedContract,
    },
    ModelImport {
        namespace: Name,
        alias: Name,
    },
    ModuleImport {
        path: Path,
        version: Name,
        alias: Name,
    },
    Function {
        public: bool,
        /// A signature supplied by a host provider; `body` is an unused parser sentinel.
        #[serde(default)]
        external: bool,
        name: Name,
        generics: Vec<Generic>,
        parameters: Vec<Parameter>,
        result: TypeId,
        body: ExprId,
    },
    Derive {
        public: bool,
        target: Path,
        receiver: Name,
        result: TypeId,
        body: ExprId,
    },
    Validate {
        public: bool,
        target: Path,
        receiver: Name,
        requirements: Vec<Located<Requirement>>,
    },
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DerivedContract {
    Count(Name),
    NoIncoming(Path),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    /// Lambda parameter whose type must come from its checked call context.
    Inferred,
    Named {
        path: Path,
        arguments: Vec<TypeId>,
    },
    Optional(TypeId),
    /// Feature value contract, using the structural Rcore multiplicity syntax.
    Multiplicity {
        element: TypeId,
        lower: usize,
        upper: i32,
        ordered: bool,
        unique: bool,
    },
    Tuple(Vec<TypeId>),
    Record(Vec<(Name, TypeId)>),
    Function {
        parameters: Vec<TypeId>,
        result: TypeId,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Literal {
    Number(String),
    String(String),
    Boolean(bool),
    None,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    /// Parser-generated intrinsic for comprehension lowering; cannot be shadowed.
    ComprehensionIntrinsic {
        flat: bool,
    },
    Literal(Literal),
    Name(Name),
    Group(ExprId),
    List(Vec<ExprId>),
    Tuple(Vec<ExprId>),
    Record(Vec<(Name, ExprId)>),
    /// `new k.Class { feature: value, ... }`: a model object to construct.
    /// Only constructing functions (see `Effect::Construct`) may contain it.
    New {
        class: Path,
        fields: Vec<(Name, ExprId)>,
    },
    Unary {
        operator: Name,
        operand: ExprId,
    },
    Binary {
        operator: Name,
        left: ExprId,
        right: ExprId,
    },
    Member {
        receiver: ExprId,
        name: Name,
        safe: bool,
    },
    Call {
        callee: ExprId,
        types: Vec<TypeId>,
        arguments: Vec<ExprId>,
    },
    Index {
        receiver: ExprId,
        index: ExprId,
    },
    If {
        condition: ExprId,
        then_branch: ExprId,
        else_branch: ExprId,
    },
    Block {
        bindings: Vec<Located<Binding>>,
        result: ExprId,
    },
    Lambda {
        parameters: Vec<Parameter>,
        body: ExprId,
    },
    Match {
        value: ExprId,
        arms: Vec<(PatternId, ExprId)>,
    },
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pattern {
    Wildcard,
    Literal(Literal),
    Binding(Name),
    Constructor {
        path: Path,
        arguments: Vec<PatternId>,
    },
}
