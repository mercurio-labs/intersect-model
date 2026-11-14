//! Handwritten, source-preserving editing for the bounded Ecore authoring profile.
//! The XML source is authoritative. Inspection is not compilation or qualification.

use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Deserializer, Serialize};
use std::ops::Range;

/// Check the retained Ecore structural profile without returning an executable
/// schema. Success does not install providers or qualify domain behavior.
pub fn validate_structure(source: &str) -> Result<(), crate::definitions::ecore::ImportError> {
    crate::definitions::compiler::inspect_ecore_schema(source).map(|_| ())
}

/// Read-only effective member metadata. No executable schema leaves the inspector.
#[derive(Debug, Serialize)]
pub struct EffectiveMemberInspection {
    pub revision: String,
    pub members: serde_json::Value,
}

pub fn inspect_members(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    entry: &crate::definitions::bundle::SourceId,
    namespace: &str,
    class: &str,
) -> Result<EffectiveMemberInspection, crate::definitions::ecore::ImportError> {
    let schema = crate::definitions::compiler::inspect_ecore_bundle(bundle, entry, namespace)?;
    let error = |message| {
        crate::definitions::ecore::ImportError::new(
            crate::definitions::ecore::ErrorKind::InvalidDefinition,
            &crate::definitions::bundle::SourceSpan {
                source: entry.clone(),
                start: 0,
                end: 0,
            },
            message,
        )
    };
    let members = serde_json::json!({
        "features": schema.effective_features(class).map_err(error)?,
        "operations": schema.effective_operations(class).map_err(error)?,
        "resolved_operations": schema.resolved_operations(class).map_err(error)?,
    });
    Ok(EffectiveMemberInspection {
        revision: schema.revision,
        members,
    })
}

const ECORE: &str = "http://www.eclipse.org/emf/2002/Ecore";
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
const CLASS_ATTRIBUTES: &[&str] = &["xsi:type", "name", "abstract", "eSuperTypes"];
const FEATURE_ATTRIBUTES: &[&str] = &[
    "xsi:type",
    "name",
    "eType",
    "lowerBound",
    "upperBound",
    "containment",
    "eOpposite",
    "defaultValueLiteral",
    "ordered",
    "unique",
    "derived",
];

#[derive(Debug, Serialize)]
pub struct Document {
    pub source: String,
    pub name: String,
    pub namespace: String,
    pub prefix: String,
    pub classes: Vec<Class>,
    pub enums: Vec<crate::definitions::enums::Enumeration>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Class {
    pub name: String,
    #[serde(rename = "abstract")]
    pub abstract_type: bool,
    pub super_types: Vec<String>,
    pub features: Vec<Feature>,
    /// Compiled declaration metadata; inspection does not establish invocation support.
    pub operations: Vec<crate::definitions::operations::Operation>,
}

#[derive(Debug, Serialize)]
pub struct Feature {
    pub name: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub lower: i64,
    pub upper: i64,
    pub containment: bool,
    pub opposite: Option<String>,
    pub default: Option<String>,
    pub ordered: bool,
    pub unique: bool,
    pub derived: bool,
}

/// Missing fields preserve source; JSON null explicitly removes an attribute.
#[derive(Debug, Default)]
pub enum NullableText {
    #[default]
    Unchanged,
    Set(String),
    Clear,
}

impl<'de> Deserialize<'de> for NullableText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)? {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    AddEnum {
        name: String,
    },
    AddLiteral {
        enumeration: String,
        name: String,
        value: i32,
        literal: Option<String>,
    },
    SetEnum {
        enumeration: String,
        name: String,
    },
    SetLiteral {
        enumeration: String,
        literal: String,
        name: Option<String>,
        value: Option<i32>,
        #[serde(default)]
        spelling: NullableText,
    },
    DeleteEnum {
        enumeration: String,
    },
    DeleteLiteral {
        enumeration: String,
        literal: String,
    },
    AddClass {
        name: String,
    },
    AddAttribute {
        class: String,
        name: String,
        #[serde(rename = "type")]
        type_name: String,
    },
    AddReference {
        class: String,
        name: String,
        #[serde(rename = "type")]
        type_name: String,
    },
    SetPackage {
        name: Option<String>,
        namespace: Option<String>,
        prefix: Option<String>,
    },
    SetClass {
        class: String,
        name: Option<String>,
        #[serde(rename = "abstract")]
        abstract_type: Option<bool>,
        super_types: Option<Vec<String>>,
    },
    SetFeature {
        class: String,
        feature: String,
        name: Option<String>,
        #[serde(rename = "type")]
        type_name: Option<String>,
        lower: Option<i64>,
        upper: Option<i64>,
        containment: Option<bool>,
        #[serde(default)]
        opposite: NullableText,
        #[serde(default)]
        default: NullableText,
        ordered: Option<bool>,
        unique: Option<bool>,
    },
    DeleteClass {
        class: String,
    },
    DeleteFeature {
        class: String,
        feature: String,
    },
}

