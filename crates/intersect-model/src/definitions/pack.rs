//! Handwritten language-pack container: bytes in, bytes out.
//!
//! A language pack is a deterministic binary envelope around a canonical JSON
//! manifest and a set of named section payloads. This module owns only the
//! container, the manifest schema, structural validation, and part keys. It
//! does not know any particular language (KerML, SysML, ...): languages own
//! their recipes, section contents, and loaders.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! magic    8 bytes   b"IXLPACK\0"
//! format   u32       PACK_FORMAT
//! length   u32       manifest byte length
//! manifest length    canonical UTF-8 JSON (`PackManifest`)
//! payloads           section payloads concatenated in section-name order
//! ```
//!
//! Section offsets are relative to the first payload byte and must be
//! contiguous, so the table admits no gaps, overlaps, or trailing bytes.
//!
//! A section is `json` (one UTF-8 JSON value, checked on read) or, since
//! format 2, `binary` (opaque bytes the language decodes and checks itself,
//! such as the prepared-library snapshot of decision D10).
//!
//! # Trust
//!
//! A pack is host-built output. It is trusted by convention under the
//! `HostCache` load mode, exactly like `VerifiedExecutable::load_trusted`: the
//! host only loads packs it wrote itself. The SHA-256 digests detect accidental
//! corruption and mismatched inputs; they are **not** a cryptographic signature
//! and give no protection against a deliberate forger. Packs received from a
//! network, a browser, or a model are out of scope and must not be loaded.
//!
//! The module is WASM-safe: it performs no I/O, uses no clocks, threads, or
//! environment, and never panics on malformed input.
use crate::definitions::bundle::SourceId;
use crate::definitions::conversion::ProviderId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// Leading magic of every language pack.
pub const PACK_MAGIC: [u8; 8] = *b"IXLPACK\0";
/// Container and manifest format written and accepted by this module.
/// Format 2 adds `binary` sections.
pub const PACK_FORMAT: u32 = 2;
/// Magic, format, and manifest length.
pub const PACK_HEADER_BYTES: usize = 16;
/// Largest whole pack accepted or produced (1 GiB; fits a 32-bit `usize`).
pub const MAX_PACK_BYTES: usize = 1 << 30;
/// Largest manifest accepted or produced (16 MiB).
pub const MAX_MANIFEST_BYTES: usize = 16 << 20;
/// Largest number of sections in one pack.
pub const MAX_PACK_SECTIONS: usize = 4096;
/// Largest number of parts in one pack.
pub const MAX_PACK_PARTS: usize = 1024;
/// Domain separator hashed into every part key.
pub const PART_KEY_DOMAIN: &str = "intersect.pack.part-key.v1";

/// Content digest of one file or section payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDigest {
    pub sha256: String,
    pub bytes: usize,
}

impl FileDigest {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            sha256: sha256_hex(bytes),
            bytes: bytes.len(),
        }
    }
}

/// Named, versioned build recipe that produced a pack.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackRecipe {
    pub name: String,
    pub version: String,
}

/// Payload encoding of one section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackSectionEncoding {
    Json,
    /// Reserved; refused on read and never produced.
    Cbor,
    /// Opaque bytes; the language that reads the section checks them.
    Binary,
}

impl fmt::Display for PackSectionEncoding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Json => "json",
            Self::Cbor => "cbor",
            Self::Binary => "binary",
        })
    }
}

/// One section flag. Declaration order is the serialized order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSectionFlag {
    /// The section is a primary output a loader cannot do without.
    Required,
    /// The section is a rebuildable cache; its absence is not an error.
    DerivedCache,
}

const FLAG_ORDER: [PackSectionFlag; 2] = [PackSectionFlag::Required, PackSectionFlag::DerivedCache];

/// Small typed flag set, serialized as a sorted array of flag names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackSectionFlags {
    bits: u8,
}

impl PackSectionFlags {
    pub const NONE: Self = Self { bits: 0 };
    pub const REQUIRED: Self = Self { bits: 1 };
    pub const DERIVED_CACHE: Self = Self { bits: 2 };

    const fn bit(flag: PackSectionFlag) -> u8 {
        match flag {
            PackSectionFlag::Required => 1,
            PackSectionFlag::DerivedCache => 2,
        }
    }

    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    pub const fn contains(self, flag: PackSectionFlag) -> bool {
        self.bits & Self::bit(flag) != 0
    }

    pub fn iter(self) -> impl Iterator<Item = PackSectionFlag> {
        FLAG_ORDER
            .into_iter()
            .filter(move |flag| self.contains(*flag))
    }
}

impl std::ops::BitOr for PackSectionFlags {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl Serialize for PackSectionFlags {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for PackSectionFlags {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let flags = Vec::<PackSectionFlag>::deserialize(deserializer)?;
        if flags.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(serde::de::Error::custom(
                "section flags must be sorted and unique",
            ));
        }
        Ok(flags.into_iter().fold(Self::NONE, |set, flag| Self {
            bits: set.bits | Self::bit(flag),
        }))
    }
}

