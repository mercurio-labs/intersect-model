//! Handwritten Rcore parser feeding direct shared definitions and explicit XML
//! export. The existing resolver and model services own structural semantics.
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind, SourceSpan},
    rcore_behavior::{self, SyntaxDiagnostic, Token, TokenKind},
    resources::DeclarationId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub declaration: DeclarationId,
    pub original: SourceSpan,
    pub generated: SourceSpan,
}

/// Both artifacts have distinct content identities. Origins are correspondence,
/// never an assertion that generated declarations have the original identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralLowering {
    pub version: u32,
    pub original: DefinitionSource,
    pub generated: DefinitionSource,
    pub origins: Vec<Origin>,
    pub classifiers: Vec<StructuralClassifier>,
    #[serde(default)]
    pub operations: Vec<StructuralOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<BehaviorLowering>,
}

/// An explicit projection of the authored behavior, consumed by the existing
/// typed frontend. It is not the original source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorLowering {
    pub source: DefinitionSource,
    pub original: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralClassifier {
    pub name: String,
    pub declaration: DeclarationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralOperation {
    pub owner: DeclarationId,
    pub name: String,
    pub declaration: DeclarationId,
    pub original: SourceSpan,
}
struct OperationSyntax {
    name: String,
    parameters: Vec<Feature>,
    result: Option<Feature>,
    type_parameters: Vec<TypeParameterSyntax>,
    exceptions: Vec<TypeSyntax>,
    annotations: Vec<AnnotationSyntax>,
    span: SourceSpan,
}

struct Feature {
    name: String,
    ty: TypeSyntax,
    reference: bool,
    lower: u32,
    upper: Option<u32>,
    flags: BTreeSet<String>,
    opposite: Option<String>,
    annotations: Vec<AnnotationSyntax>,
    default: Option<DefaultSyntax>,
    span: SourceSpan,
}
enum DefaultSyntax {
    Literal(String),
    Enum { owner: String, literal: String },
}
struct Class {
    name: String,
    abstract_type: bool,
    interface_type: bool,
    parents: Vec<TypeSyntax>,
    type_parameters: Vec<TypeParameterSyntax>,
    metadata: ClassifierMetadata,
    annotations: Vec<AnnotationSyntax>,
    features: Vec<Feature>,
    operations: Vec<OperationSyntax>,
    span: SourceSpan,
}
struct EnumLiteralSyntax {
    name: String,
    value: i32,
    literal: Option<String>,
    annotations: Vec<AnnotationSyntax>,
    span: SourceSpan,
}
struct EnumSyntax {
    name: String,
    literals: Vec<EnumLiteralSyntax>,
    serializable: bool,
    metadata: ClassifierMetadata,
    annotations: Vec<AnnotationSyntax>,
    span: SourceSpan,
}
struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    traversal: preparation::Traversal,
}
impl Parser {
    fn token(&self) -> &Token {
        &self.tokens[self.cursor]
    }
    fn error(&self, message: impl Into<String>) -> SyntaxDiagnostic {
        SyntaxDiagnostic {
            span: self.token().span.clone(),
            message: message.into(),
        }
    }
    async fn eat(&mut self, word: &str) -> bool {
        if matches!(&self.token().kind, TokenKind::Word(s) | TokenKind::Symbol(s) | TokenKind::Number(s) if s == word)
        {
            self.cursor += 1;
            self.traversal.token().await;
            true
        } else {
            false
        }
    }
    async fn need(&mut self, word: &str) -> Result<(), SyntaxDiagnostic> {
        if self.eat(word).await {
            Ok(())
        } else {
            Err(self.error(format!("Expected {word}")))
        }
    }
    async fn name(&mut self) -> Result<String, SyntaxDiagnostic> {
        if let TokenKind::QuotedName(name) = &self.token().kind {
            let name = name.clone();
            self.cursor += 1;
            self.traversal.token().await;
            return Ok(name);
        }
        let TokenKind::Word(s) = &self.token().kind else {
            return Err(self.error("Expected a structural identifier"));
        };
        if crate::definitions::rcore_names::STRUCTURE_RESERVED.contains(&s.as_str()) {
            return Err(self.error("Reserved structural identifier"));
        }
        let result = s.clone();
        self.cursor += 1;
        self.traversal.token().await;
        Ok(result)
    }
    async fn string(&mut self) -> Result<String, SyntaxDiagnostic> {
        let TokenKind::String(s) = &self.token().kind else {
            return Err(self.error("Expected a string"));
        };
        let result = s.clone();
        self.cursor += 1;
        self.traversal.token().await;
        Ok(result)
    }
    async fn number(&mut self) -> Result<u32, SyntaxDiagnostic> {
        let TokenKind::Number(s) = &self.token().kind else {
            return Err(self.error("Expected a nonnegative integer bound"));
        };
        let result = s
            .parse::<u32>()
            .map_err(|_| self.error("Invalid or overflowing multiplicity bound"))?;
        if result > i32::MAX as u32 {
            return Err(self.error("Multiplicity exceeds Ecore integer range"));
        }
        self.cursor += 1;
        self.traversal.token().await;
        Ok(result)
    }
    fn span(&self, start: usize) -> SourceSpan {
        SourceSpan {
            source: self.token().span.source.clone(),
            start,
            end: self.tokens[self.cursor.saturating_sub(1)].span.end,
        }
    }
    async fn enumeration(&mut self) -> Result<EnumSyntax, SyntaxDiagnostic> {
        let start = self.token().span.start;
        self.need("enum").await?;
        let name = self.name().await?;
        let metadata = self.classifier_metadata().await?;
        let serializable = !self.eat("nonserializable").await;
        self.need("{").await?;
        let mut literals = Vec::new();
        let mut annotations = Vec::new();
        let mut names = BTreeSet::new();
        while !self.eat("}").await {
            if self.at("annotation") {
                annotations.push(Box::pin(self.annotation(0)).await?);
                continue;
            }
            let start = self.token().span.start;
            let name = self.name().await?;
            if !names.insert(name.clone()) {
                return Err(self.error("Duplicate enum literal name"));
            }
            self.need("=").await?;
            let negative = self.eat("-").await;
            let TokenKind::Number(number) = &self.token().kind else {
                return Err(self.error("Expected a signed 32-bit enum value"));
            };
            let spelling = if negative {
                format!("-{number}")
            } else {
                number.clone()
            };
            let value = spelling
                .parse::<i32>()
                .map_err(|_| self.error("Invalid or overflowing enum value"))?;
            self.cursor += 1;
            self.traversal.token().await;
            let literal = if self.eat("literal").await {
                Some(self.string().await?)
            } else {
                None
            };
            let annotations = self.annotations().await?;
            self.need(";").await?;
            literals.push(EnumLiteralSyntax {
                name,
                value,
                literal,
                annotations,
                span: self.span(start),
            });
        }
        Ok(EnumSyntax {
            name,
            literals,
            serializable,
            metadata,
            annotations,
            span: self.span(start),
        })
    }
    async fn class(&mut self) -> Result<Class, SyntaxDiagnostic> {
        let start = self.token().span.start;
        let abstract_type = self.eat("abstract").await;
        let interface_type = self.eat("interface").await;
        self.need("class").await?;
        let name = self.name().await?;
        let type_parameters = self.type_parameters().await?;
        let metadata = self.classifier_metadata().await?;
        let mut parents = Vec::new();
        if self.eat("extends").await {
            parents.push(Box::pin(self.type_expression(0)).await?);
            while self.eat(",").await {
                parents.push(Box::pin(self.type_expression(0)).await?);
            }
        }
        self.need("{").await?;
        let mut features = Vec::new();
        let mut operations = Vec::new();
        let mut annotations = Vec::new();
        let mut names = BTreeSet::new();
        while !self.eat("}").await {
            if self.at("annotation") {
                annotations.push(Box::pin(self.annotation(0)).await?);
                continue;
            }
            if self.eat("op").await {
                operations.push(self.operation().await?);
                continue;
            }
            let feature = self.feature().await?;
            if !names.insert(feature.name.clone()) {
                return Err(SyntaxDiagnostic {
                    span: feature.span,
                    message: "Duplicate feature".into(),
                });
            }
            features.push(feature);
        }
        Ok(Class {
            name,
            abstract_type,
            interface_type,
            parents,
            type_parameters,
            metadata,
            annotations,
            features,
            operations,
            span: self.span(start),
        })
    }
    async fn operation(&mut self) -> Result<OperationSyntax, SyntaxDiagnostic> {
        let start = self.tokens[self.cursor - 1].span.start;
        let name = self.name().await?;
        let type_parameters = self.type_parameters().await?;
        self.need("(").await?;
        let mut parameters = Vec::new();
        let mut names = BTreeSet::new();
        if !self.eat(")").await {
            loop {
                let name = self.name().await?;
                if !names.insert(name.clone()) {
                    return Err(self.error("Duplicate operation parameter"));
                }
                self.need(":").await?;
                parameters.push(self.signature_slot(name).await?);
                if self.eat(")").await {
                    break;
                }
                self.need(",").await?;
            }
        }
        self.need("->").await?;
        let result = if self.eat("void").await {
            None
        } else {
            Some(self.signature_slot("result".into()).await?)
        };
        let mut exceptions = Vec::new();
        if self.eat("throws").await {
            loop {
                exceptions.push(Box::pin(self.type_expression(0)).await?);
                if !self.eat(",").await {
                    break;
                }
            }
        }
        let annotations = self.annotations().await?;
        self.need(";").await?;
        Ok(OperationSyntax {
            name,
            parameters,
            result,
            type_parameters,
            exceptions,
            annotations,
            span: self.span(start),
        })
    }
    async fn signature_slot(&mut self, name: String) -> Result<Feature, SyntaxDiagnostic> {
        let start = self.token().span.start;
        let ty = Box::pin(self.type_expression(0)).await?;
        self.need("[").await?;
        let lower = self.number().await?;
        self.need(",").await?;
        let upper = if self.eat("*").await {
            None
        } else {
            Some(self.number().await?)
        };
        self.need("]").await?;
        if upper.is_some_and(|n| lower > n) {
            return Err(self.error("Lower bound exceeds upper bound"));
        }
        let mut flags = BTreeSet::new();
        while let TokenKind::Word(flag) = &self.token().kind {
            if !["unordered", "nonunique"].contains(&flag.as_str()) {
                break;
            }
            let flag = flag.clone();
            if !flags.insert(flag) {
                return Err(self.error("Duplicate operation signature modifier"));
            }
            self.cursor += 1;
            self.traversal.token().await;
        }
        Ok(Feature {
            name,
            ty,
            reference: false,
            lower,
            upper,
            flags,
            annotations: self.annotations().await?,
            opposite: None,
            default: None,
            span: self.span(start),
        })
    }
    async fn feature(&mut self) -> Result<Feature, SyntaxDiagnostic> {
        let start = self.token().span.start;
        let reference = self.eat("ref").await;
        if !reference {
            self.need("attr").await?;
        }
        let name = self.name().await?;
        self.need(":").await?;
        let ty = Box::pin(self.type_expression(0)).await?;
        self.need("[").await?;
        let lower = self.number().await?;
        self.need(",").await?;
        let upper = if self.eat("*").await {
            None
        } else {
            Some(self.number().await?)
        };
        self.need("]").await?;
        if upper.is_some_and(|n| lower > n) {
            return Err(self.error("Lower bound exceeds upper bound"));
        }
        let mut flags = BTreeSet::new();
        let mut opposite = None;
        let mut default = None;
        let mut annotations = Vec::new();
        while !self.eat(";").await {
            if self.at("annotation") {
                annotations.push(Box::pin(self.annotation(0)).await?);
                continue;
            }
            if self.eat("default").await {
                if reference || default.is_some() {
                    return Err(self.error("Default requires an attribute and may occur once"));
                }
                let negative = self.eat("-").await;
                if !negative
                    && matches!(&self.token().kind, TokenKind::Word(s) if !["true", "false"].contains(&s.as_str()))
                {
                    let owner = self.name().await?;
                    self.need(".").await?;
                    let literal = self.name().await?;
                    default = Some(DefaultSyntax::Enum { owner, literal });
                    continue;
                }
                let value = match &self.token().kind {
                    TokenKind::Number(s) => {
                        if negative {
                            format!("-{s}")
                        } else {
                            s.clone()
                        }
                    }
                    TokenKind::String(s) if !negative => s.clone(),
                    TokenKind::Word(s) if !negative && ["true", "false"].contains(&s.as_str()) => {
                        s.clone()
                    }
                    _ => {
                        return Err(self.error(
                            "Expected a string, numeric, boolean or qualified enum default literal",
                        ));
                    }
                };
                self.cursor += 1;
                self.traversal.token().await;
                default = Some(DefaultSyntax::Literal(value));
                continue;
            }
            if self.eat("opposite").await {
                if !reference || opposite.is_some() {
                    return Err(self.error("Opposite requires a reference and may occur once"));
                }
                opposite = Some(if self.eat("external").await {
                    self.string().await?
                } else {
                    let owner = self.qualified_name().await?;
                    self.need(".").await?;
                    format!("#//{owner}/{}", self.name().await?)
                });
            } else {
                let flag = match &self.token().kind {
                    TokenKind::Word(s)
                        if [
                            "unordered",
                            "nonunique",
                            "containment",
                            "derived",
                            "readonly",
                            "transient",
                            "volatile",
                            "unsettable",
                            "id",
                            "noresolve",
                        ]
                        .contains(&s.as_str()) =>
                    {
                        s.clone()
                    }
                    _ => return Err(self.error("Unsupported structural modifier")),
                };
                if flag == "containment" && !reference {
                    return Err(self.error("Containment requires a reference"));
                }
                if !flags.insert(flag) {
                    return Err(self.error("Duplicate structural modifier"));
                }
                self.cursor += 1;
                self.traversal.token().await;
            }
        }
        Ok(Feature {
            name,
            ty,
            reference,
            lower,
            upper,
            flags,
            opposite,
            annotations,
            default,
            span: self.span(start),
        })
    }
}
fn escaped(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#9;")
}
fn attr(xml: &mut String, key: &str, value: &str) {
    xml.push_str(&format!(" {key}=\"{}\"", escaped(value)));
}
pub(crate) fn scalar(name: &str) -> Option<&str> {
    match name {
        "String" => Some("EString"),
        "Bool" => Some("EBoolean"),
        "Int32" => Some("EInt"),
        "Int64" => Some("ELong"),
        "Float32" => Some("EFloat"),
        "Float64" => Some("EDouble"),
        _ if crate::definitions::scalars::Builtin::named(name).is_some() => Some(name),
        _ => None,
    }
}

