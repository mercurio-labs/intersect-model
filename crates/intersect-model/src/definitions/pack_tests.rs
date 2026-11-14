// Handwritten language-pack container controls (language-pack step 5): round
// trip, deterministic bytes, every structural tamper refused, part keys, and
// the dump-json inspection view. The container is language-neutral; the
// sample recipe below is synthetic.
use super::*;
use serde_json::json;

fn digest(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

fn source(uri: &str, text: &str) -> SourceId {
    SourceId::for_text(uri, text)
}

fn hook(name: &str, version: &str, provenance: &str) -> PackHook {
    PackHook {
        provider: ProviderId {
            name: name.into(),
            version: version.into(),
        },
        provenance: provenance.into(),
    }
}

fn dependency(language: &str, output: &str) -> PackDependency {
    PackDependency {
        language: language.into(),
        recipe: PackRecipe {
            name: format!("{language}.recipe"),
            version: "1".into(),
        },
        output_digest: digest(output),
    }
}

fn header() -> PackHeader {
    let mut compiler = BTreeMap::new();
    compiler.insert("model".to_owned(), digest("model"));
    compiler.insert("syntax".to_owned(), digest("syntax"));
    let mut recipe_constants = BTreeMap::new();
    recipe_constants.insert("call_depth".to_owned(), json!(512));
    recipe_constants.insert("profile".to_owned(), json!({"z": 1, "a": [true, null]}));
    let mut inputs = BTreeMap::new();
    inputs.insert("grammar".to_owned(), source("urn:grammar", "grammar"));
    inputs.insert("metamodel".to_owned(), source("urn:metamodel", "metamodel"));
    PackHeader {
        language: "example.language".into(),
        recipe: PackRecipe {
            name: "example.recipe".into(),
            version: "1".into(),
        },
        compiler,
        recipe_constants,
        inputs,
        parts: vec![
            PackPart {
                name: "program".into(),
                components: vec!["syntax".into(), "model".into()],
                inputs: vec!["grammar".into()],
                depends: vec!["metamodel".into()],
                input_digest: digest("program-key"),
                output_digest: digest("program-out"),
            },
            PackPart {
                name: "metamodel".into(),
                components: vec!["model".into()],
                inputs: vec!["metamodel".into()],
                depends: Vec::new(),
                input_digest: digest("metamodel-key"),
                output_digest: digest("metamodel-out"),
            },
        ],
        // Deliberately unsorted: the builder canonicalizes hook order.
        requires_hooks: vec![
            hook("example.hook", "1", "builtin"),
            hook("another.hook", "2", "host"),
        ],
        depends_on: Vec::new(),
    }
}

fn builder() -> PackBuilder {
    PackBuilder::new(header())
        .json_section(
            "program",
            "example.program",
            PackSectionFlags::REQUIRED,
            &json!({"rules": ["a", "b"], "depth": 2}),
            digest("program-key"),
            digest("program-out"),
        )
        .section(
            "cache/tables",
            "example.tables",
            PackSectionFlags::DERIVED_CACHE,
            br#"{"rows":[1,2,3]}"#.to_vec(),
            digest("tables-key"),
            digest("tables-out"),
        )
        .json_section(
            "metamodel",
            "example.metamodel",
            PackSectionFlags::REQUIRED,
            &json!({"classes": ["Root"]}),
            digest("metamodel-key"),
            digest("metamodel-out"),
        )
}

fn built() -> Vec<u8> {
    builder().build().expect("sample pack builds")
}

fn split(bytes: &[u8]) -> (Value, Vec<u8>) {
    let length = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    let manifest = serde_json::from_slice(&bytes[16..16 + length]).expect("manifest");
    (manifest, bytes[16 + length..].to_vec())
}

/// Rewrites the manifest canonically, so structural checks (not the canonical
/// check) are what refuse the tamper.
fn reframe(bytes: &[u8], edit: impl FnOnce(&mut Value, &mut Vec<u8>)) -> Vec<u8> {
    let (mut manifest, mut payload) = split(bytes);
    edit(&mut manifest, &mut payload);
    let manifest = canonical_json(&manifest).expect("canonical");
    let mut out = Vec::new();
    out.extend_from_slice(&PACK_MAGIC);
    out.extend_from_slice(&PACK_FORMAT.to_le_bytes());
    out.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
    out.extend_from_slice(&manifest);
    out.extend_from_slice(&payload);
    out
}

fn refused(bytes: Vec<u8>) -> PackError {
    LanguagePack::read(bytes).expect_err("tampered pack must be refused")
}

#[test]
fn pack_round_trips_manifest_and_sections() {
    let pack = LanguagePack::read(built()).expect("reads");
    let manifest = pack.manifest();
    assert_eq!(manifest.format, PACK_FORMAT);
    assert_eq!(manifest.language, "example.language");
    let names: Vec<_> = manifest.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["cache/tables", "metamodel", "program"]);
    let parts: Vec<_> = manifest.parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(parts, ["metamodel", "program"]);
    assert_eq!(manifest.parts[1].components, ["model", "syntax"]);
    assert_eq!(manifest.sections[0].offset, 0);
    assert_eq!(
        pack.section("cache/tables"),
        Some(&br#"{"rows":[1,2,3]}"#[..])
    );
    let program: Value = pack.section_json("program").expect("program json");
    assert_eq!(program, json!({"depth": 2, "rules": ["a", "b"]}));
    assert_eq!(
        pack.section("program"),
        Some(&br#"{"depth":2,"rules":["a","b"]}"#[..])
    );
    assert!(pack.section("absent").is_none());
    assert_eq!(
        pack.required_section("absent"),
        Err(PackError::MissingSection("absent".into()))
    );
    let entry = pack.section_entry("metamodel").expect("entry");
    assert!(entry.flags.contains(PackSectionFlag::Required));
    assert!(!entry.flags.contains(PackSectionFlag::DerivedCache));
}

#[test]
fn pack_bytes_are_deterministic_and_order_independent() {
    let first = built();
    assert_eq!(first, built());
    let reordered = PackBuilder::new(header())
        .json_section(
            "metamodel",
            "example.metamodel",
            PackSectionFlags::REQUIRED,
            &json!({"classes": ["Root"]}),
            digest("metamodel-key"),
            digest("metamodel-out"),
        )
        .json_section(
            "program",
            "example.program",
            PackSectionFlags::REQUIRED,
            &json!({"depth": 2, "rules": ["a", "b"]}),
            digest("program-key"),
            digest("program-out"),
        )
        .section(
            "cache/tables",
            "example.tables",
            PackSectionFlags::DERIVED_CACHE,
            br#"{"rows":[1,2,3]}"#.to_vec(),
            digest("tables-key"),
            digest("tables-out"),
        )
        .build()
        .expect("builds");
    assert_eq!(first, reordered);
    let pack = LanguagePack::read(first.clone()).expect("reads");
    assert_eq!(pack.bytes(), &first[..]);
    let hooks: Vec<_> = pack
        .manifest()
        .requires_hooks
        .iter()
        .map(|hook| hook.provider.name.as_str())
        .collect();
    assert_eq!(hooks, ["another.hook", "example.hook"]);
}

fn with_header(header: PackHeader) -> Result<Vec<u8>, PackError> {
    let mut builder = builder();
    builder.header = header;
    builder.build()
}

#[test]
fn pack_bytes_are_independent_of_part_hook_and_dependency_order() {
    let mut forward = header();
    forward.depends_on = vec![dependency("base.a", "a"), dependency("base.b", "b")];
    let mut reversed = forward.clone();
    reversed.parts.reverse();
    for part in &mut reversed.parts {
        part.components.reverse();
        part.inputs.reverse();
        part.depends.reverse();
    }
    reversed.requires_hooks.reverse();
    reversed.depends_on.reverse();
    assert_ne!(forward, reversed);
    assert_eq!(
        with_header(forward).expect("builds"),
        with_header(reversed).expect("builds")
    );
}

#[test]
fn pack_flags_serialize_as_sorted_names() {
    let both = PackSectionFlags::DERIVED_CACHE | PackSectionFlags::REQUIRED;
    assert_eq!(
        serde_json::to_value(both).expect("flags"),
        json!(["required", "derived_cache"])
    );
    assert!(
        serde_json::from_value::<PackSectionFlags>(json!(["derived_cache", "required"])).is_err()
    );
    assert!(serde_json::from_value::<PackSectionFlags>(json!(["required", "required"])).is_err());
    assert!(serde_json::from_value::<PackSectionFlags>(json!(["signed"])).is_err());
}

#[test]
fn pack_refuses_bad_magic_format_and_truncation() {
    let bytes = built();
    let mut magic = bytes.clone();
    magic[0] = b'X';
    assert_eq!(refused(magic), PackError::BadMagic);
    let mut format = bytes.clone();
    format[8] = 9;
    assert_eq!(refused(format), PackError::UnsupportedFormat(9));
    assert!(matches!(
        refused(bytes[..10].to_vec()),
        PackError::Truncated { .. }
    ));
    assert_eq!(
        refused(bytes[..bytes.len() - 1].to_vec()),
        PackError::SectionPastEnd {
            section: "program".into()
        }
    );
    assert!(matches!(
        refused(Vec::new()),
        PackError::Truncated { needed: 16, .. }
    ));
}

#[test]
fn pack_refuses_manifest_length_past_end_and_oversize() {
    let bytes = built();
    let mut past = bytes.clone();
    let beyond = (bytes.len() as u32).to_le_bytes();
    past[12..16].copy_from_slice(&beyond);
    assert!(matches!(refused(past), PackError::ManifestPastEnd { .. }));
    let mut huge = bytes.clone();
    huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(refused(huge), PackError::ManifestTooLarge { .. }));
    let mut short = bytes.clone();
    let (manifest, _) = split(&bytes);
    let length = canonical_json(&manifest).expect("canonical").len() as u32 - 1;
    short[12..16].copy_from_slice(&length.to_le_bytes());
    assert!(matches!(refused(short), PackError::Manifest(_)));
}

#[test]
fn pack_refuses_trailing_bytes() {
    let mut bytes = built();
    bytes.push(b' ');
    assert_eq!(refused(bytes), PackError::TrailingBytes(1));
}

#[test]
fn pack_refuses_flipped_payload_byte() {
    let mut bytes = built();
    let last = bytes.len() - 2;
    bytes[last] ^= 0x01;
    assert_eq!(
        refused(bytes),
        PackError::SectionDigest {
            section: "program".into()
        }
    );
}

#[test]
fn pack_refuses_wrong_byte_count() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][2]["digest"]["bytes"] = json!(1);
    });
    let program_bytes = br#"{"depth":2,"rules":["a","b"]}"#.len();
    assert_eq!(refused(bytes), PackError::TrailingBytes(program_bytes - 1));
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][0]["digest"]["bytes"] = json!(3);
    });
    assert_eq!(
        refused(bytes),
        PackError::SectionOffset {
            section: "metamodel".into(),
            expected: 3,
            actual: 16
        }
    );
}