/// One entry of the section table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackSection {
    pub name: String,
    /// Language-defined payload kind, opaque to this module.
    pub kind: String,
    pub flags: PackSectionFlags,
    pub encoding: PackSectionEncoding,
    /// Byte offset relative to the first payload byte.
    pub offset: usize,
    pub digest: FileDigest,
    /// Part key (or cache key) the payload was built for.
    pub part_digest: String,
    /// Language-defined digest of the produced output.
    pub output_digest: String,
}

/// One named build part: a unit with its own key and dependencies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackPart {
    pub name: String,
    /// Compiler components (keys of `PackManifest::compiler`), sorted.
    pub components: Vec<String>,
    /// Input roles (keys of `PackManifest::inputs`), sorted.
    pub inputs: Vec<String>,
    /// Names of the parts this part depends on, sorted.
    pub depends: Vec<String>,
    /// The part key; equals the `part_digest` of the part's section.
    pub input_digest: String,
    /// Equals the `output_digest` of the part's section.
    pub output_digest: String,
}

/// A host hook the pack requires at load time, identified by the same
/// provider identity the conversion registries use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackHook {
    pub provider: ProviderId,
    pub provenance: String,
}

impl PackHook {
    fn sort_key(&self) -> (&str, &str, &str) {
        (
            &self.provider.name,
            &self.provider.version,
            &self.provenance,
        )
    }
}

impl PartialOrd for PackHook {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Canonical hook order: provider name, provider version, then provenance.
impl Ord for PackHook {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

/// A pack this pack is layered on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackDependency {
    pub language: String,
    pub recipe: PackRecipe,
    pub output_digest: String,
}

/// Manifest fields chosen by the language recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackHeader {
    pub language: String,
    pub recipe: PackRecipe,
    pub compiler: BTreeMap<String, String>,
    pub recipe_constants: BTreeMap<String, Value>,
    pub inputs: BTreeMap<String, SourceId>,
    pub parts: Vec<PackPart>,
    pub requires_hooks: Vec<PackHook>,
    pub depends_on: Vec<PackDependency>,
}

/// The pack manifest, stored as canonical JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub format: u32,
    pub language: String,
    pub recipe: PackRecipe,
    /// Component name to compiler identity digest.
    pub compiler: BTreeMap<String, String>,
    pub recipe_constants: BTreeMap<String, Value>,
    /// Input role to source identity.
    pub inputs: BTreeMap<String, SourceId>,
    pub parts: Vec<PackPart>,
    pub requires_hooks: Vec<PackHook>,
    pub depends_on: Vec<PackDependency>,
    pub sections: Vec<PackSection>,
}

/// Structural failure of a language pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackError {
    TooLarge {
        bytes: usize,
        limit: usize,
    },
    Truncated {
        needed: usize,
        available: usize,
    },
    BadMagic,
    UnsupportedFormat(u32),
    ManifestTooLarge {
        bytes: usize,
        limit: usize,
    },
    ManifestPastEnd {
        length: usize,
        available: usize,
    },
    Manifest(String),
    ManifestNotCanonical,
    FormatMismatch {
        header: u32,
        manifest: u32,
    },
    TooMany {
        what: &'static str,
        count: usize,
        limit: usize,
    },
    InvalidField {
        field: String,
        reason: String,
    },
    SectionOrder {
        previous: String,
        name: String,
    },
    ConflictingFlags(String),
    UnsupportedEncoding {
        section: String,
        encoding: PackSectionEncoding,
    },
    SectionOffset {
        section: String,
        expected: usize,
        actual: usize,
    },
    SectionPastEnd {
        section: String,
    },
    TrailingBytes(usize),
    SectionDigest {
        section: String,
    },
    MissingSection(String),
    /// The section exists, but this pack was read without verifying it
    /// (`LanguagePack::read_sections`), so its bytes are not handed out.
    UnverifiedSection(String),
    PartOrder {
        previous: String,
        name: String,
    },
    UnknownComponent {
        part: String,
        component: String,
    },
    UnknownInput {
        part: String,
        role: String,
    },
    UnknownPart {
        part: String,
        depends: String,
    },
    PartSectionMismatch {
        part: String,
        field: &'static str,
    },
    DependencyCycle(Vec<String>),
    SectionJson {
        section: String,
        reason: String,
    },
    Serialize(String),
}

