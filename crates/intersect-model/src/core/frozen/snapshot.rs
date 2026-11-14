//! Handwritten binary snapshot of a validated frozen [`Model`] (roadmap
//! decision D10): bytes in, bytes out. Not generated.
//!
//! A snapshot is a rebuildable host cache of a prepared model (the standard
//! library), never an interchange format: XMI and JSON stay the interchange
//! formats. It stores the arena exactly as it is in memory, so loading it
//! reproduces the model the snapshot was written from (same object indices,
//! same interned symbols, same slot values, lists and carried evidence) and
//! `to_model` exports the same interchange model.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! magic      8 bytes  b"IXFSNAP\0"
//! format     u32      SNAPSHOT_FORMAT
//! flags      u32      0 (reserved)
//! metamodel  32 bytes Metamodel::digest of the metamodel it was written under
//! key        32 bytes the caller's snapshot key (a content key, D10)
//! sections   u32      SECTIONS.len()
//! table      per section: tag u32, length u64, SHA-256 of the payload (32)
//! payloads   concatenated in table order; no gaps, no trailing bytes
//! ```
//!
//! Sections, in this order:
//!
//! - `strings`: count, then each interned string (u32 length, UTF-8), in
//!   symbol order; every other section names strings by symbol index.
//! - `resources`: count, then per resource its URI symbol and its resource
//!   record (u8 present flag, then u32 length and the record's JSON).
//! - `objects`: count, then per object in store order: class, identity
//!   symbol, resource index, container object and containing feature
//!   (`u32::MAX` for none), start and end (u64), then one slot per stored
//!   feature of its class.
//! - `lists`: count, then per list its length and its slots.
//! - `json`: count, then each retained JSON value (u32 length, JSON text).
//! - `proxies`: count, then owner, feature and spelling symbol of each
//!   unresolved reference (none in a validated model).
//! - `carried`: count, then per object with construction evidence, in
//!   ascending object order, its index and the evidence as JSON
//!   (initialization receipt and assignment state).
//! - `implicit`: count, then the indices of the implicit objects, ascending.
//!
//! A slot is a tag byte and its payload: 0 unassigned (the schema default,
//! which records that the feature was never assigned), 1 absent, 2 null,
//! 3 false, 4 true, 5 integer (i64), 6 string symbol, 7 object, 8 list, 9
//! JSON value, 10 proxy (u32 each). The unassigned/assigned distinction per
//! slot is the explicit-assignment state of the model.
//!
//! # Trust
//!
//! Like a language pack, a snapshot is the host's own output, trusted by
//! convention: the section digests and the key detect corruption, stale
//! inputs and a different metamodel; they are not a signature. Decoding never
//! panics: every count, length, index and tag is bounds-checked and any
//! mismatch is an error. [`SnapshotTrust::Revalidate`] also reruns the
//! structural validation of the decoded model.
//!
//! The module performs no I/O and uses no clocks, threads or environment.
use super::{
    Diagnostics, FrozenError,
    intern::{FxHashMap, Interner, Sym},
    metamodel::{ClassId, FeatureId, Metamodel},
    model::{Carried, Header, Model, ModelData, ObjId, Proxy, Slot},
};
use crate::core::{model::AssignmentState, resources::ResourceRecord};
use intersect_model::definitions::initialization::InitializationReceipt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt, sync::Arc};

/// Leading magic of every model snapshot.
pub const SNAPSHOT_MAGIC: [u8; 8] = *b"IXFSNAP\0";
/// Snapshot format written and accepted by this module.
pub const SNAPSHOT_FORMAT: u32 = 1;
/// Magic, format, flags, metamodel digest, key and section count.
pub const SNAPSHOT_HEADER_BYTES: usize = 8 + 4 + 4 + 32 + 32 + 4;
const TABLE_ENTRY_BYTES: usize = 4 + 8 + 32;
const NONE: u32 = u32::MAX;

/// The sections in format order: (tag, name).
const SECTIONS: [(u32, &str); 8] = [
    (1, "strings"),
    (2, "resources"),
    (3, "objects"),
    (4, "lists"),
    (5, "json"),
    (6, "proxies"),
    (7, "carried"),
    (8, "implicit"),
];

