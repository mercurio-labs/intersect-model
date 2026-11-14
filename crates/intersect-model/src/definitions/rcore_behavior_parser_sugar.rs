//! Handwritten syntax expansions onto existing builtins.
//!
//! Method calls, type-switch matches and the OCL-named prelude operations are
//! rewritten here, so binding, checking, IR, verification and execution see
//! only constructs they already admit. Generated builtin references use
//! `BUILTIN_PREFIX`, and generated locals use `LOCAL_PREFIX`; neither is a
//! lexable name, so source bindings cannot capture or shadow them.
use super::super::ast::*;
use super::{Parser, Result, TokenKind};
use crate::definitions::bundle::SourceSpan;
use std::collections::BTreeSet;

/// OCL standard-library operations supplied by expansion. Existing Rcore
/// builtins already cover the others (see docs/rcore-prelude.md).
const PRELUDE: &[&str] = &[
    "selectByKind",
    "oclIsKindOf",
    "reject",
    "notEmpty",
    "excluding",
    "issue",
    "issue_at",
    "issue_rule",
];

/// Prelude type names supplied by expansion. `Issue` is the record a
/// `bind validate` function returns in a `Sequence` (see `ISSUE_FIELDS`).
const PRELUDE_TYPES: &[&str] = &["Issue"];

/// The fields of the prelude `Issue` record, all `String`: `severity`
/// (`"error"`, `"warning"` or `"info"`), `code` (a stable issue code),
/// `message`, `feature` (the name of the feature the issue concerns, or
/// `""`) and `rule` (an identifier of the rule when several rules share a
/// code, or `""`). The engine reads them by these names.
const ISSUE_FIELDS: [&str; 5] = ["code", "feature", "message", "rule", "severity"];

/// Document-wide names that decide which calls are expanded.
#[derive(Default)]
pub(super) struct Names {
    /// Import aliases. `alias.f(...)` stays a qualified call.
    aliases: BTreeSet<String>,
    /// Prelude names this document declares or uses as plain names. Source
    /// declarations take precedence, as they do over builtins in binding.
    shadowed: BTreeSet<String>,
}

pub(super) fn scan(tokens: &[super::Token]) -> Names {
    let mut names = Names::default();
    for (index, token) in tokens.iter().enumerate() {
        let (name, quoted) = match &token.kind {
            TokenKind::Word(name) => (name, false),
            TokenKind::QuotedName(name) => (name, true),
            _ => continue,
        };
        let previous = index.checked_sub(1).map(|i| &tokens[i].kind);
        if matches!(previous, Some(TokenKind::Word(word)) if word == "as") {
            names.aliases.insert(name.clone());
        }
        if PRELUDE_TYPES.contains(&name.as_str()) {
            // A type name is never called; only a declaration or a quoted
            // spelling shadows it.
            let declared = matches!(previous, Some(TokenKind::Word(word)) if word == "fn");
            if quoted || declared {
                names.shadowed.insert(name.clone());
            }
            continue;
        }
        if !PRELUDE.contains(&name.as_str()) {
            continue;
        }
        let called = matches!(
            tokens.get(index + 1).map(|t| &t.kind),
            Some(TokenKind::Symbol(s)) if s == "(" || s == "::"
        );
        let declared = matches!(previous, Some(TokenKind::Word(word)) if word == "fn");
        if quoted || declared || !called {
            names.shadowed.insert(name.clone());
        }
    }
    names
}

enum Arm {
    Kind { name: Option<Name>, ty: TypeId },
    Pattern(PatternId),
}