impl fmt::Display for PackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "language pack has {bytes} bytes; limit is {limit}"
                )
            }
            Self::Truncated { needed, available } => write!(
                formatter,
                "language pack is truncated: {needed} bytes needed, {available} available"
            ),
            Self::BadMagic => formatter.write_str("not a language pack: bad magic"),
            Self::UnsupportedFormat(format) => {
                write!(formatter, "unsupported language pack format {format}")
            }
            Self::ManifestTooLarge { bytes, limit } => write!(
                formatter,
                "language pack manifest has {bytes} bytes; limit is {limit}"
            ),
            Self::ManifestPastEnd { length, available } => write!(
                formatter,
                "language pack manifest length {length} exceeds the {available} remaining bytes"
            ),
            Self::Manifest(reason) => write!(formatter, "invalid language pack manifest: {reason}"),
            Self::ManifestNotCanonical => {
                formatter.write_str("language pack manifest is not canonical JSON")
            }
            Self::FormatMismatch { header, manifest } => write!(
                formatter,
                "language pack header format {header} differs from manifest format {manifest}"
            ),
            Self::TooMany { what, count, limit } => {
                write!(
                    formatter,
                    "language pack has {count} {what}; limit is {limit}"
                )
            }
            Self::InvalidField { field, reason } => {
                write!(formatter, "invalid language pack field {field}: {reason}")
            }
            Self::SectionOrder { previous, name } => write!(
                formatter,
                "language pack section {name:?} is not strictly after {previous:?}"
            ),
            Self::ConflictingFlags(section) => write!(
                formatter,
                "language pack section {section:?} is both required and a derived cache"
            ),
            Self::UnsupportedEncoding { section, encoding } => write!(
                formatter,
                "language pack section {section:?} uses unsupported encoding {encoding}"
            ),
            Self::SectionOffset {
                section,
                expected,
                actual,
            } => write!(
                formatter,
                "language pack section {section:?} starts at {actual}; contiguous offset is {expected}"
            ),
            Self::SectionPastEnd { section } => write!(
                formatter,
                "language pack section {section:?} extends past the end of the pack"
            ),
            Self::TrailingBytes(count) => write!(
                formatter,
                "language pack has {count} trailing bytes after the last section"
            ),
            Self::SectionDigest { section } => write!(
                formatter,
                "language pack section {section:?} does not match its digest"
            ),
            Self::UnverifiedSection(section) => {
                write!(
                    formatter,
                    "language pack section {section} was not verified when the pack was read"
                )
            }
            Self::MissingSection(section) => {
                write!(formatter, "language pack has no section {section:?}")
            }
            Self::PartOrder { previous, name } => write!(
                formatter,
                "language pack part {name:?} is not strictly after {previous:?}"
            ),
            Self::UnknownComponent { part, component } => write!(
                formatter,
                "language pack part {part:?} lists undeclared component {component:?}"
            ),
            Self::UnknownInput { part, role } => write!(
                formatter,
                "language pack part {part:?} lists undeclared input role {role:?}"
            ),
            Self::UnknownPart { part, depends } => write!(
                formatter,
                "language pack part {part:?} depends on undeclared part {depends:?}"
            ),
            Self::PartSectionMismatch { part, field } => write!(
                formatter,
                "language pack part {part:?} disagrees with its section on {field}"
            ),
            Self::DependencyCycle(parts) => write!(
                formatter,
                "language pack parts form a dependency cycle among {parts:?}"
            ),
            Self::SectionJson { section, reason } => write!(
                formatter,
                "language pack section {section:?} is not the expected JSON: {reason}"
            ),
            Self::Serialize(reason) => {
                write!(formatter, "language pack serialization failed: {reason}")
            }
        }
    }
}

impl Error for PackError {}

struct PendingSection {
    name: String,
    kind: String,
    flags: PackSectionFlags,
    payload: Result<Vec<u8>, PackError>,
    encoding: PackSectionEncoding,
    /// Bytes supplied verbatim, so their JSON encoding is not yet established.
    raw: bool,
    part_digest: String,
    output_digest: String,
}

/// Deterministic pack writer. Errors are reported by [`PackBuilder::build`].
pub struct PackBuilder {
    header: PackHeader,
    sections: Vec<PendingSection>,
}

impl PackBuilder {
    pub fn new(header: PackHeader) -> Self {
        Self {
            header,
            sections: Vec::new(),
        }
    }

    /// Adds a JSON section from already-encoded bytes, stored verbatim. The
    /// bytes must be one UTF-8 JSON value; [`PackBuilder::build`] refuses
    /// anything else with [`PackError::SectionJson`].
    pub fn section(
        mut self,
        name: impl Into<String>,
        kind: impl Into<String>,
        flags: PackSectionFlags,
        payload: Vec<u8>,
        part_digest: impl Into<String>,
        output_digest: impl Into<String>,
    ) -> Self {
        self.sections.push(PendingSection {
            name: name.into(),
            kind: kind.into(),
            flags,
            payload: Ok(payload),
            encoding: PackSectionEncoding::Json,
            raw: true,
            part_digest: part_digest.into(),
            output_digest: output_digest.into(),
        });
        self
    }

    /// Adds a JSON section from a value, encoded as canonical JSON.
    pub fn json_section<T: Serialize + ?Sized>(
        mut self,
        name: impl Into<String>,
        kind: impl Into<String>,
        flags: PackSectionFlags,
        value: &T,
        part_digest: impl Into<String>,
        output_digest: impl Into<String>,
    ) -> Self {
        self.sections.push(PendingSection {
            name: name.into(),
            kind: kind.into(),
            flags,
            payload: canonical_json(value),
            encoding: PackSectionEncoding::Json,
            raw: false,
            part_digest: part_digest.into(),
            output_digest: output_digest.into(),
        });
        self
    }

