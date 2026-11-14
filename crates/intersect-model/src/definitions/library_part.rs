//! Handwritten prepared-library language-pack part (roadmap decision D10):
//! bytes in, bytes out. Not generated.
//!
//! A language whose standard library hosts would otherwise parse at every
//! start stores the prepared library in its pack as one binary section: the
//! library manifest (each source's logical URI, language and SHA-256), the
//! library source texts (so a `Recompile` check can rebuild it and
//! diagnostics can quote library lines) and a model snapshot
//! (`core::frozen::Model::to_snapshot`). The part is a rebuildable cache:
//! a host that finds no part, or a part whose key differs from the key it
//! recomputes ([`library_key`]), parses the sources instead.
//!
//! Payload layout (all integers little-endian):
//!
//! ```text
//! magic     8 bytes  b"IXLLIB\0\0"
//! format    u32      LIBRARY_PART_FORMAT
//! manifest  u32 length, canonical JSON (`LibraryManifest`)
//! sources   one per manifest entry, in manifest order: u32 length, UTF-8 text
//! snapshot  u64 length, the model snapshot bytes
//! ```
//!
//! Decoding checks the magic, format, that the manifest is canonical, sorted
//! and unique, that every source text has its manifest digest and size, and
//! that nothing trails the snapshot. The snapshot's own header, section
//! digests and key are checked when it is loaded.
//!
//! The module performs no I/O and uses no clocks, threads or environment.
use crate::definitions::{
    bundle::SourceId,
    pack::{PackError, PackRecipe, canonical_json},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Leading magic of a prepared-library part payload.
pub const LIBRARY_PART_MAGIC: [u8; 8] = *b"IXLLIB\0\0";
/// Payload format written and accepted by this module.
pub const LIBRARY_PART_FORMAT: u32 = 1;
/// Domain separator hashed into every library key.
pub const LIBRARY_KEY_DOMAIN: &str = "intersect.pack.library-key.v1";

/// One library source: its logical URI, the language that parses it and
/// the SHA-256 and size of its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryEntry {
    pub uri: String,
    pub language: String,
    pub sha256: String,
    pub bytes: usize,
}

/// The library sources, sorted by logical URI, each URI once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryManifest {
    pub entries: Vec<LibraryEntry>,
}

/// One library source with its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibrarySource {
    pub uri: String,
    pub language: String,
    pub text: String,
}

fn invalid(field: &str, reason: impl Into<String>) -> PackError {
    PackError::InvalidField {
        field: field.into(),
        reason: reason.into(),
    }
}

impl LibraryManifest {
    /// The manifest of `sources` (in any order); refuses a repeated URI or
    /// an empty URI or language.
    pub fn of_sources(sources: &[LibrarySource]) -> Result<Self, PackError> {
        let mut entries: Vec<LibraryEntry> = sources
            .iter()
            .map(|source| {
                let id = SourceId::for_text(&source.uri, &source.text);
                LibraryEntry {
                    uri: id.uri,
                    language: source.language.clone(),
                    sha256: id.sha256,
                    bytes: source.text.len(),
                }
            })
            .collect();
        entries.sort_by(|left, right| left.uri.cmp(&right.uri));
        let manifest = Self { entries };
        manifest.check()?;
        Ok(manifest)
    }
    /// Sorted, unique, non-empty URIs and languages, 64-hex digests.
    pub fn check(&self) -> Result<(), PackError> {
        for entry in &self.entries {
            if entry.uri.is_empty() || entry.language.is_empty() {
                return Err(invalid(
                    "library.entries",
                    "URI and language must be non-empty",
                ));
            }
            let hex = entry.sha256.len() == 64
                && entry
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
            if !hex {
                return Err(invalid(
                    "library.entries.sha256",
                    format!("{}: must be 64 lowercase hex digits", entry.uri),
                ));
            }
        }
        if let Some(pair) = self
            .entries
            .windows(2)
            .find(|pair| pair[0].uri >= pair[1].uri)
        {
            return Err(invalid(
                "library.entries",
                format!(
                    "must be sorted by URI without repeats ({} before {})",
                    pair[0].uri, pair[1].uri
                ),
            ));
        }
        Ok(())
    }
    /// SHA-256 hex of the canonical JSON manifest.
    pub fn digest(&self) -> Result<String, PackError> {
        Ok(format!("{:x}", Sha256::digest(canonical_json(self)?)))
    }
}