struct Attribute {
    name: String,
    value: String,
    value_range: Range<usize>,
    range: Range<usize>,
    quote: char,
}

struct Node {
    name: String,
    attrs: Vec<Attribute>,
    children: Vec<usize>,
    range: Range<usize>,
    open_end: usize,
    close_start: usize,
    empty: bool,
    extra_content: bool,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    fn text(&self, name: &str) -> String {
        self.attr(name).map_or_else(String::new, String::from)
    }

    fn is_class(&self) -> bool {
        self.name == "eClassifiers" && self.attr("xsi:type") == Some("ecore:EClass")
    }

    fn is_enum(&self) -> bool {
        self.name == "eClassifiers" && self.attr("xsi:type") == Some("ecore:EEnum")
    }

    fn is_literal(&self) -> bool {
        self.name == "eLiterals"
    }

    fn is_feature(&self) -> bool {
        self.name == "eStructuralFeatures"
            && matches!(
                self.attr("xsi:type"),
                Some("ecore:EAttribute" | "ecore:EReference")
            )
    }

    fn reference(&self) -> bool {
        self.attr("xsi:type") == Some("ecore:EReference")
    }
}

struct Xml {
    nodes: Vec<Node>,
    root: usize,
}

impl Xml {
    fn parse(source: &str) -> Result<Self, String> {
        xml_text(source)?;
        // quick-xml strips a BOM without counting it in buffer_position(). Keep
        // an explicit offset so patches still address the original UTF-8 bytes.
        let bom_length = if source.starts_with('\u{feff}') { 3 } else { 0 };
        let mut reader = Reader::from_str(&source[bom_length..]);
        reader.config_mut().check_comments = true;
        let mut nodes: Vec<Node> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut roots = Vec::new();
        loop {
            let start = bom_length + reader.buffer_position() as usize;
            let event = reader
                .read_event()
                .map_err(|e| format!("Invalid XML: {e}"))?;
            let end = bom_length + reader.buffer_position() as usize;
            match event {
                Event::Start(e) | Event::Empty(e) => {
                    if nodes.len() >= 50_000 || stack.len() >= 64 {
                        return Err("Ecore editing XML size or nesting budget exceeded".into());
                    }
                    let name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| e.to_string())?
                        .to_owned();
                    let empty = source
                        .get(start..end)
                        .is_some_and(|text| text.ends_with("/>"));
                    let ranges = attribute_ranges(source, start, end)?;
                    let mut attrs = Vec::new();
                    for (position, item) in e.attributes().enumerate() {
                        let attr = item.map_err(|e| format!("Invalid XML attribute: {e}"))?;
                        let name = std::str::from_utf8(attr.key.as_ref())
                            .map_err(|e| e.to_string())?
                            .to_owned();
                        let value = attr
                            .decoded_and_normalized_value(
                                quick_xml::XmlVersion::Implicit1_0,
                                reader.decoder(),
                            )
                            .map_err(|e| format!("Invalid XML attribute: {e}"))?
                            .into_owned();
                        let (range, value_range, quote) =
                            ranges.get(position).ok_or("Invalid XML attribute range")?;
                        attrs.push(Attribute {
                            name,
                            value,
                            range: range.clone(),
                            value_range: value_range.clone(),
                            quote: *quote,
                        });
                    }
                    let index = nodes.len();
                    if let Some(parent) = stack.last() {
                        nodes[*parent].children.push(index);
                    } else {
                        roots.push(index);
                    }
                    nodes.push(Node {
                        name,
                        attrs,
                        children: Vec::new(),
                        range: start..end,
                        open_end: end,
                        close_start: end,
                        empty,
                        extra_content: false,
                    });
                    if !empty {
                        stack.push(index);
                    }
                }
                Event::End(_) => {
                    let index = stack.pop().ok_or("Unexpected closing XML element")?;
                    nodes[index].close_start = start;
                    nodes[index].range.end = end;
                }
                Event::Text(e) => {
                    if !e.decode().map_err(|e| e.to_string())?.trim().is_empty() {
                        let parent = stack.last().ok_or("Unexpected text outside EPackage")?;
                        nodes[*parent].extra_content = true;
                    }
                }
                Event::Comment(_) | Event::PI(_) | Event::CData(_) | Event::GeneralRef(_) => {
                    if let Some(parent) = stack.last() {
                        nodes[*parent].extra_content = true;
                    } else if !matches!(event, Event::Comment(_) | Event::PI(_)) {
                        return Err("Unexpected content outside EPackage".into());
                    }
                }
                Event::Decl(_) => {
                    if start != bom_length {
                        return Err("XML declaration must precede the EPackage".into());
                    }
                }
                Event::DocType(_) => {
                    return Err("DTD definitions are unsupported for Ecore editing".into());
                }
                Event::Eof => break,
            }
        }
        if !stack.is_empty() || roots.len() != 1 {
            return Err("Exactly one complete EPackage is required".into());
        }
        let root = roots[0];
        if nodes[root].name != "ecore:EPackage"
            || nodes[root].attr("xmlns:ecore") != Some(ECORE)
            || nodes[root].attr("xmlns:xsi") != Some(XSI)
        {
            return Err(
                "Expected ecore:EPackage with standard ecore and xsi namespace bindings".into(),
            );
        }
        Ok(Self { nodes, root })
    }

    fn class(&self, name: &str) -> Result<usize, String> {
        self.unique(
            &self.nodes[self.root].children,
            name,
            Node::is_class,
            "class",
        )
    }

    fn feature(&self, class: usize, name: &str) -> Result<usize, String> {
        self.unique(
            &self.nodes[class].children,
            name,
            Node::is_feature,
            "feature",
        )
    }

    fn enumeration(&self, name: &str) -> Result<usize, String> {
        self.unique(&self.nodes[self.root].children, name, Node::is_enum, "enum")
    }

    fn literal(&self, enumeration: usize, name: &str) -> Result<usize, String> {
        self.unique(
            &self.nodes[enumeration].children,
            name,
            Node::is_literal,
            "literal",
        )
    }

    fn ensure_literal(
        &self,
        parent: usize,
        name: &str,
        spelling: &str,
        except: Option<usize>,
    ) -> Result<(), String> {
        self.ensure_new(parent, name, except)?;
        xml_text(spelling)?;
        if self.nodes[parent].children.iter().any(|index| {
            let node = &self.nodes[*index];
            Some(*index) != except
                && node.is_literal()
                && node.attr("literal").or_else(|| node.attr("name")) == Some(spelling)
        }) {
            return Err(format!("Duplicate enum literal spelling {spelling:?}"));
        }
        Ok(())
    }

    fn check_literal_defaults(&self, enumeration: &str, spelling: &str) -> Result<(), String> {
        let target = format!("#//{enumeration}");
        if self.nodes.iter().any(|node| {
            node.attr("eType")
                .and_then(|value| value.split_whitespace().last())
                == Some(target.as_str())
                && node.attr("defaultValueLiteral") == Some(spelling)
        }) {
            return Err(format!(
                "Literal {spelling:?} is used by an explicit attribute default; change or clear that default first"
            ));
        }
        Ok(())
    }

    fn unique(
        &self,
        children: &[usize],
        name: &str,
        filter: fn(&Node) -> bool,
        kind: &str,
    ) -> Result<usize, String> {
        let mut found = children
            .iter()
            .copied()
            .filter(|i| filter(&self.nodes[*i]) && self.nodes[*i].attr("name") == Some(name));
        let index = found
            .next()
            .ok_or_else(|| format!("Unknown {kind} {name}"))?;
        if found.next().is_some() {
            return Err(format!("Ambiguous duplicate {kind} {name}"));
        }
        Ok(index)
    }

    fn ensure_new(&self, parent: usize, name: &str, except: Option<usize>) -> Result<(), String> {
        identifier(name)?;
        if self.nodes[parent]
            .children
            .iter()
            .any(|i| Some(*i) != except && self.nodes[*i].attr("name") == Some(name))
        {
            return Err(format!("Name {name} already exists"));
        }
        Ok(())
    }

    fn check_namespaces(&self) -> Result<(), String> {
        if self.nodes.iter().enumerate().any(|(i, n)| {
            n.attrs.iter().any(|a| {
                (i != self.root && (a.name == "xmlns:ecore" || a.name == "xmlns:xsi"))
                    || (a.name == "xmlns" && !a.value.is_empty())
            })
        }) {
            return Err("Local ecore/xsi namespace overrides are preserved but must be resolved in source before structured editing".into());
        }
        Ok(())
    }

    fn deletable(&self, index: usize) -> Result<(), String> {
        let node = &self.nodes[index];
        let allowed = if node.is_class() {
            CLASS_ATTRIBUTES
        } else if node.is_feature() {
            FEATURE_ATTRIBUTES
        } else if node.is_enum() {
            &["xsi:type", "name", "serializable"]
        } else if node.is_literal() {
            &["name", "value", "literal"]
        } else {
            return Err(
                "Deletion would discard unsupported Ecore content; edit source explicitly".into(),
            );
        };
        if node.extra_content
            || node
                .attrs
                .iter()
                .any(|a| !allowed.contains(&a.name.as_str()))
            || ((node.is_feature() || node.is_literal()) && !node.children.is_empty())
        {
            return Err("Deletion would discard comments or unsupported Ecore content; edit source explicitly".into());
        }
        for child in &node.children {
            self.deletable(*child)?;
        }
        Ok(())
    }

    fn delete(&self, index: usize, local: &str, patches: &mut Vec<Patch>) -> Result<(), String> {
        self.deletable(index)?;
        let range = &self.nodes[index].range;
        for node in &self.nodes {
            if range.contains(&node.range.start) {
                continue;
            }
            if node.attrs.iter().any(|attr| {
                attr.value
                    .split_whitespace()
                    .any(|value| local_match(value, local))
            }) {
                return Err(format!(
                    "Cannot delete {local}: another element still references it"
                ));
            }
        }
        patches.push(Patch {
            range: range.clone(),
            text: String::new(),
        });
        Ok(())
    }

    fn rename_references(
        &self,
        old: &str,
        new: &str,
        patches: &mut Vec<Patch>,
    ) -> Result<(), String> {
        for node in &self.nodes {
            for attr in &node.attrs {
                if !attr
                    .value
                    .split_whitespace()
                    .any(|value| local_match(value, old))
                {
                    continue;
                }
                let recognized = (node.is_class() && attr.name == "eSuperTypes")
                    || (node.is_feature() && matches!(attr.name.as_str(), "eType" | "eOpposite"));
                if !recognized {
                    return Err(format!(
                        "Cannot rename: unsupported {}.{} references {old}; edit source explicitly",
                        node.name, attr.name
                    ));
                }
                // Replace only matching tokens, preserving whitespace within reference lists.
                let value = replace_local_tokens(&attr.value, old, new);
                set_attr(node, &attr.name, Some(&value), patches);
            }
        }
        Ok(())
    }

    fn check_feature_aliases(
        &self,
        source: &str,
        class: &str,
        feature: &str,
    ) -> Result<(), String> {
        // The authoring profile uses declaring Class/feature identities. Consult
        // the shared schema for other owners; never guess inheritance in drafts.
        let aliases: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|node| {
                let opposite = node.attr("eOpposite")?.strip_prefix("#//")?;
                let (owner, name) = opposite.split_once('/')?;
                (name == feature && owner != class).then_some(owner)
            })
            .collect();
        if aliases.is_empty() {
            return Ok(());
        }
        let schema = crate::definitions::compiler::inspect_ecore_schema(source).map_err(|_| {
            "Cannot safely resolve other Class/feature opposite identities in this draft; repair compilation or edit those references explicitly in source".to_owned()
        })?;
        if aliases.iter().any(|owner| schema.conforms(owner, class)) {
            return Err("Inherited opposite aliases require explicit source edits; use the declaring Class/feature identity".into());
        }
        Ok(())
    }
}

