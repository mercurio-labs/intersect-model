# Released-Model browser example support

This unpublished adapter compiles the existing shared `contract` and `native`
binding examples against `intersect-model = "=0.1.0-alpha.1"`. The source files
are shared with `tests/support`, whose qualification helpers continue to test the
workspace runtime. There is one implementation of each fixture and native function.

The Text browser host uses this crate so its example bindings have the same Rust
type identity as the released Model runtime used by KerML and registry consumers.
The adapter is fetched from the public Model repository at an exact revision;
it is not an additional crates.io product.