#[test]
fn pack_refuses_unsorted_and_duplicate_sections() {
    let bytes = reframe(&built(), |manifest, _| {
        let sections = manifest["sections"].as_array_mut().expect("sections");
        // Keep offsets contiguous so only the name order is wrong.
        sections.swap(0, 1);
        let first = sections[0]["digest"]["bytes"].clone();
        sections[0]["offset"] = json!(0);
        sections[1]["offset"] = first;
    });
    assert_eq!(
        refused(bytes),
        PackError::SectionOrder {
            previous: "metamodel".into(),
            name: "cache/tables".into()
        }
    );
    // A duplicated entry with consistent offsets and payload is refused on read.
    let bytes = reframe(&built(), |manifest, payload| {
        let sections = manifest["sections"].as_array_mut().expect("sections");
        let copy = sections[1].clone();
        let start = copy["offset"].as_u64().expect("offset") as usize;
        let length = copy["digest"]["bytes"].as_u64().expect("bytes") as usize;
        let mut duplicate = copy;
        duplicate["offset"] = json!(start + length);
        sections[2]["offset"] = json!(start + 2 * length);
        sections.insert(2, duplicate);
        let contents = payload[start..start + length].to_vec();
        payload.splice(start + length..start + length, contents);
    });
    assert_eq!(
        refused(bytes),
        PackError::SectionOrder {
            previous: "metamodel".into(),
            name: "metamodel".into()
        }
    );
    let duplicate = PackBuilder::new(header())
        .section(
            "metamodel",
            "example.metamodel",
            PackSectionFlags::REQUIRED,
            b"{}".to_vec(),
            digest("metamodel-key"),
            digest("metamodel-out"),
        )
        .section(
            "metamodel",
            "example.metamodel",
            PackSectionFlags::REQUIRED,
            b"{}".to_vec(),
            digest("metamodel-key"),
            digest("metamodel-out"),
        )
        .build();
    assert!(matches!(duplicate, Err(PackError::SectionOrder { .. })));
}