/// How much of a decoded snapshot is checked again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotTrust {
    /// Bounds checks, digests, key and metamodel only: the model is the
    /// host's own validated output.
    HostCache,
    /// Also rerun the structural validation and require it to find the
    /// recorded containers.
    Revalidate,
    /// `HostCache` for bytes whose integrity the caller already verified as
    /// a whole, such as a digest-checked language-pack section: the
    /// per-section digests, which would hash the same bytes a second time,
    /// are not recomputed. Bounds, key and metamodel are checked as always.
    VerifiedHostCache,
}

/// The fixed header of a snapshot, readable without decoding the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotHeader {
    pub format: u32,
    pub metamodel: [u8; 32],
    pub key: [u8; 32],
}

/// Why a snapshot could not be written or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    Truncated {
        section: &'static str,
        needed: usize,
        available: usize,
    },
    BadMagic,
    UnsupportedFormat(u32),
    Flags(u32),
    Metamodel,
    Key,
    SectionTable(String),
    SectionDigest(&'static str),
    TrailingBytes {
        section: &'static str,
        count: usize,
    },
    Invalid {
        section: &'static str,
        reason: String,
    },
    Encode(String),
    Revalidate(String),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated {
                section,
                needed,
                available,
            } => write!(
                f,
                "model snapshot is truncated in {section}: {needed} bytes needed, {available} available"
            ),
            Self::BadMagic => f.write_str("not a model snapshot: bad magic"),
            Self::UnsupportedFormat(format) => {
                write!(f, "unsupported model snapshot format {format}")
            }
            Self::Flags(flags) => write!(f, "model snapshot has unknown flags {flags:#x}"),
            Self::Metamodel => f.write_str(
                "model snapshot was written under another metamodel (metamodel digest differs)",
            ),
            Self::Key => f.write_str("model snapshot key differs from the expected key"),
            Self::SectionTable(reason) => {
                write!(f, "model snapshot section table is invalid: {reason}")
            }
            Self::SectionDigest(section) => {
                write!(
                    f,
                    "model snapshot section {section} does not match its digest"
                )
            }
            Self::TrailingBytes { section, count } => {
                write!(
                    f,
                    "model snapshot section {section} has {count} trailing bytes"
                )
            }
            Self::Invalid { section, reason } => {
                write!(f, "model snapshot section {section} is invalid: {reason}")
            }
            Self::Encode(reason) => write!(f, "model snapshot cannot be written: {reason}"),
            Self::Revalidate(reason) => {
                write!(f, "model snapshot failed revalidation: {reason}")
            }
        }
    }
}
impl std::error::Error for SnapshotError {}

/// Construction evidence of one object as stored in the `carried` section.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CarriedWire {
    initialization: Option<InitializationReceipt>,
    assignment_state: Option<AssignmentState>,
}