impl Parser {
    /// `receiver.f(args)` is `f(receiver, args)` unless `receiver` is a
    /// qualified name rooted at an import alias.
    pub(super) fn method_call(
        &mut self,
        callee: ExprId,
        mut arguments: Vec<ExprId>,
    ) -> (ExprId, Vec<ExprId>) {
        let Expr::Member {
            receiver,
            name,
            safe: false,
        } = &self.document.expressions[callee.0].value
        else {
            return (callee, arguments);
        };
        let (receiver, name) = (*receiver, name.clone());
        if self.qualified(receiver) {
            return (callee, arguments);
        }
        self.document.expressions[callee.0] = Located {
            span: name.span.clone(),
            value: Expr::Name(name),
        };
        arguments.insert(0, receiver);
        (callee, arguments)
    }

    fn qualified(&self, mut id: ExprId) -> bool {
        loop {
            match &self.document.expressions[id.0].value {
                Expr::Name(name) => return self.names.aliases.contains(&name.value),
                Expr::Member {
                    receiver,
                    safe: false,
                    ..
                } => id = *receiver,
                _ => return false,
            }
        }
    }

    /// Expand one prelude call, or return `None` for an ordinary call.
    pub(super) async fn prelude_call(
        &mut self,
        start: usize,
        callee: ExprId,
        types: &[TypeId],
        arguments: &[ExprId],
    ) -> Result<Option<ExprId>> {
        let Expr::Name(name) = &self.document.expressions[callee.0].value else {
            return Ok(None);
        };
        if !PRELUDE.contains(&name.value.as_str()) || self.names.shadowed.contains(&name.value) {
            return Ok(None);
        }
        let name = name.clone();
        if let Some(arity) = match name.value.as_str() {
            "issue" => Some(3),
            "issue_at" => Some(4),
            "issue_rule" => Some(5),
            _ => None,
        } {
            if !types.is_empty() || arguments.len() != arity {
                return Err(self.diagnostic(
                    &name.span,
                    format!(
                        "{} takes 0 type arguments and {arity} arguments",
                        name.value
                    ),
                ));
            }
            return Ok(Some(self.issue(start, arguments).await));
        }
        let (type_arity, arity) = match name.value.as_str() {
            "selectByKind" | "oclIsKindOf" => (1, 1),
            "notEmpty" => (0, 1),
            _ => (0, 2),
        };
        if types.len() != type_arity || arguments.len() != arity {
            return Err(self.diagnostic(
                &name.span,
                format!(
                    "{} takes {type_arity} type argument(s) and {arity} argument(s)",
                    name.value
                ),
            ));
        }
        let anchor = name.span.clone();
        let id = match name.value.as_str() {
            "selectByKind" => {
                self.select_by_kind(start, callee, types[0], arguments[0])
                    .await
            }
            "oclIsKindOf" => self.is_kind_of(start, callee, types[0], arguments[0]).await,
            "notEmpty" => {
                let empty = self
                    .reuse_call(start, callee, "isEmpty", vec![], vec![arguments[0]])
                    .await;
                self.not(start, empty).await
            }
            "reject" => {
                self.reject(start, callee, &anchor, arguments[0], arguments[1])
                    .await
            }
            _ => {
                self.excluding(start, callee, &anchor, arguments[0], arguments[1])
                    .await
            }
        };
        Ok(Some(id))
    }

    /// The prelude `Issue` type: `{code: String, feature: String, message:
    /// String, rule: String, severity: String}`. `None` when `path` is not
    /// that name.
    pub(super) async fn prelude_type(
        &mut self,
        start: usize,
        path: &Path,
        arguments: &[TypeId],
    ) -> Option<TypeId> {
        let [name] = path.as_slice() else {
            return None;
        };
        if !PRELUDE_TYPES.contains(&name.value.as_str())
            || self.names.shadowed.contains(&name.value)
            || !arguments.is_empty()
        {
            return None;
        }
        let mut fields = Vec::with_capacity(ISSUE_FIELDS.len());
        for field in ISSUE_FIELDS {
            let string = self.builtin_name(start, "String");
            let ty = self
                .type_node(
                    start,
                    Type::Named {
                        path: vec![string],
                        arguments: vec![],
                    },
                )
                .await;
            fields.push((self.located(start, field.to_owned()), ty));
        }
        Some(self.type_node(start, Type::Record(fields)).await)
    }

