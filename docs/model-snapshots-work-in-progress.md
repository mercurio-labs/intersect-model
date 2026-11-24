# Model snapshots and diff

`ModelSnapshot` uses the immutable state shared by versioned heads. A basic
model or a fork can capture its current state, including a structurally valid
draft with missing required values. Portable JSON carries exact schema and
content digests and is admitted under byte and object limits. Restore checks
native draft integrity before making the state available. Run validation
separately to check domain completeness.

`ModelDiff::between` produces typed, deterministic changes for two snapshots
with the same schema. It records object and resource changes, retained value
and assignment evidence, and ordering. Hosts own physical I/O and durable
history. Checked patches, undo and redo, custom provider admission, and
allocation and work budgets are outside this slice.

## Qualification

The Model regression suite includes ten snapshot and diff tests. The
separate-process harness writes a before snapshot, an after snapshot and a
typed diff, then restores them in an independent process. It checks source
and binary fingerprints, exact serialized snapshots, exact Unicode and
reference identities, and identical diff output after restoration.

```sh
python -B tools/format_workspace.py --check
cargo clippy -p intersect-model --all-targets --locked --offline -- -D warnings
python -B tools/test_report.py run -- --profile test-fast -p intersect-model --lib --tests --locked --offline
cargo check -p intersect-model --lib --target wasm32-unknown-unknown --locked --offline
python -B tools/model_snapshot_harness.py
```