    /// Adds a binary section, stored verbatim and not interpreted by the
    /// container (only its digest is checked on read).
    pub fn binary_section(
        mut self,
        name: impl Into<String>,
        kind: impl Into<String>,
        flags: PackSectionFlags,
        payload: Vec<u8>,
        part_digest: impl Into<String>,
        output_digest: impl Into<String>,
    ) -> Self {
        self.sections.push(PendingSection {
            name: name.into(),
            kind: kind.into(),
            flags,
            payload: Ok(payload),
            encoding: PackSectionEncoding::Binary,
            raw: false,
            part_digest: part_digest.into(),
            output_digest: output_digest.into(),
        });
        self
    }

    /// Produces the pack bytes. The same header and sections always produce
    /// identical bytes, independent of the order sections were added.
    pub fn build(self) -> Result<Vec<u8>, PackError> {
        let PackHeader {
            language,
            recipe,
            compiler,
            recipe_constants,
            inputs,
            mut parts,
            mut requires_hooks,
            mut depends_on,
        } = self.header;
        let mut pending = Vec::with_capacity(self.sections.len());
        for section in self.sections {
            let payload = section.payload?;
            if section.raw {
                require_json_payload(&section.name, &payload)?;
            }
            pending.push((
                section.name,
                section.kind,
                section.flags,
                section.encoding,
                payload,
                section.part_digest,
                section.output_digest,
            ));
        }
        pending.sort_by(|left, right| left.0.cmp(&right.0));
        for part in &mut parts {
            part.components.sort();
            part.inputs.sort();
            part.depends.sort();
        }
        parts.sort_by(|left, right| left.name.cmp(&right.name));
        requires_hooks.sort();
        depends_on.sort();

        let mut sections = Vec::with_capacity(pending.len());
        let mut offset = 0usize;
        for (name, kind, flags, encoding, payload, part_digest, output_digest) in &pending {
            sections.push(PackSection {
                name: name.clone(),
                kind: kind.clone(),
                flags: *flags,
                encoding: *encoding,
                offset,
                digest: FileDigest::of(payload),
                part_digest: part_digest.clone(),
                output_digest: output_digest.clone(),
            });
            offset = checked_end(offset, payload.len())?;
        }
        let manifest = PackManifest {
            format: PACK_FORMAT,
            language,
            recipe,
            compiler,
            recipe_constants,
            inputs,
            parts,
            requires_hooks,
            depends_on,
            sections,
        };
        validate_manifest(&manifest, offset)?;
        let manifest_bytes = canonical_json(&manifest)?;
        if manifest_bytes.len() > MAX_MANIFEST_BYTES {
            return Err(PackError::ManifestTooLarge {
                bytes: manifest_bytes.len(),
                limit: MAX_MANIFEST_BYTES,
            });
        }
        let total = checked_end(PACK_HEADER_BYTES, manifest_bytes.len())
            .and_then(|end| checked_end(end, offset))?;
        let manifest_length =
            u32::try_from(manifest_bytes.len()).map_err(|_| PackError::ManifestTooLarge {
                bytes: manifest_bytes.len(),
                limit: MAX_MANIFEST_BYTES,
            })?;
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(&PACK_MAGIC);
        bytes.extend_from_slice(&PACK_FORMAT.to_le_bytes());
        bytes.extend_from_slice(&manifest_length.to_le_bytes());
        bytes.extend_from_slice(&manifest_bytes);
        for (_, _, _, _, payload, _, _) in &pending {
            bytes.extend_from_slice(payload);
        }
        Ok(bytes)
    }
}

/// A structurally verified language pack.
#[derive(Clone, Debug)]
pub struct LanguagePack {
    manifest: PackManifest,
    bytes: Vec<u8>,
    payload_start: usize,
    /// Per manifest section: its digest and encoding were verified.
    verified: Vec<bool>,
}

/// How `LanguagePack::read_checked` establishes one section's digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionCheck {
    /// Hash the section here and compare with the manifest.
    Hash,
    /// Compare the manifest with a SHA-256 (lowercase hex) the host computed
    /// over exactly this section's bytes of exactly the bytes it hands to
    /// `read_checked` (`LanguagePack::section_ranges` gives the range). The
    /// host is the party that supplies those bytes, so it is trusted for
    /// their hash as it is for the bytes; a host with a native hash need not
    /// pay for a slower one here. A digest that differs from the manifest is
    /// refused like a section that hashes differently.
    HostDigest(String),
    /// Leave the section unverified and unreadable (see `read_sections`).
    Skip,
}

/// One section's place in the pack bytes (`LanguagePack::section_ranges`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionRange {
    pub name: String,
    /// Offset of the section's first byte from the start of the pack.
    pub start: usize,
    pub bytes: usize,
}