    /// `issue(severity, code, message)`, `issue_at(severity, code, message,
    /// feature)` and `issue_rule(severity, code, message, feature, rule)`:
    /// the `Issue` record literal, with `""` for what is not given.
    async fn issue(&mut self, start: usize, arguments: &[ExprId]) -> ExprId {
        let feature = match arguments.get(3) {
            Some(given) => *given,
            None => self.empty_string(start).await,
        };
        let rule = match arguments.get(4) {
            Some(given) => *given,
            None => self.empty_string(start).await,
        };
        let mut fields = Vec::with_capacity(ISSUE_FIELDS.len());
        for (name, value) in [
            ("severity", arguments[0]),
            ("code", arguments[1]),
            ("message", arguments[2]),
            ("feature", feature),
            ("rule", rule),
        ] {
            fields.push((self.located(start, name.to_owned()), value));
        }
        self.expr_node(start, Expr::Record(fields)).await
    }

    async fn empty_string(&mut self, start: usize) -> ExprId {
        self.expr_node(start, Expr::Literal(Literal::String(String::new())))
            .await
    }

    /// `flatMap(xs, |e| { let k: Sequence<T> = match asType::<T>(e) {
    /// Some(t) => [t], None => [] }; k })`
    async fn select_by_kind(
        &mut self,
        start: usize,
        callee: ExprId,
        ty: TypeId,
        values: ExprId,
    ) -> ExprId {
        let anchor = self.document.expressions[callee.0].span.clone();
        let anchor = &anchor;
        let item = self.hidden(anchor, 0, "item");
        let selected = self.hidden(anchor, 1, "selected");
        let kind = self.hidden(anchor, 2, "kind");
        let subject = self.expr_node(start, Expr::Name(item.clone())).await;
        let view = self
            .builtin_call(start, "asType", vec![ty], vec![subject])
            .await;
        let found = self.expr_node(start, Expr::Name(selected.clone())).await;
        let one = self.expr_node(start, Expr::List(vec![found])).await;
        let none = self.expr_node(start, Expr::List(vec![])).await;
        let binding = self.pattern_node(start, Pattern::Binding(selected)).await;
        let some = self.constructor(start, "Some", vec![binding]).await;
        let absent = self.constructor(start, "None", vec![]).await;
        let value = self
            .expr_node(
                start,
                Expr::Match {
                    value: view,
                    arms: vec![(some, one), (absent, none)],
                },
            )
            .await;
        let element_type = self.copy_type(ty).await;
        let sequence = self.builtin_name(start, "Sequence");
        let sequence = self
            .type_node(
                start,
                Type::Named {
                    path: vec![sequence],
                    arguments: vec![element_type],
                },
            )
            .await;
        let result = self.expr_node(start, Expr::Name(kind.clone())).await;
        let body = self
            .expr_node(
                start,
                Expr::Block {
                    bindings: vec![self.located(
                        start,
                        Binding {
                            name: kind,
                            ty: Some(sequence),
                            value,
                        },
                    )],
                    result,
                },
            )
            .await;
        let callback = self.lambda(start, item, body).await;
        self.reuse_call(start, callee, "flatMap", vec![], vec![values, callback])
            .await
    }

    /// `match asType::<T>(value) { Some(_) => true, None => false }`
    async fn is_kind_of(
        &mut self,
        start: usize,
        callee: ExprId,
        ty: TypeId,
        value: ExprId,
    ) -> ExprId {
        let view = self
            .reuse_call(start, callee, "asType", vec![ty], vec![value])
            .await;
        let yes = self
            .expr_node(start, Expr::Literal(Literal::Boolean(true)))
            .await;
        let no = self
            .expr_node(start, Expr::Literal(Literal::Boolean(false)))
            .await;
        let wildcard = self.pattern_node(start, Pattern::Wildcard).await;
        let some = self.constructor(start, "Some", vec![wildcard]).await;
        let none = self.constructor(start, "None", vec![]).await;
        self.expr_node(
            start,
            Expr::Match {
                value: view,
                arms: vec![(some, yes), (none, no)],
            },
        )
        .await
    }

