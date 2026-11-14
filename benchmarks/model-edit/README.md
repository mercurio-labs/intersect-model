# Model editing metrics

Reusable benchmark workloads and runners. Historical results remain in private
pre-dev. Run new measurements against explicitly identified executables and
retain their input, result and toolchain fingerprints with the output.

## Run it

Use Python 3.11+ and the repository's Rust toolchain. Run from the repository
root. Each output directory must be new so previous measurements are retained.

```powershell
cargo build --release -p intersect-model --example model_edit_metrics --locked
python -B benchmarks/model-edit/run.py --candidate frozen=target/release/examples/model_edit_metrics.exe --sizes 100 500 1000 --edits 100 --repeats 3 --output target/model-edit-results/my-run
```

On Linux omit `.exe`. Supply additional `--candidate LABEL=EXE` arguments for
paired runs. The first executable is the baseline. The runner requires matching
workload and result fingerprints before reporting a timing ratio. For the
published legacy-store comparisons, use the benchmark builder from a historical
checkout with its matching fixture and benchmark host.

The runner uses fresh serial processes and reverses candidate and workload order
on alternate repeats. Small warmups are stored separately and excluded from
summaries. Each child pauses outside its measured phases for OS memory sampling.
Default timeout is 60 seconds per child, including setup and verification. Increase
it explicitly for larger workloads. Errors and timeouts remain in the raw output.

Allocation profiles are separate runs. Select the `*-allocation.exe` executables
and a different output directory. They count allocation requests but add overhead,
so the report suppresses their timing ratios. Normal timing executables do not
wrap the allocator.

## Workloads

`--size N` counts BookCopy objects; two fixed Library roots are additional.
For construction, the starting fixture has only those roots. For other workloads,
all N copies already exist, and copy 0 belongs to one Library. Fixture construction,
adoption, initial validation, action preparation and initial checksum are outside
the operation timer. `full_validation_calls` counts only explicit workload
`validate` calls, not internal checks or transaction commit validation.

| Scenario | One workload unit | Units per run |
| --- | --- | --- |
| `basic-create` | Create one copy and set its required name | N |
| `transaction-create` | Same, inside one enclosing transaction | N |
| `basic-scalar-edit` | Set one copy's edition | K |
| `transaction-scalar-edit` | Same, inside one enclosing transaction | K |
| `set-many` | One atomic batch setting edition and title | K |
| `relationship-edit` | One atomic batch moving copy 0 between Library owners | K |
| `rollback` | Set edition inside a transaction that deliberately fails | K |
| `create-rollback` | Create and name a temporary copy in a failing transaction | K |
| `validate` | Full validation of the unchanged model | K |

`--edits K` is independent of N. Creation ignores K when selecting its unit count,
but records the argument. Create/name pairs use two checked API operations.
`set-many` and relationship moves each use one API call with two assignments.
Relationship degree stays one as unrelated model size grows.

`--patterns spread hotspot` changes the targets of scalar edits, `set-many` and
existing-object rollback. Spread targets `i % N`; hotspot repeatedly targets copy
0. That copy also has the fixed Library relationship, while most spread targets
are unrelated. Compare K scaling within a pattern; hotspot versus spread is not
a pure target-locality comparison. The other scenarios ignore the pattern but
echo it for explicit run identity.

Use `--scenarios` to select a subset. A useful second matrix keeps N fixed at
100, raises K from 100 to 1,000, and tests both patterns for transactional scalar
edits and rollback. This distinguishes model-size costs from accumulated
transaction work. Repeat `create-rollback` with larger K to track temporary-object
cleanup. No deletion performance claim is made by this suite.

## What the metrics mean

- **Total workload time:** operation time plus transaction boundary time. This is
  the primary comparison for transactions, including full commit validation.
- **Operation time:** actual checked API work. Individual-unit p50/p95/max latency
  is also recorded; percentiles are omitted below 20 samples. Per-unit clocks and
  sample recording are common harness overhead, most visible on tiny workloads.
- **Transaction boundary:** begin plus final validation and commit, or rollback.
  This is not a measurement of store commit alone.
- **Setup and verification:** reported separately. Verification includes full
  validation, exact expected values, checksums, invalid-edit rejection and rollback
  controls. These are not timed as the workload.
- **Memory:** OS resident, private and lifetime peak resident bytes at startup,
  setup, workload, verification and teardown. Windows private bytes are committed
  memory; Linux private bytes are resident pages. Neither is model heap size.
- **Allocation profiles:** live/peak requested bytes, allocation/reallocation calls
  and cumulative requested bytes. Workload deltas isolate the measured work and
  are normalized by units and API calls where meaningful. They include API and
  harness allocations, exclude allocator bookkeeping/fragmentation, and are not
  interchangeable with OS memory. Teardown retains some harness/IO metadata and
  therefore is not a pure model leak measurement.

`results.json` contains raw observations, process wall/CPU times, run order,
provenance, correctness fingerprints and all summaries. `summary.csv` contains
phase statistics; `REPORT.md` is readable without tooling. Medians and observed
min/max ranges describe these samples; three repeats do not establish a tight
confidence interval. Ratios require complete matching runs, including schema,
workload, counts, allocation mode, harness identity when supplied, and checksum.

## Track each change

Keep the initial results unchanged. After each cleanup, undo or validation change:

1. Build with a new output directory and `--label` naming that change.
2. Rerun the same N/K/pattern matrix, toolchain and build mode on the same machine.
   Do not run builds or other benchmark processes concurrently with measurements.
3. Keep raw results and source provenance next to a short note explaining the
   change. Include regression controls and total transaction time, even when only
   callback time improves.
4. Compare with both the original implementation and the previous step. The runner
   accepts multiple `--candidate LABEL=EXE` arguments; the first is the ratio
   baseline. To reuse prior binaries, merge their candidate entries into the new
   provenance JSON and preserve their original source/harness hashes. A changed
   harness requires rebuilding the comparison candidates.

No automatic performance gate is set yet. Establish repeatability and noise over
several runs before choosing a regression threshold. These are bounded native
observations of the native Library workload. SQLite, browser, arbitrary graph,
and general language performance require separate measurements.

## Check the measurement tools

```powershell
python -B -m unittest discover -s benchmarks/model-edit -p "test_*.py" -v
cargo clippy -p intersect-model --example model_edit_metrics --features model-edit-allocation-metrics --locked -- -D warnings
```

The child workload also checks its own results on every successful run. Repository
formatting, lint, native tests and WASM checks still apply to production changes.
