# Working with models

Start with the [README examples](../README.md#a-small-model). Build a DynamicSchema
in Rust, then define the same model in Rcore; both create, name and save an Item. Use the
[Rcore guide](rcore-guide.md) for definition and execution choices.

## Choose an editing API

### Application and extension boundaries

Application APIs are schema/model construction, checked edits, Rcore execution,
resource lifecycle and persistence. Start with `SchemaBuilder`, `DynamicModel`,
`RcorePackage` / `RcoreModel` or the reflective `api` facades below.

The `extensions` rustdoc is the entry point for host/provider authors. It owns
`SourceReadCallbacks` and `LOGICAL_WORD`; their old implementation module paths
are private. Storage (`storage`, `frozen`), source preparation (`definitions`),
conversion/initialization and semantic provider modules retain their existing
paths because Text, generated bindings and independent languages consume them.
These are advanced, provisional contracts, not implicit execution authority.
The root `logical_size!` macro remains public for portable downstream accounting.

Implementation-only draft validation, revision comparison, resource-lock helper,
multiplicity slicing and IR unary typing functions are private. The unused
`initialization::require_default_construction` convenience function was removed;
checked construction and initializer admission remain in the owning model APIs.
Public DTO fields remain authoring/transport data; validation is not bypassed by
deserialization. Existing compatibility paths such as `core` remain reachable
until their actual consumers can migrate; this is not a complete minimality claim.

| API | Use it for | Boundary |
| --- | --- | --- |
| `RcorePackage` / `RcoreModel` | Source-first compilation and model creation using local class names | Delegates to the shared checked model services |
| `api::BasicModel` / `ModelEdit` | Individual reflective edits against a schema | Each operation is atomic; separate calls are separate edits |
| `api::TransactionalModel` | A group of edits that must commit together | Callback error or failed commit validation rolls back the batch |
| `dynamic` | Runtime schema-scoped object/feature handles | Handles remain bound to their owning model/schema context |
| `api::VersionedModel` | Snapshots and isolated forks | In-memory versioning is not durable history or a text-editing service |
| Generated bindings | A typed domain API | Generated access uses the same checked model implementation |

`BasicModel` permits incomplete construction: required lower multiplicities may
remain unsatisfied until `validate()`. A failed `set_many` does not retain part
of its batch. A sequence of individually successful calls is not automatically
rolled back when a later call fails; use a transaction for that requirement.

Structural validation does not install language-specific rules. Select and bind
the supported semantic services explicitly. The direct Rcore facade, frozen
binding engine and provider adapters have different activation contracts.

## Signature and persistence contracts

Use `DynamicModel` or `RcorePackage` for checked application operations. Their
owned state is private. `Schema`, `Model`, source bundles and compilation recipes
are transport/authoring data: public fields and `Deserialize` do not certify
validity. Keep those DTOs mutable for builders, codecs and language adapters;
pass them through the corresponding checked constructor or loader before use.
Borrowed preparation views retain their source lifetimes. Provider traits are
explicit host extension contracts; deserialization never supplies a provider.

`RcoreModel` exposes the structural `BasicModel` facade. That does not grant
Rcore execution authority: package save/invocation checks schema equality,
structural validity and required initializer receipts. String error messages and
diagnostic prose are for display, not stable identifiers to parse. Use typed
error variants where the API provides them. Target-sized `usize` limits and
offsets must fit the receiving target; native 64-bit ranges are not a WASM32
wire-format promise.

| Format | Current contract | Admission boundary |
| --- | --- | --- |
| Dynamic JSON snapshot | Envelope version 1; schema and model data | `DynamicModel::from_json` rejects unknown envelope fields, duplicate fields and unsupported versions; rebuilds the schema context and validates the graph |
| Rcore saved model | Envelope version 2; exact bundle, entry, namespace, type selections, bindings and model | `RcorePackage::reload` rejects unknown envelope fields and unsupported versions, recompiles sources, rebinds operations and validates data/initializer receipts |
| Definition bundle | Version 1; strict source/identity/dependency records | Bundle/source validation checks content identities and dependency resolution; decoding alone is not compilation |
| Frozen binary snapshot | `SNAPSHOT_FORMAT = 1`; little-endian rebuildable host cache | Bounds, tags, key and metamodel are checked; `Revalidate` checks structure, `HostCache` assumes host-validated output, and `VerifiedHostCache` additionally assumes integrity already checked by the host |
| XMI / SQLite | Separately qualified optional interchange / host storage paths | Use their explicit adapters; neither substitutes for the checked package loader |

Strictness is per type. A strict outer snapshot does not imply every nested DTO
rejects unknown fields. Binary snapshots are not a durable interchange promise;
retain authoritative sources/data so a cache can be rebuilt. Host code bounds
input bytes and owns I/O, authentication and publication. Content digests detect
changes; they do not authenticate an untrusted producer.

The [release plan](release-plan.md) records the signature/Serde declaration
baselines and native/WASM refusal controls. Those baselines detect declaration
changes, not custom codec-body behavior or universal semver compatibility.

## Identity, resources and storage

`frozen::ArenaStore` is the preferred descriptive name for the mutable arena
store. `frozen::FrozenStore` remains the same type under its existing name, with
the same constructors, trait implementations and checkpoints. The new spelling
is a re-export, not a second backend or a change to snapshot formats. The
validated `frozen::Model` remains immutable. Most applications should use the
editing facades above instead of depending on store internals.

Keep these identities distinct:

- A resource URI names a logical resource; a host chooses its physical location.
- An element ID identifies an object across the model, not just within a file.
- An object handle refers to a particular incarnation in a particular store.
- A schema/source revision identifies definitions; matching namespace text alone
  does not prove compatibility.

Resource records and objects belong to one canonical model. Resource lifecycle,
containment/opposites and persistence should use shared services rather than a
parallel application-owned object graph. Host adapters supply bytes, codecs and
physical I/O. Keep native-only SQLite separate from portable model libraries.

Choose the persistence format deliberately. Save/reload is not a guarantee that
all earlier formats or arbitrary imported XMI are accepted. Preserve format
versions, source identities, conversion contracts and declared dependencies.

## Integration features

| Feature/package | Role |
| --- | --- |
| `intersect-model` with `xmi` | XMI interchange through supported model/conversion contracts |
| `intersect-model` with `sqlite` | Native SQLite adapter; excluded on WASM |
| `intersect-model` with `codegen`, `intersect-codegen` | Binding generation and build integration |
| `intersect-model` with `rhai` | Checked script providers under supported source-authorized contracts |
| `intersect-project-host` | Host acquisition and package-manifest contracts; hosts implement acquisition |
| `intersect-run` | Deterministic discrete-event kernel; a library, not the language CLI |

For a runnable construction and behavior example:

```sh
cargo run -p intersect-model --example rcore_model --locked
```

For programmatic schemas:

```sh
cargo run -p intersect-model --example dynamic_library --locked
```

## Detailed reference and evidence

The prerelease API review distinguishes recommended facades from advanced
implementation contracts. Public visibility alone does not promise stability for
every `core`, `definitions`, frozen or draft type. The following checks provide
concrete evidence for bounded paths; they are not a complete API freeze:

| Path | Positive / refusal evidence | Release boundary |
| --- | --- | --- |
| Rcore package facade | [Model-only consumer](../tests/model-only/src/lib.rs): construction, execution, getters, rejection and recovery | Default-feature native and Node WASM artifact rehearsal |
| Direct Rcore API | [Rcore controls](../crates/intersect-model/tests/rcore_api.rs): execution/reload, atomic argument/binding rejection, corrupt saved data, exact dependency sources and visible adapter refusal | Recommended source-first facade; no implicit filesystem/network acquisition |
| Storage transactions | [Resource-count controls](../crates/intersect-model/tests/memory_resource_count.rs): nested commit/rollback keeps live cardinality | Native regression; hosts still own publication and cancellation |
| Prepared defaults | [Null defaults](../crates/intersect-model/tests/draft_null_defaults.rs), [physical defaults](../crates/intersect-model/tests/draft_physical_defaults.rs): source/binding/budget refusal and recovery | Advanced source-bound APIs; not arbitrary provider activation |
| Package loading | [Package-load controls](../crates/intersect-model/tests/package_load/mod.rs): provenance round trips, missing payloads, foreign resource identity and atomic retag refusal | Exact retained identity and format contracts |
| XMI / SQLite | Combined optional-feature suite covers interchange refusal/round trips and persistent resource/transaction rollback | SQLite is native-only; supported combinations are those recorded in the receipt |
| Code generation / Rhai | Generated Rust consumers execute native accessors and checked providers; fresh native/Node WASM transaction reports agree | Historical exact-work replay still fails; do not promise unchanged work counts across revisions |

## Advanced storage and binding contracts

Store identity, element identity and schema identity are separate. A handle from
another store or an earlier object incarnation must not authorize access merely
because its numeric slot matches. Checked operations maintain containment,
opposites, resource membership and assignment state. Use the transaction facade
for grouped edits; a host still owns atomic publication outside the model.
Versioned forks are isolated editing contexts, not persistent history.

Dynamic schema handles belong to the schema that created them. Retain the built
schema and its class/feature handles with the model; rebuild and validate context
on snapshot load. Avoid mixing handles from similarly named but distinct schemas.

Source-bound semantic contracts include exact declarations, dependencies and
specialized types. Native adapter metadata does not install executable code.
Declare pre/postconditions before registration; argument admission and a failing
precondition precede execution, and result checks and postconditions precede
successful return. Native providers remain trusted to honor their declared effects.
Read-only/query capabilities do not grant mutation. Mutable provider paths require
their own checked host-owned transaction contract and rollback boundary.

Generated adapters and handwritten adapters use the same checked conversions.
Keep nominal datatype identity, integer widths, multiplicity and collection kind;
do not narrow integers, deduplicate unique results or coerce a nominal type merely
because its storage representation matches. Unsupported generated signatures must
fail explicitly rather than falling through to an unchecked adapter.

Package registration stages entries before installation so a duplicate/revision
failure leaves the registry unchanged. Suspended calls retain source, provider,
model and budget context; stale or foreign resumptions must be refused. Model
changes and host side effects are not made atomic by a serialized capability token.

See the executable [binding declarations](../tests/support/src/bindings/checked_contract.rs),
[generated-binding harness](../tests/semantic-bindings/build.rs) and
[dynamic example](../crates/intersect-model/examples/dynamic_library.rs).
Use rustdoc for detailed provider signatures and the [release plan](release-plan.md)
for the qualification boundary. Historical run counts stay with their source revisions.

## Frozen Rust bindings

Use `codegen::frozen::generate` with the checked metamodel used to load a
frozen model. The generated module binds a `ModelView` to that metamodel and
exposes typed getters, enum values, conversions to all ancestors, and
operation declarations with typed argument and result signatures. Derived
getters accept a provider. The frozen model API is read-only, and operation
execution belongs to the language's semantic runtime.

Generate bindings from the pinned SysML structure using its logical source URI:

```sh
cargo run --locked -p intersect-codegen -- --frozen \
  https://www.omg.org/spec/SysML/20250201 org.omg.sysml/model/SysML.ecore.rcore \
  /absolute/path/to/metamodel/SysML.ecore.rcore /tmp/sysml.rs
```

To generate, compile, and test the complete schema from a KerML checkout, pass
that same source file to the Rust bindings suite:

```sh
INTERSECT_SYSML_RCORE=/absolute/path/to/metamodel/SysML.ecore.rcore \
  python -B tools/test_report.py run -- --profile test-fast --locked \
  -p intersect-rust-bindings-tests --lib sysml_tests
```