fn local_match(value: &str, target: &str) -> bool {
    value == target
        || value
            .strip_prefix(target)
            .is_some_and(|tail| tail.starts_with('/'))
}

fn replace_local_tokens(value: &str, old: &str, new: &str) -> String {
    let mut result = String::new();
    let mut at = 0;
    while at < value.len() {
        let rest = &value[at..];
        let whitespace = rest.chars().next().is_some_and(char::is_whitespace);
        let end = rest
            .char_indices()
            .find(|(_, c)| c.is_whitespace() != whitespace)
            .map_or(value.len(), |(i, _)| at + i);
        let token = &value[at..end];
        if !whitespace && local_match(token, old) {
            result.push_str(new);
            result.push_str(&token[old.len()..]);
        } else {
            result.push_str(token);
        }
        at = end;
    }
    result
}

type AttributeRange = (Range<usize>, Range<usize>, char);

fn attribute_ranges(source: &str, start: usize, end: usize) -> Result<Vec<AttributeRange>, String> {
    let bytes = source.as_bytes();
    let mut at = start + 1;
    while at < end && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'/' | b'>') {
        at += 1;
    }
    let mut result = Vec::new();
    loop {
        let full_start = at;
        while at < end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= end || matches!(bytes[at], b'/' | b'>') {
            break;
        }
        while at < end && !bytes[at].is_ascii_whitespace() && bytes[at] != b'=' {
            at += 1;
        }
        while at < end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= end || bytes[at] != b'=' {
            return Err("XML attribute needs '='".into());
        }
        at += 1;
        while at < end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= end || !matches!(bytes[at], b'\'' | b'"') {
            return Err("XML attribute needs a quoted value".into());
        }
        let quote = bytes[at];
        at += 1;
        let value_start = at;
        while at < end && bytes[at] != quote {
            at += 1;
        }
        if at >= end {
            return Err("Unclosed XML attribute".into());
        }
        let value_end = at;
        at += 1;
        result.push((full_start..at, value_start..value_end, quote as char));
    }
    Ok(result)
}