impl Model {
    /// The snapshot of this model under `key`. Deterministic: the same model
    /// (built the same way) and key always give the same bytes.
    pub fn to_snapshot(&self, key: &[u8; 32]) -> Result<Vec<u8>, SnapshotError> {
        let metamodel = self
            .metamodel
            .digest()
            .map_err(|error| SnapshotError::Encode(error.to_string()))?;
        let payloads = encode_sections(&self.metamodel, &self.data)?;
        let mut bytes = Vec::with_capacity(
            SNAPSHOT_HEADER_BYTES
                + SECTIONS.len() * TABLE_ENTRY_BYTES
                + payloads.iter().map(Vec::len).sum::<usize>(),
        );
        bytes.extend_from_slice(&SNAPSHOT_MAGIC);
        bytes.extend_from_slice(&SNAPSHOT_FORMAT.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&metamodel);
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&(SECTIONS.len() as u32).to_le_bytes());
        for ((tag, _), payload) in SECTIONS.iter().zip(&payloads) {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&Sha256::digest(payload));
        }
        for payload in &payloads {
            bytes.extend_from_slice(payload);
        }
        Ok(bytes)
    }

    /// Load a snapshot written under `key` by [`Model::to_snapshot`] for this
    /// `metamodel` (same [`Metamodel::digest`]). Fails closed on anything
    /// else: wrong magic, format, flags, metamodel or key, a section table
    /// that does not tile the bytes, a section digest mismatch, and any count,
    /// length, index or tag out of bounds.
    pub fn from_snapshot(
        metamodel: Arc<Metamodel>,
        bytes: &[u8],
        key: &[u8; 32],
        trust: SnapshotTrust,
    ) -> Result<Self, SnapshotError> {
        let header = snapshot_header(bytes)?;
        let digest = metamodel
            .digest()
            .map_err(|error| SnapshotError::Encode(error.to_string()))?;
        if header.metamodel != digest {
            return Err(SnapshotError::Metamodel);
        }
        if header.key != *key {
            return Err(SnapshotError::Key);
        }
        let sections = section_payloads(bytes, trust != SnapshotTrust::VerifiedHostCache)?;
        let data = decode_sections(&metamodel, &sections)?;
        if trust == SnapshotTrust::Revalidate {
            let containers = super::validate::validate(&metamodel, &data, None)
                .map_err(|found: Diagnostics| SnapshotError::Revalidate(found.to_string()))?;
            let recorded = data.headers.iter().map(|h| (h.container, h.containing));
            if !containers.iter().copied().eq(recorded) {
                return Err(SnapshotError::Revalidate(
                    "recorded containers differ from the validated ones".into(),
                ));
            }
        }
        Ok(Model {
            metamodel,
            data,
            prepared: 0,
            // Revalidation ran with no delegated field.
            facts: super::model::PreparedFacts::validated(
                (trust == SnapshotTrust::Revalidate).then_some(true),
            ),
            base: None,
        })
    }
}

/// The fixed header of `bytes`: magic, format and flags are checked, the
/// metamodel digest and key are returned unchecked.
pub fn snapshot_header(bytes: &[u8]) -> Result<SnapshotHeader, SnapshotError> {
    let mut reader = Reader::new("header", bytes);
    if reader.take(8)? != SNAPSHOT_MAGIC {
        return Err(SnapshotError::BadMagic);
    }
    let format = reader.u32()?;
    if format != SNAPSHOT_FORMAT {
        return Err(SnapshotError::UnsupportedFormat(format));
    }
    let flags = reader.u32()?;
    if flags != 0 {
        return Err(SnapshotError::Flags(flags));
    }
    Ok(SnapshotHeader {
        format,
        metamodel: reader.array()?,
        key: reader.array()?,
    })
}

/// Check the section table and, with `digests`, each section digest; the
/// payloads in format order.
fn section_payloads(bytes: &[u8], digests: bool) -> Result<Vec<&[u8]>, SnapshotError> {
    let mut reader = Reader::new("header", bytes);
    reader.take(SNAPSHOT_HEADER_BYTES - 4)?;
    let count = reader.u32()? as usize;
    if count != SECTIONS.len() {
        return Err(SnapshotError::SectionTable(format!(
            "{count} sections, expected {}",
            SECTIONS.len()
        )));
    }
    let mut table = Vec::with_capacity(SECTIONS.len());
    for (tag, name) in SECTIONS {
        let found = reader.u32()?;
        if found != tag {
            return Err(SnapshotError::SectionTable(format!(
                "section {name} has tag {found}, expected {tag}"
            )));
        }
        let length = usize::try_from(reader.u64()?).map_err(|_| {
            SnapshotError::SectionTable(format!("section {name} length exceeds memory"))
        })?;
        let digest: [u8; 32] = reader.array()?;
        table.push((name, length, digest));
    }
    let mut start = reader.position();
    let mut payloads = Vec::with_capacity(table.len());
    for (name, length, digest) in table {
        let end = start
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or(SnapshotError::Truncated {
                section: name,
                needed: length,
                available: bytes.len().saturating_sub(start),
            })?;
        let payload = bytes.get(start..end).ok_or(SnapshotError::Truncated {
            section: name,
            needed: length,
            available: bytes.len().saturating_sub(start),
        })?;
        if digests && Sha256::digest(payload).as_slice() != digest {
            return Err(SnapshotError::SectionDigest(name));
        }
        payloads.push(payload);
        start = end;
    }
    if start != bytes.len() {
        return Err(SnapshotError::TrailingBytes {
            section: "snapshot",
            count: bytes.len() - start,
        });
    }
    Ok(payloads)
}

