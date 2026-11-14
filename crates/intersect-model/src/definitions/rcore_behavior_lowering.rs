//! Source-owned scalar lowering. Checked source is rebuilt for every public entry.
use super::{
    SyntaxDiagnostic,
    ast::{Declaration, Document, Expr, ExprId, Literal, Pattern, PatternId},
    bindings::{ModuleBindings, Target},
    checking::{self, ModuleReport, TypeReport},
    ir::{
        self, BinaryOp, Block, Edge, Instruction, InstructionKind, Operand, Program,
        SourceManifestEntry, Terminator, UnaryOp, Value, ValueType, VerificationLimits,
        VerifiedProgram,
    },
    modules::SymbolId,
    types::{Scalar, Type, TypeTable},
};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    conversion::ConversionRegistry,
};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, SyntaxDiagnostic>;

pub fn compile_scalar(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selected: &SymbolId,
    limits: &VerificationLimits,
) -> std::result::Result<VerifiedProgram, Vec<SyntaxDiagnostic>> {
    preflight(bundle, entry, selected, limits).map_err(|e| vec![e])?;
    let report = checking::check(bundle, entry)?;
    lower_checked(bundle, selected, &report, limits).map_err(|e| vec![e])
}

pub fn compile_scalar_with_conversions(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selected: &SymbolId,
    registry: &ConversionRegistry<'_>,
    limits: &VerificationLimits,
) -> std::result::Result<VerifiedProgram, Vec<SyntaxDiagnostic>> {
    preflight(bundle, entry, selected, limits).map_err(|e| vec![e])?;
    let report = checking::check_with_conversions(bundle, entry, registry)?;
    lower_checked(bundle, selected, &report, limits).map_err(|e| vec![e])
}

fn diagnostic(span: &SourceSpan, message: impl Into<String>) -> SyntaxDiagnostic {
    SyntaxDiagnostic {
        span: span.clone(),
        message: message.into(),
    }
}
fn preflight(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    selected: &SymbolId,
    limits: &VerificationLimits,
) -> Result<()> {
    // A hostile external selector is not part of the bundle metadata budget.
    // Use an unlocated empty source identity rather than copying oversized input
    // into the diagnostic itself; normal bounded selectors retain exact identity.
    let bounded = |id: &SourceId| {
        id.uri
            .len()
            .checked_add(id.sha256.len())
            .is_some_and(|size| size <= limits.max_source_bytes)
    };
    let span = SourceSpan {
        source: if bounded(entry) {
            entry.clone()
        } else {
            SourceId {
                uri: String::new(),
                sha256: String::new(),
            }
        },
        start: 0,
        end: 0,
    };
    if !bounded(entry) || !bounded(&selected.source) {
        return Err(diagnostic(
            &span,
            "Scalar source selector metadata budget exceeded",
        ));
    }
    ir::preflight_bundle(bundle, limits)
        .map_err(|error| diagnostic(error.span.as_ref().unwrap_or(&span), error.message))
}