/// Everything the key of a prepared library depends on (decision D10).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct LibraryKeyInput<'a> {
    /// The pack part name of the library.
    pub part: &'a str,
    pub part_version: u32,
    pub recipe: &'a PackRecipe,
    /// The library sources (URI, language, SHA-256 and size of each).
    pub manifest: &'a LibraryManifest,
    /// Part name to part key of the program parts that parse the library.
    pub programs: &'a BTreeMap<String, String>,
    /// Identity of the Rcore bindings that link and complete it.
    pub bindings: &'a str,
    /// `Metamodel::digest` of the metamodel it is stored under, hex.
    pub metamodel: &'a str,
    /// Compiler components (crate source identities) that parse, link,
    /// complete and encode it.
    pub compiler: &'a BTreeMap<String, String>,
    /// The model snapshot format (`core::frozen::SNAPSHOT_FORMAT`).
    pub snapshot_format: u32,
    /// The library part payload format ([`LIBRARY_PART_FORMAT`]).
    pub part_format: u32,
}

#[derive(Serialize)]
struct LibraryKeyDocument<'a> {
    domain: &'static str,
    key: LibraryKeyInput<'a>,
}

/// SHA-256 hex over the canonical JSON of the domain and the key input, the
/// same scheme as `pack::part_key`.
pub fn library_key(input: LibraryKeyInput<'_>) -> Result<String, PackError> {
    let document = LibraryKeyDocument {
        domain: LIBRARY_KEY_DOMAIN,
        key: input,
    };
    Ok(format!("{:x}", Sha256::digest(canonical_json(&document)?)))
}

/// A 64-hex key as the 32 bytes a model snapshot header stores.
pub fn key_bytes(key: &str) -> Result<[u8; 32], PackError> {
    let refuse = || invalid("library key", "must be 64 lowercase hex digits");
    if key.len() != 64 {
        return Err(refuse());
    }
    let mut out = [0u8; 32];
    for (index, pair) in key.as_bytes().chunks(2).enumerate() {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let (Some(high), Some(low)) = (
            pair.first().copied().and_then(digit),
            pair.get(1).copied().and_then(digit),
        ) else {
            return Err(refuse());
        };
        if let Some(cell) = out.get_mut(index) {
            *cell = high << 4 | low;
        }
    }
    Ok(out)
}

/// Encode a prepared-library payload: the manifest of `sources`, their texts
/// in manifest order, and `snapshot`.
pub fn encode_library_part(
    sources: &[LibrarySource],
    snapshot: &[u8],
) -> Result<Vec<u8>, PackError> {
    let manifest = LibraryManifest::of_sources(sources)?;
    let manifest_bytes = canonical_json(&manifest)?;
    let mut by_uri: BTreeMap<&str, &LibrarySource> = BTreeMap::new();
    for source in sources {
        by_uri.insert(&source.uri, source);
    }
    let length = |value: usize, field: &str| {
        u32::try_from(value).map_err(|_| invalid(field, "longer than 4 GiB"))
    };
    let mut bytes = Vec::with_capacity(
        manifest_bytes.len()
            + sources.iter().map(|s| s.text.len() + 4).sum::<usize>()
            + snapshot.len()
            + 32,
    );
    bytes.extend_from_slice(&LIBRARY_PART_MAGIC);
    bytes.extend_from_slice(&LIBRARY_PART_FORMAT.to_le_bytes());
    bytes.extend_from_slice(&length(manifest_bytes.len(), "library.manifest")?.to_le_bytes());
    bytes.extend_from_slice(&manifest_bytes);
    for entry in &manifest.entries {
        let source = by_uri
            .get(entry.uri.as_str())
            .ok_or_else(|| invalid("library.sources", format!("{} is missing", entry.uri)))?;
        bytes.extend_from_slice(&length(source.text.len(), "library.sources")?.to_le_bytes());
        bytes.extend_from_slice(source.text.as_bytes());
    }
    bytes.extend_from_slice(&(snapshot.len() as u64).to_le_bytes());
    bytes.extend_from_slice(snapshot);
    Ok(bytes)
}

