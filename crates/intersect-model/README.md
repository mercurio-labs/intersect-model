# intersect-model

Shared model construction, validation, persistence contracts, and Rcore execution
for Rust and WebAssembly. Rcore is the authored definition language. Ecore
migration produces editable Rcore and an explicit conversion report; unsupported
constructs require manual work.

```rust
use intersect_model::compile_rcore;

let package = compile_rcore(r#"
rcore 1;
package example version "1";
metamodel "urn:example" prefix example {
    class Item { attr name: String [1,1]; }
}
"#)?;
let mut model = package.new_model()?;
model.create("item-1", "Item", "memory:items")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Text-language compilation and interpretation belong to `intersect-text`, which
depends on this package's checked model services. This package has no grammar
backend or text-compiler feature. Optional features expose `xmi`, native `sqlite`,
and `codegen` modules.
The optional `rhai` feature adds checked script providers through
`semantic_bindings::rhai`, using the shared source-authorized contracts.
SQLite is a native host adapter; the shared runtime supports
`wasm32-unknown-unknown` without Java.

The `definitions` and `core` modules retain their logical layering inside this
package. Generated Rust bindings use the same model services. Source identities,
operation implementation selection, and execution limits remain explicit.

Run `cargo run --example rcore_model` for construction, behavior binding and
save/reload, or `cargo run --example dynamic_library` for programmatic schemas.

Application code starts with `dynamic::DynamicModel` and
`definitions::dynamic::SchemaBuilder`, or `compile_rcore` / `RcorePackage`.
Use `api` for reflective editing and `resource_set` / `interchange` for resources.
The rustdoc `extensions` module documents advanced storage, preparation and
provider contracts. Its `SourceReadCallbacks` and `LOGICAL_WORD` exports replace
the former `source_read_callbacks` and `logical_size` module paths; those modules
are private. The root `logical_size!` macro remains a downstream accounting tool.
Specialized contracts retain their documented owning modules, without duplicate
blanket re-exports. See
[API boundaries](../../docs/model-guide.md#application-and-extension-boundaries).


### Supplemental source reads

`source_model_functions::SourceModelFunction` attaches explicitly selected
read functions to the retained Rcore package or prepared definition. Use
`from_rcore_many` or `from_prepared_many` to compile several entries together;
results preserve selection order and share immutable definition sources.
The existing single-entry constructors remain available. Every image is freshly
verified and signature-checked against the exact retained schema. A failed batch
returns no functions. Entry count and aggregate selector metadata are bounded
before copying. `invoke_model` retains its caller's query budget and runtime
refusal of construction; batch compilation installs no language semantics.
