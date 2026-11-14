//! Handwritten Rcore producer of the shared declaration tree and schema IR.
//! The tree currently lives in `ecore`; no XML serialization/parser is involved.
use super::*;
use crate::definitions::{
    bundle::{DefinitionBundle, SourceKind},
    ecore::{
        Attribute, Document, ECORE, ErrorKind, ExpandedName, ImportError, Node, ParseLimits, XSI,
    },
    schema::Schema,
};

#[derive(Clone, Debug)]
pub struct DirectStructure {
    pub schema: Schema,
    pub namespace: String,
    pub classifiers: Vec<StructuralClassifier>,
    pub operations: Vec<StructuralOperation>,
}

/// Inspect authored declarations without pretending that generic templates are
/// already selected runtime classes. The shared resolver checks their signatures.
pub fn resolve_structure(
    bundle: &DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
) -> Result<(Vec<StructuralClassifier>, Vec<StructuralOperation>), Vec<SyntaxDiagnostic>> {
    let error = |e: ImportError| {
        vec![SyntaxDiagnostic {
            span: e.span,
            message: e.message,
        }]
    };
    let resolver = crate::definitions::generic_resolution::Resolver::new(bundle).map_err(error)?;
    resolver.resolve_all().map_err(error)?;
    let entry = bundle
        .sources
        .iter()
        .find(|s| s.id == *source)
        .ok_or_else(|| {
            vec![SyntaxDiagnostic {
                span: SourceSpan {
                    source: source.clone(),
                    start: 0,
                    end: 0,
                },
                message: "Missing exact Rcore entry source".into(),
            }]
        })?;
    Ok(inventories(
        &document(entry, ParseLimits::default()).map_err(error)?,
    ))
}

/// Compile exact authored Rcore into the shared schema with existing structural
/// checks. All declarations and diagnostics retain the authored source identity.
pub fn compile_structure(
    bundle: &DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
) -> Result<DirectStructure, Vec<SyntaxDiagnostic>> {
    compile_structure_selected(bundle, source, None)
}

/// Select a package explicitly when the Rcore resource contains multiple roots.
pub fn compile_structure_selected(
    bundle: &DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
    namespace: Option<&str>,
) -> Result<DirectStructure, Vec<SyntaxDiagnostic>> {
    compile_structure_context(bundle, source, namespace, None)
}

/// Compile canonical structure with explicitly installed datatype providers.
/// Provider contracts must already refer to the exact authored Rcore identities.
pub fn compile_structure_with_conversions(
    bundle: &DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
    namespace: Option<&str>,
    conversions: &crate::definitions::conversion::ConversionRegistry<'_>,
) -> Result<DirectStructure, Vec<SyntaxDiagnostic>> {
    compile_structure_context(bundle, source, namespace, Some(conversions))
}

