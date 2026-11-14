//! Handwritten recursive-descent declarations and Pratt expressions.
use super::{SyntaxDiagnostic, Token, TokenKind, ast::*};
use crate::definitions::bundle::{DefinitionSource, SourceSpan};

const MAX_DEPTH: usize = 64;
use crate::definitions::rcore_names::BEHAVIOR_RESERVED as RESERVED;
#[path = "rcore_behavior_parser_sugar.rs"]
mod sugar;
// Keep recursive parser frames small on native executable and WASM stacks.
// Diagnostics allocate only on failure; successful results carry no inline span.
type Result<T> = std::result::Result<T, Box<SyntaxDiagnostic>>;

pub fn parse(source: DefinitionSource) -> Document {
    parse_with_limits(source, Default::default())
}

pub(crate) fn parse_with_limits(
    source: DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
) -> Document {
    crate::definitions::continuation::complete(owned_future(source, limits))
}
pub(super) fn owned_future(
    source: DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
) -> impl std::future::Future<Output = Document> + Send {
    parse_retained(source, limits, Traversal::default())
}
async fn parse_retained(
    source: DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
    traversal: Traversal,
) -> Document {
    let (tokens, diagnostics) = super::lex_parts_retained(&source).await;
    let mut lexed = super::Lexed {
        source,
        tokens,
        diagnostics,
    };
    traversal
        .0
        .phase
        .store(1, std::sync::atomic::Ordering::Relaxed);
    if lexed.source.kind == crate::definitions::bundle::SourceKind::Rcore {
        match crate::definitions::rcore::behavior_tokens_retained(
            &lexed.source,
            &lexed.tokens,
            limits.source_bytes,
        )
        .await
        {
            Ok(tokens) => lexed.tokens = tokens,
            Err(errors) => lexed.diagnostics.extend(errors),
        }
    }
    traversal
        .0
        .phase
        .store(2, std::sync::atomic::Ordering::Relaxed);
    let names = sugar::scan(&lexed.tokens);
    let mut parser = Parser {
        names,
        tokens: lexed.tokens,
        cursor: 0,
        depth: 0,
        traversal,
        document: Document {
            source: lexed.source,
            syntax_version: None,
            module: None,
            declarations: vec![],
            expressions: vec![],
            types: vec![],
            patterns: vec![],
            diagnostics: lexed.diagnostics,
        },
    };
    parser.document().await;
    parser
        .document
        .diagnostics
        .sort_by_key(|d| (d.span.start, d.span.end));
    parser
        .traversal
        .0
        .phase
        .store(3, std::sync::atomic::Ordering::Relaxed);
    parser.document
}

