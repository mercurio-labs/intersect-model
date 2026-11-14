//! Handwritten, deterministic migration into editable Rcore, never a hidden runtime fallback.
//! Original source bytes and diagnostic locations remain available in the report.
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind, SourceSpan},
    ecore::{Document, ECORE, ErrorKind, ImportError, Node, ParseLimits, PayloadKind, XMI, XSI},
    resources::DeclarationId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationIssue {
    pub code: String,
    pub message: String,
    pub span: SourceSpan,
    pub blocks_conversion: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correspondence {
    pub original: DeclarationId,
    pub original_span: SourceSpan,
    pub converted: DeclarationId,
    pub converted_span: SourceSpan,
}

/// `converted` contains resolved declarations, not a claim of executable behavior.
/// Generic applications and custom datatype providers still require explicit
/// runtime selection/binding; those requirements are reported as issues.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationReport {
    pub version: u32,
    pub original: DefinitionSource,
    pub draft: String,
    pub converted: Option<DefinitionSource>,
    pub correspondence: Vec<Correspondence>,
    pub issues: Vec<MigrationIssue>,
}

fn value<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.attributes
        .iter()
        .find(|a| a.name.namespace.is_none() && a.name.local == name)
        .map(|a| a.value.as_str())
}
fn kind(node: &Node) -> Option<&str> {
    node.attributes
        .iter()
        .find(|a| a.name.namespace.as_deref() == Some(XSI) && a.name.local == "type")
        .and_then(|a| a.type_name.as_ref())
        .filter(|n| n.namespace.as_deref() == Some(ECORE))
        .map(|n| n.local.as_str())
}
fn quote(text: &str) -> String {
    serde_json::Value::String(text.into()).to_string()
}
/// Names are bare unless the bare word could be read as syntax or a scalar.
fn ident(text: &str) -> String {
    crate::definitions::rcore_names::spell(text)
}
struct Writer<'a> {
    source: &'a DefinitionSource,
    issues: Vec<MigrationIssue>,
    references: std::collections::BTreeMap<String, String>,
}
impl Writer<'_> {
    fn issue(&mut self, node: &Node, code: &str, message: impl Into<String>, blocking: bool) {
        self.issues.push(MigrationIssue {
            code: code.into(),
            message: message.into(),
            span: node.span.clone(),
            blocks_conversion: blocking,
        });
    }
    fn audit(&mut self, node: &Node, allowed: &[&str]) {
        for a in &node.attributes {
            let structural = a.name.namespace.is_none() && allowed.contains(&a.name.local.as_str());
            if structural
                && [
                    "abstract",
                    "interface",
                    "serializable",
                    "ordered",
                    "unique",
                    "containment",
                    "derived",
                    "changeable",
                    "transient",
                    "volatile",
                    "unsettable",
                    "iD",
                    "resolveProxies",
                ]
                .contains(&a.name.local.as_str())
                && !["true", "false"].contains(&a.value.as_str())
            {
                self.issues.push(MigrationIssue {
                    code: "invalid_boolean".into(),
                    message: format!("Invalid boolean {} for {}", a.value, a.name.local),
                    span: a.span.clone(),
                    blocks_conversion: true,
                });
            }
            let classifier = a.name.namespace.as_deref() == Some(XSI) && a.name.local == "type";
            let version = a.name.namespace.as_deref() == Some(XMI) && a.name.local == "version";
            let identity = a.name.namespace.as_deref() == Some(XMI)
                && a.name.local == "id"
                && allowed.contains(&"$identity");
            let namespace = a.name.namespace.as_deref() == Some("http://www.w3.org/2000/xmlns/");
            if !structural && !classifier && !version && !namespace && !identity {
                self.issues.push(MigrationIssue {
                    code: "unsupported_attribute".into(),
                    message: format!("Attribute {} needs explicit migration", a.lexical_name),
                    span: a.span.clone(),
                    blocks_conversion: true,
                });
            }
        }
        for p in &node.payloads {
            if matches!(
                p.kind,
                PayloadKind::Text | PayloadKind::CData | PayloadKind::EntityReference
            ) && self
                .source
                .text
                .get(p.span.start..p.span.end)
                .is_none_or(|s| !s.trim().is_empty())
            {
                self.issues.push(MigrationIssue {
                    code: "unsupported_payload".into(),
                    message: "Non-whitespace XML payload needs explicit migration".into(),
                    span: p.span.clone(),
                    blocks_conversion: true,
                });
            }
        }
    }
    fn type_name(&mut self, _node: &Node, spelling: &str) -> String {
        let href = spelling.split_whitespace().last().unwrap_or("");
        let href = self.references.get(href).map_or(href, String::as_str);
        if let Some(local) = href
            .strip_prefix("#//")
            .filter(|s| !s.contains('@') && !s.is_empty())
        {
            return local.split('/').map(ident).collect::<Vec<_>>().join("::");
        }
        if let Some(builtin) = href.strip_prefix(&format!("{ECORE}#//"))
            && crate::definitions::scalars::Builtin::named(builtin).is_some()
        {
            return builtin.into();
        }
        format!("external {}", quote(href))
    }
    fn generic(&mut self, node: &Node) -> String {
        self.audit(node, &["eClassifier", "eTypeParameter"]);
        let mut arguments = Vec::new();
        let mut upper = None;
        let mut lower = None;
        for child in &node.children {
            match child.name.local.as_str() {
                "eTypeArguments" => arguments.push(self.generic(child)),
                "eUpperBound" => upper = Some(self.generic(child)),
                "eLowerBound" => lower = Some(self.generic(child)),
                _ => self.unsupported(child),
            }
        }
        if let Some(parameter) = value(node, "eTypeParameter") {
            return format!(
                "parameter {}",
                quote(
                    self.references
                        .get(parameter)
                        .map_or(parameter, String::as_str)
                )
            );
        }
        if let Some(classifier) = value(node, "eClassifier") {
            let name = self.type_name(node, classifier);
            return if arguments.is_empty() {
                name
            } else {
                format!("{name}<{}>", arguments.join(", "))
            };
        }
        if let Some(upper) = upper {
            format!("? extends {upper}")
        } else if let Some(lower) = lower {
            format!("? super {lower}")
        } else {
            "?".into()
        }
    }
    fn parameters(&mut self, node: &Node) -> String {
        let mut parameters = Vec::new();
        for p in node
            .children
            .iter()
            .filter(|n| n.name.local == "eTypeParameters")
        {
            self.audit(p, &["name"]);
            let mut bounds = Vec::new();
            for child in &p.children {
                if child.name.local == "eBounds" {
                    bounds.push(self.generic(child));
                } else if child.name.local != "eAnnotations" {
                    self.unsupported(child);
                }
            }
            let mut parameter = ident(value(p, "name").unwrap_or(""));
            if !bounds.is_empty() {
                parameter.push_str(&format!(" extends {}", bounds.join(" & ")));
            }
            parameter.push_str(&self.annotations(p, 6));
            parameters.push(parameter);
        }
        if parameters.is_empty() {
            String::new()
        } else {
            format!("<{}>", parameters.join(", "))
        }
    }
    fn metadata(&mut self, node: &Node) -> String {
        let mut result = String::new();
        if let Some(id) = node
            .attributes
            .iter()
            .find(|a| a.name.namespace.as_deref() == Some(XMI) && a.name.local == "id")
        {
            result.push_str(&format!(" identity {}", quote(&id.value)));
        }
        for (key, keyword) in [
            ("instanceClassName", "instanceClass"),
            ("instanceTypeName", "instanceType"),
        ] {
            if let Some(value) = value(node, key) {
                result.push_str(&format!(" {keyword} {}", quote(value)));
            }
        }
        result
    }
    /// Annotations that follow a declaration, each starting on its own line
    /// indented by `indent` spaces.
    fn annotations(&mut self, node: &Node, indent: usize) -> String {
        let mut out = String::new();
        for child in node
            .children
            .iter()
            .filter(|n| n.name.local == "eAnnotations")
        {
            out.push_str(&self.annotation(child, indent));
        }
        out
    }
    /// An annotation that is itself a member of a class, enum, datatype or
    /// package body, on its own lines.
    fn member_annotation(&mut self, node: &Node, indent: usize) -> String {
        let text = self.annotation(node, indent);
        format!("{}\n", text.strip_prefix('\n').unwrap_or(&text))
    }
    /// Layout only: one entry per line. Every value keeps its exact spelling.
    fn annotation(&mut self, node: &Node, indent: usize) -> String {
        self.audit(node, &["source", "references"]);
        let pad = " ".repeat(indent);
        let mut entries = Vec::new();
        if let Some(references) = value(node, "references") {
            for reference in references.split_whitespace() {
                entries.push(format!(
                    "\n{pad}  reference {};",
                    quote(
                        self.references
                            .get(reference)
                            .map_or(reference, String::as_str)
                    )
                ));
            }
        }
        for child in &node.children {
            match child.name.local.as_str() {
                "details" => {
                    self.audit(child, &["key", "value"]);
                    for nested in &child.children {
                        self.unsupported(nested);
                    }
                    entries.push(format!(
                        "\n{pad}  detail {} = {};",
                        quote(value(child, "key").unwrap_or("")),
                        quote(value(child, "value").unwrap_or(""))
                    ));
                }
                "eAnnotations" => entries.push(self.annotation(child, indent + 2)),
                _ => self.unsupported(child),
            }
        }
        let source = quote(value(node, "source").unwrap_or(""));
        if entries.is_empty() {
            format!("\n{pad}annotation {source} {{ }}")
        } else {
            format!("\n{pad}annotation {source} {{{}\n{pad}}}", entries.concat())
        }
    }
    fn slot(&mut self, node: &Node) -> String {
        let ty = if let Some(generic) = node
            .children
            .iter()
            .find(|n| n.name.local == "eGenericType")
        {
            format!("generic {}", self.generic(generic))
        } else if let Some(reference) = node
            .children
            .iter()
            .find(|n| n.name.namespace.is_none() && n.name.local == "eType")
        {
            self.type_name(reference, value(reference, "href").unwrap_or(""))
        } else {
            self.type_name(node, value(node, "eType").unwrap_or(""))
        };
        let lower = value(node, "lowerBound").unwrap_or("0");
        let upper = match value(node, "upperBound").unwrap_or("1") {
            "-1" => "*",
            n => n,
        };
        let mut result = format!("{ty} [{lower},{upper}]");
        for (attr, val, flag) in [
            ("ordered", "false", "unordered"),
            ("unique", "false", "nonunique"),
        ] {
            if value(node, attr) == Some(val) {
                result.push_str(&format!(" {flag}"));
            }
        }
        result
    }
    fn href_type(&mut self, node: &Node) -> String {
        self.audit(node, &["href"]);
        for child in &node.children {
            self.unsupported(child);
        }
        self.type_name(node, value(node, "href").unwrap_or(""))
    }
    fn feature(&mut self, node: &Node) -> String {
        self.audit(
            node,
            &[
                "name",
                "eType",
                "lowerBound",
                "upperBound",
                "ordered",
                "unique",
                "containment",
                "eOpposite",
                "derived",
                "changeable",
                "transient",
                "volatile",
                "unsettable",
                "defaultValueLiteral",
                "iD",
                "resolveProxies",
            ],
        );
        for child in &node.children {
            if child.name.namespace.is_none()
                && ["eType", "eOpposite"].contains(&child.name.local.as_str())
            {
                self.href_type(child);
            } else if child.name.namespace.is_some()
                || !["eGenericType", "eAnnotations"].contains(&child.name.local.as_str())
            {
                self.unsupported(child);
            }
        }
        let opposites = node
            .children
            .iter()
            .filter(|child| child.name.namespace.is_none() && child.name.local == "eOpposite")
            .count()
            + usize::from(value(node, "eOpposite").is_some());
        if opposites > 1 {
            self.issue(
                node,
                "duplicate_reference",
                "Duplicate reference encoding for eOpposite",
                true,
            );
        }
        let reference = match kind(node) {
            Some("EReference") => true,
            Some("EAttribute") => false,
            _ => {
                self.unsupported(node);
                false
            }
        };
        let mut out = format!(
            "    {} {}: {}",
            if reference { "ref" } else { "attr" },
            ident(value(node, "name").unwrap_or("")),
            self.slot(node)
        );
        for (attr, val, flag) in [
            ("containment", "true", "containment"),
            ("derived", "true", "derived"),
            ("changeable", "false", "readonly"),
            ("transient", "true", "transient"),
            ("volatile", "true", "volatile"),
            ("unsettable", "true", "unsettable"),
            ("iD", "true", "id"),
            ("resolveProxies", "false", "noresolve"),
        ] {
            if value(node, attr) == Some(val) {
                out.push_str(&format!(" {flag}"));
            }
        }
        if let Some(opposite) = value(node, "eOpposite").or_else(|| {
            node.children
                .iter()
                .find(|child| child.name.namespace.is_none() && child.name.local == "eOpposite")
                .and_then(|child| value(child, "href"))
        }) {
            out.push_str(&format!(
                " opposite external {}",
                quote(
                    self.references
                        .get(opposite)
                        .map_or(opposite, String::as_str)
                )
            ));
        }
        // String spelling is deliberately retained; the shared datatype checker
        // interprets and validates its lexical value for the declared datatype.
        if let Some(default) = value(node, "defaultValueLiteral") {
            out.push_str(&format!(" default {}", quote(default)));
        }
        out.push_str(&self.annotations(node, 6));
        out.push_str(";\n");
        out
    }
    fn operation(&mut self, node: &Node) -> String {
        self.audit(
            node,
            &[
                "name",
                "eType",
                "lowerBound",
                "upperBound",
                "ordered",
                "unique",
                "eExceptions",
            ],
        );
        self.issue(
            node,
            "operation_implementation",
            "Operation signature is converted; an implementation must be bound explicitly",
            false,
        );
        let mut parameters = Vec::new();
        for p in &node.children {
            if p.name.namespace.is_none() && p.name.local == "eParameters" {
                self.audit(
                    p,
                    &[
                        "name",
                        "eType",
                        "lowerBound",
                        "upperBound",
                        "ordered",
                        "unique",
                    ],
                );
                for child in &p.children {
                    if child.name.namespace.is_none() && child.name.local == "eType" {
                        self.href_type(child);
                    } else if child.name.namespace.is_some()
                        || !["eGenericType", "eAnnotations"].contains(&child.name.local.as_str())
                    {
                        self.unsupported(child);
                    }
                }
                parameters.push(format!(
                    "{}: {}{}",
                    ident(value(p, "name").unwrap_or("")),
                    self.slot(p),
                    self.annotations(p, 8)
                ));
            } else if p.name.namespace.is_none()
                && matches!(p.name.local.as_str(), "eType" | "eExceptions")
            {
                self.href_type(p);
            } else if p.name.namespace.is_some()
                || ![
                    "eTypeParameters",
                    "eGenericType",
                    "eGenericExceptions",
                    "eAnnotations",
                ]
                .contains(&p.name.local.as_str())
            {
                self.unsupported(p);
            }
        }
        let result = if value(node, "eType").is_some()
            || node
                .children
                .iter()
                .any(|n| matches!(n.name.local.as_str(), "eType" | "eGenericType"))
        {
            self.slot(node)
        } else {
            "void".into()
        };
        let generic = self.parameters(node);
        let mut exceptions = Vec::new();
        for child in node
            .children
            .iter()
            .filter(|n| n.name.local == "eGenericExceptions")
        {
            exceptions.push(self.generic(child));
        }
        if exceptions.is_empty() {
            for child in node
                .children
                .iter()
                .filter(|n| n.name.namespace.is_none() && n.name.local == "eExceptions")
            {
                exceptions.push(self.type_name(child, value(child, "href").unwrap_or("")));
            }
            for href in value(node, "eExceptions").unwrap_or("").split_whitespace() {
                exceptions.push(self.type_name(node, href));
            }
        }
        let throws = if exceptions.is_empty() {
            String::new()
        } else {
            format!(" throws {}", exceptions.join(", "))
        };
        let name = ident(value(node, "name").unwrap_or(""));
        let inline = parameters.join(", ");
        // One parameter per line when they would not fit or carry annotations.
        let parameters = if parameters.iter().any(|p| p.contains('\n'))
            || name.len() + generic.len() + inline.len() > 80
        {
            format!("\n      {}\n    ", parameters.join(",\n      "))
        } else {
            inline
        };
        format!(
            "    op {name}{generic}({parameters}) -> {result}{throws}{};\n",
            self.annotations(node, 6)
        )
    }
    fn class(&mut self, node: &Node) -> String {
        self.audit(
            node,
            &[
                "name",
                "abstract",
                "interface",
                "eSuperTypes",
                "instanceClassName",
                "instanceTypeName",
                "$identity",
            ],
        );
        let abstract_type = if value(node, "abstract") == Some("true") {
            "abstract "
        } else {
            ""
        };
        let interface = if value(node, "interface") == Some("true") {
            "interface "
        } else {
            ""
        };
        let mut out = format!(
            "  {abstract_type}{interface}class {}",
            ident(value(node, "name").unwrap_or(""))
        );
        out.push_str(&self.parameters(node));
        out.push_str(&self.metadata(node));
        let mut parents = Vec::new();
        for child in node
            .children
            .iter()
            .filter(|n| n.name.local == "eGenericSuperTypes")
        {
            parents.push(format!("generic {}", self.generic(child)));
        }
        if parents.is_empty() {
            for child in node
                .children
                .iter()
                .filter(|n| n.name.namespace.is_none() && n.name.local == "eSuperTypes")
            {
                parents.push(self.type_name(child, value(child, "href").unwrap_or("")));
            }
            for parent in value(node, "eSuperTypes")
                .unwrap_or("")
                .split_whitespace()
                .filter(|word| !word.contains(':') || word.contains('/') || word.contains('#'))
            {
                parents.push(self.type_name(node, parent));
            }
        }
        if !parents.is_empty() {
            out.push_str(&format!(" extends {}", parents.join(", ")));
        }
        out.push_str(" {\n");
        for child in &node.children {
            match (child.name.namespace.as_deref(), child.name.local.as_str()) {
                (None, "eStructuralFeatures") => out.push_str(&self.feature(child)),
                (None, "eOperations") => out.push_str(&self.operation(child)),
                (None, "eAnnotations") => out.push_str(&self.member_annotation(child, 4)),
                (None, "eSuperTypes") => {
                    self.href_type(child);
                }
                (None, "eTypeParameters" | "eGenericSuperTypes") => {}
                _ => self.unsupported(child),
            }
        }
        out.push_str("  }\n");
        out
    }
    fn enumeration(&mut self, node: &Node) -> String {
        self.audit(
            node,
            &[
                "name",
                "serializable",
                "instanceClassName",
                "instanceTypeName",
                "$identity",
            ],
        );
        let flag = if value(node, "serializable") == Some("false") {
            " nonserializable"
        } else {
            ""
        };
        let mut out = format!(
            "  enum {}{}{flag} {{\n",
            ident(value(node, "name").unwrap_or("")),
            self.metadata(node)
        );
        for child in &node.children {
            if child.name.local == "eAnnotations" {
                out.push_str(&self.member_annotation(child, 4));
                continue;
            }
            if child.name.namespace.is_some() || child.name.local != "eLiterals" {
                self.unsupported(child);
                continue;
            }
            self.audit(child, &["name", "value", "literal"]);
            for c in &child.children {
                if c.name.local != "eAnnotations" {
                    self.unsupported(c);
                }
            }
            let literal = value(child, "literal")
                .map(|text| format!(" literal {}", quote(text)))
                .unwrap_or_default();
            // Ecore accepts Unicode decimal digits and signed/zero-padded
            // spellings. Emit their checked numeric value as a Rcore token;
            // the original spelling remains in the retained source/report.
            let number = match crate::definitions::scalars::Builtin::EInt
                .parse(value(child, "value").unwrap_or("0"))
            {
                Ok(number) => number.to_string(),
                Err(message) => {
                    self.issue(child, "invalid_enum_value", message, true);
                    String::from("0")
                }
            };
            out.push_str(&format!(
                "    {} = {}{literal}{};\n",
                ident(value(child, "name").unwrap_or("")),
                number,
                self.annotations(child, 6)
            ));
        }
        out.push_str("  }\n");
        out
    }
    fn datatype(&mut self, node: &Node) -> String {
        self.audit(
            node,
            &[
                "name",
                "serializable",
                "instanceClassName",
                "instanceTypeName",
                "$identity",
            ],
        );
        for child in &node.children {
            if !["eTypeParameters", "eAnnotations"].contains(&child.name.local.as_str()) {
                self.unsupported(child);
            }
        }
        let flag = if value(node, "serializable") == Some("false") {
            " nonserializable"
        } else {
            ""
        };
        let head = format!(
            "  datatype {}{}{}{flag}",
            ident(value(node, "name").unwrap_or("")),
            self.parameters(node),
            self.metadata(node),
        );
        let mut members = String::new();
        for child in node
            .children
            .iter()
            .filter(|n| n.name.local == "eAnnotations")
        {
            members.push_str(&self.member_annotation(child, 4));
        }
        if members.is_empty() {
            format!("{head} {{ }}\n")
        } else {
            format!("{head} {{\n{members}  }}\n")
        }
    }
    fn package(&mut self, node: &Node, root: bool) -> String {
        if root && node.name.namespace.as_deref() == Some(XMI) && node.name.local == "XMI" {
            self.audit(node, &[]);
            let mut out = String::from("rcore 1;\npackage migrated version \"1\";\nresource {\n");
            for child in &node.children {
                if child.name.namespace.as_deref() == Some(ECORE) && child.name.local == "EPackage"
                {
                    out.push_str(&self.package(child, false));
                } else {
                    self.unsupported(child);
                }
            }
            out.push_str("}\n");
            return out;
        }
        self.audit(node, &["name", "nsURI", "nsPrefix", "$identity"]);
        let name = value(node, "name").unwrap_or("");
        let namespace = quote(value(node, "nsURI").unwrap_or(""));
        let prefix = ident(value(node, "nsPrefix").unwrap_or(name));
        let mut out = if root {
            format!(
                "rcore 1;\npackage {} version \"1\";\nmetamodel {namespace} prefix {prefix} {{\n",
                ident(name)
            )
        } else {
            format!(
                "package {} namespace {namespace} prefix {prefix} {{\n",
                ident(name)
            )
        };
        if let Some(identity) = node
            .attributes
            .iter()
            .find(|a| a.name.namespace.as_deref() == Some(XMI) && a.name.local == "id")
        {
            out.push_str(&format!("  identity {};\n", quote(&identity.value)));
        }
        for child in &node.children {
            match (
                child.name.namespace.as_deref(),
                child.name.local.as_str(),
                kind(child),
            ) {
                (None, "eClassifiers", Some("EClass")) => out.push_str(&self.class(child)),
                (None, "eClassifiers", Some("EEnum")) => out.push_str(&self.enumeration(child)),
                (None, "eClassifiers", Some("EDataType")) => out.push_str(&self.datatype(child)),
                (None, "eSubpackages", _) => out.push_str(&self.package(child, false)),
                (None, "eAnnotations", _) => out.push_str(&self.member_annotation(child, 2)),
                _ => self.unsupported(child),
            }
        }
        out.push_str("}\n");
        out
    }
    fn unsupported(&mut self, node: &Node) {
        self.issue(
            node,
            "manual_migration",
            format!(
                "{} requires explicit migration; retained in the original source",
                node.lexical_name
            ),
            true,
        );
    }
}

