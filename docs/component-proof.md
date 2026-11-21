# Model component proof

The experimental `components` feature mounts a prepared library into independently
editable models. Start with the [runnable Model example](../crates/intersect-model/examples/model_component_usage.rs)
and the [`components` module](../crates/intersect-model/src/core/components.rs).
Model owns immutable component images, mounted reads, checked edits and snapshot
admission. Text owns editing sessions, and KerML and its hosts own KPAR acquisition,
language-specific preparation and cache storage.

## Immutable component validation

Preparation and default restore run the canonical structural audit on each closed native
arena before issuing a private immutable image. Mount construction checks schema
ownership and duplicate identities, then shares those already checked images;
it does not repeat their audits through qualified object adapters.

Final admission audits the editable project with the canonical validator. Target
and opposite lookups retain the whole mounted view. Every component object with
an inverse overlay is also checked. Overlays cannot remove base relationships or
alter containment, so the remaining immutable graph keeps its preparation
invariant. A project containment edge into a component selects the full global
audit, preserving ownership and resource-root checks across that boundary.

This is a component-specific optimization. General checked edit services and the
canonical full validator remain available unchanged. The partition controls compare
success and failure against the full validator, including broken targets, inverse
overlays, cycles and cross-boundary containment.

Object-bound initialization receipts are not yet rebased by the mounted read
adapter. Preparation explicitly rejects those images rather than caching native
evidence that the qualified full audit would reject. Other explicit assignment
markers retain their existing canonical checks.

The audit uses the same physical native slot/assignment view as mounted reads.
The general FrozenStore archive adapter can synthesize legacy assignment markers
for serialization; those markers must not replace native evidence during image
admission. A default-valued native image and its snapshot restore exercise this
distinction against the full mounted validator.

Immutable images also retain their qualified identities, class indices and
ordered containment topology. Scope, completion and validation reads rebuild
only project containment and combine it with this retained topology. They still
read attributes, reference values and reciprocal overlays from the live mounted
arenas. Per-invocation evaluation memos are not shared across project changes.
Image identities use shared strings, avoiding repeated copies of large qualified
IDs. A full-traversal comparison checks multiple mounts, child order, snapshot
restore, changing project row offsets, forks and conflicting owners.

## Host-built component caches

`restore_with_trust` adds the same explicit trust choices as frozen snapshots.
The default `restore` remains fully revalidated. `HostCache` accepts the original
host-built canonical admission and checks snapshot digests and decoding bounds;
`VerifiedHostCache` requires the caller to have verified the exact enclosing
payload already. Both still check runtime, schema, descriptor key, resource
package identity and unsupported constructor evidence. They do not materialize
and hash the graph or repeat canonical admission. Downloaded model interchange
and textual KPAR preparation must not use these trusted modes.

## Run the demonstration

From this checkout in PowerShell:

```powershell
./tools/run_component_proof.ps1
```

The script builds in the Windows temporary directory rather than the source
drive, disables incremental compilation, and writes
`target/component-proof/report.json`. Override paths when useful:

```powershell
./tools/run_component_proof.ps1 -BuildDirectory C:/build/intersect-components -ReportPath C:/reports/components.json
```

After a successful run, the script also copies the Windows executable into the
worktree. Run that copy immediately without rebuilding:

```powershell
./target/component-proof/model_component_usage.exe --out target/component-proof/report.json
```

Rerun the build script after changing source; the copied executable represents
the last successful build.

On other hosts, or to control Cargo directly:

```sh
cargo run --locked --profile test-fast -p intersect-model --features components --example model_component_usage -- --out target/component-proof/report.json
```

The first build compiles the runtime. The executable prints PASS or FAIL for
actual operations and returns a nonzero exit code on failure. The report includes
the runtime source identity, individual assertions, and any error.

## Demonstrated behavior

The small authored Rcore schema defines Nodes with names, ordinary references,
and reciprocal relationships. A library contains `shared` and `internal`; a
project deliberately also has an object whose original ID is `shared`.

- Two projects retain the same immutable library allocation and shared schema,
  descriptor, and validation context. Library objects are absent from their
  writable project arenas.
- Qualified IDs distinguish project objects and different components, while
  `component_target` preserves the original component URI, prepared package
  identity, and external element ID.
- Borrowed `ObjectView` and `ValueView` navigation resolves project-to-library
  and library-internal references to view-scoped runtime handles. Ordinary
  `ModelRead` queries see the mounted graph.
- Checked project edits use the existing core setters and validation plan.
  Read-only objects reject authored mutations. Reciprocal contributions live in
  project-owned overlays and preserve the library's authored relationships.
