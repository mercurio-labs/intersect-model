//! Handwritten structural syntax. Resolution and semantics belong to the shared resolver.
use super::*;

pub(super) struct TypeSyntax {
    pub kind: TypeSyntaxKind,
    pub span: SourceSpan,
}
pub(super) enum TypeSyntaxKind {
    Explicit(Box<TypeSyntax>),
    Named(String, Vec<TypeSyntax>, bool),
    External(String, Vec<TypeSyntax>),
    Parameter(String),
    Wildcard {
        upper: Option<Box<TypeSyntax>>,
        lower: Option<Box<TypeSyntax>>,
    },
}
pub(super) struct TypeParameterSyntax {
    pub name: String,
    pub bounds: Vec<TypeSyntax>,
    pub annotations: Vec<AnnotationSyntax>,
    pub span: SourceSpan,
}
#[derive(Default)]
pub(super) struct ClassifierMetadata {
    pub identity: Option<String>,
    pub instance_class: Option<String>,
    pub instance_type: Option<String>,
}
pub(super) struct DatatypeSyntax {
    pub name: String,
    pub type_parameters: Vec<TypeParameterSyntax>,
    pub metadata: ClassifierMetadata,
    pub serializable: bool,
    pub annotations: Vec<AnnotationSyntax>,
    pub span: SourceSpan,
}
pub(super) struct AnnotationSyntax {
    pub source: String,
    pub details: Vec<(String, String, SourceSpan)>,
    pub references: Vec<String>,
    pub annotations: Vec<AnnotationSyntax>,
    pub span: SourceSpan,
}

