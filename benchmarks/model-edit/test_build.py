"""Source snapshot and provenance controls; no Cargo builds or Git changes."""
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("model_edit_builder", Path(__file__).with_name("build.py"))
BUILDER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILDER)


def put(root, name, data):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


class SnapshotTests(unittest.TestCase):
    def test_overlay_removes_staged_deleted_and_renamed_archive_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            put(root, "crates/example/deleted.rs", b"old staged deletion")
            put(root, "crates/example/old.rs", b"old staged rename")
            put(root, "Cargo.lock", b"old lock")
            put(root, "README.md", b"outside build inputs")
            wanted = {"Cargo.lock": b"new lock", "crates/example/new.rs": b"new name"}
            BUILDER.overlay_current(root, wanted)
            self.assertEqual(set(BUILDER.source_manifest(root)["files"]), set(wanted))
            for name, content in wanted.items():
                self.assertEqual((root / name).read_bytes(), content)
            self.assertEqual((root / "README.md").read_bytes(), b"outside build inputs")

    def test_capture_once_is_independent_of_later_source_edits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "current"
            root.mkdir()
            put(root, BUILDER.HARNESS, b"first harness\r\n")
            put(root, "README.md", b"not a build input")
            names = (BUILDER.HARNESS.as_posix() + "\0README.md\0crates/deleted.rs\0").encode()
            with patch.object(BUILDER, "ROOT", root), patch.object(BUILDER, "command", return_value=names):
                inputs = BUILDER.current_inputs()
            put(root, BUILDER.HARNESS, b"later harness\n")
            self.assertEqual(inputs, {BUILDER.HARNESS.as_posix(): b"first harness\r\n"})
            for label in ("a", "b"):
                snapshot = Path(directory) / label
                snapshot.mkdir()
                BUILDER.overlay_current(snapshot, inputs)
                self.assertEqual((snapshot / BUILDER.HARNESS).read_bytes(), b"first harness\r\n")

    def test_overlay_rejects_paths_outside_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "snapshot"
            root.mkdir()
            with self.assertRaises(ValueError):
                BUILDER.overlay_current(root, {"../escaped": b"no"})
            self.assertFalse((Path(directory) / "escaped").exists())

    def test_archive_is_byte_exact_and_deterministic(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "snapshot"
            root.mkdir()
            put(root, "Cargo.lock", b"lock\r\n")
            put(root, "fixtures/data.bin", b"\x00\xff\x10\r\n")
            identity = BUILDER.source_manifest(root)
            first = Path(directory) / "first.tar.gz"
            second = Path(directory) / "second.tar.gz"
            evidence = BUILDER.archive_inputs(root, first, identity)
            BUILDER.archive_inputs(root, second, identity)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            self.assertEqual(evidence["sha256"], BUILDER.sha(first.read_bytes()))
            with tarfile.open(fileobj=io.BytesIO(first.read_bytes())) as archive:
                self.assertEqual(set(archive.getnames()), set(identity["files"]))
                for name, digest in identity["files"].items():
                    self.assertEqual(BUILDER.sha(archive.extractfile(name).read()), digest)

    def test_archive_refuses_changed_source_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "snapshot"
            root.mkdir()
            put(root, "Cargo.lock", b"before")
            identity = BUILDER.source_manifest(root)
            put(root, "Cargo.lock", b"after")
            archive = Path(directory) / "snapshot.tar.gz"
            with self.assertRaises(ValueError):
                BUILDER.archive_inputs(root, archive, identity)
            self.assertFalse(archive.exists())


if __name__ == "__main__":
    unittest.main()