    /// `filter(xs, |x| !body)` for a lambda, else `filter(xs, |e| !p(e))`.
    async fn reject(
        &mut self,
        start: usize,
        callee: ExprId,
        anchor: &SourceSpan,
        values: ExprId,
        predicate: ExprId,
    ) -> ExprId {
        let callback = if let Expr::Lambda { body, .. } =
            self.document.expressions[predicate.0].value
        {
            let inverted = self.not(start, body).await;
            if let Expr::Lambda { body, .. } = &mut self.document.expressions[predicate.0].value {
                *body = inverted;
            }
            predicate
        } else {
            let item = self.hidden(anchor, 0, "item");
            let argument = self.expr_node(start, Expr::Name(item.clone())).await;
            let test = self
                .expr_node(
                    start,
                    Expr::Call {
                        callee: predicate,
                        types: vec![],
                        arguments: vec![argument],
                    },
                )
                .await;
            let inverted = self.not(start, test).await;
            self.lambda(start, item, inverted).await
        };
        self.reuse_call(start, callee, "filter", vec![], vec![values, callback])
            .await
    }

    /// `filter(xs, |e| e != value)`. Generated locals declared outside a
    /// generated lambda would not be captured by it (capture follows source
    /// spans), so `value` is compared in place; Rcore expressions are pure.
    async fn excluding(
        &mut self,
        start: usize,
        callee: ExprId,
        anchor: &SourceSpan,
        values: ExprId,
        value: ExprId,
    ) -> ExprId {
        let item = self.hidden(anchor, 0, "item");
        let left = self.expr_node(start, Expr::Name(item.clone())).await;
        let right = self.expr_node(start, Expr::Group(value)).await;
        let operator = Located {
            value: "!=".to_string(),
            span: anchor.clone(),
        };
        let test = self
            .expr_node(
                start,
                Expr::Binary {
                    operator,
                    left,
                    right,
                },
            )
            .await;
        let callback = self.lambda(start, item, test).await;
        self.reuse_call(start, callee, "filter", vec![], vec![values, callback])
            .await
    }

    /// Parse the arms of a `match` whose value was already parsed. Arms of the
    /// form `name: Type` or `_: Type` select the first arm whose model kind
    /// the value has, and the final arm must be a `_` or binding catch-all.
    pub(super) async fn match_arms(
        &mut self,
        start: usize,
        keyword: &SourceSpan,
        value: ExprId,
    ) -> Result<ExprId> {
        let mut arms = Vec::new();
        while !self.at("}") {
            let arm = if self.kind_arm_ahead() {
                let name = if self.eat("_").await {
                    None
                } else {
                    Some(self.name().await?)
                };
                self.need(":").await?;
                let ty = Box::pin(self.ty()).await?;
                Arm::Kind { name, ty }
            } else {
                Arm::Pattern(Box::pin(self.pattern()).await?)
            };
            self.need("=>").await?;
            let expression = Box::pin(self.expr(0)).await?;
            arms.push((arm, expression));
            if !self.eat(",").await {
                break;
            }
        }
        self.need("}").await?;
        if !arms.iter().any(|(arm, _)| matches!(arm, Arm::Kind { .. })) {
            let arms = arms
                .into_iter()
                .filter_map(|(arm, body)| match arm {
                    Arm::Pattern(pattern) => Some((pattern, body)),
                    Arm::Kind { .. } => None,
                })
                .collect();
            return Ok(self.expr_node(start, Expr::Match { value, arms }).await);
        }
        self.type_switch(start, keyword, value, arms).await
    }