// ---------------------------------------------------------------- encode

fn encode_error(reason: impl Into<String>) -> SnapshotError {
    SnapshotError::Encode(reason.into())
}

fn count(value: usize, what: &str) -> Result<u32, SnapshotError> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value != NONE)
        .ok_or_else(|| encode_error(format!("too many {what}")))
}

struct Writer(Vec<u8>);
impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8], what: &str) -> Result<(), SnapshotError> {
        self.u32(count(value.len(), what)?);
        self.0.extend_from_slice(value);
        Ok(())
    }
    fn json<T: Serialize>(&mut self, value: &T, what: &str) -> Result<(), SnapshotError> {
        let text = serde_json::to_vec(value).map_err(|error| encode_error(error.to_string()))?;
        self.bytes(&text, what)
    }
    fn slot(&mut self, slot: Slot) {
        match slot {
            Slot::Default => self.u8(0),
            Slot::Absent => self.u8(1),
            Slot::Null => self.u8(2),
            Slot::Bool(false) => self.u8(3),
            Slot::Bool(true) => self.u8(4),
            Slot::Int(value) => {
                self.u8(5);
                self.0.extend_from_slice(&value.to_le_bytes());
            }
            Slot::Str(sym) => {
                self.u8(6);
                self.u32(sym.0);
            }
            Slot::Ref(obj) => {
                self.u8(7);
                self.u32(obj.0);
            }
            Slot::Many(list) => {
                self.u8(8);
                self.u32(list);
            }
            Slot::Json(index) => {
                self.u8(9);
                self.u32(index);
            }
            Slot::Proxy(index) => {
                self.u8(10);
                self.u32(index);
            }
        }
    }
}

fn encode_sections(mm: &Metamodel, data: &ModelData) -> Result<Vec<Vec<u8>>, SnapshotError> {
    let mut strings = Writer(Vec::new());
    strings.u32(count(data.strings.len(), "strings")?);
    for text in data.strings.iter() {
        strings.bytes(text.as_bytes(), "string bytes")?;
    }

    let mut resources = Writer(Vec::new());
    if data.resources.len() != data.resource_records.len() {
        return Err(encode_error("resource table and records differ in length"));
    }
    resources.u32(count(data.resources.len(), "resources")?);
    for (uri, record) in data.resources.iter().zip(&data.resource_records) {
        resources.u32(uri.0);
        match record {
            None => resources.u8(0),
            Some(record) => {
                resources.u8(1);
                resources.json(record, "resource record bytes")?;
            }
        }
    }

    let mut objects = Writer(Vec::new());
    objects.u32(count(data.headers.len(), "objects")?);
    let mut next_slot = 0usize;
    for header in &data.headers {
        let stored = mm
            .class_info(header.class)
            .ok_or_else(|| encode_error("object of an unknown class"))?
            .stored
            .len();
        // Slots are appended per object at creation, so they tile the slot
        // array in object order; the decoder relies on that.
        if header.slots as usize != next_slot {
            return Err(encode_error("object slots are not contiguous"));
        }
        next_slot += stored;
        objects.u32(header.class.0);
        objects.u32(header.id.0);
        objects.u32(header.resource);
        objects.u32(header.container);
        objects.u32(header.containing);
        objects.u64(header.start as u64);
        objects.u64(header.end as u64);
        let slots = data
            .slots
            .get(header.slots as usize..next_slot)
            .ok_or_else(|| encode_error("object slots out of range"))?;
        for slot in slots {
            objects.slot(*slot);
        }
    }
    if next_slot != data.slots.len() {
        return Err(encode_error("slots not owned by any object"));
    }

    let mut lists = Writer(Vec::new());
    lists.u32(count(data.lists.len(), "lists")?);
    for list in &data.lists {
        lists.u32(count(list.len(), "list items")?);
        for slot in list {
            lists.slot(*slot);
        }
    }

    let mut json = Writer(Vec::new());
    json.u32(count(data.json.len(), "JSON values")?);
    for value in &data.json {
        json.json(value, "JSON value bytes")?;
    }

    let mut proxies = Writer(Vec::new());
    proxies.u32(count(data.proxies.len(), "proxies")?);
    for proxy in &data.proxies {
        proxies.u32(proxy.owner.0);
        proxies.u32(proxy.feature.0);
        proxies.u32(proxy.spelling.0);
    }

    let mut carried = Writer(Vec::new());
    let mut evidence: Vec<_> = data.carried.iter().collect();
    evidence.sort_by_key(|(obj, _)| **obj);
    carried.u32(count(evidence.len(), "carried objects")?);
    for (obj, item) in evidence {
        carried.u32(obj.0);
        carried.json(
            &CarriedWire {
                initialization: item.initialization.clone(),
                assignment_state: item.assignment_state.clone(),
            },
            "carried evidence bytes",
        )?;
    }

    let mut implicit = Writer(Vec::new());
    implicit.u32(count(data.implicit.len(), "implicit objects")?);
    for obj in &data.implicit {
        implicit.u32(obj.0);
    }

    Ok(vec![
        strings.0,
        resources.0,
        objects.0,
        lists.0,
        json.0,
        proxies.0,
        carried.0,
        implicit.0,
    ])
}