fn display_type(value: &str) -> String {
    if let Some(local) = value.strip_prefix("#//") {
        return local.into();
    }
    let prefix = format!("ecore:EDataType {ECORE}#//");
    value
        .strip_prefix(&prefix)
        .map_or_else(|| value.to_owned(), String::from)
}

fn boolean(node: &Node, name: &str, default: bool, warnings: &mut Vec<String>) -> bool {
    match node.attr(name) {
        Some("true") => true,
        Some("false") => false,
        None => default,
        Some(value) => {
            warnings.push(format!(
                "Invalid {name}={value:?}; displayed with default {default}. Source is preserved."
            ));
            default
        }
    }
}

fn integer(node: &Node, name: &str, default: i64, warnings: &mut Vec<String>) -> i64 {
    match node.attr(name) {
        None => default,
        Some(value) => match value.parse() {
            Ok(value) => value,
            Err(_) => {
                warnings.push(format!("Invalid {name}={value:?}; displayed with default {default}. Source is preserved."));
                default
            }
        },
    }
}

/// Inspect parseable drafts while retaining the compiler as the semantic gate.
pub fn inspect(source: &str) -> Result<Document, String> {
    inspect_compiled(source, || {
        crate::definitions::compiler::inspect_ecore_schema(source)
    })
}