/// The header and manifest of pack bytes, checked as `LanguagePack::read`
/// checks them (magic, format, manifest bounds, canonical manifest, section
/// table, part graph); section contents are not looked at.
fn open_manifest(bytes: &[u8]) -> Result<(PackManifest, usize), PackError> {
    check_pack_length(bytes.len())?;
    let header = bytes.get(..PACK_HEADER_BYTES).ok_or(PackError::Truncated {
        needed: PACK_HEADER_BYTES,
        available: bytes.len(),
    })?;
    if header[..8] != PACK_MAGIC {
        return Err(PackError::BadMagic);
    }
    let format = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    if format != PACK_FORMAT {
        return Err(PackError::UnsupportedFormat(format));
    }
    let length = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    let length = usize::try_from(length).map_err(|_| PackError::ManifestTooLarge {
        bytes: usize::MAX,
        limit: MAX_MANIFEST_BYTES,
    })?;
    if length > MAX_MANIFEST_BYTES {
        return Err(PackError::ManifestTooLarge {
            bytes: length,
            limit: MAX_MANIFEST_BYTES,
        });
    }
    let available = bytes.len() - PACK_HEADER_BYTES;
    if length > available {
        return Err(PackError::ManifestPastEnd { length, available });
    }
    let payload_start = PACK_HEADER_BYTES + length;
    let manifest_bytes = &bytes[PACK_HEADER_BYTES..payload_start];
    let manifest: PackManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| PackError::Manifest(error.to_string()))?;
    if canonical_json(&manifest)? != manifest_bytes {
        return Err(PackError::ManifestNotCanonical);
    }
    if manifest.format != format {
        return Err(PackError::FormatMismatch {
            header: format,
            manifest: manifest.format,
        });
    }
    validate_manifest(&manifest, bytes.len() - payload_start)?;
    Ok((manifest, payload_start))
}

impl LanguagePack {
    /// Verifies the container, manifest, section table, every section digest,
    /// every section's declared encoding, and the part graph. Any mismatch is
    /// an error; nothing is repaired.
    pub fn read(bytes: Vec<u8>) -> Result<Self, PackError> {
        Self::read_sections(bytes, |_| true)
    }

    /// `read`, for a host that uses only some sections: the container,
    /// manifest, section table, section bounds and part graph are verified
    /// as in `read`, but the digest and declared encoding only of the
    /// sections `wanted` selects. Hashing is the dominant cost of reading a
    /// large pack, so a host should not pay it for sections it never opens.
    /// The other sections stay unreadable through this pack: `section`
    /// answers `None` for them and `required_section` and `section_json`
    /// refuse with `UnverifiedSection`. Nothing unverified is handed out
    /// except by `bytes`, which says so.
    pub fn read_sections(
        bytes: Vec<u8>,
        wanted: impl Fn(&PackSection) -> bool,
    ) -> Result<Self, PackError> {
        Self::read_checked(bytes, |section| {
            if wanted(section) {
                SectionCheck::Hash
            } else {
                SectionCheck::Skip
            }
        })
    }

    /// Where each section lies in `bytes`, in manifest order, after the
    /// header and manifest checks of `read`. A host that hashes sections
    /// itself (`SectionCheck::HostDigest`) hashes these ranges.
    pub fn section_ranges(bytes: &[u8]) -> Result<Vec<SectionRange>, PackError> {
        let (manifest, payload_start) = open_manifest(bytes)?;
        manifest
            .sections
            .iter()
            .map(|section| {
                let end = checked_end(section.offset, section.digest.bytes)?;
                if end > bytes.len() - payload_start {
                    return Err(PackError::SectionPastEnd {
                        section: section.name.clone(),
                    });
                }
                Ok(SectionRange {
                    name: section.name.clone(),
                    start: payload_start + section.offset,
                    bytes: section.digest.bytes,
                })
            })
            .collect()
    }

    /// `read_sections` with the digest of each section established as
    /// `check` says: hashed here, taken from the host, or skipped. Everything
    /// else is verified as in `read`, the declared encoding of every section
    /// that is not skipped included.
    pub fn read_checked(
        bytes: Vec<u8>,
        check: impl Fn(&PackSection) -> SectionCheck,
    ) -> Result<Self, PackError> {
        let (manifest, payload_start) = open_manifest(&bytes)?;
        let payload = &bytes[payload_start..];
        let mut verified = Vec::with_capacity(manifest.sections.len());
        for section in &manifest.sections {
            let end = checked_end(section.offset, section.digest.bytes)?;
            let contents =
                payload
                    .get(section.offset..end)
                    .ok_or_else(|| PackError::SectionPastEnd {
                        section: section.name.clone(),
                    })?;
            let digest = match check(section) {
                SectionCheck::Skip => {
                    verified.push(false);
                    continue;
                }
                SectionCheck::Hash => sha256_hex(contents),
                SectionCheck::HostDigest(digest) => digest,
            };
            verified.push(true);
            if digest != section.digest.sha256 {
                return Err(PackError::SectionDigest {
                    section: section.name.clone(),
                });
            }
            match section.encoding {
                PackSectionEncoding::Json => require_json_payload(&section.name, contents)?,
                PackSectionEncoding::Binary => {}
                // Already refused by `validate_sections`; kept exhaustive.
                PackSectionEncoding::Cbor => {
                    return Err(PackError::UnsupportedEncoding {
                        section: section.name.clone(),
                        encoding: section.encoding,
                    });
                }
            }
        }
        Ok(Self {
            manifest,
            bytes,
            payload_start,
            verified,
        })
    }