// ---------------------------------------------------------------- decode

struct Reader<'a> {
    section: &'static str,
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(section: &'static str, bytes: &'a [u8]) -> Self {
        Self {
            section,
            bytes,
            at: 0,
        }
    }
    fn position(&self) -> usize {
        self.at
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(SnapshotError::Truncated {
                section: self.section,
                needed: length,
                available: self.remaining(),
            })?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(SnapshotError::Truncated {
                section: self.section,
                needed: length,
                available: self.remaining(),
            })?;
        self.at = end;
        Ok(slice)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], SnapshotError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.array::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn i64(&mut self) -> Result<i64, SnapshotError> {
        Ok(i64::from_le_bytes(self.array()?))
    }
    fn usize(&mut self) -> Result<usize, SnapshotError> {
        let value = self.u64()?;
        usize::try_from(value).map_err(|_| self.invalid(format!("offset {value} exceeds memory")))
    }
    /// An element count; each element takes at least `min_bytes`, so a
    /// corrupted count cannot reserve more than the remaining bytes allow.
    fn count(&mut self, min_bytes: usize) -> Result<usize, SnapshotError> {
        let value = self.u32()? as usize;
        if value.saturating_mul(min_bytes.max(1)) > self.remaining() {
            return Err(SnapshotError::Truncated {
                section: self.section,
                needed: value.saturating_mul(min_bytes.max(1)),
                available: self.remaining(),
            });
        }
        Ok(value)
    }
    fn bytes(&mut self) -> Result<&'a [u8], SnapshotError> {
        let length = self.u32()? as usize;
        self.take(length)
    }
    fn json<T: for<'de> Deserialize<'de>>(&mut self, what: &str) -> Result<T, SnapshotError> {
        let text = self.bytes()?;
        serde_json::from_slice(text).map_err(|error| self.invalid(format!("{what}: {error}")))
    }
    fn invalid(&self, reason: impl Into<String>) -> SnapshotError {
        SnapshotError::Invalid {
            section: self.section,
            reason: reason.into(),
        }
    }
    fn finish(self) -> Result<(), SnapshotError> {
        match self.remaining() {
            0 => Ok(()),
            count => Err(SnapshotError::TrailingBytes {
                section: self.section,
                count,
            }),
        }
    }
}

