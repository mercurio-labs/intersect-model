//! Handwritten unit tests of the model snapshot codec (decision D10).
use super::*;
use crate::core::frozen::{ById, ModelBuilder};
use serde_json::json;

const SOURCE: &str = r#"rcore 1;
package snapshot version "1";
metamodel "urn:intersect:snapshot" prefix s {
  abstract class Named { attr name: String [1,1] default ""; }
  class Node extends Named {
    attr tags: String [0,*];
    ref links: Node [0,*];
    ref children: Named [0,*] containment;
    ref out: Node [0,*] opposite Node.incoming;
    ref incoming: Node [0,*] opposite Node.out;
    ref buddy: Node [0,1];
  }
  class Leaf extends Named { attr note: String [0,1]; }
}"#;
const OTHER: &str = r#"rcore 1;
package snapshot version "2";
metamodel "urn:intersect:snapshot" prefix s {
  class Node { attr name: String [1,1] default ""; }
}"#;
const KEY: [u8; 32] = [7; 32];

fn metamodel(source: &str) -> Result<Arc<Metamodel>, String> {
    let package = crate::compile_rcore_at("memory:/snapshot.rcore", source)
        .map_err(|error| error.to_string())?;
    Metamodel::freeze(package.schema())
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

fn class(mm: &Metamodel, name: &str) -> Result<ClassId, String> {
    let schema = mm.schema();
    schema
        .declarations
        .iter()
        .find(|(key, d)| d.name == name && schema.classes.contains_key(*key))
        .and_then(|(key, _)| mm.class(key))
        .ok_or_else(|| format!("class {name}"))
}

/// A validated model, plus every arena state the codec must keep: all slot
/// kinds (unassigned, absent, null, booleans, integers, strings, references,
/// lists, retained JSON, proxies), carried assignment state, implicit
/// markers and resource records.
fn model() -> Result<Model, String> {
    let mm = metamodel(SOURCE)?;
    let node = class(&mm, "Node")?;
    let leaf = class(&mm, "Leaf")?;
    let feature = |class, name: &str| mm.feature(class, name).ok_or(format!("feature {name}"));
    let (name, tags, links, children, out, buddy) = (
        feature(node, "name")?,
        feature(node, "tags")?,
        feature(node, "links")?,
        feature(node, "children")?,
        feature(node, "out")?,
        feature(node, "buddy")?,
    );
    let e = |error: FrozenError| error.to_string();
    let mut b = ModelBuilder::new(mm.clone());
    let objs = (0..6)
        .map(|i| {
            b.create_at(
                &format!("n{i}"),
                node,
                ["mem:/a", "mem:/b"][i % 2],
                i,
                i + 1,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(e)?;
    let leaf_obj = b.create("leaf", leaf, "mem:/a").map_err(e)?;
    for (i, obj) in objs.iter().enumerate() {
        b.set_str(*obj, name, &format!("node {i}")).map_err(e)?;
    }
    b.set_attribute(objs[1], tags, json!(["x", "y"]))
        .map_err(e)?;
    for i in 0..5 {
        b.add_proxy(objs[i], links, &format!("n{}", i + 1))
            .map_err(e)?;
        b.add_ref(objs[i], out, objs[i + 1]).map_err(e)?;
    }
    b.add_ref(objs[0], children, objs[2]).map_err(e)?;
    b.add_ref(objs[0], children, leaf_obj).map_err(e)?;
    b.add_ref(objs[3], buddy, objs[4]).map_err(e)?;
    b.set_str(leaf_obj, feature(leaf, "name")?, "leaf")
        .map_err(e)?;
    b.clear(objs[5], buddy).map_err(e)?;
    let mut model = b
        .link(&ById)
        .map_err(|d| d.to_string())?
        .validate()
        .map_err(|d| d.to_string())?;
    // States this fixture's builder calls do not produce; the codec must
    // keep them all the same.
    let data = &mut model.data;
    let sym = data.strings.intern("spelling").map_err(e)?;
    let json_index = u32::try_from(data.json.len()).map_err(|e| e.to_string())?;
    data.json
        .push(json!({"b": [1.5, {"c": null}], "a": -0.0, "big": 1e300}));
    data.lists.push(vec![
        Slot::Int(-42),
        Slot::Int(i64::MAX),
        Slot::Bool(true),
        Slot::Bool(false),
        Slot::Json(json_index),
        Slot::Null,
        Slot::Absent,
        Slot::Default,
        Slot::Ref(ObjId(1)),
        Slot::Str(sym),
        Slot::Proxy(0),
    ]);
    data.proxies.push(Proxy {
        owner: ObjId(1),
        feature: links,
        spelling: sym,
    });
    for (obj, features) in [(4, vec!["name"]), (2, vec!["buddy", "name"])] {
        data.carried.insert(
            ObjId(obj),
            Box::new(Carried {
                initialization: None,
                assignment_state: Some(AssignmentState::new(features)?),
            }),
        );
    }
    data.implicit.insert(ObjId(5));
    data.implicit.insert(ObjId(3));
    let mut record = ResourceRecord::new("mem:/a")?;
    record.content_revision = 9;
    record.roots = Arc::new(vec!["n0".into()]);
    if let Some(slot) = data.resource_records.get_mut(0) {
        *slot = Some(record);
    }
    Ok(model)
}

fn decode(model: &Model, bytes: &[u8]) -> Result<Model, SnapshotError> {
    Model::from_snapshot(
        model.metamodel.clone(),
        bytes,
        &KEY,
        SnapshotTrust::HostCache,
    )
}

fn section_length(bytes: &[u8], index: usize) -> usize {
    let entry = SNAPSHOT_HEADER_BYTES + index * TABLE_ENTRY_BYTES;
    let mut length = [0u8; 8];
    length.copy_from_slice(&bytes[entry + 4..entry + 12]);
    u64::from_le_bytes(length) as usize
}

/// Recompute every section digest, so a test reaches the payload checks
/// behind them.
fn reseal(bytes: &mut [u8]) {
    let mut start = SNAPSHOT_HEADER_BYTES + SECTIONS.len() * TABLE_ENTRY_BYTES;
    for index in 0..SECTIONS.len() {
        let entry = SNAPSHOT_HEADER_BYTES + index * TABLE_ENTRY_BYTES;
        let end = (start + section_length(bytes, index)).min(bytes.len());
        let digest = Sha256::digest(&bytes[start.min(end)..end]);
        bytes[entry + 12..entry + 44].copy_from_slice(&digest);
        start = end;
    }
}

#[test]
fn snapshot_round_trip_is_lossless_and_deterministic() -> Result<(), String> {
    let model = model()?;
    let bytes = model.to_snapshot(&KEY).map_err(|e| e.to_string())?;
    // Determinism: the same model built again encodes to the same bytes.
    assert_eq!(
        bytes,
        self::model()?
            .to_snapshot(&KEY)
            .map_err(|e| e.to_string())?
    );
    let loaded = decode(&model, &bytes).map_err(|e| e.to_string())?;
    // Losslessness: re-encoding the loaded model gives the same bytes, the
    // arena tables are equal and so is the export.
    assert_eq!(loaded.to_snapshot(&KEY).map_err(|e| e.to_string())?, bytes);
    assert_eq!(
        loaded.to_model().map_err(|e| e.to_string())?,
        model.to_model().map_err(|e| e.to_string())?
    );
    assert_eq!(loaded.data.slots, model.data.slots);
    assert_eq!(loaded.data.lists, model.data.lists);
    assert_eq!(loaded.data.json, model.data.json);
    assert_eq!(loaded.data.implicit, model.data.implicit);
    assert_eq!(loaded.data.resource_records, model.data.resource_records);
    assert_eq!(
        loaded.data.strings.iter().collect::<Vec<_>>(),
        model.data.strings.iter().collect::<Vec<_>>()
    );
    for obj in model.objects() {
        let other = loaded.obj(obj.id()).ok_or("object")?;
        assert_eq!(other.identity(), obj.identity());
        assert_eq!(other.span(), obj.span());
        assert_eq!(
            loaded.lookup(obj.identity()).map(|o| o.id()),
            Some(obj.id())
        );
        assert_eq!(
            other.container().map(|(o, f)| (o.id(), f)),
            obj.container().map(|(o, f)| (o.id(), f))
        );
    }
    assert_eq!(
        loaded
            .data
            .carried
            .get(&ObjId(2))
            .and_then(|c| c.assignment_state.clone()),
        Some(AssignmentState::new(["buddy", "name"])?)
    );
    let header = snapshot_header(&bytes).map_err(|e| e.to_string())?;
    assert_eq!(header.key, KEY);
    assert_eq!(header.format, SNAPSHOT_FORMAT);
    Ok(())
}

#[test]
fn revalidation_accepts_a_validated_model() -> Result<(), String> {
    let mut model = model()?;
    // The codec-only list and proxy record are not a valid model state.
    model.data.proxies.clear();
    model.data.lists.pop();
    let bytes = model.to_snapshot(&KEY).map_err(|e| e.to_string())?;
    Model::from_snapshot(
        model.metamodel.clone(),
        &bytes,
        &KEY,
        SnapshotTrust::Revalidate,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn snapshot_refuses_another_key_or_metamodel() -> Result<(), String> {
    let model = model()?;
    let bytes = model.to_snapshot(&KEY).map_err(|e| e.to_string())?;
    let wrong_key = Model::from_snapshot(
        model.metamodel.clone(),
        &bytes,
        &[8; 32],
        SnapshotTrust::HostCache,
    );
    assert_eq!(wrong_key.err(), Some(SnapshotError::Key));
    let other = metamodel(OTHER)?;
    let wrong_metamodel = Model::from_snapshot(other, &bytes, &KEY, SnapshotTrust::HostCache);
    assert_eq!(wrong_metamodel.err(), Some(SnapshotError::Metamodel));
    assert!(
        SnapshotError::Metamodel
            .to_string()
            .contains("another metamodel")
    );
    // Same schema frozen twice: same digest.
    assert_eq!(
        metamodel(SOURCE)?.digest().map_err(|e| e.to_string())?,
        model.metamodel.digest().map_err(|e| e.to_string())?
    );
    Ok(())
}

/// Every truncation and every single flipped byte is refused with an error;
/// none panics.
#[test]
fn snapshot_fails_closed_on_corruption() -> Result<(), String> {
    let model = model()?;
    let bytes = model.to_snapshot(&KEY).map_err(|e| e.to_string())?;
    for cut in 0..bytes.len() {
        assert!(decode(&model, &bytes[..cut]).is_err(), "truncated at {cut}");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(matches!(
        decode(&model, &longer),
        Err(SnapshotError::TrailingBytes { .. })
    ));
    for at in 0..bytes.len() {
        let mut flipped = bytes.clone();
        flipped[at] ^= 0x5a;
        let message = match decode(&model, &flipped) {
            Ok(_) => return Err(format!("flipped byte {at} was accepted")),
            Err(error) => error.to_string(),
        };
        assert!(message.contains("model snapshot"), "{message}");
    }
    Ok(())
}

/// Behind the digests, the payload checks never panic: every single-byte
/// change with recomputed digests is decoded or refused.
#[test]
fn payload_checks_never_panic() -> Result<(), String> {
    let model = model()?;
    let bytes = model.to_snapshot(&KEY).map_err(|e| e.to_string())?;
    let payload = SNAPSHOT_HEADER_BYTES + SECTIONS.len() * TABLE_ENTRY_BYTES;
    let mut refused = 0usize;
    for at in payload..bytes.len() {
        for mask in [0x01u8, 0x80, 0xff] {
            let mut changed = bytes.clone();
            changed[at] ^= mask;
            reseal(&mut changed);
            if decode(&model, &changed).is_err() {
                refused += 1;
            }
        }
    }
    assert!(refused > 0);
    // A bounds error names its section: the first object's class index.
    let objects = payload + section_length(&bytes, 0) + section_length(&bytes, 1);
    let mut changed = bytes.clone();
    changed[objects + 4..objects + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    reseal(&mut changed);
    let error = decode(&model, &changed).err().map(|e| e.to_string());
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("section objects") && e.contains("unknown class")),
        "{error:?}"
    );
    Ok(())
}

/// `VerifiedHostCache` decodes what `HostCache` decodes and keeps every check
/// but the section digests, which its caller already covered.
#[test]
fn verified_host_cache_skips_only_the_section_digests() -> Result<(), String> {
    let model = model()?;
    let bytes = model.to_snapshot(&KEY).map_err(|e| e.to_string())?;
    let load = |bytes: &[u8], key: &[u8; 32], trust| {
        Model::from_snapshot(model.metamodel.clone(), bytes, key, trust)
    };
    let verified =
        load(&bytes, &KEY, SnapshotTrust::VerifiedHostCache).map_err(|e| e.to_string())?;
    assert_eq!(
        verified.to_snapshot(&KEY).map_err(|e| e.to_string())?,
        bytes
    );
    assert!(matches!(
        load(&bytes, &[8; 32], SnapshotTrust::VerifiedHostCache),
        Err(SnapshotError::Key)
    ));
    assert!(matches!(
        load(
            &bytes[..bytes.len() - 1],
            &KEY,
            SnapshotTrust::VerifiedHostCache
        ),
        Err(SnapshotError::Truncated { .. })
    ));
    // A digest field changed in the table: refused only where digests are checked.
    let mut forged = bytes.clone();
    let digest_byte = SNAPSHOT_HEADER_BYTES + 4 + 8;
    forged[digest_byte] ^= 0x01;
    assert!(matches!(
        load(&forged, &KEY, SnapshotTrust::HostCache),
        Err(SnapshotError::SectionDigest(_))
    ));
    let accepted =
        load(&forged, &KEY, SnapshotTrust::VerifiedHostCache).map_err(|e| e.to_string())?;
    assert_eq!(
        accepted.to_snapshot(&KEY).map_err(|e| e.to_string())?,
        bytes
    );
    Ok(())
}