    pub fn manifest(&self) -> &PackManifest {
        &self.manifest
    }

    /// The whole pack as read. Every section is verified after `read`; after
    /// `read_sections` only the selected ones are (`is_verified`).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn section_index(&self, name: &str) -> Option<usize> {
        self.manifest
            .sections
            .binary_search_by(|section| section.name.as_str().cmp(name))
            .ok()
    }

    pub fn section_entry(&self, name: &str) -> Option<&PackSection> {
        self.section_index(name)
            .and_then(|index| self.manifest.sections.get(index))
    }

    /// Whether `name` is a section whose digest and encoding were verified.
    pub fn is_verified(&self, name: &str) -> bool {
        self.section_index(name)
            .and_then(|index| self.verified.get(index))
            .is_some_and(|verified| *verified)
    }

    /// The verified bytes of a section; `None` when it is absent or was not
    /// verified when the pack was read.
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        if !self.is_verified(name) {
            return None;
        }
        let entry = self.section_entry(name)?;
        let start = self.payload_start.checked_add(entry.offset)?;
        let end = start.checked_add(entry.digest.bytes)?;
        self.bytes.get(start..end)
    }

    /// A section the caller cannot do without; absence is an error.
    pub fn required_section(&self, name: &str) -> Result<&[u8], PackError> {
        self.section(name).ok_or_else(|| {
            if self.section_entry(name).is_some() {
                PackError::UnverifiedSection(name.to_owned())
            } else {
                PackError::MissingSection(name.to_owned())
            }
        })
    }

    pub fn section_json<T: DeserializeOwned>(&self, name: &str) -> Result<T, PackError> {
        serde_json::from_slice(self.required_section(name)?).map_err(|error| {
            PackError::SectionJson {
                section: name.to_owned(),
                reason: error.to_string(),
            }
        })
    }
}

/// Inspection view: the manifest and every JSON section parsed as JSON; a
/// binary section is shown by its size and digest only.
pub fn dump_json(pack: &LanguagePack) -> Result<Value, PackError> {
    let mut sections = serde_json::Map::new();
    for entry in &pack.manifest.sections {
        let value: Value = match entry.encoding {
            PackSectionEncoding::Binary => serde_json::json!({
                "binary": {
                    "bytes": entry.digest.bytes,
                    "sha256": entry.digest.sha256,
                }
            }),
            _ if !pack.is_verified(&entry.name) => serde_json::json!({
                "unverified": {
                    "bytes": entry.digest.bytes,
                    "sha256": entry.digest.sha256,
                }
            }),
            _ => pack.section_json(&entry.name)?,
        };
        sections.insert(entry.name.clone(), value);
    }
    let manifest = serde_json::to_value(&pack.manifest)
        .map_err(|error| PackError::Serialize(error.to_string()))?;
    let mut dump = serde_json::Map::new();
    dump.insert("manifest".into(), manifest);
    dump.insert("sections".into(), Value::Object(sections));
    Ok(Value::Object(dump))
}

/// Pretty-printed [`dump_json`].
pub fn dump_json_pretty(pack: &LanguagePack) -> Result<String, PackError> {
    serde_json::to_string_pretty(&dump_json(pack)?)
        .map_err(|error| PackError::Serialize(error.to_string()))
}

/// Everything a part key depends on.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PartKeyInput<'a> {
    pub part: &'a str,
    pub part_version: u32,
    pub recipe: &'a PackRecipe,
    /// Digests of the compiler components the part lists.
    pub components: &'a BTreeMap<String, String>,
    pub recipe_constants: &'a BTreeMap<String, Value>,
    /// Input role to source identity.
    pub inputs: &'a BTreeMap<String, SourceId>,
    /// Dependency part name to that part's output digest.
    pub dependencies: &'a BTreeMap<String, String>,
}

#[derive(Serialize)]
struct PartKeyDocument<'a> {
    domain: &'static str,
    key: PartKeyInput<'a>,
}

/// SHA-256 hex over the canonical JSON of the domain and the key input.
pub fn part_key(input: PartKeyInput<'_>) -> Result<String, PackError> {
    let document = PartKeyDocument {
        domain: PART_KEY_DOMAIN,
        key: input,
    };
    Ok(sha256_hex(&canonical_json(&document)?))
}

