//! Immutable lexical values, closures and forward control-flow lowering.
use super::*;
use crate::definitions::rcore_behavior::ast::Literal;

enum Stop {
    Diagnostic(SyntaxDiagnostic),
    Diverged,
}
impl From<SyntaxDiagnostic> for Stop {
    fn from(error: SyntaxDiagnostic) -> Self {
        Self::Diagnostic(error)
    }
}
type Result<T> = std::result::Result<T, Stop>;
fn diagnostic(span: &SourceSpan, message: impl Into<String>) -> Stop {
    Stop::Diagnostic(super::diagnostic(span, message))
}

pub(super) fn lower(compiler: &mut Compiler<'_>, task: &Task) -> super::Result<Function> {
    if task.external {
        return Ok(Function {
            identity: task.identity.clone(),
            parameters: task
                .parameters
                .iter()
                .map(|(_, ty)| compiler.ty(*ty, &task.span))
                .collect::<super::Result<Vec<_>>>()?,
            result: compiler.ty(task.result, &task.span)?,
            captures: vec![],
            blocks: vec![],
            span: task.span.clone(),
        });
    }
    match lower_inner(compiler, task) {
        Ok(function) => Ok(function),
        Err(Stop::Diagnostic(error)) => Err(error),
        Err(Stop::Diverged) => Err(super::diagnostic(
            &task.span,
            "Unconsumed divergent source flow",
        )),
    }
}
fn lower_inner(compiler: &mut Compiler<'_>, task: &Task) -> Result<Function> {
    let mut body = Body {
        compiler,
        task,
        blocks: vec![],
        current: 0,
        values: vec![],
        locals: BTreeMap::new(),
    };
    body.start_block(
        task.parameters.iter().map(|(_, ty)| *ty).collect(),
        None,
        &task.span,
    )?;
    for (index, (span, _)) in task.parameters.iter().enumerate() {
        body.locals.insert((span.start, span.end), index);
    }
    for (index, (span, ty)) in task.captures.iter().enumerate() {
        let slot = body.values.len();
        body.values.push((
            *ty,
            Operand::Capture(
                u32::try_from(index)
                    .map_err(|_| diagnostic(span, "Capture index exceeds IR range"))?,
            ),
        ));
        body.locals.insert((span.start, span.end), slot);
    }
    body.audit_source()?;
    match body.expression(task.body, 0) {
        Ok(result) => {
            let result = body.view(result, task.result, &task.span)?;
            body.blocks[body.current].terminator =
                Terminator::Return(body.operand(result, &task.span)?);
        }
        Err(Stop::Diverged) => {}
        Err(error) => return Err(error),
    }
    let parameters = task
        .parameters
        .iter()
        .map(|(_, ty)| body.compiler.ty(*ty, &task.span))
        .collect::<super::Result<Vec<_>>>()?;
    let result = body.compiler.ty(task.result, &task.span)?;
    let captures = task
        .captures
        .iter()
        .map(|(span, ty)| {
            Ok(Capture {
                declaration: span.clone(),
                ty: body.compiler.ty(*ty, span)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Function {
        identity: task.identity.clone(),
        parameters,
        result,
        captures,
        blocks: body.blocks,
        span: task.span.clone(),
    })
}
struct Body<'c, 'a> {
    compiler: &'c mut Compiler<'a>,
    task: &'c Task,
    blocks: Vec<Block>,
    current: usize,
    values: Vec<(TypeRef, Operand)>,
    locals: BTreeMap<(usize, usize), usize>,
}
#[derive(Clone)]
struct Row {
    pattern: Option<PatternId>,
    body: ExprId,
    bindings: Vec<(SourceSpan, usize)>,
}
#[derive(Clone, Copy)]
enum Split {
    Bool(usize),
    Option(usize, TypeRef),
    Result(usize, TypeRef, TypeRef),
}
enum Prepared {
    Value(usize),
    Type(TypeRef),
}
enum MatchPlan {
    Arm(Row),
    Option {
        inner: TypeRef,
        none: Vec<Row>,
        some: Vec<Row>,
    },
    Result {
        ok: TypeRef,
        error: TypeRef,
        good: Vec<Row>,
        bad: Vec<Row>,
    },
    Scalar {
        condition: usize,
        row: Row,
        rows: Vec<Row>,
    },
}
struct SplitFrame {
    split: Split,
    result: TypeRef,
    parent: usize,
    operand: Operand,
    types: Vec<TypeRef>,
    arguments: Vec<Operand>,
    locals: BTreeMap<(usize, usize), usize>,
    first_entry: usize,
    first_end: usize,
    first_arguments: Option<Vec<Operand>>,
    second_entry: usize,
}
impl Body<'_, '_> {
    fn audit_source(&mut self) -> Result<()> {
        // Inspect every lexical source expression before control-flow pruning.
        // Unsupported navigation cannot hide after an impossible match. Nested
        // lambdas are separately queued and audited as their own source functions.
        let mut pending = vec![self.task.body];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            let span = self.span(id);
            self.compiler.spend(1, &span)?;
            if !visited.insert(id.0) {
                continue;
            }
            let source = &self.compiler.report.signatures.bound.index.modules[self.task.module]
                .document
                .source
                .id;
            let report = self
                .compiler
                .report
                .modules
                .iter()
                .find(|m| &m.source == source)
                .ok_or_else(|| diagnostic(&span, "Missing source audit report"))?;
            let evidence = report.function_instantiations.get(&id.0).cloned();
            let member = self.member_contract(id, &span)?;
            if let Some(field) = &member
                && field.derived
            {
                self.compiler.source_getter(field, &span)?;
            }
            let member_value = member.as_ref().map(|field| field.value);
            if let Some(value) = member_value {
                let value = self
                    .compiler
                    .substitute(value, &self.task.substitution, &span)?;
                let value = match self
                    .compiler
                    .table
                    .get(value)
                    .map_err(|e| diagnostic(&span, e))?
                {
                    Type::Option(inner) | Type::Many { element: inner, .. } => *inner,
                    _ => value,
                };
                if !matches!(
                    self.compiler.table.get(value),
                    Ok(Type::Scalar(_)
                        | Type::Enumeration(_)
                        | Type::Datatype(_)
                        | Type::DatatypeApplication(_)
                        | Type::Model(_)
                        | Type::ModelApplication(_))
                ) {
                    return Err(diagnostic(&span, "Unsupported stored feature element type"));
                }
            }
            if let Some(evidence) = evidence {
                let actuals = evidence
                    .arguments
                    .iter()
                    .map(|ty| {
                        self.compiler
                            .substitute(*ty, &self.task.substitution, &span)
                    })
                    .collect::<super::Result<Vec<_>>>()?;
                self.compiler
                    .named_instance(&evidence.symbol, &actuals, &span)?;
                // Grouped generic callees are represented by their outer evidence;
                // visiting the inner unresolved symbol would discard its actuals.
                continue;
            }
            if matches!(self.node(id), Expr::Name(_) | Expr::Member { .. })
                && matches!(self.target(id), Some(Target::Overload(_)))
            {
                return Err(diagnostic(
                    &span,
                    "Overload value lacks fresh selected-function evidence",
                ));
            }
            if matches!(self.node(id), Expr::Lambda { .. }) {
                self.compiler.lambda_instance(self.task, id.0, &span)?;
                continue;
            }
            if matches!(self.node(id), Expr::Name(_) | Expr::Member { .. })
                && let Some(Target::Symbol(symbol)) = self.target(id)
            {
                self.compiler.named_instance(&symbol, &[], &span)?;
                continue;
            }
            pending.extend(
                context::expression_children(
                    &self.compiler.report.signatures.bound.index.modules[self.task.module].document,
                    id,
                )
                .map_err(|e| diagnostic(&span, e))?,
            );
        }
        Ok(())
    }
    fn member_contract(
        &mut self,
        id: ExprId,
        span: &SourceSpan,
    ) -> Result<Option<crate::definitions::rcore_behavior::model_types::FeatureType>> {
        let source = &self.compiler.report.signatures.bound.index.modules[self.task.module]
            .document
            .source
            .id;
        let typed = self
            .compiler
            .report
            .modules
            .iter()
            .find(|module| &module.source == source)
            .ok_or_else(|| diagnostic(span, "Missing checked member module"))?;
        if !typed.members.contains_key(&id.0) && !typed.symbolic_members.contains_key(&id.0) {
            return Ok(None);
        }
        let Expr::Member {
            receiver,
            name,
            safe,
        } = self.node(id)
        else {
            return Err(diagnostic(
                span,
                "Checked model member has no source member expression",
            ));
        };
        let mut receiver = self.expression_type(receiver, span)?;
        if safe {
            let Type::Option(inner) = self
                .compiler
                .table
                .get(receiver)
                .map_err(|error| diagnostic(span, error))?
            else {
                return Err(diagnostic(
                    span,
                    "Safe member requires an optional receiver",
                ));
            };
            receiver = *inner;
        }
        self.compiler
            .models
            .navigation_feature(&mut self.compiler.table, receiver, &name.value)
            .map(Some)
            .map_err(|error| diagnostic(span, error))
    }
    fn span(&self, id: ExprId) -> SourceSpan {
        self.compiler.report.signatures.bound.index.modules[self.task.module]
            .document
            .expressions[id.0]
            .span
            .clone()
    }
    fn expression_type(&mut self, id: ExprId, span: &SourceSpan) -> Result<TypeRef> {
        let source = &self.compiler.report.signatures.bound.index.modules[self.task.module]
            .document
            .source
            .id;
        let ty = *self
            .compiler
            .report
            .modules
            .iter()
            .find(|m| &m.source == source)
            .and_then(|m| m.expressions.get(&id.0))
            .ok_or_else(|| diagnostic(span, "Missing checked expression type"))?;
        Ok(self
            .compiler
            .substitute(ty, &self.task.substitution, span)?)
    }
    fn node(&self, id: ExprId) -> Expr {
        self.compiler.report.signatures.bound.index.modules[self.task.module]
            .document
            .expressions[id.0]
            .value
            .clone()
    }
    fn target(&self, id: ExprId) -> Option<Target> {
        self.compiler.report.signatures.bound.bindings[self.task.module]
            .expressions
            .get(&id.0)
            .cloned()
    }
    fn operand(&self, slot: usize, span: &SourceSpan) -> Result<Operand> {
        self.values
            .get(slot)
            .map(|(_, o)| *o)
            .ok_or_else(|| diagnostic(span, "Missing lowered value"))
    }
    fn value_type(&self, slot: usize, span: &SourceSpan) -> Result<TypeRef> {
        self.values
            .get(slot)
            .map(|(ty, _)| *ty)
            .ok_or_else(|| diagnostic(span, "Missing lowered value type"))
    }
    fn start_block(
        &mut self,
        base: Vec<TypeRef>,
        payload: Option<TypeRef>,
        span: &SourceSpan,
    ) -> Result<(usize, Option<usize>)> {
        self.compiler.spend(base.len().saturating_add(1), span)?;
        if self.blocks.len() >= self.compiler.limits.scalar.max_blocks
            || base.len() + usize::from(payload.is_some())
                > self.compiler.limits.scalar.max_parameters
        {
            return Err(diagnostic(span, "Function block/parameter limit exceeded"));
        }
        let mut parameters = Vec::new();
        if let Some(ty) = payload {
            parameters.push(self.compiler.ty(ty, span)?);
        }
        parameters.extend(
            base.iter()
                .map(|ty| self.compiler.ty(*ty, span))
                .collect::<super::Result<Vec<_>>>()?,
        );
        let shift = usize::from(payload.is_some());
        self.values = base
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                Ok((
                    *ty,
                    Operand::Parameter(
                        u32::try_from(i + shift)
                            .map_err(|_| diagnostic(span, "Parameter index exceeds IR range"))?,
                    ),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let payload_slot = payload.map(|ty| {
            let slot = self.values.len();
            self.values.push((ty, Operand::Parameter(0)));
            slot
        });
        let id = self.blocks.len();
        self.blocks.push(Block {
            parameters,
            instructions: vec![],
            terminator: Terminator::Return(Operand::Parameter(0)),
            span: span.clone(),
        });
        self.current = id;
        Ok((id, payload_slot))
    }
    fn emit(&mut self, ty: TypeRef, kind: InstructionKind, span: &SourceSpan) -> Result<usize> {
        self.compiler.spend(1, span)?;
        if self.blocks[self.current].instructions.len()
            >= self.compiler.limits.scalar.max_instructions
        {
            return Err(diagnostic(span, "Function instruction limit exceeded"));
        }
        let register = u32::try_from(self.blocks[self.current].instructions.len())
            .map_err(|_| diagnostic(span, "Instruction index exceeds IR range"))?;
        let wire = self.compiler.ty(ty, span)?;
        self.blocks[self.current].instructions.push(Instruction {
            ty: wire,
            kind,
            span: span.clone(),
        });
        let slot = self.values.len();
        self.values.push((ty, Operand::Register(register)));
        Ok(slot)
    }
    fn view(&mut self, slot: usize, expected: TypeRef, span: &SourceSpan) -> Result<usize> {
        let actual = self.value_type(slot, span)?;
        if actual == expected {
            return Ok(slot);
        }
        self.compiler.spend(1, span)?;
        if !self
            .compiler
            .models
            .assignable(&self.compiler.table, actual, expected)
            .map_err(|e| diagnostic(span, e))?
        {
            return Err(diagnostic(
                span,
                "Lowered value does not conform to the checked expected type",
            ));
        }
        self.emit(
            expected,
            InstructionKind::View {
                value: self.operand(slot, span)?,
            },
            span,
        )
    }
    fn literal(&mut self, ty: TypeRef, literal: &Literal, span: &SourceSpan) -> Result<usize> {
        if matches!(literal, Literal::None) {
            return self.emit(ty, InstructionKind::None, span);
        }
        let Type::Scalar(scalar) = self
            .compiler
            .table
            .get(ty)
            .map_err(|e| diagnostic(span, e))?
        else {
            return Err(diagnostic(span, "Literal requires scalar type"));
        };
        let value = ir::literal_value(*scalar, literal, &self.compiler.limits.scalar.values)
            .map_err(|e| diagnostic(span, e))?;
        let wire = self.compiler.ty(ty, span)?;
        self.emit(
            ty,
            InstructionKind::Constant(ValueWire {
                ty: wire,
                data: DataWire::Scalar(value),
            }),
            span,
        )
    }
    fn enum_literal(&mut self, target: Target, span: &SourceSpan) -> Result<usize> {
        let Target::EnumLiteral {
            enumeration,
            literal,
        } = target
        else {
            return Err(diagnostic(span, "Expected exact enum binding"));
        };
        let ty = self
            .compiler
            .table
            .intern(Type::Enumeration(enumeration.clone()))
            .map_err(|e| diagnostic(span, e))?;
        let wire = self.compiler.ty(ty, span)?;
        self.emit(
            ty,
            InstructionKind::Constant(ValueWire {
                ty: wire,
                data: DataWire::Enum(crate::definitions::enums::EnumValue::Enum {
                    enumeration,
                    literal,
                }),
            }),
            span,
        )
    }
    fn expression(&mut self, mut id: ExprId, mut depth: usize) -> Result<usize> {
        // Consume transparent wrappers without retaining recursive native frames.
        // The first block owns restoration across all nested lexical blocks.
        let mut saved = None;
        let result = loop {
            let node = self.node(id);
            if let Expr::Binary { operator, .. } = &node {
                break if matches!(operator.value.as_str(), "&&" | "||" | "??") {
                    self.lazy_binary(id, depth)
                } else {
                    self.strict_binary(id, depth)
                };
            }
            if !matches!(node, Expr::Group(_) | Expr::Block { .. }) {
                break self.expression_general(id, depth);
            }
            let span = self.span(id);
            match self.expression_prelude(id, depth, &span) {
                Ok(Prepared::Value(value)) => break Ok(value),
                Ok(Prepared::Type(_)) => {}
                Err(error) => break Err(error),
            }
            id = match node {
                Expr::Group(inner) => inner,
                node @ Expr::Block { .. } => {
                    if saved.is_none() {
                        saved = Some(self.locals.clone());
                    }
                    match self.prepare_block(node, &span, depth) {
                        Ok(inner) => inner,
                        Err(error) => break Err(error),
                    }
                }
                _ => break Err(diagnostic(&span, "Unexpected wrapper dispatch")),
            };
            depth += 1;
        };
        if let Some(saved) = saved {
            self.locals = saved;
        }
        result
    }
    // Explicit postorder traversal preserves left-to-right evaluation and source
    // depth/work limits without one large lowering stack frame per operator.
    fn strict_binary(&mut self, root: ExprId, depth: usize) -> Result<usize> {
        enum Pending {
            Visit(ExprId, usize),
            Right(ExprId, ExprId, TypeRef, usize),
            Emit(ExprId, TypeRef, usize),
        }
        let mut pending = vec![Pending::Visit(root, depth)];
        let mut returned = None;
        while let Some(next) = pending.pop() {
            match next {
                Pending::Visit(id, depth) => {
                    if let Expr::Binary {
                        operator,
                        left,
                        right,
                    } = self.node(id)
                        && !matches!(operator.value.as_str(), "&&" | "||" | "??")
                    {
                        let span = self.span(id);
                        self.compiler.spend(1, &span)?;
                        if depth > 128 {
                            return Err(diagnostic(&span, "Function source depth limit exceeded"));
                        }
                        let ty = self.expression_type(id, &span)?;
                        pending.push(Pending::Right(id, right, ty, depth));
                        pending.push(Pending::Visit(left, depth + 1));
                    } else {
                        returned = Some(self.expression(id, depth)?);
                    }
                }
                Pending::Right(id, right, ty, depth) => {
                    let lhs = returned
                        .take()
                        .ok_or_else(|| diagnostic(&self.span(id), "Missing binary left value"))?;
                    pending.push(Pending::Emit(id, ty, lhs));
                    pending.push(Pending::Visit(right, depth + 1));
                }
                Pending::Emit(id, ty, lhs) => {
                    let span = self.span(id);
                    let rhs = returned
                        .take()
                        .ok_or_else(|| diagnostic(&span, "Missing binary right value"))?;
                    let Expr::Binary { operator, .. } = self.node(id) else {
                        return Err(diagnostic(&span, "Missing binary expression"));
                    };
                    let op = binary(&operator.value)
                        .ok_or_else(|| diagnostic(&operator.span, "Unknown binary operation"))?;
                    let (lhs, rhs) = if matches!(op, ir::BinaryOp::Equal | ir::BinaryOp::NotEqual) {
                        let a = self.value_type(lhs, &span)?;
                        let b = self.value_type(rhs, &span)?;
                        let common = self
                            .compiler
                            .models
                            .join(&mut self.compiler.table, a, b)
                            .map_err(|e| diagnostic(&span, e))?;
                        (
                            self.view(lhs, common, &span)?,
                            self.view(rhs, common, &span)?,
                        )
                    } else {
                        (lhs, rhs)
                    };
                    returned = Some(self.emit(
                        ty,
                        InstructionKind::Binary {
                            op,
                            left: self.operand(lhs, &span)?,
                            right: self.operand(rhs, &span)?,
                        },
                        &operator.span,
                    )?);
                }
            }
        }
        returned.ok_or_else(|| diagnostic(&self.span(root), "Missing binary result"))
    }
    // Keep a left-associated lazy chain off the native call stack. Each right
    // operand still lowers inside the existing split, retaining short circuit
    // edges, lexical views and payload handling without eager evaluation.
    fn lazy_binary(&mut self, root: ExprId, mut depth: usize) -> Result<usize> {
        let mut pending = Vec::new();
        let mut id = root;
        while let Expr::Binary {
            operator,
            left,
            right,
        } = self.node(id)
            && matches!(operator.value.as_str(), "&&" | "||" | "??")
        {
            let span = self.span(id);
            self.compiler.spend(1, &span)?;
            if depth > 128 {
                return Err(diagnostic(&span, "Function source depth limit exceeded"));
            }
            let ty = self.expression_type(id, &span)?;
            pending.push((id, operator, right, ty, depth));
            id = left;
            depth += 1;
        }
        let mut lhs = self.expression(id, depth)?;
        while let Some((id, operator, right, ty, depth)) = pending.pop() {
            let span = self.span(id);
            lhs = match operator.value.as_str() {
                "&&" => self.split(
                    Split::Bool(lhs),
                    ty,
                    &span,
                    |this, _| this.expression(right, depth + 1),
                    |this, _| this.literal(ty, &Literal::Boolean(false), &span),
                ),
                "||" => self.split(
                    Split::Bool(lhs),
                    ty,
                    &span,
                    |this, _| this.literal(ty, &Literal::Boolean(true), &span),
                    |this, _| this.expression(right, depth + 1),
                ),
                "??" => {
                    let Type::Option(inner) = self
                        .compiler
                        .table
                        .get(self.value_type(lhs, &span)?)
                        .map_err(|e| diagnostic(&span, e))?
                        .clone()
                    else {
                        return Err(diagnostic(&span, "Coalesce requires Option"));
                    };
                    self.split(
                        Split::Option(lhs, inner),
                        ty,
                        &span,
                        |this, _| this.expression(right, depth + 1),
                        |_, payload| {
                            payload.ok_or_else(|| diagnostic(&span, "Missing coalesce payload"))
                        },
                    )
                }
                _ => Err(diagnostic(
                    &operator.span,
                    "Unexpected strict binary dispatch",
                )),
            }?;
        }
        Ok(lhs)
    }
    fn expression_prelude(
        &mut self,
        id: ExprId,
        depth: usize,
        span: &SourceSpan,
    ) -> Result<Prepared> {
        self.compiler.spend(1, span)?;
        if depth > 128 {
            return Err(diagnostic(span, "Function source depth limit exceeded"));
        }
        let source = &self.compiler.report.signatures.bound.index.modules[self.task.module]
            .document
            .source
            .id;
        let evidence = self
            .compiler
            .report
            .modules
            .iter()
            .find(|m| &m.source == source)
            .and_then(|m| m.function_instantiations.get(&id.0))
            .cloned();
        if let Some(evidence) = evidence {
            let args = evidence
                .arguments
                .iter()
                .map(|ty| self.compiler.substitute(*ty, &self.task.substitution, span))
                .collect::<super::Result<Vec<_>>>()?;
            let (function, ty) = self
                .compiler
                .named_instance(&evidence.symbol, &args, span)?;
            return self
                .emit(
                    ty,
                    InstructionKind::MakeClosure {
                        function,
                        captures: vec![],
                    },
                    span,
                )
                .map(Prepared::Value);
        }
        if matches!(self.node(id), Expr::Name(_) | Expr::Member { .. })
            && matches!(self.target(id), Some(Target::Overload(_)))
        {
            return Err(diagnostic(
                span,
                "Overload value lacks fresh selected-function evidence",
            ));
        }
        if let Some(target @ Target::EnumLiteral { .. }) = self.target(id) {
            return self.enum_literal(target, span).map(Prepared::Value);
        }
        if matches!(self.node(id), Expr::Name(_) | Expr::Member { .. })
            && let Some(Target::Symbol(symbol)) = self.target(id)
        {
            let (function, ty) = self.compiler.named_instance(&symbol, &[], span)?;
            return self
                .emit(
                    ty,
                    InstructionKind::MakeClosure {
                        function,
                        captures: vec![],
                    },
                    span,
                )
                .map(Prepared::Value);
        }
        self.expression_type(id, span).map(Prepared::Type)
    }
    fn expression_general(&mut self, id: ExprId, depth: usize) -> Result<usize> {
        let span = self.span(id);
        let ty = match self.expression_prelude(id, depth, &span)? {
            Prepared::Value(value) => return Ok(value),
            Prepared::Type(ty) => ty,
        };
        match self.node(id) {
            Expr::Literal(literal) => self.literal(ty, &literal, &span),
            Expr::Name(_) | Expr::ComprehensionIntrinsic { .. } => match self.target(id) {
                Some(Target::Local { declaration, .. }) => self
                    .locals
                    .get(&(declaration.start, declaration.end))
                    .copied()
                    .ok_or_else(|| {
                        diagnostic(
                            &span,
                            "Local value is unavailable in this lexical environment",
                        )
                    }),
                Some(Target::Builtin { name, .. }) if name == "None" => {
                    self.emit(ty, InstructionKind::None, &span)
                }
                _ => Err(diagnostic(
                    &span,
                    "Name does not identify an executable value",
                )),
            },
            Expr::Group(inner) => self.expression(inner, depth + 1),
            Expr::Block { .. } => Err(diagnostic(&span, "Unexpected block dispatch")),
            node @ Expr::Lambda { .. } => self.expression_lambda(node, span, id, ty, depth),
            node @ Expr::Call { .. } => self.expression_call(node, span, id, ty, depth),
            node @ Expr::Tuple(..) => self.expression_tuple(node, span, id, ty, depth),
            node @ Expr::Record(..) => self.expression_record(node, span, id, ty, depth),
            node @ Expr::New { .. } => self.expression_new(node, span, ty, depth),
            node @ Expr::List(..) => self.expression_list(node, span, id, ty, depth),
            node @ Expr::Index { .. } => self.expression_index(node, span, id, ty, depth),
            node @ Expr::Member { .. } => self.expression_member(node, span, id, ty, depth),
            node @ Expr::Unary { .. } => self.expression_unary(node, span, id, ty, depth),
            Expr::Binary { .. } => Err(diagnostic(&span, "Unexpected binary dispatch")),
            node @ Expr::If { .. } => self.expression_if(node, span, id, ty, depth),
            node @ Expr::Match { .. } => self.expression_match(node, span, id, ty, depth),
            Expr::Error => Err(diagnostic(&span, "Cannot lower recovered source syntax")),
        }
    }
    fn prepare_block(&mut self, node: Expr, span: &SourceSpan, depth: usize) -> Result<ExprId> {
        let Expr::Block { bindings, result } = node else {
            return Err(diagnostic(span, "Unexpected expression dispatch"));
        };

        for binding in bindings {
            let value = self.expression(binding.value.value, depth + 1)?;
            let value = if let Some(annotation) = binding.value.ty {
                let annotation = self.compiler.report.signatures.modules[self.task.module]
                    .annotations
                    .get(&annotation.0)
                    .copied()
                    .ok_or_else(|| diagnostic(&binding.span, "Missing checked local annotation"))?;
                let annotation =
                    self.compiler
                        .substitute(annotation, &self.task.substitution, &binding.span)?;
                self.view(value, annotation, &binding.span)?
            } else {
                value
            };
            self.locals.insert(
                (binding.value.name.span.start, binding.value.name.span.end),
                value,
            );
        }
        Ok(result)
    }

    fn expression_lambda(
        &mut self,
        node: Expr,
        span: SourceSpan,
        id: ExprId,
        _ty: TypeRef,
        _depth: usize,
    ) -> Result<usize> {
        let Expr::Lambda { .. } = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let (function, function_type, captures) =
            self.compiler.lambda_instance(self.task, id.0, &span)?;
        let operands = captures
            .iter()
            .map(|(decl, expected)| {
                let value = *self
                    .locals
                    .get(&(decl.start, decl.end))
                    .ok_or_else(|| diagnostic(decl, "Required lambda capture is unavailable"))?;
                let value = self.view(value, *expected, decl)?;
                self.operand(value, decl)
            })
            .collect::<Result<Vec<_>>>()?;
        self.emit(
            function_type,
            InstructionKind::MakeClosure {
                function,
                captures: operands,
            },
            &span,
        )
    }

    fn expression_call(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::Call {
            callee, arguments, ..
        } = node
        else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let mut target = callee;
        while let Expr::Group(inner) = self.node(target) {
            target = inner;
        }
        if let Some(Target::Builtin { name, .. }) = self.target(target) {
            return self.constructor(&name, &arguments, ty, &span, depth + 1);
        }
        let callable = self.expression(callee, depth + 1)?;
        let callable_type = self.value_type(callable, &span)?;
        let Type::Function { parameters, result } = self
            .compiler
            .table
            .get(callable_type)
            .map_err(|e| diagnostic(&span, e))?
            .clone()
        else {
            return Err(diagnostic(&span, "Checked callee is not callable"));
        };
        if arguments.len() != parameters.len() {
            return Err(diagnostic(&span, "Checked call arity mismatch"));
        }
        let mut values = vec![];
        for (argument, expected) in arguments.iter().zip(parameters) {
            let value = self.expression(*argument, depth + 1)?;
            values.push(self.view(value, expected, &self.span(*argument))?);
        }
        let operands = values
            .into_iter()
            .map(|v| self.operand(v, &span))
            .collect::<Result<Vec<_>>>()?;
        let value = self.emit(
            result,
            InstructionKind::Call {
                callee: self.operand(callable, &span)?,
                arguments: operands,
            },
            &span,
        )?;
        self.view(value, ty, &span)
    }

    fn expression_tuple(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::Tuple(items) = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        if items.is_empty() {
            let wire = self.compiler.ty(ty, &span)?;
            return self.emit(
                ty,
                InstructionKind::Constant(ValueWire {
                    ty: wire,
                    data: DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::Unit),
                        payload: serde_json::json!({"kind":"unit"}),
                    }),
                }),
                &span,
            );
        }
        let Type::Tuple(types) = self
            .compiler
            .table
            .get(ty)
            .map_err(|e| diagnostic(&span, e))?
            .clone()
        else {
            return Err(diagnostic(&span, "Missing tuple shape"));
        };
        let mut slots = vec![];
        for (item, expected) in items.into_iter().zip(types) {
            let value = self.expression(item, depth + 1)?;
            slots.push(self.view(value, expected, &span)?);
        }
        let values = slots
            .into_iter()
            .map(|v| self.operand(v, &span))
            .collect::<Result<Vec<_>>>()?;
        self.emit(ty, InstructionKind::Tuple(values), &span)
    }

    fn expression_record(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::Record(fields) = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let Type::Record(types) = self
            .compiler
            .table
            .get(ty)
            .map_err(|e| diagnostic(&span, e))?
            .clone()
        else {
            return Err(diagnostic(&span, "Missing record shape"));
        };
        let mut slots = BTreeMap::new();
        for (name, item) in fields {
            let value = self.expression(item, depth + 1)?;
            let expected = *types
                .get(&name.value)
                .ok_or_else(|| diagnostic(&name.span, "Missing checked record field"))?;
            slots.insert(name.value, self.view(value, expected, &span)?);
        }
        let values = slots
            .into_iter()
            .map(|(name, v)| Ok((name, self.operand(v, &span)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        self.emit(ty, InstructionKind::Record(values), &span)
    }

    fn expression_new(
        &mut self,
        node: Expr,
        span: SourceSpan,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::New { fields, .. } = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };
        let mut lowered = BTreeMap::new();
        for (name, item) in fields {
            let value = self.expression(item, depth + 1)?;
            let field = self
                .compiler
                .models
                .navigation_feature(&mut self.compiler.table, ty, &name.value)
                .map_err(|e| diagnostic(&name.span, e))?;
            if field.derived {
                return Err(diagnostic(&name.span, "`new` sets only stored features"));
            }
            // A plain value for an optional feature is wrapped in Some; a
            // many-valued feature holds a Sequence of its element type.
            let (target, optional) = match self.compiler.table.get(field.value) {
                Ok(Type::Option(inner)) => (field.value, Some(*inner)),
                Ok(Type::Many { element, .. }) => {
                    let element = *element;
                    let sequence = self
                        .compiler
                        .table
                        .intern(Type::Collection {
                            kind:
                                crate::definitions::rcore_behavior::types::CollectionKind::Sequence,
                            element,
                        })
                        .map_err(|e| diagnostic(&span, e))?;
                    (sequence, None)
                }
                _ => (field.value, None),
            };
            let actual = self.value_type(value, &span)?;
            let value = match optional {
                Some(inner) if !matches!(self.compiler.table.get(actual), Ok(Type::Option(_))) => {
                    let inner_value = self.view(value, inner, &span)?;
                    let operand = self.operand(inner_value, &span)?;
                    self.emit(field.value, InstructionKind::Some(operand), &span)?
                }
                _ => self.view(value, target, &span)?,
            };
            lowered.insert(
                name.value,
                NewField {
                    declaration: field.declaration,
                    value: self.operand(value, &span)?,
                },
            );
        }
        self.emit(ty, InstructionKind::New(lowered), &span)
    }

    fn expression_list(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::List(items) = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let Type::Collection { element, .. } = self
            .compiler
            .table
            .get(ty)
            .map_err(|e| diagnostic(&span, e))?
            .clone()
        else {
            return Err(diagnostic(&span, "Missing list element type"));
        };
        let mut slots = vec![];
        for item in items {
            let value = self.expression(item, depth + 1)?;
            slots.push(self.view(value, element, &span)?);
        }
        let values = slots
            .into_iter()
            .map(|v| self.operand(v, &span))
            .collect::<Result<Vec<_>>>()?;
        self.emit(ty, InstructionKind::Collection(values), &span)
    }

    fn expression_index(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::Index { receiver, index } = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let value = self.expression(receiver, depth + 1)?;
        let receiver_type = self.value_type(value, &span)?;
        if matches!(self.compiler.table.get(receiver_type), Ok(Type::Tuple(_))) {
            let mut index = index;
            while let Expr::Group(inner) = self.node(index) {
                index = inner;
            }
            let Expr::Literal(Literal::Number(number)) = self.node(index) else {
                return Err(diagnostic(&span, "Tuple index must be literal"));
            };
            let index = number
                .parse()
                .map_err(|_| diagnostic(&span, "Invalid tuple index"))?;
            self.emit(
                ty,
                InstructionKind::TupleField {
                    value: self.operand(value, &span)?,
                    index,
                },
                &span,
            )
        } else {
            let index = self.expression(index, depth + 1)?;
            let element = match self
                .compiler
                .table
                .get(receiver_type)
                .map_err(|e| diagnostic(&span, e))?
            {
                Type::Collection { element, .. } | Type::Many { element, .. } => *element,
                _ => {
                    return Err(diagnostic(
                        &span,
                        "Index receiver lacks a checked collection element",
                    ));
                }
            };
            let actual = self
                .compiler
                .table
                .intern(Type::Option(element))
                .map_err(|e| diagnostic(&span, e))?;
            let result = self.emit(
                actual,
                InstructionKind::Index {
                    value: self.operand(value, &span)?,
                    index: self.operand(index, &span)?,
                },
                &span,
            )?;
            self.view(result, ty, &span)
        }
    }

    fn expression_member(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::Member {
            receiver,
            name,
            safe,
        } = node
        else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let value = self.expression(receiver, depth + 1)?;
        let actual = self.value_type(value, &span)?;
        if safe {
            let Type::Option(inner) = self
                .compiler
                .table
                .get(actual)
                .map_err(|e| diagnostic(&span, e))?
                .clone()
            else {
                return Err(diagnostic(&span, "Safe member requires Option"));
            };
            self.split(
                Split::Option(value, inner),
                ty,
                &span,
                |this, _| this.emit(ty, InstructionKind::None, &span),
                |this, payload| {
                    let payload = payload
                        .ok_or_else(|| diagnostic(&span, "Missing checked optional payload"))?;
                    let field = this.record_field(payload, &name.value, &name.span)?;
                    if matches!(
                        this.compiler.table.get(this.value_type(field, &span)?),
                        Ok(Type::Option(_))
                    ) {
                        this.view(field, ty, &span)
                    } else {
                        let Type::Option(expected) = this
                            .compiler
                            .table
                            .get(ty)
                            .map_err(|e| diagnostic(&span, e))?
                            .clone()
                        else {
                            return Err(diagnostic(&span, "Missing safe field result type"));
                        };
                        let field = this.view(field, expected, &span)?;
                        this.emit(
                            ty,
                            InstructionKind::Some(this.operand(field, &span)?),
                            &span,
                        )
                    }
                },
            )
        } else {
            let field = self.record_field(value, &name.value, &name.span)?;
            self.view(field, ty, &span)
        }
    }

    fn expression_unary(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::Unary { operator, operand } = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        if matches!(operator.value.as_str(), "+" | "-") {
            let mut inner = operand;
            while let Expr::Group(child) = self.node(inner) {
                self.compiler.spend(1, &span)?;
                inner = child;
            }
            if let Expr::Literal(Literal::Number(number)) = self.node(inner) {
                let text = if operator.value == "-" {
                    format!("-{number}")
                } else {
                    number
                };
                return self.literal(ty, &Literal::Number(text), &span);
            }
        }
        let value = self.expression(operand, depth + 1)?;
        let op = match operator.value.as_str() {
            "+" => ir::UnaryOp::Plus,
            "-" => ir::UnaryOp::Negate,
            "!" => ir::UnaryOp::Not,
            _ => return Err(diagnostic(&operator.span, "Unknown unary operation")),
        };
        self.emit(
            ty,
            InstructionKind::Unary {
                op,
                value: self.operand(value, &span)?,
            },
            &operator.span,
        )
    }

    fn expression_if(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::If {
            condition,
            then_branch,
            else_branch,
        } = node
        else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let condition = self.expression(condition, depth + 1)?;
        self.split(
            Split::Bool(condition),
            ty,
            &span,
            |this, _| this.expression(then_branch, depth + 1),
            |this, _| this.expression(else_branch, depth + 1),
        )
    }

    fn expression_match(
        &mut self,
        node: Expr,
        span: SourceSpan,
        _id: ExprId,
        ty: TypeRef,
        depth: usize,
    ) -> Result<usize> {
        let Expr::Match { value, arms } = node else {
            return Err(diagnostic(&span, "Unexpected expression dispatch"));
        };

        let value = self.expression(value, depth + 1)?;
        let rows = arms
            .into_iter()
            .map(|(pattern, body)| Row {
                pattern: Some(pattern),
                body,
                bindings: vec![],
            })
            .collect();
        self.matches(value, rows, ty, &span, depth + 1)
    }
    fn record_field(&mut self, value: usize, name: &str, span: &SourceSpan) -> Result<usize> {
        let actual = self.value_type(value, span)?;
        if matches!(
            self.compiler.table.get(actual),
            Ok(Type::Model(_) | Type::ModelApplication(_))
        ) {
            let field = self
                .compiler
                .models
                .navigation_feature(&mut self.compiler.table, actual, name)
                .map_err(|e| diagnostic(span, e))?;
            if field.derived {
                let (function, callable_type) = self.compiler.source_getter(&field, span)?;
                let Type::Function { parameters, result } = self
                    .compiler
                    .table
                    .get(callable_type)
                    .map_err(|error| diagnostic(span, error))?
                    .clone()
                else {
                    return Err(diagnostic(span, "Source getter is not callable"));
                };
                let expected = *parameters
                    .first()
                    .ok_or_else(|| diagnostic(span, "Source getter lost its receiver"))?;
                let receiver = self.view(value, expected, span)?;
                let callable = self.emit(
                    callable_type,
                    InstructionKind::MakeClosure {
                        function,
                        captures: vec![],
                    },
                    span,
                )?;
                return self.emit(
                    result,
                    InstructionKind::Call {
                        callee: self.operand(callable, span)?,
                        arguments: vec![self.operand(receiver, span)?],
                    },
                    span,
                );
            }
            return self.emit(
                field.value,
                InstructionKind::StoredFeature {
                    value: self.operand(value, span)?,
                    field: name.into(),
                    declaration: field.declaration,
                },
                span,
            );
        }
        let Type::Record(fields) = self
            .compiler
            .table
            .get(actual)
            .map_err(|e| diagnostic(span, e))?
        else {
            return Err(diagnostic(
                span,
                "Model navigation is not executed before its scheduled milestone",
            ));
        };
        let ty = *fields
            .get(name)
            .ok_or_else(|| diagnostic(span, "Unknown checked record field"))?;
        self.emit(
            ty,
            InstructionKind::RecordField {
                value: self.operand(value, span)?,
                field: name.into(),
            },
            span,
        )
    }
    fn constructor(
        &mut self,
        name: &str,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        if let Some(operation) = crate::definitions::rcore_behavior::ir_v2::OwnershipOp::named(name)
        {
            return self.constructor_ownership(operation, arguments, ty, span, depth);
        }
        if name == "incoming" {
            return self.constructor_incoming(arguments, ty, span, depth);
        }
        if let Some(operation) = crate::definitions::rcore_behavior::ir_v2::AlgebraOp::named(name) {
            return self.constructor_algebra(operation, arguments, ty, span, depth);
        }
        if let Some(operation) =
            crate::definitions::rcore_behavior::ir_v2::MembershipOp::named(name)
        {
            return self.constructor_membership(operation, arguments, ty, span, depth);
        }
        if let Some(operation) =
            crate::definitions::rcore_behavior::ir_v2::CollectionOp::named(name)
        {
            return self.constructor_collection(operation, arguments, ty, span, depth);
        }
        self.constructor_value(name, arguments, ty, span, depth)
    }
    fn constructor_ownership(
        &mut self,
        operation: crate::definitions::rcore_behavior::ir_v2::OwnershipOp,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        let selected = match self
            .compiler
            .table
            .get(ty)
            .map_err(|e| diagnostic(span, e))?
        {
            Type::Option(inner) | Type::Collection { element: inner, .. } => *inner,
            _ => return Err(diagnostic(span, "Missing ownership result type")),
        };
        let value = self.expression(arguments[0], depth + 1)?;
        let selected = self.compiler.ty(selected, span)?;
        self.emit(
            ty,
            InstructionKind::Ownership {
                operation,
                value: self.operand(value, span)?,
                selected,
            },
            span,
        )
    }
    fn constructor_incoming(
        &mut self,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        let Type::Collection {
            kind: crate::definitions::rcore_behavior::types::CollectionKind::OrderedSet,
            element: source,
        } = self
            .compiler
            .table
            .get(ty)
            .map_err(|e| diagnostic(span, e))?
            .clone()
        else {
            return Err(diagnostic(span, "incoming requires OrderedSet result"));
        };
        let Expr::Literal(Literal::String(field)) = self.node(arguments[1]) else {
            return Err(diagnostic(
                span,
                "incoming requires a literal member selector",
            ));
        };
        let value = self.expression(arguments[0], depth + 1)?;
        let target = self.value_type(value, span)?;
        let member = self
            .compiler
            .models
            .incoming_feature(&mut self.compiler.table, source, target, &field)
            .map_err(|e| diagnostic(span, e))?;
        let source = self.compiler.ty(source, span)?;
        self.emit(
            ty,
            InstructionKind::Incoming {
                value: self.operand(value, span)?,
                source,
                field,
                declaration: member.declaration,
            },
            span,
        )
    }
    fn constructor_algebra(
        &mut self,
        operation: crate::definitions::rcore_behavior::ir_v2::AlgebraOp,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        let input = *arguments
            .first()
            .ok_or_else(|| diagnostic(span, "Missing algebra collection"))?;
        let input = self.expression(input, depth + 1)?;
        let other = arguments
            .get(1)
            .map(|argument| self.expression(*argument, depth + 1))
            .transpose()?;
        self.emit(
            ty,
            InstructionKind::CollectionAlgebra {
                operation,
                value: self.operand(input, span)?,
                other: other.map(|slot| self.operand(slot, span)).transpose()?,
            },
            span,
        )
    }
    fn constructor_membership(
        &mut self,
        operation: crate::definitions::rcore_behavior::ir_v2::MembershipOp,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        let source = *arguments
            .first()
            .ok_or_else(|| diagnostic(span, "Missing membership collection"))?;
        let other = *arguments
            .get(1)
            .ok_or_else(|| diagnostic(span, "Missing membership value"))?;
        let source = self.expression(source, depth + 1)?;
        let other = self.expression(other, depth + 1)?;
        let left_type = self.value_type(source, span)?;
        let right_type = self.value_type(other, span)?;
        let element =
            |table: &crate::definitions::rcore_behavior::types::TypeTable, ty| -> Result<TypeRef> {
                match table.get(ty).map_err(|e| diagnostic(span, e))? {
                    Type::Collection { element, .. } | Type::Many { element, .. } => Ok(*element),
                    _ => Err(diagnostic(span, "Membership requires a collection")),
                }
            };
        let left = element(&self.compiler.table, left_type)?;
        let right = if operation.multiple() {
            element(&self.compiler.table, right_type)?
        } else {
            right_type
        };
        let domain = self
            .compiler
            .models
            .join(&mut self.compiler.table, left, right)
            .map_err(|e| diagnostic(span, e))?;
        let element = self.compiler.ty(domain, span)?;
        self.emit(
            ty,
            InstructionKind::CollectionMembership {
                operation,
                value: self.operand(source, span)?,
                other: self.operand(other, span)?,
                element,
            },
            span,
        )
    }
    fn constructor_collection(
        &mut self,
        operation: crate::definitions::rcore_behavior::ir_v2::CollectionOp,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        let mut slots = Vec::new();
        for argument in arguments {
            slots.push(self.expression(*argument, depth + 1)?);
        }
        let value = self.operand(
            *slots
                .first()
                .ok_or_else(|| diagnostic(span, "Missing collection"))?,
            span,
        )?;
        let callback = self.operand(
            *slots
                .last()
                .ok_or_else(|| diagnostic(span, "Missing callback"))?,
            span,
        )?;
        let seed = if operation == crate::definitions::rcore_behavior::ir_v2::CollectionOp::Fold {
            Some(self.operand(slots[1], span)?)
        } else {
            None
        };
        self.emit(
            ty,
            InstructionKind::CollectionCall {
                operation,
                value,
                callback,
                seed,
            },
            span,
        )
    }
    fn constructor_value(
        &mut self,
        name: &str,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        if name == "None" {
            return self.emit(ty, InstructionKind::None, span);
        }
        let argument = *arguments
            .first()
            .ok_or_else(|| diagnostic(span, "Missing constructor argument"))?;
        let value = self.expression(argument, depth + 1)?;
        self.constructor_value_finish(name, arguments, ty, span, depth, value)
    }
    fn constructor_value_finish(
        &mut self,
        name: &str,
        arguments: &[ExprId],
        ty: TypeRef,
        span: &SourceSpan,
        depth: usize,
        value: usize,
    ) -> Result<usize> {
        let shape = self
            .compiler
            .table
            .get(ty)
            .map_err(|e| diagnostic(span, e))?
            .clone();
        let expected = match (&shape, name) {
            (Type::Option(inner), "Some") => Some(*inner),
            (Type::Result { ok, .. }, "Ok") => Some(*ok),
            (Type::Result { error, .. }, "Err") => Some(*error),
            _ => None,
        };
        let value = if let Some(expected) = expected {
            self.view(value, expected, span)?
        } else {
            value
        };
        let operand = self.operand(value, span)?;
        if matches!(name, "first" | "last" | "at") {
            let integer = self
                .compiler
                .table
                .intern(Type::Scalar(Scalar::Int64))
                .map_err(|e| diagnostic(span, e))?;
            let index = if name == "at" {
                let argument = *arguments
                    .get(1)
                    .ok_or_else(|| diagnostic(span, "Missing selection index"))?;
                self.expression(argument, depth + 1)?
            } else if name == "first" {
                self.literal(integer, &Literal::Number("0".into()), span)?
            } else {
                let size = self.emit(
                    integer,
                    InstructionKind::CollectionSize { value: operand },
                    span,
                )?;
                let one = self.literal(integer, &Literal::Number("1".into()), span)?;
                self.emit(
                    integer,
                    InstructionKind::Binary {
                        op: ir::BinaryOp::Subtract,
                        left: self.operand(size, span)?,
                        right: self.operand(one, span)?,
                    },
                    span,
                )?
            };
            return self.emit(
                ty,
                InstructionKind::Index {
                    value: self.operand(value, span)?,
                    index: self.operand(index, span)?,
                },
                span,
            );
        }
        if let Some(op) = crate::definitions::rcore_behavior::ir_v2::TextOp::named(name) {
            return self.emit(ty, InstructionKind::Text { op, value: operand }, span);
        }
        if name == "split" {
            let argument = *arguments
                .get(1)
                .ok_or_else(|| diagnostic(span, "Missing split separator"))?;
            let separator = self.expression(argument, depth + 1)?;
            return self.emit(
                ty,
                InstructionKind::Split {
                    value: operand,
                    separator: self.operand(separator, span)?,
                },
                span,
            );
        }
        let kind = match name {
            "flatten" => InstructionKind::CollectionFlatten { value: operand },
            "size" => InstructionKind::CollectionSize { value: operand },
            "isEmpty" => InstructionKind::CollectionIsEmpty { value: operand },
            "Some" => InstructionKind::Some(operand),
            "Ok" => InstructionKind::Ok(operand),
            "Err" => InstructionKind::Err(operand),
            "Sequence" | "Set" | "OrderedSet" | "Bag" => {
                InstructionKind::ConvertCollection { value: operand }
            }
            _ => {
                return Err(diagnostic(
                    span,
                    "Builtin execution belongs to a later query/provider milestone",
                ));
            }
        };
        self.emit(ty, kind, span)
    }
    fn split<F, G>(
        &mut self,
        split: Split,
        result: TypeRef,
        span: &SourceSpan,
        first: F,
        second: G,
    ) -> Result<usize>
    where
        F: FnOnce(&mut Self, Option<usize>) -> Result<usize>,
        G: FnOnce(&mut Self, Option<usize>) -> Result<usize>,
    {
        // Only the suspended branch state remains on this recursive frame.
        // Admission and branch publication use separate, nonrecursive frames.
        let (mut frame, payload) = self.split_start(split, result, span)?;
        let value = first(self, payload);
        let payload = self.split_second(&mut frame, value, span)?;
        let value = second(self, payload);
        self.split_finish(frame, value, span)
    }
    fn split_start(
        &mut self,
        split: Split,
        result: TypeRef,
        span: &SourceSpan,
    ) -> Result<(SplitFrame, Option<usize>)> {
        let parent = self.current;
        let locals = self.locals.clone();
        let (subject, payload) = match split {
            Split::Bool(s) | Split::Option(s, _) => (s, None),
            Split::Result(s, a, _) => (s, Some(a)),
        };
        let operand = self.operand(subject, span)?;
        let types: Vec<_> = self.values.iter().map(|v| v.0).collect();
        let arguments = self.values.iter().map(|v| v.1).collect();
        let (first_entry, payload) = self.start_block(types.clone(), payload, span)?;
        Ok((
            SplitFrame {
                split,
                result,
                parent,
                operand,
                types,
                arguments,
                locals,
                first_entry,
                first_end: first_entry,
                first_arguments: None,
                second_entry: first_entry,
            },
            payload,
        ))
    }
    fn split_arguments(
        &mut self,
        value: Result<usize>,
        result: TypeRef,
        baseline: usize,
        span: &SourceSpan,
    ) -> Result<Option<Vec<Operand>>> {
        let value = match value {
            Ok(value) => self.view(value, result, span)?,
            Err(Stop::Diverged) => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut arguments: Vec<_> = self.values.iter().take(baseline).map(|v| v.1).collect();
        arguments.push(self.operand(value, span)?);
        Ok(Some(arguments))
    }
    fn split_second(
        &mut self,
        frame: &mut SplitFrame,
        value: Result<usize>,
        span: &SourceSpan,
    ) -> Result<Option<usize>> {
        frame.first_arguments =
            self.split_arguments(value, frame.result, frame.types.len(), span)?;
        frame.first_end = self.current;
        self.locals = frame.locals.clone();
        let payload = match frame.split {
            Split::Bool(_) => None,
            Split::Option(_, ty) | Split::Result(_, _, ty) => Some(ty),
        };
        let (entry, payload) = self.start_block(frame.types.clone(), payload, span)?;
        frame.second_entry = entry;
        Ok(payload)
    }
    fn split_finish(
        &mut self,
        frame: SplitFrame,
        value: Result<usize>,
        span: &SourceSpan,
    ) -> Result<usize> {
        let arguments = self.split_arguments(value, frame.result, frame.types.len(), span)?;
        let end = self.current;
        self.locals = frame.locals;
        let id = |n| u32::try_from(n).map_err(|_| diagnostic(span, "Block index exceeds IR range"));
        self.blocks[frame.parent].terminator = match frame.split {
            Split::Bool(_) => Terminator::Branch {
                condition: frame.operand,
                then_edge: Edge {
                    block: id(frame.first_entry)?,
                    arguments: frame.arguments.clone(),
                },
                else_edge: Edge {
                    block: id(frame.second_entry)?,
                    arguments: frame.arguments,
                },
            },
            Split::Option(_, _) => Terminator::Option {
                subject: frame.operand,
                none: Edge {
                    block: id(frame.first_entry)?,
                    arguments: frame.arguments.clone(),
                },
                some: PayloadEdge {
                    block: id(frame.second_entry)?,
                    arguments: frame.arguments,
                },
            },
            Split::Result(_, _, _) => Terminator::Result {
                subject: frame.operand,
                ok: PayloadEdge {
                    block: id(frame.first_entry)?,
                    arguments: frame.arguments.clone(),
                },
                error: PayloadEdge {
                    block: id(frame.second_entry)?,
                    arguments: frame.arguments,
                },
            },
        };
        if frame.first_arguments.is_none() && arguments.is_none() {
            return Err(Stop::Diverged);
        }
        let result_slot = frame.types.len();
        let mut joined = frame.types;
        joined.push(frame.result);
        let (join, _) = self.start_block(joined, None, span)?;
        if let Some(arguments) = frame.first_arguments {
            self.blocks[frame.first_end].terminator = Terminator::Jump(Edge {
                block: id(join)?,
                arguments,
            });
        }
        if let Some(arguments) = arguments {
            self.blocks[end].terminator = Terminator::Jump(Edge {
                block: id(join)?,
                arguments,
            });
        }
        Ok(result_slot)
    }
    fn arm(&mut self, row: &Row, depth: usize) -> Result<usize> {
        let saved = self.locals.clone();
        for (span, slot) in &row.bindings {
            self.locals.insert((span.start, span.end), *slot);
        }
        let result = self.expression(row.body, depth + 1);
        self.locals = saved;
        result
    }
    fn matches(
        &mut self,
        subject: usize,
        rows: Vec<Row>,
        result: TypeRef,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<usize> {
        match self.prepare_match(subject, rows, span, depth)? {
            MatchPlan::Arm(row) => self.arm(&row, depth),
            MatchPlan::Option { inner, none, some } => self.split(
                Split::Option(subject, inner),
                result,
                span,
                |this, _| this.matches(subject, none, result, span, depth + 1),
                |this, payload| {
                    this.matches(
                        payload.ok_or_else(|| diagnostic(span, "Missing Some payload"))?,
                        some,
                        result,
                        span,
                        depth + 1,
                    )
                },
            ),
            MatchPlan::Result {
                ok,
                error,
                good,
                bad,
            } => self.split(
                Split::Result(subject, ok, error),
                result,
                span,
                |this, payload| {
                    this.matches(
                        payload.ok_or_else(|| diagnostic(span, "Missing Ok payload"))?,
                        good,
                        result,
                        span,
                        depth + 1,
                    )
                },
                |this, payload| {
                    this.matches(
                        payload.ok_or_else(|| diagnostic(span, "Missing Err payload"))?,
                        bad,
                        result,
                        span,
                        depth + 1,
                    )
                },
            ),
            MatchPlan::Scalar {
                condition,
                row,
                rows,
            } => self.split(
                Split::Bool(condition),
                result,
                span,
                |this, _| this.arm(&row, depth),
                |this, _| this.matches(subject, rows, result, span, depth + 1),
            ),
        }
    }
    fn prepare_match(
        &mut self,
        subject: usize,
        mut rows: Vec<Row>,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<MatchPlan> {
        self.compiler.spend(rows.len().saturating_add(1), span)?;
        if depth > 128 {
            return Err(diagnostic(span, "Function match depth limit exceeded"));
        }
        if rows.is_empty() {
            self.blocks[self.current].terminator = Terminator::Absurd {
                value: self.operand(subject, span)?,
            };
            return Err(Stop::Diverged);
        }
        let first = rows
            .first_mut()
            .ok_or_else(|| diagnostic(span, "Missing match rows"))?;
        if let Some(pattern) = first.pattern {
            match self.compiler.report.signatures.bound.index.modules[self.task.module]
                .document
                .patterns[pattern.0]
                .value
                .clone()
            {
                Pattern::Wildcard => first.pattern = None,
                Pattern::Binding(name) => {
                    first.bindings.push((name.span, subject));
                    first.pattern = None;
                }
                _ => {}
            }
        }
        if first.pattern.is_none() {
            return Ok(MatchPlan::Arm(first.clone()));
        }
        let subject_type = self.value_type(subject, span)?;
        let shape = self
            .compiler
            .table
            .get(subject_type)
            .map_err(|e| diagnostic(span, e))?
            .clone();
        match shape {
            Type::Option(inner) => {
                let none = self.partition(&rows, subject, "None", span)?;
                let some = self.partition(&rows, subject, "Some", span)?;
                Ok(MatchPlan::Option { inner, none, some })
            }
            Type::Result { ok, error } => {
                let good = self.partition(&rows, subject, "Ok", span)?;
                let bad = self.partition(&rows, subject, "Err", span)?;
                Ok(MatchPlan::Result {
                    ok,
                    error,
                    good,
                    bad,
                })
            }
            _ => {
                let row = rows.remove(0);
                if rows.is_empty() {
                    return Ok(MatchPlan::Arm(row));
                }
                let pattern = row
                    .pattern
                    .ok_or_else(|| diagnostic(span, "Missing scalar pattern"))?;
                let node = self.compiler.report.signatures.bound.index.modules[self.task.module]
                    .document
                    .patterns[pattern.0]
                    .clone();
                let value = match node.value {
                    Pattern::Literal(literal) => {
                        self.literal(subject_type, &literal, &node.span)?
                    }
                    Pattern::Constructor { .. } => {
                        let target = self.compiler.report.signatures.bound.bindings
                            [self.task.module]
                            .constructors
                            .get(&pattern.0)
                            .cloned()
                            .ok_or_else(|| diagnostic(&node.span, "Missing enum pattern"))?;
                        self.enum_literal(target, &node.span)?
                    }
                    _ => return Err(diagnostic(&node.span, "Unsupported scalar match pattern")),
                };
                let boolean = self
                    .compiler
                    .table
                    .intern(Type::Scalar(Scalar::Bool))
                    .map_err(|e| diagnostic(span, e))?;
                let condition = self.emit(
                    boolean,
                    InstructionKind::Binary {
                        op: ir::BinaryOp::Equal,
                        left: self.operand(subject, span)?,
                        right: self.operand(value, span)?,
                    },
                    &node.span,
                )?;
                Ok(MatchPlan::Scalar {
                    condition,
                    row,
                    rows,
                })
            }
        }
    }
    fn partition(
        &self,
        rows: &[Row],
        subject: usize,
        variant: &str,
        span: &SourceSpan,
    ) -> Result<Vec<Row>> {
        let mut result = vec![];
        for row in rows {
            let mut row = row.clone();
            let Some(pattern) = row.pattern else {
                result.push(row);
                continue;
            };
            let node = &self.compiler.report.signatures.bound.index.modules[self.task.module]
                .document
                .patterns[pattern.0];
            match &node.value {
                Pattern::Wildcard => {
                    row.pattern = None;
                    result.push(row);
                }
                Pattern::Binding(name) => {
                    row.bindings.push((name.span.clone(), subject));
                    row.pattern = None;
                    result.push(row);
                }
                Pattern::Literal(Literal::None) if variant == "None" => {
                    row.pattern = None;
                    result.push(row);
                }
                Pattern::Constructor { arguments, .. } => {
                    if matches!(self.compiler.report.signatures.bound.bindings[self.task.module].constructors.get(&pattern.0),Some(Target::Builtin{name,..}) if name==variant)
                    {
                        row.pattern = arguments.first().copied();
                        result.push(row);
                    }
                }
                Pattern::Literal(Literal::None) => {}
                _ => {
                    return Err(diagnostic(
                        span,
                        "Variant match has incompatible checked pattern",
                    ));
                }
            }
        }
        Ok(result)
    }
}
fn binary(operator: &str) -> Option<ir::BinaryOp> {
    Some(match operator {
        "+" => ir::BinaryOp::Add,
        "-" => ir::BinaryOp::Subtract,
        "*" => ir::BinaryOp::Multiply,
        "/" => ir::BinaryOp::Divide,
        "%" => ir::BinaryOp::Remainder,
        "<" => ir::BinaryOp::Less,
        "<=" => ir::BinaryOp::LessEqual,
        ">" => ir::BinaryOp::Greater,
        ">=" => ir::BinaryOp::GreaterEqual,
        "==" => ir::BinaryOp::Equal,
        "!=" => ir::BinaryOp::NotEqual,
        _ => return None,
    })
}
