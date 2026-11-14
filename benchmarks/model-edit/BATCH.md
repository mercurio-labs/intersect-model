# Model editing measurement contract

Measure native checked model construction and edits before further storage cleanup
or undo changes. Compare the original full-scan implementation at
`d99379d37d1c2fa28e8dc1284c6ce94347a4feef` with the current bounded validation slice.
Use identical benchmark source, fixtures, Cargo.lock, release toolchain and machine
for both. Preserve exact source/binary hashes and the current uncommitted source
snapshot; a Git HEAD alone is not its identity.

This batch adds host-only measurement code and an optional allocation-accounting
feature used only by the benchmark example. Production semantic implementations
and storage cleanup behavior remain unchanged. No Java comparison or general
language/scale qualification is part of this batch.

The workload matrix covers basic and transactional creation, scalar edits,
two-field set_many batches, singleton relationship moves, existing-object rollback, new-object
rollback and explicit full validation. Vary baseline object count independently
from edit count, with spread and repeated single-object edit patterns. Keep graph
degree and payload size explicit. Fixtures are prepared outside edit timers and
checked before use. Creation measures real checked creation, not raw insertion.

Measure release operation time, throughput, per-operation latency where meaningful,
transaction callback and boundary time, fixture setup and post-workload validation,
and OS memory with model state retained at phase handshakes. Transaction boundary
time includes begin, full commit validation and store completion; it is not a
standalone pure-commit measurement. Keep checksums and rejection/rollback controls
outside operation timers, and require equal semantic outcomes across revisions.

Run uninstrumented timing binaries separately from allocation-profile binaries.
Allocation counts report requested live/peak bytes, successful allocation calls
and cumulative requested bytes; they exclude allocator metadata and fragmentation.
Allocation-profile time must not be used as an uninstrumented speed measurement.
OS resident/private memory is a separate measure with explicit platform semantics.

Use fresh serial processes, excluded warmups, alternating candidate order, repeated
samples and a per-process timeout. Retain all raw observations, errors and run order.
Summarize medians and observed ranges. Ratios require successful matching workloads
and matching checksums for every requested repeat; timeouts are not successful work.

Acceptance: both versions build with the same harness; positive and rollback
controls pass; runner tests exercise incomplete/error/mismatched evidence; raw JSON,
CSV, reports and reproducible commands are retained. Run applicable formatting,
linting, native checks and WASM build. Prior production-code verification remains
separate from newly added host benchmark controls.