/// Refuses a `json` section whose payload is not exactly one UTF-8 JSON value.
/// Parses without building a value, so it allocates nothing for the document.
fn require_json_payload(section: &str, payload: &[u8]) -> Result<(), PackError> {
    let refuse = |reason: String| PackError::SectionJson {
        section: section.to_owned(),
        reason,
    };
    // `from_slice` skips string bytes it ignores without checking UTF-8, so
    // check the encoding explicitly before parsing.
    let text = std::str::from_utf8(payload).map_err(|error| refuse(error.to_string()))?;
    serde_json::from_str::<serde::de::IgnoredAny>(text)
        .map(|_| ())
        .map_err(|error| refuse(error.to_string()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The length guard of [`LanguagePack::read`], applied before the header or
/// manifest is read.
fn check_pack_length(bytes: usize) -> Result<(), PackError> {
    if bytes > MAX_PACK_BYTES {
        return Err(PackError::TooLarge {
            bytes,
            limit: MAX_PACK_BYTES,
        });
    }
    Ok(())
}

fn checked_end(start: usize, length: usize) -> Result<usize, PackError> {
    start
        .checked_add(length)
        .filter(|end| *end <= MAX_PACK_BYTES)
        .ok_or(PackError::TooLarge {
            bytes: usize::MAX,
            limit: MAX_PACK_BYTES,
        })
}

/// Compact JSON with object keys sorted bytewise at every depth, independent
/// of `serde_json`'s map ordering features. This is the exact encoding
/// [`PackBuilder::json_section`] stores, so a language can compute a section's
/// output digest before [`PackBuilder::build`] (see [`canonical_digest`]).
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, PackError> {
    let value =
        serde_json::to_value(value).map_err(|error| PackError::Serialize(error.to_string()))?;
    let mut bytes = Vec::new();
    write_canonical(&value, &mut bytes)?;
    Ok(bytes)
}

/// Digest of [`canonical_json`]: equal to the `digest` [`PackBuilder::build`]
/// records for a [`PackBuilder::json_section`] of the same value.
pub fn canonical_digest<T: Serialize + ?Sized>(value: &T) -> Result<FileDigest, PackError> {
    Ok(FileDigest::of(&canonical_json(value)?))
}

fn write_canonical(value: &Value, bytes: &mut Vec<u8>) -> Result<(), PackError> {
    match value {
        Value::Array(items) => {
            bytes.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    bytes.push(b',');
                }
                write_canonical(item, bytes)?;
            }
            bytes.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            bytes.push(b'{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index != 0 {
                    bytes.push(b',');
                }
                serde_json::to_writer(&mut *bytes, key)
                    .map_err(|error| PackError::Serialize(error.to_string()))?;
                bytes.push(b':');
                write_canonical(item, bytes)?;
            }
            bytes.push(b'}');
        }
        leaf => serde_json::to_writer(&mut *bytes, leaf)
            .map_err(|error| PackError::Serialize(error.to_string()))?,
    }
    Ok(())
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> PackError {
    PackError::InvalidField {
        field: field.into(),
        reason: reason.into(),
    }
}

fn require_name(field: &str, value: &str) -> Result<(), PackError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "must be non-empty without control characters",
        ));
    }
    Ok(())
}

fn require_sha256(field: &str, value: &str) -> Result<(), PackError> {
    let hex = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !hex {
        return Err(invalid(field, "must be 64 lowercase hex digits"));
    }
    Ok(())
}

fn require_sorted_unique(field: &str, values: &[String]) -> Result<(), PackError> {
    for value in values {
        require_name(field, value)?;
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid(field, "must be sorted and unique"));
    }
    Ok(())
}

/// Shared by the builder and the reader, so a built pack is always readable.
fn validate_manifest(manifest: &PackManifest, payload_len: usize) -> Result<(), PackError> {
    require_name("language", &manifest.language)?;
    require_name("recipe.name", &manifest.recipe.name)?;
    require_name("recipe.version", &manifest.recipe.version)?;
    for (component, digest) in &manifest.compiler {
        require_name("compiler", component)?;
        require_name("compiler digest", digest)?;
    }
    for key in manifest.recipe_constants.keys() {
        require_name("recipe_constants", key)?;
    }
    for (role, source) in &manifest.inputs {
        require_name("inputs", role)?;
        require_name("inputs.uri", &source.uri)?;
        require_sha256("inputs.sha256", &source.sha256)?;
    }
    if manifest
        .requires_hooks
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid("requires_hooks", "must be sorted and unique"));
    }
    for hook in &manifest.requires_hooks {
        require_name("requires_hooks.provider.name", &hook.provider.name)?;
        require_name("requires_hooks.provider.version", &hook.provider.version)?;
        require_name("requires_hooks.provenance", &hook.provenance)?;
    }
    if manifest
        .depends_on
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid("depends_on", "must be sorted and unique"));
    }
    for dependency in &manifest.depends_on {
        require_name("depends_on.language", &dependency.language)?;
        require_name("depends_on.recipe.name", &dependency.recipe.name)?;
        require_name("depends_on.recipe.version", &dependency.recipe.version)?;
        require_sha256("depends_on.output_digest", &dependency.output_digest)?;
    }
    validate_sections(&manifest.sections, payload_len)?;
    validate_parts(manifest)
}

