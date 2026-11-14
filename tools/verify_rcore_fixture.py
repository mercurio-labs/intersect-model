"""Verify the demonstration projection against pinned, unmodified upstream bytes."""
from pathlib import Path
import hashlib
import json
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parents[1] / "fixtures/rcore/day01"
manifest = json.loads((root / "kerml-provenance.json").read_text(encoding="utf-8"))
raw = (root / "upstream/kerml.ecore").read_bytes()
assert hashlib.sha256(raw).hexdigest() == manifest["source_sha256"], "upstream digest"
original = ET.fromstring(raw)
projection = ET.parse(root / "kerml-specialization.ecore").getroot()
assert original.get("nsURI") == manifest["original_namespace"]
assert projection.get("nsURI") == manifest["projection_namespace"]
assert manifest["original_namespace"] != manifest["projection_namespace"], "identity separation"
classes = original.findall("eClassifiers")
projected = projection.findall("eClassifiers")
assert len(projected) == len(manifest["declarations"]) == 2
for mapped, row in zip(projected, manifest["declarations"]):
    source = classes[row["original_classifier_path"][0]]
    assert source.get("name") == row["original_name"] == mapped.get("name")
    assert source.get("eSuperTypes") == row["original_super_types"]
    assert mapped.get("eSuperTypes") is None, "inheritance is explicitly excluded"
    fields = source.findall("eStructuralFeatures")
    actual = mapped.findall("eStructuralFeatures")
    assert len(actual) == len(row["features"])
    for field, origin in zip(actual, row["features"]):
        upstream = fields[origin["original_feature_index"]]
        assert upstream.get("name") == field.get("name") == origin["projected_feature"]
        assert upstream.attrib == origin["original_attributes"] == field.attrib, "changed relationship contract"
print("Verified pinned KerML bytes, two classifier mappings and exact general/specific contracts.")

# Authored demonstration sources use the repository LF policy. Imported upstream
# inputs are deliberately excluded: their pinned original bytes stay authoritative.
authored = root.parent
source_suffixes = {".ecore", "-behavior.rcore", ".rcore", ".xtext", ".demo"}
checked_sources = 0
for directory in ("day01", "day02", "day03", "day04", "web"):
    for path in sorted((authored / directory).iterdir()):
        if path.is_file() and path.suffix in source_suffixes:
            assert b"\r\n" not in path.read_bytes(), f"Authored fixture differs from Git LF policy: {path}"
            checked_sources += 1

# Check real-file SourceIds independently of the compiler/harness. Generated
# Ecore and in-memory scoping have no source file; live native/WASM controls
# verify those generation contracts separately.
checked_ids = set()
def verify_source_ids(value):
    if isinstance(value, dict):
        if isinstance(value.get("uri"), str) and isinstance(value.get("sha256"), str):
            uri = value["uri"]
            if uri.startswith("fixture:rcore/"):
                path = authored / uri.removeprefix("fixture:rcore/")
                if path.is_file():
                    assert hashlib.sha256(path.read_bytes()).hexdigest() == value["sha256"], f"Stale report source identity: {uri}"
                    checked_ids.add(uri)
        for child in value.values():
            verify_source_ids(child)
    elif isinstance(value, list):
        for child in value:
            verify_source_ids(child)
for day in ("day04", "day05", "day06"):
    verify_source_ids(json.loads((authored / day / "verification.json").read_text(encoding="utf-8")))
assert checked_ids, "No authored report source identities verified"
print(f"Verified {checked_sources} LF source fixtures and {len(checked_ids)} current report SourceIds.")