/// The length guard `LanguagePack::read` runs first refuses exactly the
/// lengths over `MAX_PACK_BYTES`, without allocating a pack that size.
#[test]
fn pack_length_guard_refuses_only_oversized_packs() {
    assert_eq!(check_pack_length(MAX_PACK_BYTES), Ok(()));
    assert_eq!(
        check_pack_length(MAX_PACK_BYTES + 1),
        Err(PackError::TooLarge {
            bytes: MAX_PACK_BYTES + 1,
            limit: MAX_PACK_BYTES,
        })
    );
}

/// A pack over `MAX_PACK_BYTES` is refused by its length alone, before the
/// header or manifest is read. The zeroed buffer's pages are never touched
/// (so not resident), but on Windows the allocation is still charged against
/// the commit limit, so this end-to-end check is opt-in:
/// `cargo test -p intersect-model --lib pack_refuses_oversized -- --ignored`.
#[cfg(target_pointer_width = "64")]
#[test]
#[ignore = "allocates and commits MAX_PACK_BYTES + 1 bytes"]
fn pack_refuses_oversized_pack_before_parsing() {
    let bytes = vec![0u8; MAX_PACK_BYTES + 1];
    assert_eq!(
        refused(bytes),
        PackError::TooLarge {
            bytes: MAX_PACK_BYTES + 1,
            limit: MAX_PACK_BYTES,
        }
    );
}

/// A section whose declared byte count runs past the end of the file is
/// refused by the section table check, before its bytes are hashed or parsed,
/// including counts that overflow `offset + bytes`.
#[cfg(target_pointer_width = "64")]
#[test]
fn pack_refuses_section_size_past_end_of_file() {
    let bytes = built();
    for declared in [bytes.len() as u64, 1 << 40, u64::MAX] {
        for (index, name) in [(0, "cache/tables"), (2, "program")] {
            let tampered = reframe(&bytes, |manifest, _| {
                manifest["sections"][index]["digest"]["bytes"] = json!(declared);
            });
            assert_eq!(
                refused(tampered),
                PackError::SectionPastEnd {
                    section: name.into()
                },
                "{name} declared {declared} bytes"
            );
        }
    }
}

#[test]
fn pack_refuses_overlapping_offsets() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][1]["offset"] = json!(0);
    });
    assert!(matches!(
        refused(bytes),
        PackError::SectionOffset {
            expected: 16,
            actual: 0,
            ..
        }
    ));
}

#[test]
fn pack_refuses_unknown_manifest_field_and_non_canonical_manifest() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["signature"] = json!("trusted");
    });
    assert!(
        matches!(refused(bytes), PackError::Manifest(reason) if reason.contains("unknown field"))
    );
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][0]["extra"] = json!(1);
    });
    assert!(
        matches!(refused(bytes), PackError::Manifest(reason) if reason.contains("unknown field `extra`"))
    );
    let bytes = built();
    let (manifest, payload) = split(&bytes);
    let pretty = serde_json::to_vec_pretty(&manifest).expect("pretty");
    let mut out = bytes[..12].to_vec();
    out.extend_from_slice(&(pretty.len() as u32).to_le_bytes());
    out.extend_from_slice(&pretty);
    out.extend_from_slice(&payload);
    assert_eq!(refused(out), PackError::ManifestNotCanonical);
}

