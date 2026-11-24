# Model snapshots and diff: implementation checkpoint

Worktree: `.worktrees/model-snapshots-diff`, branch `codex/model-snapshots-diff`.
Explicit base: develop `d95bfd0c812cd03ccdb34d8e922372718b69156a`.

Implementation is in progress. The user switched the active goal to Mercurio's
repository-per-project storage work before final qualification of this slice.

The initial implementation reuses immutable `api::Snapshot` as `ModelSnapshot`,
adds basic-model and fork capture, bounded portable JSON state with exact schema
and content digests, native draft-integrity validation on restore, and typed,
deterministic `ModelDiff::between`. It keeps physical I/O and durable host history
outside the library. Diff reports are descriptive; checked patches and undo/redo
remain future work. Custom provider profiles and observed allocation/work budgets
are not qualified by this slice.

Ten focused integration tests passed before the final lint adjustments, recorded
at http://localhost:8765/model-snapshots-diff/20261118-223003/.
The source has since boxed diff records for the strict enum-size lint and added
the example's missing `ModelEdit` import. Final regression, lint, WASM and
separate-process harness qualification remain required; this checkpoint is not a
completion claim. Cargo locks and dependencies are unchanged.

Resume with a C-drive build cache where appropriate:

```powershell
python -B tools/format_workspace.py --check
cargo clippy -p intersect-model --all-targets --locked --offline -- -D warnings
python -B tools/test_report.py run -- --profile test-fast -p intersect-model --lib --tests --locked --offline
cargo check -p intersect-model --lib --target wasm32-unknown-unknown --locked --offline
python -B tools/model_snapshot_harness.py
```

The earlier focused run used `CARGO_PROFILE_TEST_FAST_OPT_LEVEL=0` to bound
fresh-build time and a C-drive target directory. Its ten scenarios completed in
0.12 seconds; the initial build took approximately 145 seconds. The new
`model_snapshot_harness.py` is intended to run the example in two independent
processes and retain source/binary fingerprints and artifacts. It has not yet
been run successfully at this checkpoint.