fn value_type(
    table: &TypeTable,
    ty: super::types::TypeRef,
) -> std::result::Result<ValueType, String> {
    match table.get(ty)? {
        Type::Scalar(scalar) => Ok(ValueType::Scalar(*scalar)),
        Type::Enumeration(id) => Ok(ValueType::Enum(id.clone())),
        _ => Err("S05 execution requires scalar or enum types".into()),
    }
}
fn lower_checked(
    bundle: &DefinitionBundle,
    selected: &SymbolId,
    report: &TypeReport,
    limits: &VerificationLimits,
) -> Result<VerifiedProgram> {
    let fallback = SourceSpan {
        source: selected.source.clone(),
        start: 0,
        end: 0,
    };
    let signatures = &report.signatures;
    let module_index = signatures
        .bound
        .index
        .modules
        .iter()
        .position(|m| m.document.source.id == selected.source)
        .ok_or_else(|| {
            diagnostic(
                &fallback,
                "Selected scalar function is not reachable from the entry source",
            )
        })?;
    let doc = &signatures.bound.index.modules[module_index].document;
    let declaration = doc
        .declarations
        .get(selected.declaration)
        .ok_or_else(|| diagnostic(&fallback, "Selected scalar declaration does not exist"))?;
    let Declaration::Function {
        parameters,
        body,
        external,
        ..
    } = &declaration.value
    else {
        return Err(diagnostic(
            &declaration.span,
            "Scalar execution entry must be a function",
        ));
    };
    if *external {
        return Err(diagnostic(
            &declaration.span,
            "External functions require a registered implementation",
        ));
    }
    let signature = signatures
        .functions
        .iter()
        .find(|f| &f.symbol == selected)
        .ok_or_else(|| {
            diagnostic(
                &declaration.span,
                "Missing checked scalar function signature",
            )
        })?;
    if !signature.generics.is_empty() {
        return Err(diagnostic(
            &declaration.span,
            "S05 execution does not specialize generic functions",
        ));
    }
    let types = signature
        .parameters
        .iter()
        .map(|ty| value_type(&signatures.types, *ty))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| diagnostic(&declaration.span, e))?;
    let result = value_type(&signatures.types, signature.result)
        .map_err(|e| diagnostic(&declaration.span, e))?;
    let typed = report
        .modules
        .iter()
        .find(|m| m.source == selected.source)
        .ok_or_else(|| {
            diagnostic(
                &declaration.span,
                "Missing checked source expression report",
            )
        })?;
    let bindings = &signatures.bound.bindings[module_index];
    let mut lower = Lower {
        doc,
        typed,
        bindings,
        table: &signatures.types,
        limits,
        remaining: limits.max_work,
        instructions: 0,
        blocks: Vec::new(),
        current: 0,
        values: Vec::new(),
        locals: BTreeMap::new(),
    };
    lower.start_block(types.clone(), &declaration.span)?;
    for (index, parameter) in parameters.iter().enumerate() {
        lower.locals.insert(parameter.name.span.start, index);
    }
    let value = lower.expression(*body, 0)?;
    lower.blocks[lower.current].terminator =
        Terminator::Return(lower.operand(value, &declaration.span)?);
    let mut sources: Vec<_> = bundle
        .sources
        .iter()
        .map(|s| SourceManifestEntry {
            source: s.id.clone(),
        })
        .collect();
    sources.sort_by(|a, b| a.source.cmp(&b.source));
    let program = Program {
        version: ir::VERSION,
        sources,
        symbol: selected.clone(),
        parameters: types,
        result,
        blocks: lower.blocks,
    };
    ir::verify(program, bundle, limits)
        .map_err(|e| diagnostic(e.span.as_ref().unwrap_or(&declaration.span), e.message))
}

