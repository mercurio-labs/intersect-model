# Invocation-local stored-read preparation

The user approved this internal correction on 2026-11-19 under Views
`docs/m14-read-boundary-correction-spec.md`. Its prerequisite is Model ef2b43f.
No public API, serialized format or execution limit changes.

The three crate-owned model function/getter entry points create a private reader
for one invocation. It reuses the existing `observed_member` implementation and
retains only successful exact `(receiver class, source declaration)` metadata
resolutions under that reader's immutable schema owner. Cache keys/bookkeeping
are precharged to the original observation before copying. Lookup, cancellation,
source/revision/type, multiplicity and stored-field guards remain active. No
object values, references, failures or pending results are cached. The reader is
dropped at invocation return, including errors.

The public reusable `SourceModelReads::new` path keeps its original behavior;
caller-supplied capabilities are not silently given a cross-invocation cache.

Six focused cases cover real paged values and lower repeated preparation costs,
fresh values and source/type/revision guards, independent invocation budgets,
last-precharge failures without cache publication, pending/unavailable references,
and exact inherited/redefined declaration resolution versus the original reader.
The complete quick tier passed 934 tests with 21 milestone tests ignored. Strict
all-target lint, formatting and the WASM library check passed. Commands, source
hashes and qualifications are in `m14-read-preparation.json` and
`docs/evidence/m14-read-preparation/`.

This is core qualification, not M14 completion. The immutable paired Text and
isolated Views/KerML/Mercurio consumers still need their actual native/WASM
artifacts, compatibility checks and full M14 gates.