/// Inspect a verified projection with its exact external resource context.
/// Standalone draft inspection retains its existing tolerant contract.
pub fn inspect_in_bundle(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    entry: &crate::definitions::bundle::SourceId,
    namespace: &str,
) -> Result<Document, String> {
    let source = bundle.source(entry)?;
    let schema = crate::definitions::compiler::inspect_ecore_bundle(bundle, entry, namespace)
        .map_err(|e| e.to_string())?;
    inspect_compiled(&source.text, || Ok(schema))
}

fn inspect_compiled(
    source: &str,
    compiled: impl FnOnce() -> Result<
        crate::definitions::schema::Schema,
        crate::definitions::ecore::ImportError,
    >,
) -> Result<Document, String> {
    let xml = Xml::parse(source)?;
    let root = &xml.nodes[xml.root];
    let mut warnings = Vec::new();
    let compiled = compiled();
    if let Err(error) = &compiled {
        warnings.push(format!(
            "Ecore is not compilable in the supported profile: {error}"
        ));
    }
    let namespaces_safe = xml.check_namespaces().is_ok();
    if !namespaces_safe {
        warnings.push("Local namespace overrides are retained; structured editing is unavailable until they are resolved in source.".into());
    }
    let mut classes = Vec::new();
    let enums: Vec<_> = compiled
        .as_ref()
        .map(|schema| {
            schema
                .enumerations
                .values()
                .filter(|enumeration| enumeration.declaration().path.len() == 1)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    for index in &root.children {
        let node = &xml.nodes[*index];
        if node.attr("xsi:type") == Some("ecore:EEnum")
            && enums
                .iter()
                .any(|enumeration| enumeration.name() == node.text("name"))
        {
            continue;
        }
        if !node.is_class() || !namespaces_safe {
            warnings.push(format!(
                "{} is preserved in source but omitted from the structured view",
                node.name
            ));
            continue;
        }
        let mut features = Vec::new();
        let compiled_class = compiled
            .as_ref()
            .ok()
            .and_then(|schema| schema.classes.get(&node.text("name")));
        let operations = compiled_class
            .map(|class| class.operations.clone())
            .unwrap_or_default();
        for child in &node.children {
            let feature = &xml.nodes[*child];
            if feature.name == "eGenericSuperTypes" && compiled_class.is_some() {
                continue;
            }
            if feature.name == "eOperations"
                && operations
                    .iter()
                    .any(|operation| operation.name == feature.text("name"))
            {
                continue;
            }
            if !feature.is_feature() {
                warnings.push(format!(
                    "{}.{} is preserved in source but omitted from the structured view",
                    node.text("name"),
                    feature.name
                ));
                continue;
            }
            features.push(Feature {
                name: feature.text("name"),
                kind: if feature.reference() {
                    "reference"
                } else {
                    "attribute"
                }
                .into(),
                type_name: display_type(&feature.text("eType")),
                lower: integer(feature, "lowerBound", 0, &mut warnings),
                upper: integer(feature, "upperBound", 1, &mut warnings),
                containment: boolean(feature, "containment", false, &mut warnings),
                opposite: feature.attr("eOpposite").map(display_type),
                default: feature.attr("defaultValueLiteral").map(String::from),
                ordered: boolean(feature, "ordered", true, &mut warnings),
                unique: boolean(feature, "unique", true, &mut warnings),
                derived: boolean(feature, "derived", false, &mut warnings),
            });
        }
        classes.push(Class {
            name: node.text("name"),
            abstract_type: boolean(node, "abstract", false, &mut warnings),
            super_types: compiled_class.map_or_else(
                || {
                    node.attr("eSuperTypes").map_or_else(Vec::new, |value| {
                        value.split_whitespace().map(display_type).collect()
                    })
                },
                |class| class.super_types().map(String::from).collect(),
            ),
            features,
            operations,
        });
    }
    Ok(Document {
        source: source.into(),
        name: root.text("name"),
        namespace: root.text("nsURI"),
        prefix: root.text("nsPrefix"),
        classes,
        enums,
        warnings,
    })
}

struct Patch {
    range: Range<usize>,
    text: String,
}

fn escaped(value: &str, quote: char) -> String {
    let mut result = String::new();
    for c in value.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '"' if quote == '"' => result.push_str("&quot;"),
            '\'' if quote == '\'' => result.push_str("&apos;"),
            '\n' => result.push_str("&#10;"),
            '\r' => result.push_str("&#13;"),
            '\t' => result.push_str("&#9;"),
            _ => result.push(c),
        }
    }
    result
}

