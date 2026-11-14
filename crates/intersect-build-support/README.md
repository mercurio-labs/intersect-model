# Intersect build support

This package runs only on the build host. Model and Xtext use it through
`[build-dependencies]`; production model/Xtext libraries do not depend on its
filesystem or environment operations.

`emit` hashes the owning package's manifest, build script, source tree, literal
embedded inputs, enabled Cargo features and this scanner's implementation. It
does not locate or scan a runtime dependency's source checkout. Available
ancestor lock/manifest context conservatively invalidates keys; a package does
not need a surrounding workspace or lockfile to build. File bytes and framed
relative paths are deterministic. Unsupported dynamic embedding and missing
inputs fail explicitly.

Source identity recipe 3 replaces the workspace scanner under `tools/build`.
The package identity covers owned inputs; language packages compose installed
dependency identities in their libraries. Xtext exposes `compiler_identity`
and `compiler_cache_identity` for that purpose. These are cache keys for a
host's own compiler output, not signatures or admission capabilities.

The scanner intentionally includes literal references in test sources and
comments, as the previous scanner did. Distribution staging must therefore
carry the complete embedded-input closure, including exact fixture bytes,
rather than relying on the original repository being present.