/// Writes the manifest compactly with its top-level keys in reverse order:
/// valid JSON and a valid manifest, but not the canonical encoding.
#[test]
fn pack_refuses_compact_manifest_with_unsorted_keys() {
    let bytes = built();
    let (manifest, payload) = split(&bytes);
    let object = manifest.as_object().expect("object");
    let mut text = vec![b'{'];
    for (index, (key, value)) in object.iter().rev().enumerate() {
        if index != 0 {
            text.push(b',');
        }
        text.extend_from_slice(&serde_json::to_vec(key).expect("key"));
        text.push(b':');
        text.extend_from_slice(&canonical_json(value).expect("value"));
    }
    text.push(b'}');
    let reparsed: PackManifest = serde_json::from_slice(&text).expect("still a manifest");
    let canonical = canonical_json(&reparsed).expect("canonical");
    assert_eq!(canonical.len(), text.len());
    assert_ne!(canonical, text);
    let mut out = bytes[..12].to_vec();
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(&text);
    out.extend_from_slice(&payload);
    assert_eq!(refused(out), PackError::ManifestNotCanonical);
}

#[derive(Serialize)]
struct Unsorted {
    zeta: u8,
    alpha: Inner,
}

#[derive(Serialize)]
struct Inner {
    y: u8,
    b: u8,
}

/// Field declaration order is not alphabetical, so a writer that followed
/// serde's emission order (or `serde_json`'s `preserve_order`) would differ.
#[test]
fn canonical_json_sorts_keys_regardless_of_declaration_order() {
    let value = Unsorted {
        zeta: 1,
        alpha: Inner { y: 2, b: 3 },
    };
    let canonical = canonical_json(&value).expect("canonical");
    assert_eq!(canonical, br#"{"alpha":{"b":3,"y":2},"zeta":1}"#.to_vec());
    assert_ne!(
        serde_json::to_vec(&value).expect("declaration order"),
        canonical
    );
}

#[test]
fn pack_refuses_format_mismatch_between_header_and_manifest() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["format"] = json!(7);
    });
    assert_eq!(
        refused(bytes),
        PackError::FormatMismatch {
            header: PACK_FORMAT,
            manifest: 7
        }
    );
}

#[test]
fn pack_refuses_cbor_encoding() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][0]["encoding"] = json!("cbor");
    });
    assert!(matches!(
        refused(bytes),
        PackError::UnsupportedEncoding {
            encoding: PackSectionEncoding::Cbor,
            ..
        }
    ));
}

#[test]
fn pack_refuses_conflicting_flags() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][0]["flags"] = json!(["required", "derived_cache"]);
    });
    assert_eq!(
        refused(bytes),
        PackError::ConflictingFlags("cache/tables".into())
    );
}

#[test]
fn pack_refuses_missing_required_section() {
    let missing = PackBuilder::new(header())
        .json_section(
            "metamodel",
            "example.metamodel",
            PackSectionFlags::REQUIRED,
            &json!({}),
            digest("metamodel-key"),
            digest("metamodel-out"),
        )
        .build();
    assert_eq!(missing, Err(PackError::MissingSection("program".into())));
    // Dropping a part's section from a built pack is refused on read too.
    let bytes = reframe(&built(), |manifest, payload| {
        let sections = manifest["sections"].as_array_mut().expect("sections");
        let program = sections.pop().expect("program");
        let length = program["digest"]["bytes"].as_u64().expect("bytes") as usize;
        payload.truncate(payload.len() - length);
    });
    assert_eq!(refused(bytes), PackError::MissingSection("program".into()));
    // A part's section must be required, not a cache.
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][2]["flags"] = json!(["derived_cache"]);
    });
    assert!(matches!(
        refused(bytes),
        PackError::PartSectionMismatch { field: "flags", .. }
    ));
}

#[test]
fn pack_derived_cache_section_may_be_absent() {
    let without_cache = PackBuilder::new(header())
        .json_section(
            "metamodel",
            "example.metamodel",
            PackSectionFlags::REQUIRED,
            &json!({"classes": ["Root"]}),
            digest("metamodel-key"),
            digest("metamodel-out"),
        )
        .json_section(
            "program",
            "example.program",
            PackSectionFlags::REQUIRED,
            &json!({}),
            digest("program-key"),
            digest("program-out"),
        )
        .build()
        .expect("builds");
    let pack = LanguagePack::read(without_cache).expect("reads");
    assert!(pack.section("cache/tables").is_none());
}

#[test]
fn pack_refuses_part_section_digest_mismatch() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["parts"][0]["output_digest"] = json!(digest("other"));
    });
    assert!(matches!(
        refused(bytes),
        PackError::PartSectionMismatch {
            field: "output_digest",
            ..
        }
    ));
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][1]["part_digest"] = json!(digest("other"));
    });
    assert!(matches!(
        refused(bytes),
        PackError::PartSectionMismatch {
            field: "part_digest",
            ..
        }
    ));
}