    fn kind_arm_ahead(&self) -> bool {
        let named = match &self.token().kind {
            TokenKind::Word(word) => word == "_" || !super::RESERVED.contains(&word.as_str()),
            TokenKind::QuotedName(_) => true,
            _ => false,
        };
        named
            && matches!(
                self.tokens.get(self.cursor + 1).map(|t| &t.kind),
                Some(TokenKind::Symbol(s)) if s == ":"
            )
    }

    async fn type_switch(
        &mut self,
        start: usize,
        keyword: &SourceSpan,
        value: ExprId,
        mut arms: Vec<(Arm, ExprId)>,
    ) -> Result<ExprId> {
        let subject = self.hidden(keyword, 0, "subject");
        let (last, fallback) = arms
            .pop()
            .ok_or_else(|| self.diagnostic(keyword, "Type-switch match requires arms"))?;
        let catch_all = match last {
            Arm::Pattern(pattern) => match &self.document.patterns[pattern.0].value {
                Pattern::Wildcard => Some(None),
                Pattern::Binding(name) => Some(Some(name.clone())),
                _ => None,
            },
            Arm::Kind { .. } => None,
        };
        let Some(catch_all) = catch_all else {
            return Err(self.diagnostic(
                &self.document.expressions[fallback.0].span,
                "A type-switch match must end with a `_` or binding catch-all arm",
            ));
        };
        let mut chain = match catch_all {
            None => fallback,
            Some(name) => {
                let value = self.expr_node(start, Expr::Name(subject.clone())).await;
                let binding = self.located(
                    start,
                    Binding {
                        name,
                        ty: None,
                        value,
                    },
                );
                self.expr_node(
                    start,
                    Expr::Block {
                        bindings: vec![binding],
                        result: fallback,
                    },
                )
                .await
            }
        };
        for (arm, body) in arms.into_iter().rev() {
            let Arm::Kind { name, ty } = arm else {
                return Err(self.diagnostic(
                    &self.document.expressions[body.0].span,
                    "Type-switch arms cannot be mixed with other patterns",
                ));
            };
            let operand = self.expr_node(start, Expr::Name(subject.clone())).await;
            let view = self
                .builtin_call(start, "asType", vec![ty], vec![operand])
                .await;
            let inner = match name {
                Some(name) => Pattern::Binding(name),
                None => Pattern::Wildcard,
            };
            let inner = self.pattern_node(start, inner).await;
            let some = self.constructor(start, "Some", vec![inner]).await;
            let none = self.constructor(start, "None", vec![]).await;
            chain = self
                .expr_node(
                    start,
                    Expr::Match {
                        value: view,
                        arms: vec![(some, body), (none, chain)],
                    },
                )
                .await;
        }
        let binding = self.located(
            start,
            Binding {
                name: subject,
                ty: None,
                value,
            },
        );
        Ok(self
            .expr_node(
                start,
                Expr::Block {
                    bindings: vec![binding],
                    result: chain,
                },
            )
            .await)
    }

    /// A generated local. Its declaration identity is a distinct prefix of the
    /// anchoring source token, so each generated local keeps a unique span.
    /// Callers stay within the anchor: prelude names have at least six
    /// characters and use three ordinals; `match` uses one.
    fn hidden(&self, anchor: &SourceSpan, ordinal: usize, purpose: &str) -> Name {
        let start = (anchor.start + ordinal).min(anchor.end.saturating_sub(1));
        Located {
            value: format!("{LOCAL_PREFIX}{purpose}"),
            span: SourceSpan {
                source: anchor.source.clone(),
                start,
                end: start + 1,
            },
        }
    }

    fn builtin_name(&self, start: usize, name: &str) -> Name {
        self.located(start, format!("{BUILTIN_PREFIX}{name}"))
    }

    async fn builtin_call(
        &mut self,
        start: usize,
        name: &str,
        types: Vec<TypeId>,
        arguments: Vec<ExprId>,
    ) -> ExprId {
        let callee = self.builtin_name(start, name);
        let callee = self.expr_node(start, Expr::Name(callee)).await;
        self.expr_node(
            start,
            Expr::Call {
                callee,
                types,
                arguments,
            },
        )
        .await
    }