fn set_attr(node: &Node, name: &str, value: Option<&str>, patches: &mut Vec<Patch>) {
    if let Some(attr) = node.attrs.iter().find(|a| a.name == name) {
        if value == Some(attr.value.as_str()) {
            return;
        }
        let patch = match value {
            Some(value) => Patch {
                range: attr.value_range.clone(),
                text: escaped(value, attr.quote),
            },
            None => Patch {
                range: attr.range.clone(),
                text: String::new(),
            },
        };
        // A rename and property update may both target an opposite or type attribute.
        if let Some(existing) = patches.iter_mut().find(|p| p.range == patch.range) {
            *existing = patch;
        } else {
            patches.push(patch);
        }
    } else if let Some(value) = value {
        let at = node.open_end - if node.empty { 2 } else { 1 };
        let text = format!(" {name}=\"{}\"", escaped(value, '"'));
        if let Some(existing) = patches.iter_mut().find(|p| p.range == (at..at)) {
            existing.text.push_str(&text);
        } else {
            patches.push(Patch {
                range: at..at,
                text,
            });
        }
    }
}

fn identifier(value: &str) -> Result<(), String> {
    let mut chars = value.chars();
    if !chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        || !chars.all(|c| c == '_' || c.is_alphanumeric())
    {
        return Err(format!(
            "{value:?} must be an identifier beginning with a letter or underscore"
        ));
    }
    Ok(())
}

fn xml_text(value: &str) -> Result<(), String> {
    if value.chars().any(|c| !matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
        return Err("Value contains a character forbidden in XML 1.0".into());
    }
    Ok(())
}

fn encoded_type(xml: &Xml, name: &str, reference: bool) -> Result<String, String> {
    if reference {
        xml.class(name)?;
        Ok(format!("#//{name}"))
    } else if xml.nodes[xml.root].children.iter().any(|index| {
        let node = &xml.nodes[*index];
        node.attr("xsi:type") == Some("ecore:EEnum") && node.attr("name") == Some(name)
    }) {
        Ok(format!("#//{name}"))
    } else if crate::definitions::scalars::Builtin::named(name).is_some() {
        Ok(format!("ecore:EDataType {ECORE}#//{name}"))
    } else {
        Err(
            "Attribute type must be a supported Ecore scalar or local enum in this editor profile"
                .into(),
        )
    }
}

fn insert_child(source: &str, node: &Node, child: &str, patches: &mut Vec<Patch>) {
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let line_start = source[..node.range.start].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &source[line_start..node.range.start];
    let indent = if prefix.chars().all(char::is_whitespace) {
        prefix
    } else {
        ""
    };
    if node.empty {
        patches.push(Patch {
            range: node.open_end - 2..node.open_end,
            text: format!(
                ">{newline}{indent}  {child}{newline}{indent}</{}>",
                node.name
            ),
        });
    } else {
        let close_line_start = source[..node.close_start].rfind('\n').map_or(0, |i| i + 1);
        let close_indent = &source[close_line_start..node.close_start];
        if close_indent.chars().all(char::is_whitespace) {
            patches.push(Patch {
                range: close_line_start..close_line_start,
                text: format!("{indent}  {child}{newline}"),
            });
        } else {
            patches.push(Patch {
                range: node.close_start..node.close_start,
                text: format!("{newline}{indent}  {child}{newline}{indent}"),
            });
        }
    }
}

