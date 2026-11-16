# Intersect Model

**Define a modeling language, build models, and execute their behavior in Rust and WebAssembly.**

Intersect Model was designed around the same conceptual ideas as the
[Eclipse Modeling Framework (EMF)](https://eclipse.dev/emf/): define a metamodel,
create instances, and share model services across tools. It brings those ideas to
Rust and WebAssembly, with authored Rcore definitions and explicit execution limits.

It is the language-independent foundation for Intersect. It provides
shared services for objects, attributes, references, containment, validation,
transactions, persistence, and Rcore behavior. Its goal is to let typed APIs,
reflective tools, text editors, and applications work with the same model semantics.

Use it to build a domain modeling tool, embed a model runtime in an application,
or give an existing metamodel a native implementation. Production execution does
not require Java.

## What it provides

- **Authored definitions:** Rcore describes model structure and supported behavior.
  Ecore migration produces editable Rcore with an explicit conversion report.
- **Checked model operations:** create and edit objects, maintain references and
  containment, validate models, and group changes into transactions.
- **Reusable execution:** compile and invoke supported Rcore operations through
  the shared runtime, with explicit execution limits and source identities.
- **Integration options:** generated Rust bindings, optional XMI interchange,
  native SQLite storage, and Rhai providers.

The objective is consistent behavior across native and browser consumers.
Support is qualified by API, feature, and execution path; importing a definition
does not mean all of its semantics are executable.

## Install

The first registry release is `0.1.0-alpha.1`, for evaluation and feedback:

```toml
[dependencies]
intersect-model = "=0.1.0-alpha.1"
serde_json = "1"
```

APIs may change between alpha releases. Optional features are `xmi`, native-only
`sqlite`, `codegen`, and `rhai`. The default runtime also supports WebAssembly.

## A small model

Both examples define an `Item` with one required name, create `item-1`, set its
name to `First item`, and save the valid model.

### Define a DynamicSchema in Rust

Use `SchemaBuilder` to construct a `DynamicSchema` and retain handles for its
classes and features. `DynamicModel` checks edits at the transaction boundary.

```rust
use intersect_model::{
    definitions::dynamic::{ScalarType, SchemaBuilder},
    dynamic::DynamicModel,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = SchemaBuilder::new("example", "urn:example");
    let item = builder.class("Item")?;
    let name = item.attribute("name", ScalarType::String).required()?;
    let schema = builder.build()?; // DynamicSchema
    let mut model = DynamicModel::new(schema)?;
    model.transaction(|tx| {
        let mut resource = tx.create_resource("memory:items")?;
        let object = resource.create(&item, "item-1")?;
        resource.set(&object, &name, "First item")?;
        Ok(())
    })?;
    let saved = model.to_json()?;
    println!("Saved a valid model ({} bytes)", saved.len());
    Ok(())
}
```

### Define the same schema in Rcore

Rcore makes the definition an editable source file. `compile_rcore` produces a
package whose models use the same checked structural services.

```rust
use intersect_model::{api::ModelEdit, compile_rcore};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let package = compile_rcore(r#"
        rcore 1;
        package example version "1";
        metamodel "urn:example" prefix example {
            class Item { attr name: String [1,1]; }
        }
    "#)?;

    let mut model = package.new_model()?;
    model.create("item-1", "Item", "memory:items")?;
    model.set("item-1", "name", serde_json::json!("First item"))?;
    model.validate()?;
    let saved = package.save(&model)?;
    println!("Saved a valid model ({} bytes)", saved.len());
    Ok(())
}
```

The second snippet also uses `serde_json`. Run both alternatives from this
checkout:

```sh
cargo run -p intersect-model --example schema_choices --locked
```

For an Rcore example with operation execution and save/reload:

```sh
cargo run -p intersect-model --example rcore_model --locked
```

See also [programmatic schema construction](crates/intersect-model/examples/dynamic_library.rs)
and the [model crate guide](crates/intersect-model/README.md).

## How this relates to EMF

| Concept | EMF | Intersect Model |
| --- | --- | --- |
| Metamodel | Ecore | DynamicSchema or authored Rcore; explicit Ecore migration |
| Model access | Generated Java and reflective APIs | Rust handles, checked reflection and generated bindings |
| Model services | References, containment, validation and persistence | Shared structural services, transactions and explicit providers |
| Interchange | XMI persistence | Optional, bounded XMI support; separate native snapshots |
| Tool integration | Java/Eclipse tooling, including EMF.Edit | Native Rust and WASM consumers; browser tools live in Examples |

This is conceptual alignment, not Java API or file-format compatibility with every
EMF extension. See the [EMF project](https://eclipse.dev/emf/) and our
[support boundaries](docs/model-guide.md).

## Runnable browser example

Hosting is currently disabled. Download **examples-site.zip** from the
[verified preview build](https://github.com/mercurio-labs/intersect-lang-examples/releases/tag/examples-preview-2026-11-13), unzip it,
then run `python -m http.server 8000` in the extracted directory and open
`http://localhost:8000/`. The site links below are reserved for future hosting.
Future successful Browser examples workflow runs also provide an `examples-site`
artifact; the preview download is built and checked locally.

[Open the interactive example site](https://mercurio-labs.github.io/intersect-lang-examples/) to run the Rust/WASM examples.
The [Library simulator](https://mercurio-labs.github.io/intersect-lang-examples/examples/library/) supports object edits,
reference changes and synchronized source text with undo/redo. Petri pairs a
model with animated simulation; Catalog explores cross-resource references.
The [Toolkit source and build instructions](https://github.com/mercurio-labs/intersect-lang-examples/tree/develop/toolkit)
live in the examples repository.

## How the projects fit together

**Applications → language packages → Intersect Text → Intersect Model.**
Applications and language packages can also use Model directly, without Text.

[Intersect Text](https://github.com/mercurio-labs/intersect-text) adds Xtext grammar
compilation, parsing, linking, and model-to-text editing. The
[KerML/SysML package](https://github.com/mercurio-labs/intersect-lang-kerml) owns
those languages' semantics and standard libraries. Model does not depend on either.

Within this repository, `intersect-model` is the main runtime,
`intersect-build-support` computes compiler identities, `intersect-codegen`
supports binding generation, `intersect-project-host` defines host acquisition
contracts, and `intersect-run` is a deterministic discrete-event kernel.
`intersect-run` is a library, not a model-loading command-line application.

## Status and documentation

The repositories are preparing their first release after the Model/Text split.
Use the
[current release plan](docs/release-plan.md) for the new scope and open gates.
The registry release set is `intersect-build-support`, `intersect-model`, and
`intersect-text`; the support crate runs only during builds. See the
[package and API scope](docs/release-plan.md#package-count-and-public-api-decision).
The [registry release procedure](docs/registry-release.md) explains standalone
packages, retained upstream notices, and release verification.

- [Architecture](docs/architecture.md) and [model guide](docs/model-guide.md)
- [Rcore guide](docs/rcore-guide.md)
- [Documentation index](docs/README.md)
- [Repository development and dependency setup](docs/repository-development.md)
- [Baseline provenance](docs/baseline-provenance.json)

## Contributing

The minimum Rust version is **1.88** (edition 2024). The development toolchain
remains pinned separately below. See [repository development](docs/repository-development.md)
for the minimum-version and isolated artifact checks.

Use `develop` as the explicit branch base. Install Rust 1.99.1, Python 3.11+,
Node 22.15.1 for WASM execution, and the `wasm32-unknown-unknown` Rust target.
Run `cargo fetch --locked` once; ordinary builds use the committed lockfile.

### Test tiers

After a change, run only changed crates, without `milestone-tests`:

```sh
python -B tools/format_workspace.py --check
cargo clippy -p intersect-model --all-targets --locked -- -D warnings
python -B tools/test_report.py run -- --profile test-fast -p intersect-model --lib --tests --locked
cargo check -p intersect-model --lib --target wasm32-unknown-unknown --locked
```

Quick checks must finish in under ten minutes, build included. Whole-library,
language equivalence, binding rebuilds, Pilot and full workspace checks are
milestone-only. Preserve existing case names, assertions and ignore statuses.
The report dashboard is http://localhost:8765/.

Hosted CI must be qualified on the final baseline; workflow definitions
and skipped platform checks are not passing evidence.
Registry publication is a separate operation.

## License

Intersect-authored code uses [MIT](LICENSE). Imported fixtures and generated
Unicode data retain their upstream terms. Source bundles include a third-party
notice index; see [notice coverage](docs/repository-development.md#source-bundle-notices).