impl Parser {
    pub(super) fn at(&self, word: &str) -> bool {
        matches!(&self.token().kind, TokenKind::Word(s) | TokenKind::Symbol(s) if s == word)
    }
    pub(super) async fn qualified_name(&mut self) -> Result<String, SyntaxDiagnostic> {
        let mut name = self.name().await?;
        while self.eat("::").await {
            name.push('/');
            name.push_str(&self.name().await?);
        }
        Ok(name)
    }
    pub(super) async fn type_expression(
        &mut self,
        depth: usize,
    ) -> Result<TypeSyntax, SyntaxDiagnostic> {
        if depth >= 64 {
            return Err(self.error("Type nesting budget exceeded"));
        }
        let start = self.token().span.start;
        let kind = if self.eat("generic").await {
            TypeSyntaxKind::Explicit(Box::new(Box::pin(self.type_expression(depth + 1)).await?))
        } else if self.eat("?").await {
            let upper = if self.eat("extends").await {
                Some(Box::new(Box::pin(self.type_expression(depth + 1)).await?))
            } else {
                None
            };
            let lower = if upper.is_none() && self.eat("super").await {
                Some(Box::new(Box::pin(self.type_expression(depth + 1)).await?))
            } else {
                None
            };
            TypeSyntaxKind::Wildcard { upper, lower }
        } else if self.eat("parameter").await {
            TypeSyntaxKind::Parameter(self.string().await?)
        } else {
            let external = self.eat("external").await;
            let quoted = matches!(self.token().kind, TokenKind::QuotedName(_));
            let name = if external {
                self.string().await?
            } else {
                self.qualified_name().await?
            };
            let mut arguments = Vec::new();
            if self.eat("<").await {
                loop {
                    arguments.push(Box::pin(self.type_expression(depth + 1)).await?);
                    if self.eat(">").await {
                        break;
                    }
                    self.need(",").await?;
                }
            }
            if external {
                TypeSyntaxKind::External(name, arguments)
            } else {
                TypeSyntaxKind::Named(name, arguments, quoted)
            }
        };
        Ok(TypeSyntax {
            kind,
            span: self.span(start),
        })
    }
    pub(super) async fn type_parameters(
        &mut self,
    ) -> Result<Vec<TypeParameterSyntax>, SyntaxDiagnostic> {
        let mut parameters = Vec::new();
        if !self.eat("<").await {
            return Ok(parameters);
        }
        let mut names = BTreeSet::new();
        loop {
            let start = self.token().span.start;
            let name = self.name().await?;
            if !names.insert(name.clone()) {
                return Err(self.error("Duplicate type parameter"));
            }
            let mut bounds = Vec::new();
            if self.eat("extends").await {
                loop {
                    bounds.push(Box::pin(self.type_expression(0)).await?);
                    if !self.eat("&").await {
                        break;
                    }
                }
            }
            let annotations = self.annotations().await?;
            parameters.push(TypeParameterSyntax {
                name,
                bounds,
                annotations,
                span: self.span(start),
            });
            if self.eat(">").await {
                break;
            }
            self.need(",").await?;
        }
        Ok(parameters)
    }
    pub(super) async fn classifier_metadata(
        &mut self,
    ) -> Result<ClassifierMetadata, SyntaxDiagnostic> {
        let mut result = ClassifierMetadata::default();
        loop {
            let slot = if self.eat("identity").await {
                &mut result.identity
            } else if self.eat("instanceClass").await {
                &mut result.instance_class
            } else if self.eat("instanceType").await {
                &mut result.instance_type
            } else {
                break;
            };
            if slot.is_some() {
                return Err(self.error("Duplicate classifier metadata"));
            }
            *slot = Some(self.string().await?);
        }
        Ok(result)
    }
    pub(super) async fn annotations(&mut self) -> Result<Vec<AnnotationSyntax>, SyntaxDiagnostic> {
        let mut annotations = Vec::new();
        while self.at("annotation") {
            annotations.push(Box::pin(self.annotation(0)).await?);
        }
        Ok(annotations)
    }
    pub(super) async fn annotation(
        &mut self,
        depth: usize,
    ) -> Result<AnnotationSyntax, SyntaxDiagnostic> {
        if depth >= 64 {
            return Err(self.error("Annotation nesting budget exceeded"));
        }
        let start = self.token().span.start;
        self.need("annotation").await?;
        let source = self.string().await?;
        self.need("{").await?;
        let mut details = Vec::new();
        let mut references = Vec::new();
        let mut annotations = Vec::new();
        while !self.eat("}").await {
            let start = self.token().span.start;
            if self.eat("detail").await {
                let key = self.string().await?;
                self.need("=").await?;
                let value = self.string().await?;
                self.need(";").await?;
                details.push((key, value, self.span(start)));
            } else if self.eat("reference").await {
                references.push(self.string().await?);
                self.need(";").await?;
            } else if self.at("annotation") {
                annotations.push(Box::pin(self.annotation(depth + 1)).await?);
            } else {
                return Err(
                    self.error("Expected annotation detail, reference or nested annotation")
                );
            }
        }
        Ok(AnnotationSyntax {
            source,
            details,
            references,
            annotations,
            span: self.span(start),
        })
    }
    async fn datatype(&mut self) -> Result<DatatypeSyntax, SyntaxDiagnostic> {
        let start = self.token().span.start;
        self.need("datatype").await?;
        let name = self.name().await?;
        let type_parameters = self.type_parameters().await?;
        let metadata = self.classifier_metadata().await?;
        let serializable = !self.eat("nonserializable").await;
        self.need("{").await?;
        let annotations = self.annotations().await?;
        self.need("}").await?;
        Ok(DatatypeSyntax {
            name,
            type_parameters,
            metadata,
            serializable,
            annotations,
            span: self.span(start),
        })
    }
    pub(super) async fn package_body(
        &mut self,
        package: String,
        namespace: String,
        prefix: String,
        start: usize,
        depth: usize,
    ) -> Result<Structure, SyntaxDiagnostic> {
        if depth >= 64 {
            return Err(self.error("Package nesting budget exceeded"));
        }
        let mut structure = Structure {
            resource: false,
            identity: None,
            package,
            namespace,
            prefix,
            classes: vec![],
            enums: vec![],
            datatypes: vec![],
            packages: vec![],
            annotations: vec![],
            span: self.span(start),
            behavior_range: None,
        };
        let mut names = BTreeSet::new();
        while !self.eat("}").await {
            let name = if self.eat("identity").await {
                if structure.identity.is_some() {
                    return Err(self.error("Duplicate package identity"));
                }
                structure.identity = Some(self.string().await?);
                self.need(";").await?;
                continue;
            } else if self.at("annotation") {
                structure
                    .annotations
                    .push(Box::pin(self.annotation(0)).await?);
                continue;
            } else if self.at("package") {
                let start = self.token().span.start;
                self.need("package").await?;
                let name = self.name().await?;
                self.need("namespace").await?;
                let namespace = self.string().await?;
                self.need("prefix").await?;
                let prefix = self.name().await?;
                self.need("{").await?;
                structure.packages.push(
                    Box::pin(self.package_body(name.clone(), namespace, prefix, start, depth + 1))
                        .await?,
                );
                name
            } else if self.at("enum") {
                let en = self.enumeration().await?;
                let name = en.name.clone();
                structure.enums.push(en);
                name
            } else if self.at("datatype") {
                let datatype = self.datatype().await?;
                let name = datatype.name.clone();
                structure.datatypes.push(datatype);
                name
            } else {
                let class = self.class().await?;
                let name = class.name.clone();
                structure.classes.push(class);
                name
            };
            if !names.insert(name) {
                return Err(self.error("Duplicate classifier/package name"));
            }
        }
        structure.span = self.span(start);
        Ok(structure)
    }
}