/// Bounds for the indices a slot may name.
struct Limits {
    strings: usize,
    objects: usize,
    lists: usize,
    json: usize,
    proxies: usize,
}
impl Limits {
    fn slot(&self, reader: &mut Reader<'_>, in_list: bool) -> Result<Slot, SnapshotError> {
        let tag = reader.u8()?;
        let index = |reader: &mut Reader<'_>, limit: usize, what: &str| {
            let value = reader.u32()?;
            if (value as usize) < limit {
                Ok(value)
            } else {
                Err(reader.invalid(format!("{what} index {value} out of range ({limit})")))
            }
        };
        Ok(match tag {
            0 => Slot::Default,
            1 => Slot::Absent,
            2 => Slot::Null,
            3 => Slot::Bool(false),
            4 => Slot::Bool(true),
            5 => Slot::Int(reader.i64()?),
            6 => Slot::Str(Sym(index(reader, self.strings, "string")?)),
            7 => Slot::Ref(ObjId(index(reader, self.objects, "object")?)),
            // A list never holds a list (nested arrays are JSON values), so
            // decoding a slot cannot recurse without bound.
            8 if in_list => return Err(reader.invalid("a list item is a list")),
            8 => Slot::Many(index(reader, self.lists, "list")?),
            9 => Slot::Json(index(reader, self.json, "JSON value")?),
            10 => Slot::Proxy(index(reader, self.proxies, "proxy")?),
            other => return Err(reader.invalid(format!("unknown slot tag {other}"))),
        })
    }
}

/// Peek a section's leading count without consuming it.
fn leading_count(section: &'static str, payload: &[u8]) -> Result<usize, SnapshotError> {
    Ok(Reader::new(section, payload).u32()? as usize)
}