fn validate_sections(sections: &[PackSection], payload_len: usize) -> Result<(), PackError> {
    if sections.len() > MAX_PACK_SECTIONS {
        return Err(PackError::TooMany {
            what: "sections",
            count: sections.len(),
            limit: MAX_PACK_SECTIONS,
        });
    }
    let mut expected = 0usize;
    let mut previous: Option<&str> = None;
    for section in sections {
        require_name("sections.name", &section.name)?;
        require_name("sections.kind", &section.kind)?;
        if let Some(previous) = previous
            && previous >= section.name.as_str()
        {
            return Err(PackError::SectionOrder {
                previous: previous.to_owned(),
                name: section.name.clone(),
            });
        }
        previous = Some(&section.name);
        if section.flags.contains(PackSectionFlag::Required)
            && section.flags.contains(PackSectionFlag::DerivedCache)
        {
            return Err(PackError::ConflictingFlags(section.name.clone()));
        }
        if section.encoding == PackSectionEncoding::Cbor {
            return Err(PackError::UnsupportedEncoding {
                section: section.name.clone(),
                encoding: section.encoding,
            });
        }
        require_sha256("sections.digest.sha256", &section.digest.sha256)?;
        require_sha256("sections.part_digest", &section.part_digest)?;
        require_sha256("sections.output_digest", &section.output_digest)?;
        if section.offset != expected {
            return Err(PackError::SectionOffset {
                section: section.name.clone(),
                expected,
                actual: section.offset,
            });
        }
        expected = section
            .offset
            .checked_add(section.digest.bytes)
            .filter(|end| *end <= payload_len)
            .ok_or_else(|| PackError::SectionPastEnd {
                section: section.name.clone(),
            })?;
    }
    if expected != payload_len {
        return Err(PackError::TrailingBytes(payload_len - expected));
    }
    Ok(())
}

fn validate_parts(manifest: &PackManifest) -> Result<(), PackError> {
    let parts = &manifest.parts;
    if parts.len() > MAX_PACK_PARTS {
        return Err(PackError::TooMany {
            what: "parts",
            count: parts.len(),
            limit: MAX_PACK_PARTS,
        });
    }
    for pair in parts.windows(2) {
        if pair[0].name >= pair[1].name {
            return Err(PackError::PartOrder {
                previous: pair[0].name.clone(),
                name: pair[1].name.clone(),
            });
        }
    }
    let index_of = |name: &str| {
        parts
            .binary_search_by(|part| part.name.as_str().cmp(name))
            .ok()
    };
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); parts.len()];
    let mut remaining: Vec<usize> = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        require_name("parts.name", &part.name)?;
        require_sorted_unique("parts.components", &part.components)?;
        require_sorted_unique("parts.inputs", &part.inputs)?;
        require_sorted_unique("parts.depends", &part.depends)?;
        require_sha256("parts.input_digest", &part.input_digest)?;
        require_sha256("parts.output_digest", &part.output_digest)?;
        if let Some(component) = part
            .components
            .iter()
            .find(|component| !manifest.compiler.contains_key(*component))
        {
            return Err(PackError::UnknownComponent {
                part: part.name.clone(),
                component: component.clone(),
            });
        }
        if let Some(role) = part
            .inputs
            .iter()
            .find(|role| !manifest.inputs.contains_key(*role))
        {
            return Err(PackError::UnknownInput {
                part: part.name.clone(),
                role: role.clone(),
            });
        }
        for depends in &part.depends {
            let target = index_of(depends).ok_or_else(|| PackError::UnknownPart {
                part: part.name.clone(),
                depends: depends.clone(),
            })?;
            if let Some(list) = dependents.get_mut(target) {
                list.push(index);
            }
        }
        remaining.push(part.depends.len());
        let section = manifest
            .sections
            .binary_search_by(|section| section.name.as_str().cmp(&part.name))
            .ok()
            .and_then(|index| manifest.sections.get(index))
            .ok_or_else(|| PackError::MissingSection(part.name.clone()))?;
        if !section.flags.contains(PackSectionFlag::Required) {
            return Err(PackError::PartSectionMismatch {
                part: part.name.clone(),
                field: "flags",
            });
        }
        if section.part_digest != part.input_digest {
            return Err(PackError::PartSectionMismatch {
                part: part.name.clone(),
                field: "part_digest",
            });
        }
        if section.output_digest != part.output_digest {
            return Err(PackError::PartSectionMismatch {
                part: part.name.clone(),
                field: "output_digest",
            });
        }
    }
    // Kahn's algorithm: every part must become ready, or a cycle remains.
    let mut ready: Vec<usize> = (0..parts.len())
        .filter(|index| remaining.get(*index) == Some(&0))
        .collect();
    let mut resolved = 0usize;
    while let Some(index) = ready.pop() {
        resolved += 1;
        for dependent in dependents.get(index).into_iter().flatten() {
            if let Some(count) = remaining.get_mut(*dependent) {
                *count -= 1;
                if *count == 0 {
                    ready.push(*dependent);
                }
            }
        }
    }
    if resolved != parts.len() {
        let cycle = parts
            .iter()
            .zip(&remaining)
            .filter(|(_, count)| **count != 0)
            .map(|(part, _)| part.name.clone())
            .collect();
        return Err(PackError::DependencyCycle(cycle));
    }
    Ok(())
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