fn compile_structure_context(
    bundle: &DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
    namespace: Option<&str>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<DirectStructure, Vec<SyntaxDiagnostic>> {
    let failure = |message: String| {
        vec![SyntaxDiagnostic {
            span: SourceSpan {
                source: source.clone(),
                start: 0,
                end: 0,
            },
            message,
        }]
    };
    bundle.validate().map_err(failure)?;
    let entry = bundle
        .sources
        .iter()
        .find(|s| s.id == *source)
        .ok_or_else(|| failure("Missing exact Rcore entry source".into()))?;
    if entry.kind != SourceKind::Rcore {
        return Err(failure("Expected an authored Rcore source".into()));
    }
    if let Some(imported) = bundle
        .sources
        .iter()
        .find(|source| source.kind == SourceKind::Ecore)
    {
        return Err(vec![SyntaxDiagnostic {
            span: SourceSpan {
                source: imported.id.clone(),
                start: 0,
                end: imported.text.len(),
            },
            message: "Migrate every Ecore source before compiling Rcore structure".into(),
        }]);
    }
    let parsed = parse_structure(entry)?;
    let namespace = namespace
        .or_else(|| (!parsed.resource).then_some(parsed.namespace.as_str()))
        .ok_or_else(|| {
            failure("A multi-package resource requires an explicit namespace selection".into())
        })?;
    let schema = match conversions {
        Some(registry) => crate::definitions::compiler::compile_structure_bundle_with_conversions(
            bundle, source, namespace, registry,
        ),
        None => crate::definitions::compiler::compile_structure_bundle(bundle, source, namespace),
    }
    .map_err(|e| {
        vec![SyntaxDiagnostic {
            span: e.span,
            message: e.message,
        }]
    })?;
    let document = document(entry, ParseLimits::default()).map_err(|e| {
        vec![SyntaxDiagnostic {
            span: e.span,
            message: e.message,
        }]
    })?;
    let (classifiers, operations) = inventories(&document);
    Ok(DirectStructure {
        schema,
        namespace: namespace.into(),
        classifiers,
        operations,
    })
}

async fn node(
    name: &str,
    namespace: Option<&str>,
    span: &SourceSpan,
    traversal: &preparation::Traversal,
) -> Node {
    let result = Node {
        name: ExpandedName {
            namespace: namespace.map(str::to_owned),
            local: name.into(),
        },
        lexical_name: name.into(),
        attributes: vec![],
        children: vec![],
        payloads: vec![],
        opening: span.clone(),
        span: span.clone(),
    };
    traversal.tree().await;
    result
}
async fn attribute(
    node: &mut Node,
    name: &str,
    value: impl Into<String>,
    traversal: &preparation::Traversal,
) {
    let value = value.into();
    let (namespace, local) = if name == "xsi:type" {
        (Some(XSI.into()), "type")
    } else if name == "xmi:id" {
        (Some(crate::definitions::ecore::XMI.into()), "id")
    } else {
        (None, name)
    };
    let type_name = if name == "xsi:type" {
        Some(ExpandedName {
            namespace: Some(ECORE.into()),
            local: value.trim_start_matches("ecore:").into(),
        })
    } else if name == "eType" && value.starts_with("ecore:EDataType ") {
        Some(ExpandedName {
            namespace: Some(ECORE.into()),
            local: "EDataType".into(),
        })
    } else {
        None
    };
    node.attributes.push(Attribute {
        name: ExpandedName {
            namespace,
            local: local.into(),
        },
        lexical_name: name.into(),
        value,
        span: node.span.clone(),
        type_name,
    });
    traversal.tree().await;
}
fn failure(span: &SourceSpan, message: impl Into<String>) -> ImportError {
    ImportError::new(ErrorKind::InvalidDefinition, span, message)
}
pub(crate) fn document(
    source: &DefinitionSource,
    limits: ParseLimits,
) -> Result<Document, ImportError> {
    check_document_limits(source, limits)?;
    let context = crate::definitions::conversion::InvocationContext::default();
    let mut lexical = rcore_behavior::LexicalPreparation::new(source);
    while !lexical.advance(&context).map_err(|e| {
        failure(
            &SourceSpan {
                source: source.id.clone(),
                start: 0,
                end: source.text.len(),
            },
            e,
        )
    })? {}
    let output = lexical.finish().map_err(|e| {
        failure(
            &SourceSpan {
                source: source.id.clone(),
                start: 0,
                end: source.text.len(),
            },
            e,
        )
    })?;
    let mut pending = DocumentPreparation::from_lexical(output, limits)?;
    while !pending
        .advance(&context)
        .map_err(|error| import_failure(source, error))?
    {}
    pending
        .finish()
        .map_err(|error| import_failure(source, error))
}
fn import_failure(
    source: &DefinitionSource,
    error: crate::definitions::ecore::ObservedImportError,
) -> ImportError {
    match error {
        crate::definitions::ecore::ObservedImportError::Import(error) => error,
        crate::definitions::ecore::ObservedImportError::Observation(message) => failure(
            &SourceSpan {
                source: source.id.clone(),
                start: 0,
                end: source.text.len(),
            },
            message,
        ),
    }
}

pub(crate) fn check_document_limits(
    source: &DefinitionSource,
    limits: ParseLimits,
) -> Result<(), ImportError> {
    if source.kind == SourceKind::Rcore && source.text.len() <= limits.source_bytes {
        return Ok(());
    }
    let span = SourceSpan {
        source: source.id.clone(),
        start: 0,
        end: source.text.len(),
    };
    if source.kind != SourceKind::Rcore {
        return Err(failure(&span, "Expected Rcore source"));
    }
    if source.text.len() > limits.source_bytes {
        return Err(ImportError::new(
            ErrorKind::ResourceLimit,
            &span,
            "Rcore source size budget exceeded",
        ));
    }
    Ok(())
}
pub(super) async fn build_root(
    parsed: &Structure,
    traversal: &preparation::Traversal,
) -> Result<Node, ImportError> {
    structure_nodes::build(parsed, traversal).await
}

/// Select behavior tokens without replacing source bytes or their content identity.
pub(crate) fn behavior_tokens(
    source: &DefinitionSource,
    tokens: &[Token],
    source_bytes: usize,
) -> Result<Vec<Token>, Vec<SyntaxDiagnostic>> {
    preparation::complete(behavior_tokens_retained(source, tokens, source_bytes))
}
pub(crate) async fn behavior_tokens_retained(
    source: &DefinitionSource,
    tokens: &[Token],
    source_bytes: usize,
) -> Result<Vec<Token>, Vec<SyntaxDiagnostic>> {
    let parsed = parse_structure_with_limit_retained(source, source_bytes).await?;
    let structure_start = parsed.span.start;
    let mut selected = Vec::new();
    for token in tokens {
        if token.kind == TokenKind::End
            || token.span.start < structure_start
            || parsed
                .behavior_range
                .is_some_and(|r| token.span.start >= r.body_start && token.span.end <= r.body_end)
        {
            selected.push(token.clone());
        }
        crate::definitions::continuation::pause().await;
    }
    Ok(selected)
}

pub(super) fn inventories(
    document: &Document,
) -> (Vec<StructuralClassifier>, Vec<StructuralOperation>) {
    fn visit(
        node: &Node,
        path: &[usize],
        source: &crate::definitions::bundle::SourceId,
        classifiers: &mut Vec<StructuralClassifier>,
        operations: &mut Vec<StructuralOperation>,
    ) {
        let name = node
            .attributes
            .iter()
            .find(|a| a.name.namespace.is_none() && a.name.local == "name")
            .map(|a| a.value.clone());
        let id = DeclarationId {
            source: source.clone(),
            path: path.to_vec(),
        };
        if node.name.local == "eClassifiers" {
            if let Some(name) = name {
                classifiers.push(StructuralClassifier {
                    name,
                    declaration: id,
                });
            }
        } else if node.name.local == "eOperations"
            && let Some(name) = name
        {
            operations.push(StructuralOperation {
                name,
                declaration: id,
                owner: DeclarationId {
                    source: source.clone(),
                    path: path[..path.len() - 1].to_vec(),
                },
                original: node.span.clone(),
            });
        }
        for (i, child) in node.children.iter().enumerate() {
            let mut next = path.to_vec();
            next.push(i);
            visit(child, &next, source, classifiers, operations);
        }
    }
    let mut classifiers = Vec::new();
    let mut operations = Vec::new();
    visit(
        &document.root,
        &[],
        &document.source.id,
        &mut classifiers,
        &mut operations,
    );
    (classifiers, operations)
}
/// MOF property redefinitions that a structure source declares with
/// `redefines` annotations on its features: (redefining, redefined) feature
/// declarations, in source order. Each reference is an EMF fragment within the
/// same source (`#//@eClassifiers.N/@eStructuralFeatures.M`) and is resolved
/// against the same declaration tree that numbers the schema's declarations.
/// References to other sources are rejected; they are not supported yet.
pub fn feature_redefinitions(
    source: &DefinitionSource,
    limits: ParseLimits,
) -> Result<Vec<(DeclarationId, DeclarationId)>, ImportError> {
    fn walk(
        node: &Node,
        path: &mut Vec<usize>,
        root: &Node,
        source: &DefinitionSource,
        out: &mut Vec<(DeclarationId, DeclarationId)>,
    ) -> Result<(), ImportError> {
        if node.name.local == "eStructuralFeatures" {
            let redefines = node.children.iter().filter(|child| {
                child.name.local == "eAnnotations"
                    && attribute(child, "source") == Some("redefines")
            });
            for annotation in redefines {
                for reference in attribute(annotation, "references")
                    .unwrap_or("")
                    .split_whitespace()
                {
                    let target = fragment(root, reference).ok_or_else(|| {
                        failure(
                            &annotation.span,
                            format!("Unsupported or unresolved redefinition reference {reference}"),
                        )
                    })?;
                    out.push((
                        DeclarationId {
                            source: source.id.clone(),
                            path: path.clone(),
                        },
                        DeclarationId {
                            source: source.id.clone(),
                            path: target,
                        },
                    ));
                }
            }
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            walk(child, path, root, source, out)?;
            path.pop();
        }
        Ok(())
    }
    fn attribute<'n>(node: &'n Node, name: &str) -> Option<&'n str> {
        node.attributes
            .iter()
            .find(|a| a.name.namespace.is_none() && a.name.local == name)
            .map(|a| a.value.as_str())
    }
    // `#//@name.index/...`: the index-th child with that element name, per step.
    fn fragment(root: &Node, reference: &str) -> Option<Vec<usize>> {
        let mut node = root;
        let mut path = Vec::new();
        for step in reference.strip_prefix("#//")?.split('/') {
            let (name, index) = step.strip_prefix('@')?.rsplit_once('.')?;
            let index: usize = index.parse().ok()?;
            let (position, child) = node
                .children
                .iter()
                .enumerate()
                .filter(|(_, child)| child.name.local == name)
                .nth(index)?;
            path.push(position);
            node = child;
        }
        (node.name.local == "eStructuralFeatures").then_some(path)
    }
    let document = document(source, limits)?;
    let mut out = Vec::new();
    walk(
        &document.root,
        &mut Vec::new(),
        &document.root,
        source,
        &mut out,
    )?;
    Ok(out)
}
#[path = "rcore_structure_nodes.rs"]
mod structure_nodes;