/// Apply one atomic source edit. Unchanged bytes, including unknown XML, survive.
pub fn edit(source: &str, edit: Edit) -> Result<Document, String> {
    let xml = Xml::parse(source)?;
    xml.check_namespaces()?;
    let root = &xml.nodes[xml.root];
    let mut patches = Vec::new();
    let add_reference = matches!(&edit, Edit::AddReference { .. });
    match edit {
        Edit::AddEnum { name } => {
            xml.ensure_new(xml.root, &name, None)?;
            insert_child(
                source,
                root,
                &format!("<eClassifiers xsi:type=\"ecore:EEnum\" name=\"{name}\"/>"),
                &mut patches,
            );
        }
        Edit::AddLiteral {
            enumeration,
            name,
            value,
            literal,
        } => {
            let index = xml.enumeration(&enumeration)?;
            xml.ensure_literal(index, &name, literal.as_deref().unwrap_or(&name), None)?;
            let spelling = literal.as_ref().map_or_else(String::new, |value| {
                format!(" literal=\"{}\"", escaped(value, '"'))
            });
            insert_child(
                source,
                &xml.nodes[index],
                &format!("<eLiterals name=\"{name}\" value=\"{value}\"{spelling}/>"),
                &mut patches,
            );
        }
        Edit::SetEnum { enumeration, name } => {
            let index = xml.enumeration(&enumeration)?;
            xml.ensure_new(xml.root, &name, Some(index))?;
            if name != enumeration {
                xml.rename_references(
                    &format!("#//{enumeration}"),
                    &format!("#//{name}"),
                    &mut patches,
                )?;
            }
            set_attr(&xml.nodes[index], "name", Some(&name), &mut patches);
        }
        Edit::SetLiteral {
            enumeration,
            literal,
            name,
            value,
            spelling,
        } => {
            let parent = xml.enumeration(&enumeration)?;
            let index = xml.literal(parent, &literal)?;
            let node = &xml.nodes[index];
            let new_name = name.as_deref().unwrap_or(&literal);
            let explicit = match &spelling {
                NullableText::Unchanged => node.attr("literal"),
                NullableText::Clear => None,
                NullableText::Set(value) => Some(value.as_str()),
            };
            let old_spelling = node.attr("literal").unwrap_or(&literal);
            let new_spelling = explicit.unwrap_or(new_name);
            xml.ensure_literal(parent, new_name, new_spelling, Some(index))?;
            if old_spelling != new_spelling {
                xml.check_literal_defaults(&enumeration, old_spelling)?;
            }
            if new_name != literal {
                xml.rename_references(
                    &format!("#//{enumeration}/{literal}"),
                    &format!("#//{enumeration}/{new_name}"),
                    &mut patches,
                )?;
            }
            set_attr(node, "name", Some(new_name), &mut patches);
            if let Some(value) = value {
                set_attr(node, "value", Some(&value.to_string()), &mut patches);
            }
            set_attr(node, "literal", explicit, &mut patches);
        }
        Edit::DeleteEnum { enumeration } => {
            xml.delete(
                xml.enumeration(&enumeration)?,
                &format!("#//{enumeration}"),
                &mut patches,
            )?;
        }
        Edit::DeleteLiteral {
            enumeration,
            literal,
        } => {
            let index = xml.literal(xml.enumeration(&enumeration)?, &literal)?;
            xml.check_literal_defaults(
                &enumeration,
                xml.nodes[index].attr("literal").unwrap_or(&literal),
            )?;
            xml.delete(index, &format!("#//{enumeration}/{literal}"), &mut patches)?;
        }
        Edit::AddClass { name } => {
            xml.ensure_new(xml.root, &name, None)?;
            insert_child(
                source,
                root,
                &format!("<eClassifiers xsi:type=\"ecore:EClass\" name=\"{name}\"/>"),
                &mut patches,
            );
        }
        Edit::AddAttribute {
            class,
            name,
            type_name,
        }
        | Edit::AddReference {
            class,
            name,
            type_name,
        } => {
            let class = xml.class(&class)?;
            xml.ensure_new(class, &name, None)?;
            let reference = add_reference;
            let target = encoded_type(&xml, &type_name, reference)?;
            let kind = if reference {
                "EReference"
            } else {
                "EAttribute"
            };
            insert_child(
                source,
                &xml.nodes[class],
                &format!(
                    "<eStructuralFeatures xsi:type=\"ecore:{kind}\" name=\"{name}\" eType=\"{}\"/>",
                    escaped(&target, '"')
                ),
                &mut patches,
            );
        }
        Edit::SetPackage {
            name,
            namespace,
            prefix,
        } => {
            for (key, value) in [("name", name), ("nsURI", namespace), ("nsPrefix", prefix)] {
                if let Some(value) = value {
                    xml_text(&value)?;
                    if key == "nsURI" {
                        if value.trim().is_empty() {
                            return Err("Package namespace must not be empty".into());
                        }
                    } else {
                        identifier(&value)?;
                    }
                    set_attr(root, key, Some(&value), &mut patches);
                }
            }
        }
        Edit::SetClass {
            class,
            name,
            abstract_type,
            super_types,
        } => {
            let index = xml.class(&class)?;
            let node = &xml.nodes[index];
            if let Some(name) = name {
                xml.ensure_new(xml.root, &name, Some(index))?;
                if name != class {
                    xml.rename_references(
                        &format!("#//{class}"),
                        &format!("#//{name}"),
                        &mut patches,
                    )?;
                }
                set_attr(node, "name", Some(&name), &mut patches);
            }
            if let Some(value) = abstract_type {
                set_attr(
                    node,
                    "abstract",
                    Some(if value { "true" } else { "false" }),
                    &mut patches,
                );
            }
            if let Some(parents) = super_types {
                if parents.len() > 1 {
                    return Err("The supported compiler profile allows one superclass".into());
                }
                for parent in &parents {
                    if parent == &class {
                        return Err("A class cannot extend itself".into());
                    }
                    xml.class(parent)?;
                }
                let value = parents
                    .iter()
                    .map(|p| format!("#//{p}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                set_attr(
                    node,
                    "eSuperTypes",
                    if value.is_empty() { None } else { Some(&value) },
                    &mut patches,
                );
            }
        }
        Edit::SetFeature {
            class,
            feature,
            name,
            type_name,
            lower,
            upper,
            containment,
            opposite,
            default,
            ordered,
            unique,
        } => {
            let class_index = xml.class(&class)?;
            let index = xml.feature(class_index, &feature)?;
            let node = &xml.nodes[index];
            if let Some(name) = name {
                xml.ensure_new(class_index, &name, Some(index))?;
                if name != feature {
                    xml.check_feature_aliases(source, &class, &feature)?;
                    xml.rename_references(
                        &format!("#//{class}/{feature}"),
                        &format!("#//{class}/{name}"),
                        &mut patches,
                    )?;
                }
                set_attr(node, "name", Some(&name), &mut patches);
            }
            if let Some(value) = type_name {
                let target = encoded_type(&xml, &value, node.reference())?;
                set_attr(node, "eType", Some(&target), &mut patches);
            }
            if lower.is_some() || upper.is_some() {
                let bound = |key: &str, value: Option<i64>, fallback: i64| -> Result<i64, String> {
                    value.map_or_else(
                        || {
                            node.attr(key).map_or(Ok(fallback), |v| {
                                v.parse().map_err(|_| {
                                    format!("Invalid existing {key}; supply a replacement")
                                })
                            })
                        },
                        Ok,
                    )
                };
                let lo = bound("lowerBound", lower, 0)?;
                let hi = bound("upperBound", upper, 1)?;
                if lo < 0
                    || lo > i32::MAX as i64
                    || hi < -1
                    || hi == 0
                    || hi > i32::MAX as i64
                    || (hi >= 0 && lo > hi)
                {
                    return Err("Multiplicity requires EInt bounds with 0 <= lower <= upper, or upper = -1; upper = 0 is unsupported".into());
                }
                if let Some(value) = lower {
                    set_attr(node, "lowerBound", Some(&value.to_string()), &mut patches);
                }
                if let Some(value) = upper {
                    set_attr(node, "upperBound", Some(&value.to_string()), &mut patches);
                }
            }
            if containment == Some(true) && !node.reference() {
                return Err("Containment requires a reference".into());
            }
            for (key, value) in [
                ("containment", containment),
                ("ordered", ordered),
                ("unique", unique),
            ] {
                if let Some(value) = value {
                    set_attr(
                        node,
                        key,
                        Some(if value { "true" } else { "false" }),
                        &mut patches,
                    );
                }
            }
            match opposite {
                NullableText::Unchanged => {}
                NullableText::Clear => set_attr(node, "eOpposite", None, &mut patches),
                NullableText::Set(value) => {
                    if !node.reference() {
                        return Err("Opposites require a reference".into());
                    }
                    let value = value.strip_prefix("#//").map_or(value.as_str(), |v| v);
                    let (target_class, target_feature) = value
                        .split_once('/')
                        .ok_or("Opposite must be Class/feature")?;
                    let target = xml.feature(xml.class(target_class)?, target_feature)?;
                    if !xml.nodes[target].reference() {
                        return Err("Opposite must target a reference".into());
                    }
                    set_attr(
                        node,
                        "eOpposite",
                        Some(&format!("#//{value}")),
                        &mut patches,
                    );
                }
            }
            match default {
                NullableText::Unchanged => {}
                NullableText::Clear => set_attr(node, "defaultValueLiteral", None, &mut patches),
                NullableText::Set(value) => {
                    xml_text(&value)?;
                    if node.reference() {
                        return Err("Reference defaults are unsupported".into());
                    }
                    set_attr(node, "defaultValueLiteral", Some(&value), &mut patches);
                }
            }
        }
        Edit::DeleteClass { class } => {
            xml.delete(xml.class(&class)?, &format!("#//{class}"), &mut patches)?
        }
        Edit::DeleteFeature { class, feature } => {
            xml.check_feature_aliases(source, &class, &feature)?;
            xml.delete(
                xml.feature(xml.class(&class)?, &feature)?,
                &format!("#//{class}/{feature}"),
                &mut patches,
            )?;
        }
    }
    patches.sort_by_key(|p| (p.range.start, p.range.end));
    if patches
        .windows(2)
        .any(|p| p[0].range.end > p[1].range.start)
    {
        return Err("Conflicting source edits; apply the changes separately".into());
    }
    let mut source = source.to_owned();
    for patch in patches.into_iter().rev() {
        source.replace_range(patch.range, &patch.text);
    }
    inspect(&source)
}
