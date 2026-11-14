//! Handwritten safety-preserving lift of an existing verified scalar capability.
//! This authority has no bundle/provider context and is never a v2 source proof.
use super::*;

/// Non-wire inspection metadata for a scalar capability. Constants borrow their
/// original verified values; no executable proof can be deserialized here.
#[derive(Debug, PartialEq, Eq)]
pub struct ScalarProgram<'a> {
    pub entry_source: SourceId,
    pub sources: Vec<ir::SourceManifestEntry>,
    pub types: Vec<TypeNode>,
    pub functions: Vec<ScalarFunction<'a>>,
    pub entry: FunctionId,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ScalarFunction<'a> {
    pub identity: FunctionIdentity,
    pub parameters: Vec<TypeId>,
    pub result: TypeId,
    pub captures: Vec<Capture>,
    pub blocks: Vec<ScalarBlock<'a>>,
    pub span: SourceSpan,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ScalarBlock<'a> {
    pub parameters: Vec<TypeId>,
    pub instructions: Vec<ScalarInstruction<'a>>,
    pub terminator: Terminator,
    pub span: SourceSpan,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ScalarInstruction<'a> {
    pub ty: TypeId,
    pub kind: ScalarInstructionKind<'a>,
    pub span: SourceSpan,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ScalarInstructionKind<'a> {
    Constant(&'a ir::Value),
    Operation(InstructionKind),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ScalarKey {
    Scalar(Scalar),
    Enum(DeclarationId),
}
impl From<&ir::ValueType> for ScalarKey {
    fn from(value: &ir::ValueType) -> Self {
        match value {
            ir::ValueType::Scalar(scalar) => Self::Scalar(*scalar),
            ir::ValueType::Enum(id) => Self::Enum(id.clone()),
        }
    }
}

/// Execution authority inherited exclusively from an immutable v1 verifier result.
///
/// The normalized program is inspection data. This capability deliberately has no
/// Deserialize implementation or context certificate. Runtime compatibility must
/// validate/account original values lazily, rather than preparing v2 constants.
#[derive(Debug)]
pub struct VerifiedScalarExecutable<'a> {
    original: &'a ir::VerifiedProgram,
    program: ScalarProgram<'a>,
    value_types: Vec<ir::ValueType>,
    indices: BTreeMap<ScalarKey, TypeId>,
}

impl<'a> VerifiedScalarExecutable<'a> {
    pub fn original(&self) -> &'a ir::VerifiedProgram {
        self.original
    }
    /// The one-function normalized representation is not independently verified
    /// source replay and must not acquire a fabricated runtime context.
    pub fn program(&self) -> &ScalarProgram<'a> {
        &self.program
    }
    pub fn type_node(&self, id: TypeId) -> Result<&TypeNode, IrError> {
        self.program
            .types
            .get(id.0 as usize)
            .ok_or_else(|| invalid("Unknown lifted scalar type", None))
    }
    pub fn value_type(&self, id: TypeId) -> Result<&ir::ValueType, IrError> {
        self.value_types
            .get(id.0 as usize)
            .ok_or_else(|| invalid("Unknown lifted scalar type", None))
    }
    pub fn type_id(&self, ty: &ir::ValueType) -> Result<TypeId, IrError> {
        self.indices
            .get(&ScalarKey::from(ty))
            .copied()
            .ok_or_else(|| invalid("Type is not retained by the lifted scalar image", None))
    }
    pub fn permits(&self, actual: TypeId, expected: TypeId) -> Result<bool, IrError> {
        self.type_node(actual)?;
        self.type_node(expected)?;
        Ok(actual == expected)
    }
    pub fn validate_value(
        &self,
        value: &ir::Value,
        limits: &ir::ValueLimits,
    ) -> Result<(), String> {
        self.original.validate_value(value, limits)
    }
    pub fn validate_value_checked(
        &self,
        value: &ir::Value,
        limits: &ir::ValueLimits,
    ) -> Result<(), IrError> {
        self.original.validate_value_checked(value, limits)
    }
}