/// Explicit structural XML export with an optional behavior projection.
/// Generic templates are declarations; export does not select runtime types.
pub fn lower_structure(
    source: DefinitionSource,
    generated_uri: &str,
) -> Result<StructuralLowering, Vec<SyntaxDiagnostic>> {
    lower_structure_context(source, generated_uri, true)
}

/// Explicit, source-bound XML projections for a complete canonical resource graph.
/// The original and projected graphs are verified independently; generated XML
/// identities never replace their authored Rcore identities.
#[derive(Clone, Debug, Serialize)]
pub struct StructuralBundleLowering {
    pub original: crate::definitions::bundle::DefinitionBundle,
    pub bundle: crate::definitions::bundle::DefinitionBundle,
    pub entry: crate::definitions::bundle::SourceId,
    pub namespace: String,
    pub lowerings: Vec<StructuralLowering>,
}

pub fn lower_structure_bundle(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    entry: &crate::definitions::bundle::SourceId,
    namespace: &str,
    generated_uri_prefix: &str,
) -> Result<StructuralBundleLowering, Vec<SyntaxDiagnostic>> {
    let failure = |message: String| {
        vec![SyntaxDiagnostic {
            span: SourceSpan {
                source: entry.clone(),
                start: 0,
                end: 0,
            },
            message,
        }]
    };
    bundle.validate().map_err(failure)?;
    // Validate authored identities and structural references before projecting.
    direct::compile_structure_selected(bundle, entry, Some(namespace))?;
    let mut lowerings = Vec::new();
    for (index, source) in bundle.sources.iter().enumerate() {
        if source.kind == SourceKind::Rcore {
            lowerings.push(lower_structure_context(
                source.clone(),
                &format!("{generated_uri_prefix}/{index}.ecore"),
                false,
            )?);
        }
    }
    let replacements = lowerings
        .iter()
        .map(|l| (l.original.id.clone(), l.generated.id.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let replace = |id: &crate::definitions::bundle::SourceId| {
        replacements.get(id).cloned().unwrap_or_else(|| id.clone())
    };
    let mut projected = bundle.clone();
    for source in &mut projected.sources {
        if let Some(lowering) = lowerings.iter().find(|l| l.original.id == source.id) {
            *source = lowering.generated.clone();
            // Xtext/behavior edges retain provenance in `original`, but are
            // not structural resource grants. Match Rcore admission's existing
            // structural-only traversal when the target representation is XML.
            source.dependencies = lowering
                .original
                .dependencies
                .iter()
                .filter(|id| replacements.contains_key(*id))
                .map(&replace)
                .collect();
        } else {
            source.dependencies = source.dependencies.iter().map(&replace).collect();
        }
    }
    for binding in &mut projected.namespaces {
        binding.source = replace(&binding.source);
    }
    let projected_entry = replace(entry);
    let diagnostic = |error: crate::definitions::ecore::ImportError| {
        let span = lowerings
            .iter()
            .find(|l| l.generated.id == error.span.source)
            .map(|l| {
                l.origins
                    .iter()
                    .filter(|o| {
                        o.generated.start <= error.span.start && error.span.start < o.generated.end
                    })
                    .min_by_key(|o| o.generated.end - o.generated.start)
                    .map(|o| o.original.clone())
                    .unwrap_or_else(|| SourceSpan {
                        source: l.original.id.clone(),
                        start: 0,
                        end: l.original.text.len(),
                    })
            })
            .unwrap_or(error.span);
        vec![SyntaxDiagnostic {
            span,
            message: error.message,
        }]
    };
    crate::definitions::generic_resolution::Resolver::new(&projected)
        .and_then(|r| r.resolve_all())
        .map_err(diagnostic)?;
    crate::definitions::compiler::inspect_ecore_bundle(&projected, &projected_entry, namespace)
        .map_err(diagnostic)?;
    Ok(StructuralBundleLowering {
        original: bundle.clone(),
        bundle: projected,
        entry: projected_entry,
        namespace: namespace.to_owned(),
        lowerings,
    })
}

fn lower_structure_context(
    source: DefinitionSource,
    generated_uri: &str,
    standalone_validation: bool,
) -> Result<StructuralLowering, Vec<SyntaxDiagnostic>> {
    let parsed = parse_structure(&source)?;
    let behavior_range = parsed.behavior_range;
    let error = |message: String| {
        vec![SyntaxDiagnostic {
            span: SourceSpan {
                source: source.id.clone(),
                start: 0,
                end: source.text.len(),
            },
            message,
        }]
    };
    let document = direct::document(&source, crate::definitions::ecore::ParseLimits::default())
        .map_err(|e| {
            vec![SyntaxDiagnostic {
                span: e.span,
                message: e.message,
            }]
        })?;
    let (xml, spans) = export::xml(&document);
    if xml.len() > 256_000 {
        return Err(error("Generated definition size budget exceeded".into()));
    }
    let generated = DefinitionSource::new(generated_uri, SourceKind::Ecore, xml);
    generated.validate().map_err(error)?;
    // Validate standalone exports using the shared resolver. Cross-resource exports
    // require the caller's explicitly mapped export bundle.
    if standalone_validation && source.dependencies.is_empty() {
        let diagnostic = |e: crate::definitions::ecore::ImportError| {
            let origin = spans
                .iter()
                .filter(|(_, _, start, end)| *start <= e.span.start && e.span.start < *end)
                .min_by_key(|(_, _, start, end)| end - start);
            vec![SyntaxDiagnostic {
                span: origin.map_or_else(
                    || document.root.span.clone(),
                    |(_, span, _, _)| span.clone(),
                ),
                message: e.message,
            }]
        };
        let exported = crate::definitions::bundle::DefinitionBundle {
            version: 1,
            sources: vec![generated.clone()],
            namespaces: vec![],
        };
        crate::definitions::generic_resolution::Resolver::new(&exported)
            .and_then(|r| r.resolve_all())
            .map_err(diagnostic)?;
        let namespaces: Vec<_> = if parsed.resource {
            parsed
                .packages
                .iter()
                .map(|p| p.namespace.as_str())
                .collect()
        } else {
            vec![parsed.namespace.as_str()]
        };
        for namespace in namespaces {
            if let Err(error) = crate::definitions::compiler::inspect_ecore_bundle(
                &exported,
                &generated.id,
                namespace,
            ) && error.kind != crate::definitions::ecore::ErrorKind::Unsupported
            {
                return Err(diagnostic(error));
            }
        }
    }
    let origins = spans
        .into_iter()
        .map(|(path, original, start, end)| Origin {
            declaration: DeclarationId {
                source: generated.id.clone(),
                path,
            },
            original,
            generated: SourceSpan {
                source: generated.id.clone(),
                start,
                end,
            },
        })
        .collect();
    let (mut classifiers, mut operations) = direct::inventories(&document);
    for c in &mut classifiers {
        c.declaration.source = generated.id.clone();
    }
    for o in &mut operations {
        o.declaration.source = generated.id.clone();
        o.owner.source = generated.id.clone();
    }
    let behavior = if let Some(BehaviorRange {
        structure_start,
        body_start,
        body_end,
    }) = behavior_range
    {
        let text = {
            // Mask by byte, preserving every UTF-8 offset and line ending. The
            // package header is shared; only the structure and section wrappers
            // disappear from the semantic projection.
            let mut bytes = source.text.as_bytes().to_vec();
            for range in [structure_start..body_start, body_end..bytes.len()] {
                for byte in &mut bytes[range] {
                    if !matches!(*byte, b'\r' | b'\n') {
                        *byte = b' ';
                    }
                }
            }
            String::from_utf8(bytes).map_err(|e| error(e.to_string()))?
        };
        let mut projected = DefinitionSource::new(
            format!("{generated_uri}.behavior.rcore"),
            SourceKind::RcoreBehavior,
            text,
        );
        projected.dependencies = vec![generated.id.clone()];
        projected.validate().map_err(error)?;
        {
            let parsed = rcore_behavior::parse(projected.clone());
            if !parsed.diagnostics.is_empty() {
                return Err(parsed
                    .diagnostics
                    .into_iter()
                    .map(|mut d| {
                        d.span.source = source.id.clone();
                        d
                    })
                    .collect());
            }
        }
        Some(BehaviorLowering {
            source: projected,
            original: SourceSpan {
                source: source.id.clone(),
                start: body_start,
                end: body_end,
            },
        })
    } else {
        None
    };
    Ok(StructuralLowering {
        version: 1,
        original: source,
        generated,
        origins,
        classifiers,
        operations,
        behavior,
    })
}

// The handwritten parser feeds both direct definitions and the explicit XML
// export used by existing editor consumers. Direct compilation never exports XML.
#[derive(Clone, Copy)]
struct BehaviorRange {
    structure_start: usize,
    body_start: usize,
    body_end: usize,
}
struct Structure {
    resource: bool,
    identity: Option<String>,
    package: String,
    namespace: String,
    prefix: String,
    classes: Vec<Class>,
    enums: Vec<EnumSyntax>,
    datatypes: Vec<DatatypeSyntax>,
    packages: Vec<Structure>,
    annotations: Vec<AnnotationSyntax>,
    span: SourceSpan,
    behavior_range: Option<BehaviorRange>,
}
fn parse_structure(source: &DefinitionSource) -> Result<Structure, Vec<SyntaxDiagnostic>> {
    parse_structure_with_limit(source, 256_000)
}
fn parse_structure_with_limit(
    source: &DefinitionSource,
    source_bytes: usize,
) -> Result<Structure, Vec<SyntaxDiagnostic>> {
    preparation::complete(parse_structure_with_limit_retained(source, source_bytes))
}
async fn parse_structure_with_limit_retained(
    source: &DefinitionSource,
    source_bytes: usize,
) -> Result<Structure, Vec<SyntaxDiagnostic>> {
    let error = |message: String| {
        vec![SyntaxDiagnostic {
            span: SourceSpan {
                source: source.id.clone(),
                start: 0,
                end: source.text.len(),
            },
            message,
        }]
    };
    let mut validation = crate::definitions::bundle::SourceValidation::new(source);
    loop {
        let complete = validation.advance().map_err(error)?;
        crate::definitions::continuation::pause().await;
        if complete {
            break;
        }
    }
    if source.text.len() > source_bytes {
        return Err(error("Definition size budget exceeded".into()));
    }
    let (tokens, diagnostics) = rcore_behavior::lex_parts_retained(source).await;
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    parse_structure_tokens_retained(tokens, Default::default()).await
}
async fn parse_structure_tokens_retained(
    tokens: Vec<Token>,
    traversal: preparation::Traversal,
) -> Result<Structure, Vec<SyntaxDiagnostic>> {
    let mut parser = Parser {
        tokens,
        cursor: 0,
        traversal,
    };
    let parsed = async {
        parser.need("rcore").await?;
        parser.need("1").await?;
        parser.need(";").await?;
        parser.need("package").await?;
        let mut package = parser.name().await?;
        while parser.eat(".").await {
            package.push('.');
            package.push_str(&parser.name().await?);
        }
        parser.need("version").await?;
        parser.string().await?;
        parser.need(";").await?;
        let structure_start = parser.token().span.start;
        let resource = parser.eat("resource").await;
        let (namespace, prefix) = if resource {
            (String::new(), String::new())
        } else {
            parser.need("metamodel").await?;
            let namespace = parser.string().await?;
            parser.need("prefix").await?;
            (namespace, parser.name().await?)
        };
        parser.need("{").await?;
        let mut structure =
            Box::pin(parser.package_body(package, namespace, prefix, structure_start, 0)).await?;
        structure.resource = resource;
        if resource
            && (structure.identity.is_some()
                || structure.packages.is_empty()
                || !structure.classes.is_empty()
                || !structure.enums.is_empty()
                || !structure.datatypes.is_empty()
                || !structure.annotations.is_empty())
        {
            return Err(parser.error("A resource contains one or more package declarations"));
        }
        let behavior = if parser.eat("behavior").await {
            parser.need("{").await?;
            let body_start = parser.tokens[parser.cursor - 1].span.end;
            let mut depth = 1usize;
            while depth != 0 {
                if parser.token().kind == TokenKind::End {
                    return Err(parser.error("Unclosed Rcore behavior section"));
                }
                if parser.eat("{").await {
                    depth += 1;
                    if depth > 64 {
                        return Err(parser.error("Behavior nesting budget exceeded"));
                    }
                } else if parser.eat("}").await {
                    depth -= 1;
                } else {
                    parser.cursor += 1;
                    parser.traversal.token().await;
                }
            }
            let closing = &parser.tokens[parser.cursor - 1].span;
            Some(BehaviorRange {
                structure_start,
                body_start,
                body_end: closing.start,
            })
        } else {
            None
        };
        if parser.token().kind != TokenKind::End {
            return Err(
                parser.error("Unsupported declaration after structural metamodel or behavior")
            );
        }
        structure.behavior_range = behavior;
        Ok(structure)
    }
    .await;
    parsed.map_err(|e| vec![e])
}

/// Read the namespace of a single-package structural source. Multi-package
/// resources require an explicit namespace selection at compilation.
pub fn declared_namespace(source: &DefinitionSource) -> Result<String, Vec<SyntaxDiagnostic>> {
    let structure = parse_structure(source)?;
    if structure.resource {
        return Err(vec![SyntaxDiagnostic {
            span: structure.span,
            message: "A multi-package resource requires an explicit namespace selection".into(),
        }]);
    }
    Ok(structure.namespace)
}

#[path = "rcore_direct.rs"]
mod direct;
pub use direct::{
    DirectStructure, compile_structure, compile_structure_selected,
    compile_structure_with_conversions, feature_redefinitions, resolve_structure,
};
pub(crate) use direct::{
    behavior_tokens, behavior_tokens_retained, check_document_limits, document,
};

#[path = "rcore_structure_syntax.rs"]
mod structure_syntax;
use structure_syntax::*;
#[path = "rcore_export.rs"]
mod export;

#[path = "rcore_document_preparation.rs"]
mod preparation;
pub use preparation::{DocumentPreparation, DocumentProgress, DocumentStage};