fn mappings(
    original: &Node,
    converted: &Node,
    from: Vec<usize>,
    to: Vec<usize>,
    out: &mut Vec<Correspondence>,
) {
    out.push(Correspondence {
        original: DeclarationId {
            source: original.span.source.clone(),
            path: from.clone(),
        },
        original_span: original.span.clone(),
        converted: DeclarationId {
            source: converted.span.source.clone(),
            path: to.clone(),
        },
        converted_span: converted.span.clone(),
    });
    for (i, node) in original.children.iter().enumerate() {
        let ordinal = original.children[..i]
            .iter()
            .filter(|n| n.name.local == node.name.local && value(n, "name") == value(node, "name"))
            .count();
        if let Some((j, target)) = converted
            .children
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.name.local == node.name.local && value(n, "name") == value(node, "name")
            })
            .nth(ordinal)
        {
            let mut a = from.clone();
            a.push(i);
            let mut b = to.clone();
            b.push(j);
            mappings(node, target, a, b, out);
        }
    }
}

/// Convert a single local metamodel. Unsupported inputs produce an explicit
/// report and editable draft, with no executable `converted` source.
pub fn migrate_ecore(
    source: DefinitionSource,
    target_uri: &str,
) -> Result<MigrationReport, ImportError> {
    migrate_ecore_with_limits(source, target_uri, ParseLimits::default())
}