#[test]
fn pack_refuses_part_depending_on_unknown_part() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["parts"][1]["depends"] = json!(["ghost"]);
    });
    assert_eq!(
        refused(bytes),
        PackError::UnknownPart {
            part: "program".into(),
            depends: "ghost".into()
        }
    );
    let bytes = reframe(&built(), |manifest, _| {
        manifest["parts"][1]["components"] = json!(["ghost"]);
    });
    assert_eq!(
        refused(bytes),
        PackError::UnknownComponent {
            part: "program".into(),
            component: "ghost".into()
        }
    );
    let bytes = reframe(&built(), |manifest, _| {
        manifest["parts"][1]["inputs"] = json!(["ghost"]);
    });
    assert_eq!(
        refused(bytes),
        PackError::UnknownInput {
            part: "program".into(),
            role: "ghost".into()
        }
    );
    let bytes = reframe(&built(), |manifest, _| {
        manifest["parts"][1]["depends"] = json!(["metamodel", "metamodel"]);
    });
    assert_invalid(refused(bytes), "parts.depends");
}

#[test]
fn pack_refuses_dependency_cycle() {
    let bytes = reframe(&built(), |manifest, _| {
        manifest["parts"][0]["depends"] = json!(["program"]);
    });
    assert_eq!(
        refused(bytes),
        PackError::DependencyCycle(vec!["metamodel".into(), "program".into()])
    );
    let bytes = reframe(&built(), |manifest, _| {
        manifest["parts"][0]["depends"] = json!(["metamodel"]);
    });
    assert_eq!(
        refused(bytes),
        PackError::DependencyCycle(vec!["metamodel".into(), "program".into()])
    );
}

#[test]
fn pack_refuses_unsorted_parts_and_malformed_digests() {
    let bytes = reframe(&built(), |manifest, _| {
        let parts = manifest["parts"].as_array_mut().expect("parts");
        parts.swap(0, 1);
    });
    assert_eq!(
        refused(bytes),
        PackError::PartOrder {
            previous: "program".into(),
            name: "metamodel".into()
        }
    );
    let bytes = reframe(&built(), |manifest, _| {
        manifest["sections"][0]["output_digest"] = json!("ABC");
    });
    assert_invalid(refused(bytes), "sections.output_digest");
}

fn assert_invalid(error: PackError, expected: &str) {
    assert!(
        matches!(&error, PackError::InvalidField { field, .. } if field == expected),
        "expected InvalidField({expected}), got {error:?}"
    );
}

fn refused_with(edit: impl FnOnce(&mut Value)) -> PackError {
    refused(reframe(&built(), |manifest, _| edit(manifest)))
}

fn dependency_json(name: &str, version: &str, output_digest: &str) -> Value {
    json!({
        "language": "base",
        "recipe": {"name": name, "version": version},
        "output_digest": output_digest,
    })
}

#[test]
fn pack_refuses_unsorted_duplicate_and_malformed_hooks() {
    assert_invalid(
        refused_with(|m| {
            let hooks = m["requires_hooks"].as_array_mut().expect("hooks");
            hooks.swap(0, 1);
        }),
        "requires_hooks",
    );
    assert_invalid(
        refused_with(|m| {
            let first = m["requires_hooks"][0].clone();
            m["requires_hooks"] = json!([first.clone(), first]);
        }),
        "requires_hooks",
    );
    assert_invalid(
        refused_with(|m| m["requires_hooks"][0]["provider"]["name"] = json!("")),
        "requires_hooks.provider.name",
    );
    assert_invalid(
        refused_with(|m| m["requires_hooks"][1]["provider"]["version"] = json!("1\u{7}")),
        "requires_hooks.provider.version",
    );
    // A bare provider string is not a provider identity.
    assert!(matches!(
        refused_with(|m| m["requires_hooks"][0]["provider"] = json!("example.hook@1")),
        PackError::Manifest(_)
    ));
    let mut bad = header();
    bad.requires_hooks.push(hook("x", "", "host"));
    assert_invalid(
        with_header(bad).expect_err("refused"),
        "requires_hooks.provider.version",
    );
}

#[test]
fn pack_refuses_unsorted_duplicate_and_malformed_dependencies() {
    let a = dependency_json("a", "1", &digest("a"));
    let b = dependency_json("b", "1", &digest("b"));
    let accepted = reframe(&built(), |m, _| {
        m["depends_on"] = json!([a.clone(), b.clone()]);
    });
    let pack = LanguagePack::read(accepted).expect("sorted dependencies read");
    assert_eq!(pack.manifest().depends_on.len(), 2);
    assert_invalid(
        refused_with(|m| m["depends_on"] = json!([b.clone(), a.clone()])),
        "depends_on",
    );
    assert_invalid(
        refused_with(|m| m["depends_on"] = json!([a.clone(), a.clone()])),
        "depends_on",
    );
    assert_invalid(
        refused_with(|m| m["depends_on"] = json!([dependency_json("a", "1", "ABC")])),
        "depends_on.output_digest",
    );
    assert_invalid(
        refused_with(|m| m["depends_on"] = json!([dependency_json("", "1", &digest("a"))])),
        "depends_on.recipe.name",
    );
    assert_invalid(
        refused_with(|m| m["depends_on"] = json!([dependency_json("a", "\u{0}", &digest("a"))])),
        "depends_on.recipe.version",
    );
    // The builder shares the check, so it cannot write such a pack either.
    let mut bad = header();
    let mut empty_name = dependency("base", "a");
    empty_name.recipe.name = String::new();
    bad.depends_on = vec![empty_name];
    assert_invalid(
        with_header(bad).expect_err("refused"),
        "depends_on.recipe.name",
    );
}