struct Parser {
    names: sugar::Names,
    tokens: Vec<Token>,
    cursor: usize,
    depth: usize,
    document: Document,
    traversal: Traversal,
}
impl Parser {
    fn token(&self) -> &Token {
        &self.tokens[self.cursor]
    }
    fn at(&self, value: &str) -> bool {
        matches!(&self.token().kind, TokenKind::Word(s) | TokenKind::Symbol(s) if s == value)
    }
    fn end(&self) -> bool {
        self.token().kind == TokenKind::End
    }
    async fn bump(&mut self) -> Token {
        let token = self.token().clone();
        if !self.end() {
            self.cursor += 1;
            self.traversal.token().await;
        }
        token
    }
    async fn eat(&mut self, value: &str) -> bool {
        if self.at(value) {
            self.bump().await;
            true
        } else {
            false
        }
    }
    fn error(&self, message: impl Into<String>) -> Box<SyntaxDiagnostic> {
        Box::new(SyntaxDiagnostic {
            span: self.token().span.clone(),
            message: message.into(),
        })
    }
    async fn need(&mut self, value: &str) -> Result<()> {
        if self.eat(value).await {
            Ok(())
        } else {
            Err(self.error(format!("Expected {value}")))
        }
    }
    fn span(&self, start: usize) -> SourceSpan {
        let end = if self.cursor == 0 {
            start
        } else {
            self.tokens[self.cursor - 1].span.end
        };
        SourceSpan {
            source: self.document.source.id.clone(),
            start,
            end: end.max(start),
        }
    }
    fn located<T>(&self, start: usize, value: T) -> Located<T> {
        Located {
            value,
            span: self.span(start),
        }
    }
    async fn name(&mut self) -> Result<Name> {
        match &self.token().kind {
            TokenKind::Word(s) if !RESERVED.contains(&s.as_str()) => {}
            TokenKind::QuotedName(_) => {}
            _ => return Err(self.error("Expected an identifier")),
        }
        let token = self.bump().await;
        let value = match token.kind {
            TokenKind::Word(s) | TokenKind::QuotedName(s) => s,
            _ => return Err(self.error("Expected an identifier")),
        };
        if value.starts_with('#') {
            return Err(Box::new(SyntaxDiagnostic {
                span: token.span,
                message: "Names starting with # are reserved for parser expansions".into(),
            }));
        }
        Ok(Located {
            value,
            span: token.span,
        })
    }
    async fn string(&mut self) -> Result<Name> {
        let TokenKind::String(value) = &self.token().kind else {
            return Err(self.error("Expected a string"));
        };
        let value = value.clone();
        let token = self.bump().await;
        Ok(Located {
            value,
            span: token.span,
        })
    }
    async fn path(&mut self) -> Result<Path> {
        let mut path = vec![self.name().await?];
        while self.eat(".").await {
            path.push(self.name().await?);
        }
        Ok(path)
    }
    fn boundary(&self) -> bool {
        [
            "pub",
            "private",
            "fn",
            "derive",
            "validate",
            "import",
            "rcore",
            "package",
            "initialize",
            "projection",
            "convert",
            "extern",
            "bind",
        ]
        .iter()
        .any(|s| self.at(s))
    }
    async fn recover(&mut self, previous: usize) {
        // A declaration keyword cannot occur unquoted inside an expression.
        // It remains a reliable recovery point even after an unmatched brace.
        if self.cursor == previous && !self.end() {
            self.bump().await;
        }
        let mut depth = 0usize;
        while !self.end() && !self.boundary() {
            if self.eat("{").await || self.eat("(").await || self.eat("[").await {
                depth += 1;
            } else if self.eat("}").await || self.eat(")").await || self.eat("]").await {
                depth = depth.saturating_sub(1);
            } else if self.eat(";").await {
                if depth == 0 {
                    break;
                }
            } else {
                self.bump().await;
            }
        }
    }
    async fn document(&mut self) {
        let header = async {
            self.need("rcore").await?;
            let expected = "1";
            let TokenKind::Number(value) = &self.token().kind else {
                return Err(self.error(format!("Expected syntax version {expected}")));
            };
            let version = Located {
                value: value.clone(),
                span: self.token().span.clone(),
            };
            self.bump().await;
            if version.value != expected {
                self.document.diagnostics.push(SyntaxDiagnostic {
                    span: version.span.clone(),
                    message: "Unsupported Rcore syntax version; expected 1".into(),
                });
            }
            self.document.syntax_version = Some(version);
            self.need(";").await
        }
        .await;
        if let Err(error) = header {
            self.document.diagnostics.push(*error);
            if !self.boundary() {
                self.recover(0).await;
            }
        }
        let start = self.token().span.start;
        let position = self.cursor;
        let module: Result<Located<Module>> = async {
            self.need("package").await?;
            let name = self.path().await?;
            self.need("version").await?;
            let version = self.string().await?;
            self.need(";").await?;
            Ok(self.located(start, Module { name, version }))
        }
        .await;
        match module {
            Ok(module) => self.document.module = Some(module),
            Err(error) => {
                self.document.diagnostics.push(*error);
                if !self.boundary() {
                    self.recover(position).await;
                }
            }
        }
        while !self.end() {
            let start = self.token().span.start;
            let position = self.cursor;
            let result = self.declaration().await;
            let value = match result {
                Ok(value) => value,
                Err(error) => {
                    self.document.diagnostics.push(*error);
                    self.recover(position).await;
                    Declaration::Error
                }
            };
            self.document.declarations.push(self.located(start, value));
            self.traversal.node().await;
        }
    }
    async fn unsigned(&mut self) -> Result<usize> {
        let TokenKind::Number(value) = &self.token().kind else {
            return Err(self.error("Expected an unsigned integer"));
        };
        let value = value
            .parse()
            .map_err(|_| self.error("Expected an unsigned integer"))?;
        self.bump().await;
        Ok(value)
    }
    async fn declaration(&mut self) -> Result<Declaration> {
        if self.eat("bind").await {
            let default = self.eat("default").await;
            let implicit = !default && self.eat("implicit").await;
            let validate = !default && !implicit && self.eat("validate").await;
            if !default && !implicit && !validate {
                self.need("scope").await?;
            }
            let target = self.path().await?;
            self.need("=").await?;
            let implementation = self.path().await?;
            self.need(";").await?;
            return Ok(if validate {
                Declaration::ValidateBinding {
                    target,
                    implementation,
                }
            } else if default {
                Declaration::DefaultBinding {
                    target,
                    implementation,
                }
            } else if implicit {
                Declaration::ImplicitBinding {
                    target,
                    implementation,
                }
            } else {
                Declaration::ScopeBinding {
                    target,
                    implementation,
                }
            });
        }
        if self.eat("initialize").await {
            let target = self.path().await?;
            self.need("using").await?;
            let provider = self.string().await?;
            self.need("version").await?;
            let version = self.string().await?;
            self.need("{").await?;
            let mut members = Vec::new();
            while !self.at("}") && !self.end() {
                let name = self.name().await?;
                let value = if self.eat("=").await {
                    Some(Box::pin(self.expr(0)).await?)
                } else {
                    None
                };
                self.need(";").await?;
                members.push((name, value));
            }
            self.need("}").await?;
            return Ok(Declaration::Initialize {
                target,
                provider,
                version,
                members,
            });
        }
        if self.eat("projection").await {
            let target = self.path().await?;
            self.need("from").await?;
            let input = self.name().await?;
            let writable = self.eat("writable").await;
            self.need(";").await?;
            return Ok(Declaration::Projection {
                target,
                input,
                writable,
            });
        }
        if self.eat("convert").await {
            use crate::definitions::{
                bundle::SourceId, conversion::ProviderId, rule_conversion::RuleConversion,
            };
            let context = self.string().await?.value;
            self.need("rule").await?;
            let rule = self.string().await?.value;
            self.need("from").await?;
            let uri = self.string().await?.value;
            self.need("sha256").await?;
            let sha256 = self.string().await?.value;
            self.need("span").await?;
            let start = self.unsigned().await?;
            let end = self.unsigned().await?;
            if end < start {
                return Err(self.error("Conversion source span is reversed"));
            }
            self.need("to").await?;
            let datatype = self.string().await?.value;
            self.need("using").await?;
            let name = self.string().await?.value;
            self.need("version").await?;
            let version = self.string().await?.value;
            self.need("{").await?;
            let mut keywords = std::collections::BTreeMap::new();
            while !self.at("}") && !self.end() {
                let keyword = self.string().await?.value;
                self.need("=").await?;
                let value = if self.eat("true").await {
                    true
                } else {
                    self.need("false").await?;
                    false
                };
                if keywords.insert(keyword, value).is_some() {
                    return Err(self.error("Duplicate conversion keyword"));
                }
                self.need(";").await?;
            }
            self.need("}").await?;
            return Ok(Declaration::RuleConversion(RuleConversion {
                context,
                rule,
                declaration: SourceSpan {
                    source: SourceId { uri, sha256 },
                    start,
                    end,
                },
                datatype,
                provider: ProviderId { name, version },
                keywords,
            }));
        }
        if self.eat("import").await {
            let declaration = if self.eat("model").await {
                let namespace = self.string().await?;
                self.need("as").await?;
                Declaration::ModelImport {
                    namespace,
                    alias: self.name().await?,
                }
            } else {
                self.need("package").await?;
                let path = self.path().await?;
                self.need("version").await?;
                let version = self.string().await?;
                self.need("as").await?;
                Declaration::ModuleImport {
                    path,
                    version,
                    alias: self.name().await?,
                }
            };
            self.need(";").await?;
            return Ok(declaration);
        }
        let public = self.eat("pub").await;
        if !public {
            self.eat("private").await;
        }
        let external = self.eat("extern").await;
        if external {
            self.need("fn").await?;
        }
        if external || self.eat("fn").await {
            let name = self.name().await?;
            let mut generics = Vec::new();
            if self.eat("<").await {
                loop {
                    let name = self.name().await?;
                    let constraint = if self.eat(":").await {
                        Some(Box::pin(self.ty()).await?)
                    } else {
                        None
                    };
                    generics.push(Generic { name, constraint });
                    if !self.eat(",").await {
                        break;
                    }
                }
                self.need(">").await?;
            }
            self.need("(").await?;
            let parameters = self.parameters(")", false).await?;
            self.need("->").await?;
            let result = Box::pin(self.ty()).await?;
            let body = if external {
                let terminator = self.token().span.start;
                self.need(";").await?;
                self.expr_node(terminator, Expr::Error).await
            } else if self.at("{") {
                let body = Box::pin(self.expr(0)).await?;
                self.eat(";").await;
                body
            } else {
                self.need("=").await?;
                let body = Box::pin(self.expr(0)).await?;
                self.need(";").await?;
                body
            };
            return Ok(Declaration::Function {
                public,
                external,
                name,
                generics,
                parameters,
                result,
                body,
            });
        }
        let derive = self.eat("derive").await;
        if !derive {
            self.need("validate").await?;
        }
        let target = self.path().await?;
        if derive && self.eat("using").await {
            let count = self.eat("count").await;
            if !count {
                self.need("noIncoming").await?;
            }
            self.need("(").await?;
            let contract = if count {
                DerivedContract::Count(self.name().await?)
            } else {
                DerivedContract::NoIncoming(self.path().await?)
            };
            self.need(")").await?;
            self.need(";").await?;
            return Ok(Declaration::DerivedContract {
                public,
                target,
                contract,
            });
        }
        self.need("(").await?;
        let receiver = self.name().await?;
        self.need(")").await?;
        if derive {
            self.need("->").await?;
            let result = Box::pin(self.ty()).await?;
            self.need("=").await?;
            let body = Box::pin(self.expr(0)).await?;
            self.need(";").await?;
            Ok(Declaration::Derive {
                public,
                target,
                receiver,
                result,
                body,
            })
        } else {
            self.need("{").await?;
            let mut requirements = Vec::new();
            while !self.at("}") && !self.end() {
                let start = self.token().span.start;
                self.need("require").await?;
                let condition = Box::pin(self.expr(0)).await?;
                self.need("code").await?;
                let code = self.string().await?;
                self.need("message").await?;
                let message = Box::pin(self.expr(0)).await?;
                self.need(";").await?;
                requirements.push(self.located(
                    start,
                    Requirement {
                        condition,
                        code,
                        message,
                    },
                ));
            }
            self.need("}").await?;
            Ok(Declaration::Validate {
                public,
                target,
                receiver,
                requirements,
            })
        }
    }
    async fn parameters(&mut self, end: &str, infer: bool) -> Result<Vec<Parameter>> {
        let mut parameters = Vec::new();
        if !self.at(end) {
            loop {
                let name = self.name().await?;
                let ty = if infer && !self.at(":") {
                    self.type_node(name.span.start, Type::Inferred).await
                } else {
                    self.need(":").await?;
                    Box::pin(self.ty()).await?
                };
                parameters.push(Parameter { name, ty });
                if !self.eat(",").await || self.at(end) {
                    break;
                }
            }
        }
        self.need(end).await?;
        Ok(parameters)
    }
    async fn type_node(&mut self, start: usize, value: Type) -> TypeId {
        let id = TypeId(self.document.types.len());
        self.document.types.push(self.located(start, value));
        self.traversal.node().await;
        id
    }
    async fn ty(&mut self) -> Result<TypeId> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error("Rcore syntax nesting exceeds 64 levels"));
        }
        self.depth += 1;
        let result = self.type_inner().await;
        self.depth -= 1;
        result
    }
    async fn type_inner(&mut self) -> Result<TypeId> {
        let start = self.token().span.start;
        let mut id = if self.eat("(").await {
            let parameters = self.type_list(")").await?;
            if self.eat("->").await {
                let result = Box::pin(self.ty()).await?;
                self.type_node(start, Type::Function { parameters, result })
                    .await
            } else {
                self.type_node(start, Type::Tuple(parameters)).await
            }
        } else if self.eat("{").await {
            let mut fields = Vec::new();
            while !self.at("}") {
                let name = self.name().await?;
                self.need(":").await?;
                fields.push((name, Box::pin(self.ty()).await?));
                if !self.eat(",").await {
                    break;
                }
            }
            self.need("}").await?;
            self.type_node(start, Type::Record(fields)).await
        } else {
            let path = self.path().await?;
            let arguments = if self.eat("<").await {
                if self.at(">") {
                    return Err(self.error("Expected at least one type argument"));
                }
                self.type_list(">").await?
            } else {
                vec![]
            };
            match self.prelude_type(start, &path, &arguments).await {
                Some(id) => id,
                None => self.type_node(start, Type::Named { path, arguments }).await,
            }
        };
        loop {
            if self.eat("?").await {
                id = self.type_node(start, Type::Optional(id)).await;
            } else if self.eat("[").await {
                let lower = self.multiplicity_bound().await?;
                self.need(",").await?;
                let upper = if self.eat("*").await {
                    -1
                } else {
                    i32::try_from(self.multiplicity_bound().await?)
                        .map_err(|_| self.error("Upper multiplicity exceeds Int32"))?
                };
                self.need("]").await?;
                if upper >= 0 && lower > upper as usize {
                    return Err(self.error("Lower multiplicity exceeds upper multiplicity"));
                }
                let mut order = None;
                let mut uniqueness = None;
                loop {
                    let flag = if self.at("ordered") {
                        Some((&mut order, true))
                    } else if self.at("unordered") {
                        Some((&mut order, false))
                    } else if self.at("unique") {
                        Some((&mut uniqueness, true))
                    } else if self.at("nonunique") {
                        Some((&mut uniqueness, false))
                    } else {
                        None
                    };
                    let Some((slot, value)) = flag else {
                        break;
                    };
                    if slot.replace(value).is_some() {
                        return Err(self.error("Repeated behavior multiplicity flag"));
                    }
                    self.bump().await;
                }
                id = self
                    .type_node(
                        start,
                        Type::Multiplicity {
                            element: id,
                            lower,
                            upper,
                            ordered: order.unwrap_or(true),
                            unique: uniqueness.unwrap_or(true),
                        },
                    )
                    .await;
            } else {
                break;
            }
        }
        Ok(id)
    }
    async fn multiplicity_bound(&mut self) -> Result<usize> {
        let TokenKind::Number(value) = &self.token().kind else {
            return Err(self.error("Expected a nonnegative multiplicity bound"));
        };
        let value = value
            .parse::<usize>()
            .map_err(|_| self.error("Invalid multiplicity bound"))?;
        self.bump().await;
        Ok(value)
    }
    async fn type_list(&mut self, end: &str) -> Result<Vec<TypeId>> {
        let mut types = Vec::new();
        if !self.at(end) {
            loop {
                types.push(Box::pin(self.ty()).await?);
                if !self.eat(",").await || self.at(end) {
                    break;
                }
            }
        }
        self.need(end).await?;
        Ok(types)
    }
    async fn comprehension(&mut self, start: usize) -> Result<ExprId> {
        let mut generators = Vec::new();
        loop {
            let name = self.name().await?;
            self.need("in").await?;
            let source = Box::pin(self.expr(0)).await?;
            generators.push((name, source));
            if !self.eat(",").await {
                break;
            }
        }
        let predicate = if self.eat("where").await {
            Some(Box::pin(self.expr(0)).await?)
        } else {
            None
        };
        self.need("yield").await?;
        let mut body = Box::pin(self.expr(0)).await?;
        self.need("]").await?;
        if let Some(condition) = predicate {
            let position = self.document.expressions[condition.0].span.start;
            let then_branch = self.expr_node(position, Expr::List(vec![body])).await;
            let else_branch = self.expr_node(position, Expr::List(vec![])).await;
            body = self
                .expr_node(
                    position,
                    Expr::If {
                        condition,
                        then_branch,
                        else_branch,
                    },
                )
                .await;
        }
        for (ordinal, (name, source)) in generators.into_iter().rev().enumerate() {
            let position = name.span.start;
            let ty = self.type_node(position, Type::Inferred).await;
            let callback = self
                .expr_node(
                    position,
                    Expr::Lambda {
                        parameters: vec![Parameter { name, ty }],
                        body,
                    },
                )
                .await;
            let callee = self
                .expr_node(
                    position,
                    Expr::ComprehensionIntrinsic {
                        flat: ordinal != 0 || predicate.is_some(),
                    },
                )
                .await;
            body = self
                .expr_node(
                    position,
                    Expr::Call {
                        callee,
                        types: vec![],
                        arguments: vec![source, callback],
                    },
                )
                .await;
        }
        // Keep the complete outer syntax span, while each generated lambda owns
        // its generator and dependent body for capture/provenance analysis.
        Ok(self.expr_node(start, Expr::Group(body)).await)
    }
    fn expr_node(
        &mut self,
        start: usize,
        value: Expr,
    ) -> std::pin::Pin<Box<impl std::future::Future<Output = ExprId> + Send + '_>> {
        Box::pin(async move {
            let id = ExprId(self.document.expressions.len());
            self.document.expressions.push(self.located(start, value));
            self.traversal.node().await;
            id
        })
    }
    async fn expr(&mut self, minimum: u8) -> Result<ExprId> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error("Rcore syntax nesting exceeds 64 levels"));
        }
        self.depth += 1;
        // Keep large child future states out of every recursive poll frame.
        let result = Box::pin(self.expression(minimum)).await;
        self.depth -= 1;
        result
    }
    async fn expression(&mut self, minimum: u8) -> Result<ExprId> {
        let start = self.token().span.start;
        let mut left = Box::pin(self.prefix()).await?;
        loop {
            if self.at(".") || self.at("?.") {
                left = Box::pin(self.member_suffix(start, left)).await?;
                continue;
            }
            if self.at("(") || self.at("::") {
                left = Box::pin(self.call_suffix(start, left)).await?;
                continue;
            }
            if self.at("[") {
                left = Box::pin(self.index_suffix(start, left)).await?;
                continue;
            }
            let (precedence, right_associative) = match &self.token().kind {
                TokenKind::Symbol(op) => match op.as_str() {
                    "??" => (1, true),
                    "||" => (2, false),
                    "&&" => (3, false),
                    "==" | "!=" => (4, false),
                    "<" | ">" | "<=" | ">=" => (5, false),
                    "+" | "-" => (6, false),
                    "*" | "/" | "%" => (7, false),
                    _ => break,
                },
                _ => break,
            };
            if precedence < minimum {
                break;
            }
            let right_minimum = if right_associative {
                precedence
            } else {
                precedence + 1
            };
            left = Box::pin(self.binary_suffix(start, left, right_minimum)).await?;
        }
        Ok(left)
    }
    async fn member_suffix(&mut self, start: usize, receiver: ExprId) -> Result<ExprId> {
        let safe = self.eat("?.").await;
        if !safe {
            self.need(".").await?;
        }
        let name = self.name().await?;
        Ok(self
            .expr_node(
                start,
                Expr::Member {
                    receiver,
                    name,
                    safe,
                },
            )
            .await)
    }
    async fn call_suffix(&mut self, start: usize, callee: ExprId) -> Result<ExprId> {
        let types = if self.eat("::").await {
            self.need("<").await?;
            if self.at(">") {
                return Err(self.error("Expected at least one type argument"));
            }
            self.type_list(">").await?
        } else {
            vec![]
        };
        self.need("(").await?;
        let arguments = self.expr_list(")").await?;
        let (callee, arguments) = self.method_call(callee, arguments);
        if let Some(expanded) = self.prelude_call(start, callee, &types, &arguments).await? {
            return Ok(expanded);
        }
        Ok(self
            .expr_node(
                start,
                Expr::Call {
                    callee,
                    types,
                    arguments,
                },
            )
            .await)
    }
    async fn index_suffix(&mut self, start: usize, receiver: ExprId) -> Result<ExprId> {
        self.bump().await;
        let index = Box::pin(self.expr(0)).await?;
        self.need("]").await?;
        Ok(self.expr_node(start, Expr::Index { receiver, index }).await)
    }
    async fn binary_suffix(
        &mut self,
        start: usize,
        left: ExprId,
        right_minimum: u8,
    ) -> Result<ExprId> {
        let token = self.bump().await;
        let TokenKind::Symbol(value) = token.kind else {
            return Err(self.error("Expected operator"));
        };
        let operator = Located {
            value,
            span: token.span,
        };
        let right = Box::pin(self.expr(right_minimum)).await?;
        Ok(self
            .expr_node(
                start,
                Expr::Binary {
                    operator,
                    left,
                    right,
                },
            )
            .await)
    }
    async fn literal(&mut self) -> Option<Literal> {
        let value = match &self.token().kind {
            TokenKind::Number(s) => Literal::Number(s.clone()),
            TokenKind::String(s) => Literal::String(s.clone()),
            TokenKind::Word(s) if s == "true" || s == "false" => Literal::Boolean(s == "true"),
            TokenKind::Word(s) if s == "none" => Literal::None,
            _ => return None,
        };
        self.bump().await;
        Some(value)
    }
    async fn prefix(&mut self) -> Result<ExprId> {
        let start = self.token().span.start;
        if let Some(literal) = Box::pin(self.literal()).await {
            return Ok(self.expr_node(start, Expr::Literal(literal)).await);
        }
        if self.at("!") || self.at("-") || self.at("+") {
            return Box::pin(self.prefix_unary(start)).await;
        }
        if self.at("if") {
            return Box::pin(self.prefix_if(start)).await;
        }
        if self.at("match") {
            return Box::pin(self.prefix_match(start)).await;
        }
        if self.at("|") {
            return Box::pin(self.prefix_lambda(start)).await;
        }
        if self.at("||") {
            return Box::pin(self.prefix_empty_lambda(start)).await;
        }
        if self.at_new() {
            return Box::pin(self.prefix_new(start)).await;
        }
        if self.at("[") {
            return Box::pin(self.prefix_list(start)).await;
        }
        if self.at("(") {
            return Box::pin(self.prefix_group(start)).await;
        }
        if self.at("{") {
            if self.at_record() {
                return Box::pin(self.prefix_record(start)).await;
            }
            return Box::pin(self.block()).await;
        }
        let name = Box::pin(self.name()).await?;
        Ok(self.expr_node(start, Expr::Name(name)).await)
    }

    async fn prefix_unary(&mut self, start: usize) -> Result<ExprId> {
        let token = self.bump().await;
        let TokenKind::Symbol(value) = token.kind else {
            return Err(self.error("Expected unary operator"));
        };
        let operand = Box::pin(self.expr(8)).await?;
        Ok(self
            .expr_node(
                start,
                Expr::Unary {
                    operator: Located {
                        value,
                        span: token.span,
                    },
                    operand,
                },
            )
            .await)
    }
    async fn prefix_if(&mut self, start: usize) -> Result<ExprId> {
        self.bump().await;
        let condition = Box::pin(self.expr(0)).await?;
        let then_branch = Box::pin(self.block()).await?;
        self.need("else").await?;
        let else_branch = Box::pin(self.block()).await?;
        Ok(self
            .expr_node(
                start,
                Expr::If {
                    condition,
                    then_branch,
                    else_branch,
                },
            )
            .await)
    }
    async fn prefix_match(&mut self, start: usize) -> Result<ExprId> {
        let keyword = self.bump().await.span;
        let value = Box::pin(self.expr(0)).await?;
        self.need("{").await?;
        self.match_arms(start, &keyword, value).await
    }
    async fn prefix_lambda(&mut self, start: usize) -> Result<ExprId> {
        self.bump().await;
        let parameters = self.parameters("|", true).await?;
        let body = Box::pin(self.expr(0)).await?;
        Ok(self
            .expr_node(start, Expr::Lambda { parameters, body })
            .await)
    }
    async fn prefix_empty_lambda(&mut self, start: usize) -> Result<ExprId> {
        self.bump().await;
        let body = Box::pin(self.expr(0)).await?;
        Ok(self
            .expr_node(
                start,
                Expr::Lambda {
                    parameters: vec![],
                    body,
                },
            )
            .await)
    }
    async fn prefix_list(&mut self, start: usize) -> Result<ExprId> {
        self.bump().await;
        if self.eat("for").await {
            return self.comprehension(start).await;
        }
        let values = self.expr_list("]").await?;
        Ok(self.expr_node(start, Expr::List(values)).await)
    }
    async fn prefix_group(&mut self, start: usize) -> Result<ExprId> {
        self.bump().await;
        if self.eat(")").await {
            return Ok(self.expr_node(start, Expr::Tuple(vec![])).await);
        }
        let first = Box::pin(self.expr(0)).await?;
        if self.eat(",").await {
            let mut values = vec![first];
            values.extend(Box::pin(self.expr_list(")")).await?);
            return Ok(self.expr_node(start, Expr::Tuple(values)).await);
        }
        self.need(")").await?;
        Ok(self.expr_node(start, Expr::Group(first)).await)
    }
    fn at_record(&self) -> bool {
        // A leading name followed by ':' identifies a record; {} is empty record.
        self.tokens
            .get(self.cursor + 1)
            .is_some_and(|t| t.kind == TokenKind::Symbol("}".into()))
            || self
                .tokens
                .get(self.cursor + 2)
                .is_some_and(|t| t.kind == TokenKind::Symbol(":".into()))
    }
    fn at_new(&self) -> bool {
        // `new` is contextual: only `new <path> {` constructs, so it stays
        // usable as a name.
        self.at("new")
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|t| matches!(t.kind, TokenKind::Word(_)))
    }
    async fn prefix_new(&mut self, start: usize) -> Result<ExprId> {
        self.need("new").await?;
        let class = self.path().await?;
        self.need("{").await?;
        let mut fields = Vec::new();
        while !self.at("}") {
            let name = self.name().await?;
            self.need(":").await?;
            fields.push((name, Box::pin(self.expr(0)).await?));
            if !self.eat(",").await {
                break;
            }
        }
        self.need("}").await?;
        Ok(self.expr_node(start, Expr::New { class, fields }).await)
    }
    async fn prefix_record(&mut self, start: usize) -> Result<ExprId> {
        self.need("{").await?;
        let mut fields = Vec::new();
        while !self.at("}") {
            let name = self.name().await?;
            self.need(":").await?;
            fields.push((name, Box::pin(self.expr(0)).await?));
            if !self.eat(",").await {
                break;
            }
        }
        self.need("}").await?;
        Ok(self.expr_node(start, Expr::Record(fields)).await)
    }
    async fn expr_list(&mut self, end: &str) -> Result<Vec<ExprId>> {
        let mut values = Vec::new();
        if !self.at(end) {
            loop {
                values.push(Box::pin(self.expr(0)).await?);
                if !self.eat(",").await || self.at(end) {
                    break;
                }
            }
        }
        self.need(end).await?;
        Ok(values)
    }
    async fn block(&mut self) -> Result<ExprId> {
        let start = self.token().span.start;
        self.need("{").await?;
        let mut bindings = Vec::new();
        while self.at("let") {
            bindings.push(Box::pin(self.binding()).await?);
        }
        let result = Box::pin(self.expr(0)).await?;
        self.need("}").await?;
        Ok(self
            .expr_node(start, Expr::Block { bindings, result })
            .await)
    }
    async fn binding(&mut self) -> Result<Located<Binding>> {
        let start = self.token().span.start;
        self.bump().await;
        let name = self.name().await?;
        let ty = if self.eat(":").await {
            Some(Box::pin(self.ty()).await?)
        } else {
            None
        };
        self.need("=").await?;
        let value = Box::pin(self.expr(0)).await?;
        self.need(";").await?;
        Ok(self.located(start, Binding { name, ty, value }))
    }
    async fn pattern(&mut self) -> Result<PatternId> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error("Rcore syntax nesting exceeds 64 levels"));
        }
        self.depth += 1;
        let result = self.pattern_inner().await;
        self.depth -= 1;
        result
    }
    async fn pattern_inner(&mut self) -> Result<PatternId> {
        let start = self.token().span.start;
        let value = if self.eat("_").await {
            Pattern::Wildcard
        } else if self.eat("-").await {
            let TokenKind::Number(number) = &self.token().kind else {
                return Err(self.error("Expected number after pattern minus"));
            };
            let value = Pattern::Literal(Literal::Number(format!("-{number}")));
            self.bump().await;
            value
        } else if let Some(literal) = self.literal().await {
            Pattern::Literal(literal)
        } else {
            let path = self.path().await?;
            if self.eat("(").await {
                let mut arguments = Vec::new();
                if !self.at(")") {
                    loop {
                        arguments.push(Box::pin(self.pattern()).await?);
                        if !self.eat(",").await || self.at(")") {
                            break;
                        }
                    }
                }
                self.need(")").await?;
                Pattern::Constructor { path, arguments }
            } else if path.len() == 1 && path[0].value != "None" {
                let name = path
                    .into_iter()
                    .next()
                    .ok_or_else(|| self.error("Expected pattern"))?;
                Pattern::Binding(name)
            } else {
                Pattern::Constructor {
                    path,
                    arguments: vec![],
                }
            }
        };
        let id = PatternId(self.document.patterns.len());
        self.document.patterns.push(self.located(start, value));
        self.traversal.node().await;
        Ok(id)
    }
}

