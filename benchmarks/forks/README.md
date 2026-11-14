# Native fork memory experiment

This focused experiment retains one million simultaneous sibling forks of an
immutable baseline, measures them untouched, then changes one field in every
fork and measures again. It verifies selected sibling values and an unchanged
baseline before dropping all forks and measuring teardown.

The target is **one shared baseline + fixed metadata per retained fork + current
changed values and persistent-map overhead**. Empty forks cannot have zero cost:
the caller retains a `Vec<ModelFork>` plus any per-fork bookkeeping. Measurements
include all of those allocations. This is a native synthetic storage workload,
not a SysML, EMF compatibility, browser, or general large-graph performance claim.

## Run

From the repository root, with Python 3.11+ and the Rust toolchain:

```sh
cargo build -p intersect-model --example fork_memory --release --locked
python benchmarks/forks/run.py
```

The default matrix runs each case in a fresh process three times, alternating
case order. Raw phase observations, source and executable hashes, relevant Git
status/diff digest, parameters, and platform/toolchain information are retained
in `target/fork-benchmark/results.json`; `REPORT.md` presents medians. No Java,
network, additional package, or external solver is required. Rebuild the release
binary after source changes; the runner fingerprints but does not build it.

| Case | Forks | Baseline objects | Unchanged target ballast | Change in each fork |
| --- | ---: | ---: | ---: | --- |
| scalar-1k | 1,000,000 | 1 | 1 KiB | Distinct EInt |
| scalar-16m | 1,000,000 | 1 | 16 MiB | Distinct EInt |
| empty-graph | 1,000,000 | 10,000 | 1 KiB | None |
| text-128 | 100,000 | 1 | 1 KiB | Distinct 128-byte EString |
| text-1024 | 100,000 | 1 | 1 KiB | Distinct 1,024-byte EString |

The unchanged ballast and edited counter/text are on the **same object**, exposing
whole-object as well as whole-model copying. Only the first object has ballast.
All fields have valid defaults in the handwritten fixture schema. Setup uses one
bulk storage transaction, followed by checked facade adoption. Attribute indexing
is enabled for the counter only. Editing still uses normal checked `ModelEdit`
operations, including their validation; it does not bypass semantic consistency.

A small harness check or selected cases can be run with:

```sh
python benchmarks/forks/run.py --forks 100 --text-forks 100 --graph-elements 100 --repeats 1 --output target/fork-smoke
python benchmarks/forks/run.py --cases scalar-1k scalar-16m --repeats 3 --output target/fork-scalars
```

The example also runs directly, without interactive pauses:

```sh
cargo run -p intersect-model --example fork_memory --release --locked -- --forks 1000000 --elements 1 --baseline-payload-bytes 16777216 --mode scalar
```

Modes are `empty`, `scalar`, and `text`; `--edit-bytes` controls changed text size
(at least 16 bytes for a distinct sibling marker). `--handshake` is reserved for
the runner: it waits for a newline at startup and after each measurement so the
parent can sample OS memory while all relevant allocations remain alive.

## Measurement boundaries

- `baseline`: the populated versioned model and an immutable snapshot.
- `empty`: all requested forks alive in the Vec; no edits.
- `edited`: every fork changed; baseline and sampled sibling values verified.
- `forks_dropped`: entire Vec and all fork-owned data dropped; baseline retained.
- `all_dropped`: model and snapshot dropped as well.

The executable forwards `alloc`, `alloc_zeroed`, `realloc`, and `dealloc` directly
to Rust's `System` allocator, tracking successful requested allocation sizes.
Instrumentation uses allocation-free atomics. Its small documented unsafe block
is isolated to this measurement executable. Live counters are sampled before JSON
serialization, and startup initializes I/O buffers before baseline measurement.
Phase peaks reset to current live bytes before the phase and include temporary
allocations and post-loop verification. A successful `realloc` counts its changed
requested size; internal allocator copies during realloc are not visible here.

Reported live bytes include Vec capacity, fixed fork values, schema/model state,
maps, changed values, and runtime heap allocations. They exclude allocator
metadata, fragmentation, stacks, mapped code, and uncommitted virtual pages.
The per-phase OS samples report RSS and process-lifetime peak RSS on Windows and
Linux; private bytes are Windows commit charge or Linux private resident pages,
and those private metrics are **not interchangeable across platforms**. OS errors
are recorded as unavailable observations. Freed memory may remain in allocator
pages, so allocator live teardown and OS RSS teardown answer different questions.

Use `empty - baseline` to measure retained empty-fork cost and `edited - empty`
for changed-data/map cost. The report's per-fork increment is an observed delta
divided by the actual retained count, not a theoretical `size_of` estimate.
`fork_size_bytes` and `fork_vec_requested_bytes` are also recorded separately.
Compare the two scalar cases to detect retained growth with untouched payload;
compare the two text cases to measure growth with changed payload. The large
graph case checks empty-fork cost independently of baseline object count.

These controls cover simultaneous sibling forks and small individual edits.
They do not establish bounded large-graph validation time, collection-item delta
storage, deep history cost, merging, persistent history, browser memory limits,
or language conformance. Timeouts and process errors are explicit unsuccessful
rows, never evidence of a completed measurement.