#[test]
fn pack_refuses_malformed_inputs_and_names() {
    assert_invalid(
        refused_with(|m| m["inputs"]["grammar"]["sha256"] = json!("ABC")),
        "inputs.sha256",
    );
    assert_invalid(
        refused_with(|m| m["inputs"]["grammar"]["uri"] = json!("")),
        "inputs.uri",
    );
    assert_invalid(
        refused_with(|m| m["inputs"]["metamodel"]["uri"] = json!("urn:\u{1b}x")),
        "inputs.uri",
    );
    assert_invalid(refused_with(|m| m["language"] = json!("")), "language");
    assert_invalid(
        refused_with(|m| m["sections"][0]["name"] = json!("a\u{1}")),
        "sections.name",
    );
    let mut bad = header();
    if let Some(input) = bad.inputs.get_mut("grammar") {
        input.uri = String::new();
    }
    assert_invalid(with_header(bad).expect_err("refused"), "inputs.uri");
}

#[test]
fn pack_refuses_too_many_parts_and_sections() {
    let mut many = header();
    many.parts = (0..=MAX_PACK_PARTS)
        .map(|index| PackPart {
            name: format!("p{index:05}"),
            components: Vec::new(),
            inputs: Vec::new(),
            depends: Vec::new(),
            input_digest: digest("k"),
            output_digest: digest("o"),
        })
        .collect();
    assert_eq!(
        with_header(many),
        Err(PackError::TooMany {
            what: "parts",
            count: MAX_PACK_PARTS + 1,
            limit: MAX_PACK_PARTS
        })
    );
    let mut sections = PackBuilder::new(header());
    for index in 0..=MAX_PACK_SECTIONS {
        sections = sections.section(
            format!("s{index:05}"),
            "example.cache",
            PackSectionFlags::DERIVED_CACHE,
            b"{}".to_vec(),
            digest("k"),
            digest("o"),
        );
    }
    assert_eq!(
        sections.build(),
        Err(PackError::TooMany {
            what: "sections",
            count: MAX_PACK_SECTIONS + 1,
            limit: MAX_PACK_SECTIONS
        })
    );
}

#[test]
fn pack_section_json_reports_type_mismatch() {
    let pack = LanguagePack::read(built()).expect("reads");
    let wrong: Result<Vec<u32>, _> = pack.section_json("program");
    assert!(matches!(wrong, Err(PackError::SectionJson { .. })));
}

fn key_with(edit: impl FnOnce(&mut KeyFixture)) -> String {
    let mut fixture = KeyFixture::default();
    edit(&mut fixture);
    part_key(PartKeyInput {
        part: &fixture.part,
        part_version: fixture.part_version,
        recipe: &fixture.recipe,
        components: &fixture.components,
        recipe_constants: &fixture.constants,
        inputs: &fixture.inputs,
        dependencies: &fixture.dependencies,
    })
    .expect("part key")
}

struct KeyFixture {
    part: String,
    part_version: u32,
    recipe: PackRecipe,
    components: BTreeMap<String, String>,
    constants: BTreeMap<String, Value>,
    inputs: BTreeMap<String, SourceId>,
    dependencies: BTreeMap<String, String>,
}

impl Default for KeyFixture {
    fn default() -> Self {
        let header = header();
        let mut dependencies = BTreeMap::new();
        dependencies.insert("metamodel".to_owned(), digest("metamodel-out"));
        Self {
            part: "program".into(),
            part_version: 1,
            recipe: header.recipe,
            components: header.compiler,
            constants: header.recipe_constants,
            inputs: header.inputs,
            dependencies,
        }
    }
}

#[test]
fn part_key_is_stable_and_sensitive_to_every_input() {
    let base = key_with(|_| {});
    assert_eq!(base, key_with(|_| {}));
    assert_eq!(base.len(), 64);
    let variants = [
        key_with(|f| f.part = "programs".into()),
        key_with(|f| f.part_version = 2),
        key_with(|f| f.recipe.name = "other".into()),
        key_with(|f| f.recipe.version = "2".into()),
        key_with(|f| {
            f.components.insert("model".into(), digest("model2"));
        }),
        key_with(|f| {
            f.components.remove("syntax");
        }),
        key_with(|f| {
            f.constants.insert("call_depth".into(), json!(513));
        }),
        key_with(|f| {
            f.constants
                .insert("profile".into(), json!({"z": 1, "a": [true]}));
        }),
        key_with(|f| {
            f.inputs
                .insert("grammar".into(), source("urn:grammar", "grammar2"));
        }),
        key_with(|f| {
            f.inputs
                .insert("grammar".into(), source("urn:grammar2", "grammar"));
        }),
        key_with(|f| {
            f.dependencies.insert("metamodel".into(), digest("other"));
        }),
        key_with(|f| f.dependencies.clear()),
    ];
    for (index, variant) in variants.iter().enumerate() {
        assert_ne!(&base, variant, "variant {index} must change the key");
    }
    let mut unique = variants.to_vec();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), variants.len());
}