struct Lower<'a> {
    doc: &'a Document,
    typed: &'a ModuleReport,
    bindings: &'a ModuleBindings,
    table: &'a TypeTable,
    limits: &'a VerificationLimits,
    remaining: usize,
    instructions: usize,
    blocks: Vec<Block>,
    current: usize,
    // Stable expression slots are rebound to block-local operands across edges.
    values: Vec<(ValueType, Operand)>,
    locals: BTreeMap<usize, usize>,
}
impl Lower<'_> {
    fn spend(&mut self, amount: usize, span: &SourceSpan) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or_else(|| diagnostic(span, "Scalar lowering work budget exceeded"))?;
        Ok(())
    }
    fn start_block(&mut self, parameters: Vec<ValueType>, span: &SourceSpan) -> Result<usize> {
        self.spend(parameters.len().saturating_add(1), span)?;
        if self.blocks.len() >= self.limits.max_blocks
            || parameters.len() > self.limits.max_parameters
        {
            return Err(diagnostic(
                span,
                "Scalar lowering block/parameter budget exceeded",
            ));
        }
        let mut values = Vec::new();
        for (index, ty) in parameters.iter().enumerate() {
            values.push((
                ty.clone(),
                Operand::Parameter(
                    u32::try_from(index).map_err(|_| {
                        diagnostic(span, "Scalar parameter index exceeds IR limits")
                    })?,
                ),
            ));
        }
        let block = self.blocks.len();
        self.blocks.push(Block {
            parameters,
            instructions: vec![],
            terminator: Terminator::Return(Operand::Parameter(0)),
            span: span.clone(),
        });
        self.current = block;
        self.values = values;
        Ok(block)
    }
    fn operand(&self, slot: usize, span: &SourceSpan) -> Result<Operand> {
        self.values
            .get(slot)
            .map(|v| v.1)
            .ok_or_else(|| diagnostic(span, "Missing lowered scalar value"))
    }
    fn emit(&mut self, ty: ValueType, kind: InstructionKind, span: &SourceSpan) -> Result<usize> {
        self.spend(1, span)?;
        if self.instructions >= self.limits.max_instructions {
            return Err(diagnostic(
                span,
                "Scalar lowering instruction budget exceeded",
            ));
        }
        let register = u32::try_from(self.blocks[self.current].instructions.len())
            .map_err(|_| diagnostic(span, "Scalar register index exceeds IR limits"))?;
        self.blocks[self.current].instructions.push(Instruction {
            ty: ty.clone(),
            kind,
            span: span.clone(),
        });
        self.instructions += 1;
        let slot = self.values.len();
        self.values.push((ty, Operand::Register(register)));
        Ok(slot)
    }
    fn constant(&mut self, value: Value, span: &SourceSpan) -> Result<usize> {
        self.emit(value.ty.clone(), InstructionKind::Constant(value), span)
    }
    fn literal(&mut self, ty: &ValueType, literal: &Literal, span: &SourceSpan) -> Result<usize> {
        let ValueType::Scalar(scalar) = ty else {
            return Err(diagnostic(span, "Scalar literal cannot construct an enum"));
        };
        let value = ir::literal_value(*scalar, literal, &self.limits.values)
            .map_err(|e| diagnostic(span, e))?;
        self.constant(value, span)
    }
    fn enum_value(&mut self, target: &Target, span: &SourceSpan) -> Result<usize> {
        let Target::EnumLiteral {
            enumeration,
            literal,
        } = target
        else {
            return Err(diagnostic(span, "Expected exact enum literal binding"));
        };
        let payload = serde_json::to_value(crate::definitions::enums::EnumValue::Enum {
            enumeration: enumeration.clone(),
            literal: literal.clone(),
        })
        .map_err(|e| diagnostic(span, e.to_string()))?;
        self.constant(
            Value {
                ty: ValueType::Enum(enumeration.clone()),
                payload,
            },
            span,
        )
    }
    fn expression(&mut self, id: ExprId, depth: usize) -> Result<usize> {
        let node = self
            .doc
            .expressions
            .get(id.0)
            .ok_or_else(|| {
                diagnostic(
                    &SourceSpan {
                        source: self.doc.source.id.clone(),
                        start: 0,
                        end: 0,
                    },
                    "Missing source expression",
                )
            })?
            .clone();
        self.spend(1, &node.span)?;
        if depth > 128 {
            return Err(diagnostic(
                &node.span,
                "Scalar lowering expression depth budget exceeded",
            ));
        }
        let ty = self
            .typed
            .expressions
            .get(&id.0)
            .ok_or_else(|| diagnostic(&node.span, "Missing checked scalar expression type"))?;
        let ty = value_type(self.table, *ty).map_err(|e| diagnostic(&node.span, e))?;
        if let Some(target @ Target::EnumLiteral { .. }) = self.bindings.expressions.get(&id.0) {
            return self.enum_value(&target.clone(), &node.span);
        }
        match node.value {
            Expr::Literal(literal) => self.literal(&ty, &literal, &node.span),
            Expr::Tuple(items) if items.is_empty() => self.constant(Value { ty, payload: serde_json::json!({"kind":"unit"}) }, &node.span),
            Expr::Name(_) => match self.bindings.expressions.get(&id.0) {
                Some(Target::Local { declaration, .. }) => self.locals.get(&declaration.start).copied().ok_or_else(|| diagnostic(&node.span, "S05 execution supports entry parameters and scalar pattern bindings only")),
                _ => Err(diagnostic(&node.span, "S05 execution does not load function or provider values")),
            },
            Expr::Group(inner) => self.expression(inner, depth + 1),
            Expr::Block { bindings, result } if bindings.is_empty() => self.expression(result, depth + 1),
            Expr::Unary { operator, operand } => {
                if matches!(operator.value.as_str(), "+" | "-") {
                    let mut inner = operand;
                    while let Expr::Group(child) = self.doc.expressions[inner.0].value.clone() { self.spend(1, &node.span)?; inner = child; }
                    if let Expr::Literal(Literal::Number(number)) = &self.doc.expressions[inner.0].value {
                        let number = if operator.value == "-" { format!("-{number}") } else { number.clone() };
                        return self.literal(&ty, &Literal::Number(number), &node.span);
                    }
                }
                let slot = self.expression(operand, depth + 1)?;
                let op = match operator.value.as_str() { "+" => UnaryOp::Plus, "-" => UnaryOp::Negate, "!" => UnaryOp::Not, _ => return Err(diagnostic(&operator.span, "Unsupported scalar unary operator")) };
                self.emit(ty, InstructionKind::Unary { op, value: self.operand(slot, &node.span)? }, &operator.span)
            }
            Expr::Binary { operator, left, right } => {
                let a = self.expression(left, depth + 1)?;
                if operator.value == "&&" || operator.value == "||" {
                    let and = operator.value == "&&";
                    return self.branch(a, ty, &node.span,
                        |this| if and { this.expression(right, depth + 1) } else { this.literal(&ValueType::Scalar(Scalar::Bool), &Literal::Boolean(true), &node.span) },
                        |this| if and { this.literal(&ValueType::Scalar(Scalar::Bool), &Literal::Boolean(false), &node.span) } else { this.expression(right, depth + 1) });
                }
                let b = self.expression(right, depth + 1)?;
                let op = binary(&operator.value).ok_or_else(|| diagnostic(&operator.span, "S05 scalar execution does not support this operator"))?;
                self.emit(ty, InstructionKind::Binary { op, left: self.operand(a, &node.span)?, right: self.operand(b, &node.span)? }, &operator.span)
            }
            Expr::If { condition, then_branch, else_branch } => {
                let condition = self.expression(condition, depth + 1)?;
                self.branch(condition, ty, &node.span, |this| this.expression(then_branch, depth + 1), |this| this.expression(else_branch, depth + 1))
            }
            Expr::Match { value, arms } => {
                let subject = self.expression(value, depth + 1)?;
                self.match_arms(subject, &arms, ty, &node.span, depth + 1)
            }
            _ => Err(diagnostic(&node.span, "S05 scalar lowering does not execute calls, let bindings, closures, structural values or model operations")),
        }
    }
    fn branch<F, G>(
        &mut self,
        condition: usize,
        result: ValueType,
        span: &SourceSpan,
        then_body: F,
        else_body: G,
    ) -> Result<usize>
    where
        F: FnOnce(&mut Self) -> Result<usize>,
        G: FnOnce(&mut Self) -> Result<usize>,
    {
        let parent = self.current;
        let condition = self.operand(condition, span)?;
        let baseline = self.values.clone();
        let parameters: Vec<_> = baseline.iter().map(|v| v.0.clone()).collect();
        let arguments: Vec<_> = baseline.iter().map(|v| v.1).collect();
        let then_block = self.start_block(parameters.clone(), span)?;
        let then_result = then_body(self)?;
        let then_end = self.current;
        let mut then_args: Vec<_> = self
            .values
            .iter()
            .take(baseline.len())
            .map(|v| v.1)
            .collect();
        then_args.push(self.operand(then_result, span)?);
        let else_block = self.start_block(parameters.clone(), span)?;
        let else_result = else_body(self)?;
        let else_end = self.current;
        let mut else_args: Vec<_> = self
            .values
            .iter()
            .take(baseline.len())
            .map(|v| v.1)
            .collect();
        else_args.push(self.operand(else_result, span)?);
        let mut joined = parameters;
        joined.push(result);
        let join = self.start_block(joined, span)?;
        let id = |index| {
            u32::try_from(index)
                .map_err(|_| diagnostic(span, "Scalar block index exceeds IR limits"))
        };
        self.blocks[parent].terminator = Terminator::Branch {
            condition,
            then_edge: Edge {
                block: id(then_block)?,
                arguments: arguments.clone(),
            },
            else_edge: Edge {
                block: id(else_block)?,
                arguments,
            },
        };
        self.blocks[then_end].terminator = Terminator::Jump(Edge {
            block: id(join)?,
            arguments: then_args,
        });
        self.blocks[else_end].terminator = Terminator::Jump(Edge {
            block: id(join)?,
            arguments: else_args,
        });
        Ok(baseline.len())
    }
    fn match_arms(
        &mut self,
        subject: usize,
        arms: &[(PatternId, ExprId)],
        result: ValueType,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        self.spend(1, span)?;
        if depth > 128 {
            return Err(diagnostic(
                span,
                "Scalar lowering match depth budget exceeded",
            ));
        }
        let (pattern, body) = arms
            .first()
            .ok_or_else(|| diagnostic(span, "Cannot execute an empty scalar match"))?;
        let node = self.doc.patterns[pattern.0].clone();
        let subject_type = self
            .values
            .get(subject)
            .ok_or_else(|| diagnostic(span, "Missing scalar match subject"))?
            .0
            .clone();
        let binding = match &node.value {
            Pattern::Binding(name) => Some(name.span.start),
            _ => None,
        };
        let arm = |this: &mut Self| {
            let previous = binding.and_then(|name| this.locals.insert(name, subject));
            let value = this.expression(*body, depth + 1);
            if let Some(name) = binding {
                if let Some(previous) = previous {
                    this.locals.insert(name, previous);
                } else {
                    this.locals.remove(&name);
                }
            }
            value
        };
        let literal = match &node.value {
            Pattern::Wildcard | Pattern::Binding(_) => return arm(self),
            Pattern::Literal(literal) => self.literal(&subject_type, literal, &node.span)?,
            Pattern::Constructor { arguments, .. } if arguments.is_empty() => {
                let target = self
                    .bindings
                    .constructors
                    .get(&pattern.0)
                    .ok_or_else(|| diagnostic(&node.span, "Missing exact scalar enum pattern"))?
                    .clone();
                self.enum_value(&target, &node.span)?
            }
            _ => {
                return Err(diagnostic(
                    &node.span,
                    "S05 matching requires scalar or enum patterns",
                ));
            }
        };
        // The fresh checker proved exhaustiveness; the last scalar/enum arm is total
        // over values left after previous tests, without an untyped trap terminator.
        if arms.len() == 1 {
            return arm(self);
        }
        let condition = self.emit(
            ValueType::Scalar(Scalar::Bool),
            InstructionKind::Binary {
                op: BinaryOp::Equal,
                left: self.operand(subject, span)?,
                right: self.operand(literal, span)?,
            },
            &node.span,
        )?;
        self.branch(condition, result.clone(), span, arm, |this| {
            this.match_arms(subject, &arms[1..], result, span, depth + 1)
        })
    }
}
fn binary(operator: &str) -> Option<BinaryOp> {
    Some(match operator {
        "+" => BinaryOp::Add,
        "-" => BinaryOp::Subtract,
        "*" => BinaryOp::Multiply,
        "/" => BinaryOp::Divide,
        "%" => BinaryOp::Remainder,
        "<" => BinaryOp::Less,
        "<=" => BinaryOp::LessEqual,
        ">" => BinaryOp::Greater,
        ">=" => BinaryOp::GreaterEqual,
        "==" => BinaryOp::Equal,
        "!=" => BinaryOp::NotEqual,
        _ => return None,
    })
}