fn decode_sections(mm: &Metamodel, sections: &[&[u8]]) -> Result<ModelData, SnapshotError> {
    let [
        strings,
        resources,
        objects,
        lists,
        json,
        proxies,
        carried,
        implicit,
    ] = sections
    else {
        return Err(SnapshotError::SectionTable("wrong section count".into()));
    };
    let mut data = ModelData::default();

    let mut reader = Reader::new("strings", strings);
    let total = reader.count(4)?;
    let mut interner = Interner::default();
    for index in 0..total {
        let text = std::str::from_utf8(reader.bytes()?)
            .map_err(|error| reader.invalid(format!("string {index}: {error}")))?;
        let sym = interner
            .intern(text)
            .map_err(|error| reader.invalid(error.to_string()))?;
        if sym.0 as usize != index {
            return Err(reader.invalid(format!("string {index} repeats string {}", sym.0)));
        }
    }
    reader.finish()?;
    data.strings = interner;

    let limits = Limits {
        strings: data.strings.len(),
        objects: leading_count("objects", objects)?,
        lists: leading_count("lists", lists)?,
        json: leading_count("json", json)?,
        proxies: leading_count("proxies", proxies)?,
    };
    let symbol = |reader: &mut Reader<'_>, what: &str| -> Result<Sym, SnapshotError> {
        let value = reader.u32()?;
        if (value as usize) < limits.strings {
            Ok(Sym(value))
        } else {
            Err(reader.invalid(format!("{what} string {value} out of range")))
        }
    };

    let mut reader = Reader::new("resources", resources);
    let total = reader.count(5)?;
    let mut seen = BTreeSet::new();
    for _ in 0..total {
        let uri = symbol(&mut reader, "resource URI")?;
        if !seen.insert(uri) {
            return Err(reader.invalid(format!(
                "resource {} appears twice",
                data.strings.resolve(uri)
            )));
        }
        let record: Option<ResourceRecord> = match reader.u8()? {
            0 => None,
            1 => Some(reader.json("resource record")?),
            other => return Err(reader.invalid(format!("unknown record flag {other}"))),
        };
        data.resources.push(uri);
        data.resource_records.push(record);
    }
    reader.finish()?;

    let mut reader = Reader::new("objects", objects);
    let total = reader.count(36)?;
    data.headers.reserve(total);
    let features = mm.feature_count();
    let mut by_id: FxHashMap<Sym, ObjId> = FxHashMap::default();
    by_id.reserve(total);
    for index in 0..total {
        let class = ClassId(reader.u32()?);
        let info = mm
            .class_info(class)
            .ok_or_else(|| reader.invalid(format!("object {index}: unknown class {}", class.0)))?;
        let id = symbol(&mut reader, "object identity")?;
        let resource = reader.u32()?;
        if resource as usize >= data.resources.len() {
            return Err(reader.invalid(format!("object {index}: resource {resource} out of range")));
        }
        let container = reader.u32()?;
        let containing = reader.u32()?;
        if (container == NONE) != (containing == NONE)
            || (container != NONE && container as usize >= total)
            || (containing != NONE && containing as usize >= features)
        {
            return Err(reader.invalid(format!("object {index}: container out of range")));
        }
        let start = reader.usize()?;
        let end = reader.usize()?;
        let obj = ObjId(index as u32);
        if by_id.insert(id, obj).is_some() {
            return Err(reader.invalid(format!(
                "object {index}: identity {} repeats",
                data.strings.resolve(id)
            )));
        }
        let slots = u32::try_from(data.slots.len())
            .map_err(|_| reader.invalid("too many slots for this platform"))?;
        for _ in 0..info.stored.len() {
            data.slots.push(limits.slot(&mut reader, false)?);
        }
        data.headers.push(Header {
            class,
            id,
            resource,
            slots,
            container,
            containing,
            start,
            end,
        });
    }
    reader.finish()?;
    data.by_id = by_id;

    let mut reader = Reader::new("lists", lists);
    let total = reader.count(4)?;
    data.lists.reserve(total);
    for _ in 0..total {
        let length = reader.count(1)?;
        let mut items = Vec::with_capacity(length);
        for _ in 0..length {
            items.push(limits.slot(&mut reader, true)?);
        }
        data.lists.push(items);
    }
    reader.finish()?;

    let mut reader = Reader::new("json", json);
    let total = reader.count(4)?;
    data.json.reserve(total);
    for index in 0..total {
        data.json.push(reader.json(&format!("JSON value {index}"))?);
    }
    reader.finish()?;

    let mut reader = Reader::new("proxies", proxies);
    let total = reader.count(12)?;
    for index in 0..total {
        let owner = reader.u32()?;
        let feature = reader.u32()?;
        if owner as usize >= data.headers.len() || feature as usize >= features {
            return Err(reader.invalid(format!("proxy {index} out of range")));
        }
        let spelling = symbol(&mut reader, "proxy spelling")?;
        data.proxies.push(Proxy {
            owner: ObjId(owner),
            feature: FeatureId(feature),
            spelling,
        });
    }
    reader.finish()?;

    let mut reader = Reader::new("carried", carried);
    let total = reader.count(8)?;
    let mut previous: Option<u32> = None;
    for _ in 0..total {
        let obj = reader.u32()?;
        if obj as usize >= data.headers.len() || previous.is_some_and(|p| p >= obj) {
            return Err(
                reader.invalid(format!("carried object {obj} out of range or out of order"))
            );
        }
        previous = Some(obj);
        let wire: CarriedWire = reader.json("carried evidence")?;
        data.carried.insert(
            ObjId(obj),
            Box::new(Carried {
                initialization: wire.initialization,
                assignment_state: wire.assignment_state,
            }),
        );
    }
    reader.finish()?;

    let mut reader = Reader::new("implicit", implicit);
    let total = reader.count(4)?;
    let mut previous: Option<u32> = None;
    for _ in 0..total {
        let obj = reader.u32()?;
        if obj as usize >= data.headers.len() || previous.is_some_and(|p| p >= obj) {
            return Err(reader.invalid(format!(
                "implicit object {obj} out of range or out of order"
            )));
        }
        previous = Some(obj);
        data.implicit.insert(ObjId(obj));
    }
    reader.finish()?;
    Ok(data)
}

/// A frozen-store failure while preparing a snapshot.
impl From<FrozenError> for SnapshotError {
    fn from(error: FrozenError) -> Self {
        SnapshotError::Encode(error.to_string())
    }
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