#[test]
fn part_key_is_independent_of_constant_object_key_order() {
    let ordered = key_with(|f| {
        f.constants.insert("nested".into(), json!({"a": 1, "b": 2}));
    });
    let mut reversed = serde_json::Map::new();
    reversed.insert("b".into(), json!(2));
    reversed.insert("a".into(), json!(1));
    let shuffled = key_with(|f| {
        f.constants.insert("nested".into(), Value::Object(reversed));
    });
    assert_eq!(ordered, shuffled);
}

#[test]
fn dump_json_shows_manifest_and_parsed_sections() {
    let pack = LanguagePack::read(built()).expect("reads");
    let dump = dump_json(&pack).expect("dump");
    assert_eq!(dump["manifest"]["language"], json!("example.language"));
    assert_eq!(dump["manifest"]["format"], json!(PACK_FORMAT));
    assert_eq!(
        dump["manifest"]["sections"][0]["flags"],
        json!(["derived_cache"])
    );
    assert_eq!(dump["manifest"]["sections"][0]["encoding"], json!("json"));
    assert_eq!(dump["sections"]["cache/tables"], json!({"rows": [1, 2, 3]}));
    assert_eq!(dump["sections"]["metamodel"], json!({"classes": ["Root"]}));
    assert_eq!(dump["sections"]["program"]["depth"], json!(2));
    let pretty = dump_json_pretty(&pack).expect("pretty");
    assert!(pretty.contains("\n  \"manifest\": {"));
    let reparsed: Value = serde_json::from_str(&pretty).expect("reparses");
    assert_eq!(reparsed, dump);
}

fn with_cache_payload(payload: &[u8]) -> Result<Vec<u8>, PackError> {
    PackBuilder::new(header())
        .section(
            "cache/tables",
            "example.tables",
            PackSectionFlags::DERIVED_CACHE,
            payload.to_vec(),
            digest("tables-key"),
            digest("tables-out"),
        )
        .json_section(
            "metamodel",
            "example.metamodel",
            PackSectionFlags::REQUIRED,
            &json!({}),
            digest("metamodel-key"),
            digest("metamodel-out"),
        )
        .json_section(
            "program",
            "example.program",
            PackSectionFlags::REQUIRED,
            &json!({}),
            digest("program-key"),
            digest("program-out"),
        )
        .build()
}

#[test]
fn pack_builder_refuses_raw_section_that_is_not_json() {
    for payload in [&b"not json"[..], b"", b"{} {}", b"{\"a\":", b"\"\xff\""] {
        assert!(
            matches!(
                with_cache_payload(payload),
                Err(PackError::SectionJson { ref section, .. }) if section == "cache/tables"
            ),
            "payload {payload:?} must be refused"
        );
    }
    // Any single JSON value, with surrounding whitespace, is stored verbatim.
    let bytes = with_cache_payload(b" [1, 2] \n").expect("builds");
    let pack = LanguagePack::read(bytes).expect("reads");
    assert_eq!(pack.section("cache/tables"), Some(&b" [1, 2] \n"[..]));
    assert!(dump_json(&pack).is_ok());
}

/// Replaces the cache payload and rewrites its digest, so only the declared
/// encoding is wrong.
#[test]
fn pack_reader_refuses_json_section_that_is_not_json() {
    for replacement in [&b"not json!"[..], b"\"\xff\xfe\""] {
        let bytes = reframe(&built(), |manifest, payload| {
            let sections = manifest["sections"].as_array_mut().expect("sections");
            assert_eq!(sections[0]["name"], json!("cache/tables"));
            let length = sections[0]["digest"]["bytes"].as_u64().expect("bytes") as usize;
            payload.splice(..length, replacement.iter().copied());
            sections[0]["digest"] = json!({
                "sha256": sha256_hex(replacement),
                "bytes": replacement.len(),
            });
            for later in sections.iter_mut().skip(1) {
                let offset = later["offset"].as_u64().expect("offset") as usize;
                later["offset"] = json!(offset + replacement.len() - length);
            }
        });
        assert!(
            matches!(
                refused(bytes),
                PackError::SectionJson { ref section, .. } if section == "cache/tables"
            ),
            "replacement {replacement:?} must be refused"
        );
    }
}

/// A language computes a section's output digest before `build`, so the
/// public canonical encoding must be byte-identical to what `json_section`
/// stores and its digest equal to the recorded section digest.
#[test]
fn canonical_digest_equals_the_built_json_section_digest() {
    let value =
        json!({"rules": ["a", "b"], "depth": 2, "nested": {"z": 1, "a": [{"y": 0, "b": 1}]}});
    let encoded = canonical_json(&value).expect("canonical");
    assert_eq!(
        encoded,
        br#"{"depth":2,"nested":{"a":[{"b":1,"y":0}],"z":1},"rules":["a","b"]}"#
    );
    let computed = canonical_digest(&value).expect("digest");
    assert_eq!(computed, FileDigest::of(&encoded));
    let bytes = PackBuilder::new(PackHeader {
        parts: vec![],
        ..header()
    })
    .json_section(
        "program",
        "example.program",
        PackSectionFlags::REQUIRED,
        &value,
        digest("program-key"),
        computed.sha256.clone(),
    )
    .build()
    .expect("builds");
    let pack = LanguagePack::read(bytes).expect("reads");
    let entry = pack.section_entry("program").expect("entry");
    assert_eq!(entry.digest, computed);
    assert_eq!(pack.section("program"), Some(&encoded[..]));
}