// Staged syntax continuation. Module binding, typing,
// lowering, ordering and actual retained compiler hosts remain open in the
// declared behavior-compilation work package.
#[derive(Clone, Default)]
struct Traversal(std::sync::Arc<Counts>);
#[derive(Default)]
struct Counts {
    phase: std::sync::atomic::AtomicUsize,
    tokens: std::sync::atomic::AtomicUsize,
    nodes: std::sync::atomic::AtomicUsize,
}
impl Traversal {
    async fn token(&self) {
        self.0
            .tokens
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::definitions::continuation::pause().await;
    }
    async fn node(&self) {
        self.0
            .nodes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::definitions::continuation::pause().await;
    }
}

#[cfg(test)]
mod retained_syntax_controls {
    use super::*;
    use std::{
        future::Future,
        sync::atomic::Ordering,
        task::{Context, Poll, Waker},
    };

    fn observe_stack_case(
        case: &serde_json::Value,
        quantum: usize,
    ) -> (serde_json::Value, Vec<[usize; 4]>) {
        let source: DefinitionSource = serde_json::from_value(case["source"].clone()).unwrap();
        let traversal = Traversal::default();
        let mut pending = Box::pin(parse_retained(
            source,
            Default::default(),
            traversal.clone(),
        ));
        let mut trace = vec![];
        loop {
            let mut context = Context::from_waker(Waker::noop());
            for _ in 0..quantum {
                let outcome = pending.as_mut().poll(&mut context);
                trace.push([
                    traversal.0.phase.load(Ordering::Relaxed),
                    traversal.0.tokens.load(Ordering::Relaxed),
                    traversal.0.nodes.load(Ordering::Relaxed),
                    usize::from(outcome.is_ready()),
                ]);
                if let Poll::Ready(document) = outcome {
                    return (serde_json::to_value(document).unwrap(), trace);
                }
            }
        }
    }