/// Apply one explicit per-source limit to original validation, converted
/// validation and declaration correspondence. Larger limits do not relax semantics.
pub fn migrate_ecore_with_limits(
    source: DefinitionSource,
    target_uri: &str,
    limits: ParseLimits,
) -> Result<MigrationReport, ImportError> {
    let document = Document::parse_with_limits(source.clone(), limits)?;
    if source.id.uri == target_uri {
        return Err(ImportError::new(
            ErrorKind::InvalidDefinition,
            &document.root.span,
            "Migration output needs its own source URI",
        ));
    }
    let root = &document.root;
    let mut writer = Writer {
        source: &source,
        issues: vec![],
        references: std::collections::BTreeMap::new(),
    };
    if source.dependencies.is_empty() {
        let input = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let remapped = crate::definitions::resources::ResourceSet::new_with_limits(&input, limits)
            .and_then(|resources| {
                bundle_migration::rewrite_references(
                    root,
                    &source,
                    &resources,
                    &std::collections::BTreeMap::from([(source.id.clone(), target_uri.into())]),
                    &mut writer.references,
                )
            });
        if let Err(error) = remapped {
            writer.issues.push(MigrationIssue {
                code: "invalid_original".into(),
                message: error.message,
                span: error.span,
                blocks_conversion: true,
            });
        }
    }
    if !supported_root(root) {
        writer.unsupported(root);
    }
    if !source.dependencies.is_empty() {
        writer.issue(
            root,
            "dependencies",
            "Explicit dependency migration is required",
            true,
        );
    }
    let namespace = value(root, "nsURI").unwrap_or("");
    let draft = writer.package(root, true);
    let mut report = MigrationReport {
        version: 1,
        original: source.clone(),
        draft,
        converted: None,
        correspondence: vec![],
        issues: writer.issues,
    };
    if report.issues.iter().any(|i| i.blocks_conversion) {
        return Ok(report);
    }
    let source_bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    if !validate_definitions(
        &source_bundle,
        &report.original.id,
        namespace,
        "invalid_original",
        &mut report.issues,
        limits,
    ) {
        return Ok(report);
    }
    let converted = DefinitionSource::new(target_uri, SourceKind::Rcore, &report.draft);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![converted.clone()],
        namespaces: vec![],
    };
    if validate_definitions(
        &bundle,
        &converted.id,
        namespace,
        "target_rejected",
        &mut report.issues,
        limits,
    ) {
        let resources =
            crate::definitions::resources::ResourceSet::new_with_limits(&bundle, limits)?;
        let target = resources
            .document(&converted.id)
            .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, &root.span, e))?;
        mappings(
            root,
            &target.root,
            vec![],
            vec![],
            &mut report.correspondence,
        );
        report.converted = Some(converted);
    }
    Ok(report)
}