/// A binary section is stored verbatim, digest-checked, never parsed as
/// JSON, and dumped by size and digest.
#[test]
fn binary_sections_are_opaque_and_digest_checked() {
    let blob = vec![0u8, 255, 1, b'{', 7];
    let bytes = builder()
        .binary_section(
            "cache/blob",
            "example.blob",
            PackSectionFlags::DERIVED_CACHE,
            blob.clone(),
            digest("blob-key"),
            digest("blob-out"),
        )
        .build()
        .expect("builds");
    let pack = LanguagePack::read(bytes.clone()).expect("reads");
    let entry = pack.section_entry("cache/blob").expect("entry");
    assert_eq!(entry.encoding, PackSectionEncoding::Binary);
    assert_eq!(pack.section("cache/blob"), Some(&blob[..]));
    let dump = dump_json(&pack).expect("dump");
    assert_eq!(
        dump["sections"]["cache/blob"]["binary"]["bytes"],
        json!(blob.len())
    );
    // A flipped byte in the binary payload fails its digest.
    let at = pack.payload_start + entry.offset;
    let mut flipped = bytes;
    flipped[at] ^= 1;
    assert_eq!(
        refused(flipped),
        PackError::SectionDigest {
            section: "cache/blob".into()
        }
    );
}

/// A host that selects sections pays no digest for the others, cannot read
/// them, and is still refused a tampered section it did select.
/// A host that hashes the sections itself reads the same pack; a digest that
/// is not the manifest's is refused, whoever computed it.
#[test]
fn host_digests_stand_in_for_hashing_and_are_still_compared() {
    use sha2::{Digest, Sha256};
    let bytes = built();
    let whole = LanguagePack::read(bytes.clone()).expect("reads");
    let ranges = LanguagePack::section_ranges(&bytes).expect("ranges");
    assert_eq!(
        ranges.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        whole
            .manifest()
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
    );
    for range in &ranges {
        assert_eq!(
            Some(&bytes[range.start..range.start + range.bytes]),
            whole.section(&range.name)
        );
    }
    let digest = |bytes: &[u8], name: &str| {
        let range = ranges.iter().find(|r| r.name == name).expect("range");
        format!(
            "{:x}",
            Sha256::digest(&bytes[range.start..range.start + range.bytes])
        )
    };
    let hosted = LanguagePack::read_checked(bytes.clone(), |section| {
        SectionCheck::HostDigest(digest(&bytes, &section.name))
    })
    .expect("host digests read");
    assert_eq!(hosted.manifest(), whole.manifest());
    for section in &whole.manifest().sections {
        assert!(hosted.is_verified(&section.name));
        assert_eq!(hosted.section(&section.name), whole.section(&section.name));
    }
    // A wrong host digest is a digest mismatch, as is the true digest of
    // changed bytes.
    let wrong = LanguagePack::read_checked(bytes.clone(), |section| {
        if section.name == "program" {
            SectionCheck::HostDigest("0".repeat(64))
        } else {
            SectionCheck::Hash
        }
    });
    assert_eq!(
        wrong.err(),
        Some(PackError::SectionDigest {
            section: "program".into()
        })
    );
    let mut flipped = bytes.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 1;
    let honest = LanguagePack::read_checked(flipped.clone(), |section| {
        SectionCheck::HostDigest(digest(&flipped, &section.name))
    });
    assert_eq!(
        honest.err(),
        Some(PackError::SectionDigest {
            section: "program".into()
        })
    );
    // The declared encoding is still checked under a host digest, and the
    // ranges refuse what `read` refuses before the sections.
    assert!(LanguagePack::section_ranges(&bytes[..10]).is_err());
}

#[test]
fn selected_sections_are_verified_and_the_rest_stay_unreadable() {
    let whole = LanguagePack::read(built()).expect("reads");
    let pack = LanguagePack::read_sections(built(), |section| section.name == "metamodel")
        .expect("reads selected");
    assert_eq!(pack.manifest(), whole.manifest());
    assert_eq!(pack.section("metamodel"), whole.section("metamodel"));
    assert!(pack.is_verified("metamodel"));
    for name in ["program", "cache/tables"] {
        assert!(whole.is_verified(name));
        assert!(!pack.is_verified(name));
        assert!(pack.section_entry(name).is_some());
        assert!(pack.section(name).is_none());
        assert_eq!(
            pack.required_section(name),
            Err(PackError::UnverifiedSection(name.into()))
        );
    }
    assert_eq!(
        pack.section_json::<Value>("program"),
        Err(PackError::UnverifiedSection("program".into()))
    );
    assert!(!pack.is_verified("absent"));
    assert_eq!(
        pack.required_section("absent"),
        Err(PackError::MissingSection("absent".into()))
    );
    let dump = dump_json(&pack).expect("dumps");
    assert!(dump["sections"]["program"]["unverified"]["sha256"].is_string());
    assert_eq!(dump["sections"]["metamodel"], json!({"classes": ["Root"]}));

    // The flipped byte is in `program`, the last section.
    let mut bytes = built();
    let last = bytes.len() - 2;
    bytes[last] ^= 0x01;
    let skipped = LanguagePack::read_sections(bytes.clone(), |section| section.name != "program")
        .expect("unselected tamper is not hashed");
    assert!(skipped.section("program").is_none());
    assert_eq!(
        LanguagePack::read_sections(bytes, |section| section.name == "program").err(),
        Some(PackError::SectionDigest {
            section: "program".into()
        })
    );
}