    #[test]
    fn deep_syntax_preserves_complete_documents_and_poll_traces_on_one_mib() {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let baseline: serde_json::Value = serde_json::from_str(include_str!(
                    "../../assets/fixtures/rcore-behavior-parser-stack-baseline.json"
                ))
                .unwrap();
                for case in baseline["cases"].as_array().unwrap() {
                    let expected: Vec<[usize; 4]> =
                        serde_json::from_value(case["trace"].clone()).unwrap();
                    for quantum in [1, 3, 256] {
                        eprintln!("trace {} q{quantum}", case["id"]);
                        let (document, trace) = observe_stack_case(case, quantum);
                        assert_eq!(document, case["document"], "{} q{quantum}", case["id"]);
                        assert_eq!(trace, expected, "{} q{quantum}", case["id"]);
                    }
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn deep_syntax_drop_at_every_poll_releases_state_and_recovers_on_one_mib() {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let baseline: serde_json::Value = serde_json::from_str(include_str!(
                    "../../assets/fixtures/rcore-behavior-parser-stack-baseline.json"
                ))
                .unwrap();
                for case in baseline["cases"].as_array().unwrap() {
                    let source: DefinitionSource =
                        serde_json::from_value(case["source"].clone()).unwrap();
                    let expected: Vec<[usize; 4]> =
                        serde_json::from_value(case["trace"].clone()).unwrap();
                    eprintln!("drop {} ({} polls)", case["id"], expected.len());
                    for prefix in 0..expected.len() {
                        let traversal = Traversal::default();
                        let mut pending = Box::pin(parse_retained(
                            source.clone(),
                            Default::default(),
                            traversal.clone(),
                        ));
                        let mut context = Context::from_waker(Waker::noop());
                        for row in expected.iter().take(prefix) {
                            assert!(pending.as_mut().poll(&mut context).is_pending());
                            assert_eq!(
                                [
                                    traversal.0.phase.load(Ordering::Relaxed),
                                    traversal.0.tokens.load(Ordering::Relaxed),
                                    traversal.0.nodes.load(Ordering::Relaxed),
                                    0
                                ],
                                *row
                            );
                        }
                        drop(pending);
                        assert_eq!(std::sync::Arc::strong_count(&traversal.0), 1);
                    }
                    let (document, trace) = observe_stack_case(case, 1);
                    assert_eq!(document, case["document"]);
                    assert_eq!(trace, expected);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn lexical_and_structural_range_preludes_yield_before_behavior_arena_mutation() {
        use crate::definitions::bundle::SourceKind;
        for kind in [SourceKind::RcoreBehavior, SourceKind::Rcore] {
            let header = format!("rcore 1;package p version \"1\";/*{}*/", "λ🚀".repeat(4000));
            let text = if kind == SourceKind::Rcore {
                let classes = (0..1000)
                    .map(|i| format!("class C{i} {{}}"))
                    .collect::<String>();
                format!(
                    "{header}metamodel \"urn:prelude\" prefix p {{{classes}}}behavior {{fn run()->Int32=7;}}"
                )
            } else {
                format!("{header}fn run()->Int32=7;")
            };
            let source = DefinitionSource::new("memory:prelude.rcore", kind, text);
            let expected = parse(source.clone());
            assert!(expected.diagnostics.is_empty());
            let traversal = Traversal::default();
            let mut pending = Box::pin(parse_retained(
                source.clone(),
                Default::default(),
                traversal.clone(),
            ));
            let mut context = Context::from_waker(Waker::noop());
            let mut preludes = [0, 0];
            let result = loop {
                let outcome = pending.as_mut().poll(&mut context);
                let phase = traversal.0.phase.load(Ordering::Relaxed);
                if phase < 2 {
                    preludes[phase] += 1;
                    assert_eq!(traversal.0.tokens.load(Ordering::Relaxed), 0);
                    assert_eq!(traversal.0.nodes.load(Ordering::Relaxed), 0);
                    assert!(matches!(outcome, Poll::Pending));
                }
                if let Poll::Ready(result) = outcome {
                    break result;
                }
            };
            assert_eq!(result, expected);
            assert!(preludes[0] > source.text.len() / 256);
            if kind == SourceKind::Rcore {
                assert!(preludes[1] > 1000);
            } else {
                assert_eq!(preludes[1], 0);
            }
            assert_eq!(traversal.0.phase.load(Ordering::Relaxed), 3);
        }
    }
    #[test]
    fn syntax_future_retains_token_and_arena_progress_across_quanta_and_recovery() {
        for text in [
            "rcore 1;package p version \"1\";fn run(x:Sequence<Int32>)->Sequence<Int32>=[for a in x,b in x where a>0 yield a+b];",
            "rcore 1;package p version \"1\";fn bad( ->Int32=;fn good(x:Sequence<Int32?>)->Int32=match x {some(v)=>v,_=>0};",
        ] {
            let source = DefinitionSource::new(
                "memory:retained-syntax.rcore",
                crate::definitions::bundle::SourceKind::RcoreBehavior,
                text,
            );
            let expected = parse(source.clone());
            for quantum in [1, 3, 256] {
                let traversal = Traversal::default();
                let mut pending = Box::pin(parse_retained(
                    source.clone(),
                    Default::default(),
                    traversal.clone(),
                ));
                let mut context = Context::from_waker(Waker::noop());
                let mut previous = (0, 0);
                let mut pauses = 0;
                let result = 'resume: loop {
                    for _ in 0..quantum {
                        let outcome = pending.as_mut().poll(&mut context);
                        let current = (
                            traversal.0.tokens.load(Ordering::Relaxed),
                            traversal.0.nodes.load(Ordering::Relaxed),
                        );
                        assert!(current.0 - previous.0 + current.1 - previous.1 <= 1);
                        previous = current;
                        match outcome {
                            Poll::Ready(result) => break 'resume result,
                            Poll::Pending => pauses += 1,
                        }
                    }
                };
                assert_eq!(result, expected);
                assert!(pauses > 20 && previous.0 > 20 && previous.1 > 3);
            }
        }
    }
}
