# Performance changes carried forward on November 15, 2026

The implementation changes committed after the pre-dev baseline were ported onto
the independent active `develop` baseline. The original runtime files matched
that baseline exactly, so the runtime patch applied without conflict. This does
not import pre-dev Git ancestry or revive the archived repository as a dependency.

The port includes incremental arena collection growth, validated URI reuse,
completed-reference conformance reuse, canonical XMI construction and containment
metadata reuse, in-place reference-cell resolution, resource-touch journaling,
dense identity indexing, and one-pass structural acceptance with canonical audit
fallback. Their regression tests and the collection-growth diagnostic are included.

The API review adds storage extension hooks with compatibility defaults and the
structural-facts contracts they consume. It removes no existing named export and
changes no existing exported function signature. The reviewed default and
all-feature API inventories include these additions; `FrozenStore`'s contract
record also changes because it has additional private bookkeeping fields.

`performance-carry-forward.json` lists all ten source commits, original source
identities, and hashes of imported runtime and benchmark files. No rejected or
prototype patch was applied. Historical benchmark JSON remains with the archived
source; its results do not qualify the new Git source identity. Full EMF/Xtext
performance parity remains open.

Use the README quick checks, including `--features xmi` for the affected importer,
and check the WASM target. The host-only collection diagnostic is documented in
`benchmarks/java-parity/README.md`. Tests must preserve failure diagnostics,
resource identity, ordering, rollback, optional features, and budget accounting.

Consumers must select this implementation through the canonical Model Git URL
and one full immutable revision. A local coordinated-pair check establishes local
behavior; canonical GitHub fetching is a separate check after the migration
branches are pushed. Existing release artifacts and consumer pins are not
automatically requalified by this source port.
