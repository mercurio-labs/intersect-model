//! Handwritten strict XML reader for the bounded XMI profile.
use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;

pub const XMI: &str = "http://www.omg.org/XMI";
pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
pub const IX: &str = "urn:intersect:xmi:1";
pub type Name = (String, String);
#[derive(Default, Debug)]
pub struct Node {
    pub name: Name,
    pub attrs: BTreeMap<Name, String>,
    pub namespaces: BTreeMap<String, String>,
    pub children: Vec<Node>,
    pub text: String,
    pub start: usize,
    pub end: usize,
}
impl Node {
    pub fn attr(&self, ns: &str, name: &str) -> Option<&str> {
        self.attrs
            .get(&(ns.into(), name.into()))
            .map(String::as_str)
    }
    pub fn attributes(&self, allowed: &[(&str, &str)]) -> Result<(), String> {
        for (ns, name) in self.attrs.keys() {
            if !allowed.contains(&(ns.as_str(), name.as_str())) {
                return Err(format!("Unsupported XML attribute {{{ns}}}{name}"));
            }
        }
        Ok(())
    }
    pub fn whitespace(&self) -> Result<(), String> {
        if self
            .text
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
        {
            Ok(())
        } else {
            Err("Unexpected XML text".into())
        }
    }
    pub fn qname(&self, value: &str) -> Result<Name, String> {
        expand(value, &self.namespaces, false)
    }
}
fn expand(
    value: &str,
    namespaces: &BTreeMap<String, String>,
    attribute: bool,
) -> Result<Name, String> {
    match value.split_once(':') {
        Some((prefix, local)) if valid_name(prefix) && valid_name(local) => {
            let namespace = namespaces
                .get(prefix)
                .filter(|ns| !ns.is_empty())
                .ok_or_else(|| format!("Unbound XML prefix: {prefix}"))?;
            Ok((namespace.clone(), local.into()))
        }
        None if valid_name(value) => Ok((
            if attribute {
                String::new()
            } else {
                namespaces.get("").cloned().unwrap_or_default()
            },
            value.into(),
        )),
        _ => Err(format!("Unsupported XML name: {value}")),
    }
}
// The compiled definition profile uses ASCII identifiers; reject wider names
// explicitly rather than producing malformed XML.
pub fn valid_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
pub fn characters(value: &str) -> Result<(), String> {
    if value.chars().all(|c| matches!(c as u32, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)) {
        Ok(())
    } else { Err("Invalid XML 1.0 character".into()) }
}
pub fn escape(value: &str) -> Result<String, String> {
    characters(value)?;
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#9;"))
}
pub fn parse(source: &str) -> Result<Node, String> {
    characters(source)?;
    let mut reader = Reader::from_str(source);
    reader.config_mut().check_comments = true;
    let mut stack = vec![Node::default()];
    let mut declaration = false;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event().map_err(|e| e.to_string())?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let mut namespaces = stack.last().ok_or("Missing XML parent")?.namespaces.clone();
                let mut raw = Vec::new();
                for attribute in e.attributes() {
                    let attribute = attribute.map_err(|e| e.to_string())?;
                    let name = std::str::from_utf8(attribute.key.as_ref())
                        .map_err(|e| e.to_string())?
                        .to_owned();
                    let value = attribute
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|e| e.to_string())?
                        .into_owned();
                    characters(&value)?;
                    if name == "xmlns" {
                        namespaces.insert(String::new(), value);
                    } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                        if !valid_name(prefix)
                            || matches!(prefix, "xml" | "xmlns")
                            || value.is_empty()
                        {
                            return Err("Unsupported namespace declaration".into());
                        }
                        namespaces.insert(prefix.into(), value);
                    } else {
                        raw.push((name, value));
                    }
                }
                let mut node = Node {
                    name: expand(
                        std::str::from_utf8(e.name().as_ref()).map_err(|e| e.to_string())?,
                        &namespaces,
                        false,
                    )?,
                    namespaces,
                    start,
                    end: reader.buffer_position() as usize,
                    ..Node::default()
                };
                for (name, value) in raw {
                    let name = expand(&name, &node.namespaces, true)?;
                    if node.attrs.insert(name, value).is_some() {
                        return Err("Duplicate expanded XML attribute".into());
                    }
                }
                if empty {
                    stack
                        .last_mut()
                        .ok_or("Missing XML parent")?
                        .children
                        .push(node);
                } else {
                    stack.push(node);
                    if stack.len() > 128 {
                        return Err("XMI nesting limit (127) exceeded".into());
                    }
                }
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err("Unexpected XML closing tag".into());
                }
                let mut node = stack.pop().ok_or("Missing XML element")?;
                node.end = reader.buffer_position() as usize;
                stack
                    .last_mut()
                    .ok_or("Missing XML parent")?
                    .children
                    .push(node);
            }
            Event::Text(e) => stack.last_mut().ok_or("Missing XML parent")?.text.push_str(
                &e.decode()
                    .map_err(|e| e.to_string())?
                    .replace("\r\n", "\n")
                    .replace('\r', "\n"),
            ),
            Event::GeneralRef(e) => {
                let reference = format!("&{};", e.decode().map_err(|e| e.to_string())?);
                let value = quick_xml::escape::unescape(&reference).map_err(|e| e.to_string())?;
                characters(&value)?;
                stack
                    .last_mut()
                    .ok_or("Missing XML parent")?
                    .text
                    .push_str(&value);
            }
            Event::Decl(e) => {
                if declaration || start != 0 && start != 3 {
                    return Err("Misplaced XML declaration".into());
                }
                declaration = true;
                if e.version().map_err(|e| e.to_string())?.as_ref() != b"1.0" {
                    return Err("Only XML 1.0 is supported".into());
                }
                if let Some(encoding) = e.encoding() {
                    let encoding = encoding.map_err(|e| e.to_string())?;
                    let ascii = encoding.eq_ignore_ascii_case(b"ASCII")
                        || encoding.eq_ignore_ascii_case(b"US-ASCII");
                    if !(encoding.eq_ignore_ascii_case(b"UTF-8") || ascii && source.is_ascii()) {
                        return Err("Only UTF-8 or valid ASCII XMI is supported".into());
                    }
                }
            }
            Event::Comment(_) => {}
            Event::Eof => break,
            _ => {
                return Err(
                    "Unsupported XML construct (DTD, CDATA or processing instruction)".into(),
                );
            }
        }
    }
    if stack.len() != 1 {
        return Err("Unclosed XML element".into());
    }
    let mut document = stack.pop().ok_or("Missing XML document")?;
    document.whitespace()?;
    if document.children.len() != 1 {
        return Err("XMI requires exactly one XML root".into());
    }
    Ok(document.children.remove(0))
}
