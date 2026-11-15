# Collection-growth diagnostic

This host diagnostic exercises ordinary `FrozenStore` append and bulk assignment,
checks every final value and its order, and verifies rollback. It reports requested
allocator bytes and instrumented timing; these are not Java-parity measurements.

Build and run from the active Model repository:

```powershell
cargo build --release -p intersect-model --example collection_growth_metrics --locked
python -B benchmarks/java-parity/run_collection_growth.py --output target/java-parity/collection-growth --sizes 500 1000 2000 4000 --repeats 2
```

The output directory must not already exist. The report retains the source
revision/status, binary and harness hashes, individual runs, and medians. Timing
includes allocation-counter overhead. Broader comparisons remain milestone work.

The original criteria and measurements remain in the archived pre-dev benchmark
directory. See `docs/performance-carry-forward.json` for exact source provenance;
historical numbers are not evidence for this new source revision.
