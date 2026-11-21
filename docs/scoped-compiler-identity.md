# Scoped compiler source identity

The user approved the separate shared-loader/cache correction on 2026-11-18.
Its Model change is confined to host-only build support, tests, documentation and
maintained locks. Model runtime APIs are unchanged by this correction.

`intersect_build_support::emit` retains the conservative workspace-context policy.
Opt-in `emit_scoped` reuses its owned-input scanner and hashes the resolved owning
package's normal/build dependency closure. Registry inputs use locked checksums;
resolved Git/path packages use actual owned source bytes. A repository revision
can change for a sibling package, so a repository-wide revision is not itself the
owned package's semantic identity. Ambiguous duplicate package identities refuse.

Owning enabled features are checked against the actual Cargo build-script
environment. Dependency features and target-conditional edges come from locked
Cargo metadata. The bounded native/WASM portability profile includes every
target-conditional dependency alternative and its condition in the key. It checks
the actual requested target separately. This deliberately supports the same
portable compiled-language data in native pack builders and WASM; actual language
pack equivalence must still be qualified before integration is called complete.
Targets other than the native host and wasm32-unknown-unknown are refused.

Cargo metadata preparation occurs outside build scripts, before the build lock:

```text
python -B tools/compiler_build.py -- check -p PACKAGE --locked --offline
python -B tools/compiler_build.py --report -- test -p PACKAGE --profile test-fast --locked --offline
```

The wrapper creates a new private temporary context for that child only, with
matching feature flags and the full locked resolution. It watches all resolved
manifests, the workspace manifest/lock, and present/absent Cargo configuration
paths. These are freshness guards, separate from the semantic cache key. Stale,
missing, incomplete, wrong-target and wrong-feature contexts refuse. Direct Cargo
without the wrapper refuses for opted-in crates. Custom rustflags, rustc wrappers
and command-line Cargo configuration overrides are outside the admitted profile
and refuse rather than guessing their effects. Unchanged conservative callers
do not require this wrapper.

The source scanner retains exact bytes, recursively tracks literal embedded
inputs and external path modules, and rejects dynamic embedded paths. Its lexical
pass distinguishes actual macro/attribute tokens from Rust examples in comments,
normal/raw strings and character literals. Build-script embedded inputs are
tracked as well. The scanner, policy and preparation tool participate in the key.

Qualification so far: nine quick unit tests and strict all-target lint passed.
The independent `tools/test_scoped_build.py` runs real Cargo and actual WASM
builds. Its final receipt is recorded in docs/evidence. Fixture results are not a
claim that the extracted loader, full language pack, or consumer is qualified.

The correction keeps the qualified Model 9ebf0d1 source baseline. Current develop
d95bfd0 contains separately developed immutable-component APIs; incorporating
those runtime changes is not required by this correction. The initial isolated
develop-based worktree and carried prerequisite were inspected, then its new
correction commit was moved onto the recorded baseline. Original branches,
checkouts and the newer develop work remain intact.