fn operand(value: ir::Operand) -> Operand {
    match value {
        ir::Operand::Parameter(index) => Operand::Parameter(index),
        ir::Operand::Register(index) => Operand::Register(index),
    }
}
fn edge(value: &ir::Edge) -> Edge {
    Edge {
        block: value.block,
        arguments: value.arguments.iter().copied().map(operand).collect(),
    }
}

/// Normalize only an already-verified scalar image. No additional default limits,
/// source checking, executable wire trust or fabricated context are introduced.
pub fn lift_scalar(
    original: &ir::VerifiedProgram,
) -> Result<VerifiedScalarExecutable<'_>, IrError> {
    let source = original.program();
    let span = source
        .blocks
        .first()
        .ok_or_else(|| invalid("Verified scalar image has no entry block", None))?
        .span
        .clone();
    let mut indices = BTreeMap::new();
    let mut value_types = Vec::new();
    let mut types = Vec::new();
    let mut intern = |ty: &ir::ValueType| -> Result<TypeId, IrError> {
        let key = ScalarKey::from(ty);
        if let Some(id) = indices.get(&key) {
            return Ok(*id);
        }
        let id = TypeId(
            u32::try_from(types.len())
                .map_err(|_| invalid("Scalar lift type index overflow", None))?,
        );
        types.push(match ty {
            ir::ValueType::Scalar(s) => TypeNode::Scalar(*s),
            ir::ValueType::Enum(id) => TypeNode::Enum(id.clone()),
        });
        value_types.push(ty.clone());
        indices.insert(key, id);
        Ok(id)
    };
    let parameters = source
        .parameters
        .iter()
        .map(&mut intern)
        .collect::<Result<Vec<_>, _>>()?;
    let result = intern(&source.result)?;
    let mut blocks = Vec::with_capacity(source.blocks.len());
    for block in &source.blocks {
        let parameters = block
            .parameters
            .iter()
            .map(&mut intern)
            .collect::<Result<Vec<_>, _>>()?;
        let mut instructions = Vec::with_capacity(block.instructions.len());
        for instruction in &block.instructions {
            let ty = intern(&instruction.ty)?;
            let kind = match &instruction.kind {
                ir::InstructionKind::Constant(value) => ScalarInstructionKind::Constant(value),
                ir::InstructionKind::Unary { op, value } => {
                    ScalarInstructionKind::Operation(InstructionKind::Unary {
                        op: *op,
                        value: operand(*value),
                    })
                }
                ir::InstructionKind::Binary { op, left, right } => {
                    ScalarInstructionKind::Operation(InstructionKind::Binary {
                        op: *op,
                        left: operand(*left),
                        right: operand(*right),
                    })
                }
            };
            instructions.push(ScalarInstruction {
                ty,
                kind,
                span: instruction.span.clone(),
            });
        }
        let terminator = match &block.terminator {
            ir::Terminator::Return(value) => Terminator::Return(operand(*value)),
            ir::Terminator::Jump(value) => Terminator::Jump(edge(value)),
            ir::Terminator::Branch {
                condition,
                then_edge,
                else_edge,
            } => Terminator::Branch {
                condition: operand(*condition),
                then_edge: edge(then_edge),
                else_edge: edge(else_edge),
            },
        };
        blocks.push(ScalarBlock {
            parameters,
            instructions,
            terminator,
            span: block.span.clone(),
        });
    }
    Ok(VerifiedScalarExecutable {
        original,
        value_types,
        indices,
        program: ScalarProgram {
            entry_source: source.symbol.source.clone(),
            sources: source.sources.clone(),
            types,
            functions: vec![ScalarFunction {
                identity: FunctionIdentity::Named {
                    symbol: source.symbol.clone(),
                    type_arguments: vec![],
                },
                parameters,
                result,
                captures: vec![],
                blocks,
                span,
            }],
            entry: FunctionId(0),
        },
    })
}
