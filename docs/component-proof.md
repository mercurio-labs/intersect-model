# Model component proof

This experimental M1 proof mounts a prepared library into independently editable
models. It is language-neutral and requires the opt-in `components` feature.
It does not download or prepare KPARs, run Text editing sessions, or provide a
durable component cache. Those are later milestones in the component usage plan.

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