fn validate_definitions(
    bundle: &DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    code: &str,
    issues: &mut Vec<MigrationIssue>,
    limits: ParseLimits,
) -> bool {
    let resolved =
        crate::definitions::generic_resolution::Resolver::new_with_limits(bundle, limits)
            .and_then(|r| r.resolve_all());
    if let Err(error) = resolved {
        issues.push(MigrationIssue {
            code: code.into(),
            message: error.message,
            span: error.span,
            blocks_conversion: true,
        });
        return false;
    }
    if namespace.is_empty() {
        let resources =
            match crate::definitions::resources::ResourceSet::new_with_limits(bundle, limits) {
                Ok(r) => r,
                Err(_) => return false,
            };
        let namespaces: Vec<_> = resources
            .document(source)
            .ok()
            .into_iter()
            .flat_map(|d| &d.root.children)
            .filter_map(|n| value(n, "nsURI"))
            .collect();
        return !namespaces.is_empty()
            && namespaces
                .iter()
                .all(|n| validate_definitions(bundle, source, n, code, issues, limits));
    }
    if let Err(error) = crate::definitions::compiler::inspect_ecore_bundle_with_limits(
        bundle, source, namespace, limits,
    ) {
        let blocks_conversion = error.kind != ErrorKind::Unsupported;
        issues.push(MigrationIssue {
            code: if blocks_conversion {
                code
            } else {
                "runtime_requirement"
            }
            .into(),
            message: error.message,
            span: error.span,
            blocks_conversion,
        });
        return !blocks_conversion;
    }
    true
}

fn supported_root(root: &Node) -> bool {
    matches!(
        (root.name.namespace.as_deref(), root.name.local.as_str()),
        (Some(ECORE), "EPackage") | (Some(XMI), "XMI")
    )
}

#[path = "ecore_migration_bundle.rs"]
mod bundle_migration;
pub use bundle_migration::{
    BundleMigrationReport, BundleSelectionRemapping, CompletedBundleMigration,
    migrate_ecore_bundle, migrate_ecore_bundle_completed_with_limits,
    migrate_ecore_bundle_with_limits,
};
