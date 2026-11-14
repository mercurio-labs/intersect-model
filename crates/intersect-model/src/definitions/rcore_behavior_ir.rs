//! Versioned scalar IR. Deserialization is data, never executable proof.
use super::{modules::SymbolId, types::Scalar};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    resources::DeclarationId,
};
use serde::{Deserialize, Serialize};

#[path = "rcore_behavior_scalar_validation.rs"]
pub(crate) mod scalar_validation;
#[path = "rcore_behavior_ir_values.rs"]
mod values;
pub use values::{literal_value, validate_scalar_value};

pub const VERSION: u32 = 1;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueType {
    Scalar(Scalar),
    Enum(#[serde(deserialize_with = "deserialize_declaration_id")] DeclarationId),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeclarationId {
    source: SourceId,
    path: Vec<usize>,
}
fn deserialize_declaration_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<DeclarationId, D::Error> {
    let id = StrictDeclarationId::deserialize(deserializer)?;
    Ok(DeclarationId {
        source: id.source,
        path: id.path,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictSymbolId {
    source: SourceId,
    declaration: usize,
}
/// Strict scalar-IR selector adapter; legacy SymbolId wire behavior is unchanged.
fn deserialize_symbol_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SymbolId, D::Error> {
    let id = StrictSymbolId::deserialize(deserializer)?;
    Ok(SymbolId {
        source: id.source,
        declaration: id.declaration,
    })
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Value<Payload = serde_json::Value> {
    pub ty: ValueType,
    pub payload: Payload,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOp {
    Plus,
    Negate,
    Not,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operand {
    Parameter(u32),
    Register(u32),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum InstructionKind {
    Constant(Value),
    Unary {
        op: UnaryOp,
        value: Operand,
    },
    Binary {
        op: BinaryOp,
        left: Operand,
        right: Operand,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    pub ty: ValueType,
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
pub enum Terminator {
    Return(Operand),
    Jump(Edge),
    Branch {
        condition: Operand,
        then_edge: Edge,
        else_edge: Edge,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub parameters: Vec<ValueType>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceManifestEntry {
    pub source: SourceId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub version: u32,
    pub sources: Vec<SourceManifestEntry>,
    #[serde(deserialize_with = "deserialize_symbol_id")]
    pub symbol: SymbolId,
    pub parameters: Vec<ValueType>,
    pub result: ValueType,
    pub blocks: Vec<Block>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueLimits {
    pub max_value_bytes: usize,
    pub max_digits: usize,
}
impl Default for ValueLimits {
    fn default() -> Self {
        Self {
            max_value_bytes: 262_144,
            max_digits: 4096,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationLimits {
    pub max_source_bytes: usize,
    pub max_blocks: usize,
    pub max_instructions: usize,
    pub max_parameters: usize,
    pub max_total_value_bytes: usize,
    pub max_work: usize,
    pub values: ValueLimits,
}
impl Default for VerificationLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 4_194_304,
            max_blocks: 4096,
            max_instructions: 65_536,
            max_parameters: 4096,
            max_total_value_bytes: 1_048_576,
            max_work: 1_048_576,
            values: ValueLimits::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrErrorKind {
    Invalid,
    Unsupported,
    ResourceExhausted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrError {
    pub kind: IrErrorKind,
    pub message: String,
    pub span: Option<SourceSpan>,
}
impl std::fmt::Display for IrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for IrError {}

/// Only `verify` can construct this immutable execution capability.
#[derive(Clone, Debug)]
pub struct VerifiedProgram {
    program: Program,
    enums: std::collections::BTreeMap<DeclarationId, crate::definitions::enums::Enumeration>,
}
impl VerifiedProgram {
    pub fn program(&self) -> &Program {
        &self.program
    }
    pub fn validate_value(&self, value: &Value, limits: &ValueLimits) -> Result<(), String> {
        values::check_size(&value.payload, limits)?;
        match &value.ty {
            ValueType::Scalar(_) => validate_scalar_value(value, limits),
            ValueType::Enum(id) => {
                let enumeration = self
                    .enums
                    .get(id)
                    .ok_or("Enum is not retained by this verified program")?;
                let parsed: crate::definitions::enums::EnumValue =
                    serde_json::from_value(value.payload.clone()).map_err(|e| e.to_string())?;
                enumeration.validate_value(&parsed)?;
                if serde_json::to_value(&parsed).map_err(|e| e.to_string())? != value.payload {
                    return Err("Noncanonical enum payload or unknown identity field".into());
                }
                Ok(())
            }
        }
    }
    pub fn validate_value_checked(
        &self,
        value: &Value,
        limits: &ValueLimits,
    ) -> Result<(), IrError> {
        validate_value_resources(value, limits)?;
        self.validate_value(value, limits)
            .map_err(|message| invalid(message, None))
    }
}

/// Shared resource preflight before recursive codecs or arithmetic decoding.
pub fn validate_value_resources(value: &Value, limits: &ValueLimits) -> Result<(), IrError> {
    let mut cursor = scalar_validation::ScalarValidation::resources(value, limits);
    while !cursor.advance(limits).map_err(|e| e.into_ir())? {}
    Ok(())
}

fn invalid(message: impl Into<String>, span: Option<&SourceSpan>) -> IrError {
    IrError {
        kind: IrErrorKind::Invalid,
        message: message.into(),
        span: span.cloned(),
    }
}
fn charge(remaining: &mut usize, amount: usize) -> Result<(), IrError> {
    *remaining = remaining.checked_sub(amount).ok_or_else(|| IrError {
        kind: IrErrorKind::ResourceExhausted,
        message: "IR verification resource budget exceeded".into(),
        span: None,
    })?;
    Ok(())
}
struct MetadataBudget {
    bytes: usize,
    work: usize,
}
impl MetadataBudget {
    fn new(limits: &VerificationLimits) -> Self {
        Self {
            bytes: limits.max_source_bytes,
            work: limits.max_work,
        }
    }
    fn source(&mut self, source: &SourceId) -> Result<(), IrError> {
        charge(&mut self.work, 1)?;
        charge(&mut self.bytes, source.uri.len())?;
        charge(&mut self.bytes, source.sha256.len())
    }
    fn declaration(&mut self, id: &DeclarationId) -> Result<(), IrError> {
        self.source(&id.source)?;
        charge(&mut self.work, id.path.len())?;
        // Fixed accounting unit, independent of native versus wasm pointer width.
        charge(
            &mut self.bytes,
            id.path
                .len()
                .checked_mul(8)
                .ok_or_else(|| invalid("Declaration path size overflow", None))?,
        )
    }
    fn ty(&mut self, ty: &ValueType) -> Result<(), IrError> {
        charge(&mut self.work, 1)?;
        if let ValueType::Enum(id) = ty {
            self.declaration(id)?;
        }
        Ok(())
    }
    fn bundle(&mut self, bundle: &DefinitionBundle) -> Result<(), IrError> {
        charge(&mut self.work, bundle.sources.len())?;
        charge(&mut self.work, bundle.namespaces.len())?;
        for source in &bundle.sources {
            charge(&mut self.bytes, source.text.len())?;
            self.source(&source.id)?;
            charge(&mut self.work, source.dependencies.len())?;
            for dependency in &source.dependencies {
                self.source(dependency)?;
            }
        }
        for namespace in &bundle.namespaces {
            charge(&mut self.bytes, namespace.namespace.len())?;
            self.source(&namespace.source)?;
        }
        if let Some(source) = bundle
            .sources
            .iter()
            .find(|source| source.kind == crate::definitions::bundle::SourceKind::Ecore)
        {
            return Err(IrError {
                kind: IrErrorKind::Unsupported,
                message: "Ecore sources are migration inputs; migrate the complete bundle before compiling or verifying behavior".into(),
                span: Some(SourceSpan { source: source.id.clone(), start: 0, end: source.text.len() }),
            });
        }
        Ok(())
    }
    fn program(&mut self, program: &Program, limits: &VerificationLimits) -> Result<(), IrError> {
        charge(&mut self.work, program.sources.len())?;
        for source in &program.sources {
            self.source(&source.source)?;
        }
        self.source(&program.symbol.source)?;
        self.ty(&program.result)?;
        for ty in &program.parameters {
            self.ty(ty)?;
        }
        for block in &program.blocks {
            self.source(&block.span.source)?;
            for ty in &block.parameters {
                self.ty(ty)?;
            }
            for instruction in &block.instructions {
                self.source(&instruction.span.source)?;
                self.ty(&instruction.ty)?;
                if let InstructionKind::Constant(value) = &instruction.kind {
                    self.ty(&value.ty)?;
                    validate_value_resources(value, &limits.values)?;
                }
            }
            match &block.terminator {
                Terminator::Return(_) => {}
                Terminator::Jump(edge) => charge(&mut self.work, edge.arguments.len())?,
                Terminator::Branch {
                    then_edge,
                    else_edge,
                    ..
                } => {
                    charge(&mut self.work, then_edge.arguments.len())?;
                    charge(&mut self.work, else_edge.arguments.len())?;
                }
            }
        }
        Ok(())
    }
}
/// Bound complete source metadata before cloning, indexing or digest validation.
/// Require canonical executable sources after bounding metadata. This does not
/// establish source validity or source-body proof.
pub fn preflight_bundle(
    bundle: &DefinitionBundle,
    limits: &VerificationLimits,
) -> Result<(), IrError> {
    MetadataBudget::new(limits).bundle(bundle)
}
fn source_type(
    table: &super::types::TypeTable,
    ty: super::types::TypeRef,
) -> Result<ValueType, IrError> {
    match table.get(ty).map_err(|e| invalid(e, None))? {
        super::types::Type::Scalar(scalar) => Ok(ValueType::Scalar(*scalar)),
        super::types::Type::Enumeration(id) => Ok(ValueType::Enum(id.clone())),
        _ => Err(IrError {
            kind: IrErrorKind::Unsupported,
            message: "Scalar IR requires scalar or enum signature types".into(),
            span: None,
        }),
    }
}
/// Rechecks IR safety and source/signature identities. Does not certify that the
/// instructions were lowered from the selected source body.
pub fn verify(
    program: Program,
    bundle: &DefinitionBundle,
    limits: &VerificationLimits,
) -> Result<VerifiedProgram, IrError> {
    let mut initial_blocks = limits.max_blocks;
    let mut initial_instructions = limits.max_instructions;
    let mut initial_parameters = limits.max_parameters;
    charge(&mut initial_blocks, program.blocks.len())?;
    charge(&mut initial_parameters, program.parameters.len())?;
    for block in &program.blocks {
        charge(&mut initial_instructions, block.instructions.len())?;
        charge(&mut initial_parameters, block.parameters.len())?;
    }
    let mut metadata = MetadataBudget::new(limits);
    metadata.bundle(bundle)?;
    metadata.program(&program, limits)?;
    let mut work = metadata.work;
    bundle.validate().map_err(|e| invalid(e, None))?;
    if program.version != VERSION {
        return Err(IrError {
            kind: IrErrorKind::Unsupported,
            message: "Unsupported scalar IR version".into(),
            span: None,
        });
    }
    let mut expected: Vec<_> = bundle
        .sources
        .iter()
        .map(|s| SourceManifestEntry {
            source: s.id.clone(),
        })
        .collect();
    expected.sort_by(|a, b| a.source.cmp(&b.source));
    if program.sources != expected {
        return Err(invalid(
            "IR source manifest must exactly match the canonical selected bundle",
            None,
        ));
    }
    let signatures =
        super::signatures::index(bundle, &program.symbol.source).map_err(|errors| {
            errors
                .first()
                .map(|e| invalid(e.message.clone(), Some(&e.span)))
                .unwrap_or_else(|| invalid("Invalid source signature context", None))
        })?;
    let signature = signatures
        .functions
        .iter()
        .find(|s| s.symbol == program.symbol)
        .ok_or_else(|| invalid("Unknown selected function symbol", None))?;
    if !signature.generics.is_empty() {
        return Err(invalid(
            "Scalar IR entry cannot have generic parameters",
            None,
        ));
    }
    let parameters = signature
        .parameters
        .iter()
        .map(|ty| source_type(&signatures.types, *ty))
        .collect::<Result<Vec<_>, _>>()?;
    if parameters != program.parameters
        || source_type(&signatures.types, signature.result)? != program.result
    {
        return Err(invalid(
            "IR signature does not match the exact source function",
            None,
        ));
    }
    let mut blocks_left = limits.max_blocks;
    charge(&mut blocks_left, program.blocks.len())?;
    let entry = program
        .blocks
        .first()
        .ok_or_else(|| invalid("IR requires an entry block", None))?;
    if entry.parameters != program.parameters {
        return Err(invalid(
            "Entry block parameters differ from signature",
            Some(&entry.span),
        ));
    }
    let mut parameters_left = limits.max_parameters;
    let mut instructions_left = limits.max_instructions;
    let mut total_values = limits.max_total_value_bytes;
    let mut enum_ids = std::collections::BTreeSet::new();
    let mut collect_type = |ty: &ValueType| {
        if let ValueType::Enum(id) = ty {
            enum_ids.insert(id.clone());
        }
    };
    collect_type(&program.result);
    for ty in &program.parameters {
        collect_type(ty);
    }
    for block in &program.blocks {
        charge(&mut parameters_left, block.parameters.len())?;
        charge(&mut instructions_left, block.instructions.len())?;
        charge(&mut work, block.parameters.len())?;
        for ty in &block.parameters {
            collect_type(ty);
        }
        for instruction in &block.instructions {
            collect_type(&instruction.ty);
        }
    }
    let mut enums = std::collections::BTreeMap::new();
    for id in enum_ids {
        charge(&mut work, 1)?;
        let enumeration = crate::definitions::compiler::import_enum_bundle(bundle, &id)
            .map_err(|e| invalid(e.message, Some(&e.span)))?;
        enums.insert(id, enumeration);
    }
    let verified = VerifiedProgram { program, enums };
    let program = &verified.program;
    let check_span = |span: &SourceSpan| -> Result<(), IrError> {
        let source = bundle
            .sources
            .iter()
            .find(|s| s.id == span.source)
            .ok_or_else(|| invalid("Unknown IR span source", Some(span)))?;
        if span.start > span.end
            || span.end > source.text.len()
            || !source.text.is_char_boundary(span.start)
            || !source.text.is_char_boundary(span.end)
        {
            return Err(invalid("Invalid IR source span", Some(span)));
        }
        Ok(())
    };
    let mut reached = vec![false; program.blocks.len()];
    reached[0] = true;
    for (index, block) in program.blocks.iter().enumerate() {
        charge(&mut work, 1)?;
        if !reached[index] {
            return Err(invalid("Unreachable IR block", Some(&block.span)));
        }
        check_span(&block.span)?;
        for (position, instruction) in block.instructions.iter().enumerate() {
            charge(&mut work, 1)?;
            check_span(&instruction.span)?;
            let operand = |operand: Operand| {
                operand_type(block, position, operand)
                    .map_err(|e| invalid(e, Some(&instruction.span)))
            };
            let result = match &instruction.kind {
                InstructionKind::Constant(value) => {
                    verified
                        .validate_value_checked(value, &limits.values)
                        .map_err(|mut error| {
                            error.span = Some(instruction.span.clone());
                            error
                        })?;
                    let bytes = serde_json::to_vec(&value.payload)
                        .map_err(|e| invalid(e.to_string(), Some(&instruction.span)))?
                        .len();
                    charge(&mut total_values, bytes)?;
                    value.ty.clone()
                }
                InstructionKind::Unary { op, value } => unary_type(*op, operand(*value)?)
                    .map_err(|e| invalid(e, Some(&instruction.span)))?,
                InstructionKind::Binary { op, left, right } => {
                    binary_type(*op, operand(*left)?, operand(*right)?)
                        .map_err(|e| invalid(e, Some(&instruction.span)))?
                }
            };
            if result != instruction.ty {
                return Err(invalid(
                    "Instruction result type mismatch",
                    Some(&instruction.span),
                ));
            }
        }
        let operand = |operand: Operand| {
            operand_type(block, block.instructions.len(), operand)
                .map_err(|e| invalid(e, Some(&block.span)))
        };
        let mut edge = |edge: &Edge| -> Result<(), IrError> {
            charge(&mut work, edge.arguments.len().saturating_add(1))?;
            let target = usize::try_from(edge.block)
                .map_err(|_| invalid("Block index overflow", Some(&block.span)))?;
            if target <= index {
                return Err(invalid("IR edges must be forward", Some(&block.span)));
            }
            let destination = program
                .blocks
                .get(target)
                .ok_or_else(|| invalid("Unknown target block", Some(&block.span)))?;
            if destination.parameters.len() != edge.arguments.len() {
                return Err(invalid("Edge argument arity mismatch", Some(&block.span)));
            }
            for (actual, expected) in edge.arguments.iter().zip(&destination.parameters) {
                if operand(*actual)? != expected {
                    return Err(invalid("Edge argument type mismatch", Some(&block.span)));
                }
            }
            reached[target] = true;
            Ok(())
        };
        match &block.terminator {
            Terminator::Return(value) => {
                if operand(*value)? != &program.result {
                    return Err(invalid("Return type mismatch", Some(&block.span)));
                }
            }
            Terminator::Jump(target) => edge(target)?,
            Terminator::Branch {
                condition,
                then_edge,
                else_edge,
            } => {
                if operand(*condition)? != &ValueType::Scalar(Scalar::Bool) {
                    return Err(invalid("Branch requires Bool", Some(&block.span)));
                }
                edge(then_edge)?;
                edge(else_edge)?;
            }
        }
    }
    Ok(verified)
}
fn operand_type(block: &Block, before: usize, operand: Operand) -> Result<&ValueType, String> {
    match operand {
        Operand::Parameter(index) => block.parameters.get(index as usize),
        Operand::Register(index) if (index as usize) < before => {
            block.instructions.get(index as usize).map(|i| &i.ty)
        }
        Operand::Register(_) => None,
    }
    .ok_or_else(|| "Invalid or not-yet-defined local operand".into())
}
pub(crate) fn unary_type(op: UnaryOp, ty: &ValueType) -> Result<ValueType, String> {
    match (op, ty) {
        (UnaryOp::Not, ValueType::Scalar(Scalar::Bool)) => Ok(ty.clone()),
        (UnaryOp::Plus | UnaryOp::Negate, ValueType::Scalar(scalar)) if scalar.numeric() => {
            Ok(ty.clone())
        }
        _ => Err("Invalid unary operand type".into()),
    }
}
pub fn binary_type(op: BinaryOp, left: &ValueType, right: &ValueType) -> Result<ValueType, String> {
    if left != right {
        return Err("Binary operands require exact matching types".into());
    }
    match op {
        BinaryOp::Equal | BinaryOp::NotEqual => Ok(ValueType::Scalar(Scalar::Bool)),
        BinaryOp::Add if left == &ValueType::Scalar(Scalar::String) => Ok(left.clone()),
        _ => match left {
            ValueType::Scalar(scalar) if scalar.numeric() => match op {
                BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual => Ok(ValueType::Scalar(Scalar::Bool)),
                _ => Ok(left.clone()),
            },
            _ => Err("Numeric operator requires numeric operands".into()),
        },
    }
}