/// A decoded prepared-library payload, borrowing the pack's bytes.
#[derive(Clone, Debug)]
pub struct LibraryPart<'a> {
    pub manifest: LibraryManifest,
    /// The source texts, in manifest order.
    pub texts: Vec<&'a str>,
    pub snapshot: &'a [u8],
}

impl LibraryPart<'_> {
    /// The library sources with their texts, in manifest order.
    pub fn sources(&self) -> Vec<LibrarySource> {
        self.manifest
            .entries
            .iter()
            .zip(&self.texts)
            .map(|(entry, text)| LibrarySource {
                uri: entry.uri.clone(),
                language: entry.language.clone(),
                text: (*text).to_owned(),
            })
            .collect()
    }
}

/// Decode and check a prepared-library payload (see the module docs).
pub fn decode_library_part(bytes: &[u8]) -> Result<LibraryPart<'_>, PackError> {
    let mut at = 0usize;
    let mut take = |length: usize| -> Result<&[u8], PackError> {
        let end = at
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or(PackError::Truncated {
                needed: length,
                available: bytes.len().saturating_sub(at),
            })?;
        let slice = bytes.get(at..end).ok_or(PackError::Truncated {
            needed: length,
            available: bytes.len().saturating_sub(at),
        })?;
        at = end;
        Ok(slice)
    };
    let word = |slice: &[u8]| -> Result<u32, PackError> {
        let array: [u8; 4] = slice
            .try_into()
            .map_err(|_| invalid("library part", "short integer"))?;
        Ok(u32::from_le_bytes(array))
    };
    if take(8)? != LIBRARY_PART_MAGIC {
        return Err(invalid(
            "library part",
            "not a prepared-library payload (bad magic)",
        ));
    }
    let format = word(take(4)?)?;
    if format != LIBRARY_PART_FORMAT {
        return Err(invalid(
            "library part",
            format!("unsupported prepared-library format {format}"),
        ));
    }
    let length = word(take(4)?)? as usize;
    let manifest_bytes = take(length)?;
    let manifest: LibraryManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| invalid("library.manifest", error.to_string()))?;
    if canonical_json(&manifest)? != manifest_bytes {
        return Err(invalid("library.manifest", "not canonical JSON"));
    }
    manifest.check()?;
    let mut texts = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let length = word(take(4)?)? as usize;
        let text = std::str::from_utf8(take(length)?)
            .map_err(|error| invalid("library.sources", format!("{}: {error}", entry.uri)))?;
        if text.len() != entry.bytes
            || format!("{:x}", Sha256::digest(text.as_bytes())) != entry.sha256
        {
            return Err(invalid(
                "library.sources",
                format!("{} does not match its manifest digest", entry.uri),
            ));
        }
        texts.push(text);
    }
    let length_bytes: [u8; 8] = take(8)?
        .try_into()
        .map_err(|_| invalid("library part", "short snapshot length"))?;
    let length = usize::try_from(u64::from_le_bytes(length_bytes))
        .map_err(|_| invalid("library.snapshot", "longer than memory"))?;
    let snapshot = take(length)?;
    if at != bytes.len() {
        return Err(PackError::TrailingBytes(bytes.len() - at));
    }
    Ok(LibraryPart {
        manifest,
        texts,
        snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Vec<LibrarySource> {
        vec![
            LibrarySource {
                uri: "lib/b.sysml".into(),
                language: "sysml".into(),
                text: "package B;".into(),
            },
            LibrarySource {
                uri: "lib/a.kerml".into(),
                language: "kerml".into(),
                text: "package A;".into(),
            },
        ]
    }

    #[test]
    fn library_part_round_trips_and_sorts_its_manifest() -> Result<(), PackError> {
        let bytes = encode_library_part(&sources(), b"snapshot")?;
        let part = decode_library_part(&bytes)?;
        let uris: Vec<_> = part
            .manifest
            .entries
            .iter()
            .map(|e| e.uri.as_str())
            .collect();
        assert_eq!(uris, ["lib/a.kerml", "lib/b.sysml"]);
        assert_eq!(part.texts, ["package A;", "package B;"]);
        assert_eq!(part.snapshot, b"snapshot");
        assert_eq!(bytes, encode_library_part(&sources(), b"snapshot")?);
        Ok(())
    }

    #[test]
    fn library_part_refuses_corruption() -> Result<(), PackError> {
        let bytes = encode_library_part(&sources(), b"snapshot")?;
        for cut in [0, 7, 12, bytes.len() - 1] {
            assert!(decode_library_part(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(
            decode_library_part(&longer).err(),
            Some(PackError::TrailingBytes(1))
        );
        // Flip one byte of the first source text: its digest no longer matches.
        let text = bytes
            .windows(10)
            .position(|w| w == b"package A;")
            .ok_or_else(|| invalid("test", "text not found"))?;
        let mut flipped = bytes.clone();
        flipped[text] ^= 0x01;
        let error = decode_library_part(&flipped).err().map(|e| e.to_string());
        assert!(
            error
                .as_deref()
                .is_some_and(|e| e.contains("manifest digest")),
            "{error:?}"
        );
        Ok(())
    }

    #[test]
    fn library_manifest_refuses_repeated_uris() {
        let mut twice = sources();
        twice.push(twice[0].clone());
        assert!(LibraryManifest::of_sources(&twice).is_err());
    }

    #[test]
    fn library_key_moves_with_every_input() -> Result<(), PackError> {
        let manifest = LibraryManifest::of_sources(&sources())?;
        let recipe = PackRecipe {
            name: "r".into(),
            version: "1".into(),
        };
        let programs: BTreeMap<String, String> = [("program/a".into(), "k".into())].into();
        let compiler: BTreeMap<String, String> = [("crate".into(), "id".into())].into();
        let input = LibraryKeyInput {
            part: "library/x",
            part_version: 1,
            recipe: &recipe,
            manifest: &manifest,
            programs: &programs,
            bindings: "b",
            metamodel: "m",
            compiler: &compiler,
            snapshot_format: 1,
            part_format: LIBRARY_PART_FORMAT,
        };
        let base = library_key(input)?;
        assert_eq!(base, library_key(input)?);
        let mut edited = sources();
        edited[0].text.push(' ');
        let other_manifest = LibraryManifest::of_sources(&edited)?;
        let other_programs: BTreeMap<String, String> = [("program/a".into(), "k2".into())].into();
        let other_compiler: BTreeMap<String, String> = [("crate".into(), "id2".into())].into();
        for changed in [
            LibraryKeyInput {
                manifest: &other_manifest,
                ..input
            },
            LibraryKeyInput {
                programs: &other_programs,
                ..input
            },
            LibraryKeyInput {
                bindings: "b2",
                ..input
            },
            LibraryKeyInput {
                metamodel: "m2",
                ..input
            },
            LibraryKeyInput {
                compiler: &other_compiler,
                ..input
            },
            LibraryKeyInput {
                snapshot_format: 2,
                ..input
            },
        ] {
            assert_ne!(base, library_key(changed)?);
        }
        let bytes = key_bytes(&base)?;
        assert_eq!(
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            base
        );
        assert!(key_bytes("zz").is_err());
        Ok(())
    }
}