- Failed transactions restore project data and overlays together. Forks copy
  project state and overlays while retaining the shared component image.
- Handles cannot cross independent mounted views or forks. Structural type
  checks and reference validation remain active.

## Implementation boundaries

`SharedSchema` explicitly owns a reusable schema context. Existing FrozenStore
constructors and ordinary model facades retain their behavior. Component-specific
borrow context fields compile only with the experimental feature.

`ComponentImage::prepare` consumes a linked, validated frozen model. It hashes a
temporary canonical projection once, then retains the arena without a second
materialized object graph. The image exposes no mutable arena. Different projects
share it through `Arc`; the sharing controls also compare borrowed value addresses.

`MountedModel` is a narrow checked facade over a private store, not a replacement
for every existing model facade. It delegates creation and mutation to the same
core services as BasicModel. The private store permits reciprocal overlay writes;
the public facade permits authored writes only to project objects. It does not
claim incremental-validation capability, so graph checks remain conservative.

The initial supported boundary is one explicit shared schema context, closed
library images, scalar and collection references, and inverse overlays on
non-containment reference features without unsettable assignment history.
Cross-component containment and changes that would displace a library-authored
relationship are refused atomically. Authored internal library relationships are
preserved. Different versions of one logical component URI cannot be mounted in
the same view.

This proof does not qualify generated accessors, compiled/interpreted language
execution, prepared cross-component dependency graphs, standard export, durable
snapshots, resource unloading, or concurrent editing. `Arc` expresses shared
ownership and does not promise that a schema/runtime is thread-safe. Qualification
of those paths must follow before advertising broader support.

Qualified adapter IDs encode component ownership explicitly. They are not a new
standard interchange format. The generic observed-snapshot export remains
unsupported rather than silently flattening mounted ownership.

## Focused verification

Run the new controls through the repository's test reporter:

```sh
python -B tools/test_report.py run -- --profile test-fast -p intersect-model --test all --features components --locked component_usage
```

Controls exercise allocation sharing, descriptor scope, colliding identities,
qualified navigation, inverse isolation, scalar displacement, failed batches,
containment refusal, bounded query pagination, forks, foreign handles, incompatible
schemas, conflicting mounts, and attribute strings that resemble qualified IDs.

Also run the changed crate's ordinary quick tests, strict lint, formatting,
boundary checks, and WASM compilation with the feature disabled and enabled.
Use a separate target directory if the source drive is short of space.

## Follow-on Text and hosted-archive prototype

The opt-in API now includes `MountedDraft` for project-only construction and
`ComponentImage::prepare_with_snapshot`/`restore` for resolved local snapshots.
`SharedSchema::from_metamodel` retains admitted redefinition layouts. Mounted
structural audits select that same layout, while ordinary required fields keep
their checks. The native Rcore VM and direct executor share a borrowed read
adapter across frozen and mounted models, including class-specific read slots.

The follow-on controls cover exact snapshot restoration and corruption refusal,
cross-arena source scopes, redefined-field reads, direct/VM agreement, VM error
fallback, defaults confined to the project, and redefinition validation. The
final component-enabled Model regression run passed 937 tests. The Text/KerML
worktrees add retained editor sessions and a real hosted-KPAR harness; see that
worktree's `docs/hosted-kpar-prototype.md` and validation receipt.

## Develop integration and API review

The component surface remains experimental behind `components`. Default and
all-feature API inventories record the reviewed additions. The existing
`frozen_scopes::FrozenScopeRead<'v, 'a>` name is now a type alias for the shared
`StoredScopeRead<'v, LinkView<'a>>`; existing construction and ScopeRead use keep
their signatures and behavior. `IndexedScopeRead` supplies identity lookup to
the same VM observation implementation for both native and mounted readers.
LinkView gains that additive trait implementation. These are provisional
runtime adapter APIs, not a claim of a new stable application contract.

CI keeps the ordinary scoped lane and additionally checks the component-enabled
native runtime controls, strict lint and WASM build. Downstream development
consumers must select the committed Model revision consistently; local sibling
patches are not part of the integrated dependency graph.

The performance follow-up reviews `ComponentImage::key()` and
`ComponentImage::restore_with_trust(...)` in both public module paths, together
with the retained immutable image representation. The key accessor exposes the
existing immutable identity. The restore method takes an explicit
`SnapshotTrust`; default `restore()` still fully revalidates, while trusted
host-cache modes require the admission and enclosing-payload verification
described above. The all-feature export and signature inventories include these
additions. Default-feature declarations remain unchanged.