    /// Call a builtin through the original callee node, which keeps its span.
    async fn reuse_call(
        &mut self,
        start: usize,
        callee: ExprId,
        name: &str,
        types: Vec<TypeId>,
        arguments: Vec<ExprId>,
    ) -> ExprId {
        let node = &mut self.document.expressions[callee.0];
        node.value = Expr::Name(Located {
            value: format!("{BUILTIN_PREFIX}{name}"),
            span: node.span.clone(),
        });
        self.expr_node(
            start,
            Expr::Call {
                callee,
                types,
                arguments,
            },
        )
        .await
    }

    async fn not(&mut self, start: usize, operand: ExprId) -> ExprId {
        let operator = self.located(start, "!".to_string());
        let operand = self.expr_node(start, Expr::Group(operand)).await;
        self.expr_node(start, Expr::Unary { operator, operand })
            .await
    }

    async fn lambda(&mut self, start: usize, parameter: Name, body: ExprId) -> ExprId {
        let ty = self.type_node(parameter.span.start, Type::Inferred).await;
        self.expr_node(
            start,
            Expr::Lambda {
                parameters: vec![Parameter {
                    name: parameter,
                    ty,
                }],
                body,
            },
        )
        .await
    }

    async fn constructor(
        &mut self,
        start: usize,
        name: &str,
        arguments: Vec<PatternId>,
    ) -> PatternId {
        let path = vec![self.builtin_name(start, name)];
        self.pattern_node(start, Pattern::Constructor { path, arguments })
            .await
    }

    pub(super) async fn pattern_node(&mut self, start: usize, value: Pattern) -> PatternId {
        let id = PatternId(self.document.patterns.len());
        self.document.patterns.push(self.located(start, value));
        self.traversal.node().await;
        id
    }

    /// Duplicate a parsed type tree so each expansion use has its own node.
    async fn copy_type(&mut self, id: TypeId) -> TypeId {
        let node = self.document.types[id.0].clone();
        let value = match node.value {
            Type::Named { path, arguments } => {
                let mut copied = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    copied.push(Box::pin(self.copy_type(argument)).await);
                }
                Type::Named {
                    path,
                    arguments: copied,
                }
            }
            Type::Optional(inner) => Type::Optional(Box::pin(self.copy_type(inner)).await),
            Type::Multiplicity {
                element,
                lower,
                upper,
                ordered,
                unique,
            } => Type::Multiplicity {
                element: Box::pin(self.copy_type(element)).await,
                lower,
                upper,
                ordered,
                unique,
            },
            Type::Tuple(items) => {
                let mut copied = Vec::with_capacity(items.len());
                for item in items {
                    copied.push(Box::pin(self.copy_type(item)).await);
                }
                Type::Tuple(copied)
            }
            Type::Record(fields) => {
                let mut copied = Vec::with_capacity(fields.len());
                for (name, item) in fields {
                    copied.push((name, Box::pin(self.copy_type(item)).await));
                }
                Type::Record(copied)
            }
            Type::Function { parameters, result } => {
                let mut copied = Vec::with_capacity(parameters.len());
                for parameter in parameters {
                    copied.push(Box::pin(self.copy_type(parameter)).await);
                }
                Type::Function {
                    parameters: copied,
                    result: Box::pin(self.copy_type(result)).await,
                }
            }
            Type::Inferred => Type::Inferred,
        };
        let id = TypeId(self.document.types.len());
        self.document.types.push(Located {
            value,
            span: node.span,
        });
        self.traversal.node().await;
        id
    }

    fn diagnostic(
        &self,
        span: &SourceSpan,
        message: impl Into<String>,
    ) -> Box<super::SyntaxDiagnostic> {
        Box::new(super::SyntaxDiagnostic {
            span: span.clone(),
            message: message.into(),
        })
    }
}
