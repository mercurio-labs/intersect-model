# Architecture

Intersect Model owns: Native modeling, definitions, Rcore/Ecore import, execution, code generation and host adapters.

The dependency direction is host/application → language package → Model → definitions.
Model owns structural services, native resource storage and execution. Text
owns grammar compilation, inference, scoping adapters and text interpretation.
Language packages own domain algorithms, standard libraries and delegates.
Unsupported constructs remain explicit. Importing data does not qualify native
semantic support; imported, consumed and semantically verified are separate.

Model-only acceptance, Rust bindings, semantic bindings and shared Model test
support live in Model. Text acceptance, model/text parity, resource-lifecycle
integration, Library and Web live in Text; Toolkit lives in Examples. Production libraries do not
depend on harness support. The shared harness helpers are implemented once in
Model and consumed at the same Model Git revision as the runtime library.

Clocks, I/O, scheduling and host resources use explicit adapters. Libraries
remain compatible with wasm32-unknown-unknown; Java is an independent oracle,
never a production requirement. Generated and handwritten boundaries are marked.

See [repository development](repository-development.md) and the current
[release plan](release-plan.md) for source pairing and qualification.

## Consumer boundaries

| Layer | Responsibility | Start here |
| --- | --- | --- |
| Host/application | Physical I/O, cancellation, memory admission and publication | `intersect-project-host`, application adapters |
| Language package | Domain rules, standard library, scope and provider policy | Independent language repositories |
| Text | Grammar compilation, inference, linking adapters, documents and text edits | Intersect Text; optional for Model consumers |
| Model | Checked object/resource operations, storage, transactions and execution | `compile_rcore`, `api`, `resource_set`, `storage` |
| Definitions | Source graph, structural checking, materialization and behavior compilation | `definitions`; advanced public surface under release review |

Model's `core` and `definitions` are modules of one crate, not separate runtime
packages. Their logical dependency direction still matters. `intersect-run` is a
separate deterministic execution kernel. Execution hosts use
`intersect-run-catalog` for versioned engine IDs and capabilities, declare
which engines they install, and validate their language-specific bindings.
`intersect-build-support` runs on the build host to calculate compiler identities.

The [model guide](model-guide.md) explains facade selection and atomicity. The
[Rcore guide](rcore-guide.md) distinguishes direct package execution, frozen
bindings and provider adapters. Metadata registration, type checking and runtime
activation are distinct steps. The [release plan](release-plan.md)
identifies which public contracts need qualification before compatibility freezes.

## Released runtime adapters

Unpublished host/example adapters consume the exact released Model/Text crates.
Model owns `intersect-project-host` and `intersect-model-example-support`.
Visualization contracts and the client moved to the independent local
`intersect-views` repository in view-engine M10, after consumer qualification.
The example adapter compiles the
existing shared binding fixture sources against the registry Model types; it
does not duplicate their implementations. Text Library/Web adapters likewise use
registry runtime types. Source qualification harnesses retain their original
workspace/Git runtime identities. Ordinary downstream builds use no local patches.
