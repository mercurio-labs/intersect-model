// Verification-only host; shared Rust checks every bundle rule and cancellation boundary.
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const root=fileURLToPath(new URL('../',import.meta.url));
const build=spawnSync('cargo',['build','-p','intersect-model-acceptance','--example','bundle_validation_probe','--target','wasm32-unknown-unknown','--locked'],{cwd:root,stdio:'inherit'});
assert.equal(build.status,0,build.error?.message);
const target=resolve(root,process.env.CARGO_TARGET_DIR||'target');
const {instance}=await WebAssembly.instantiate(readFileSync(resolve(target,'wasm32-unknown-unknown/debug/examples/bundle_validation_probe.wasm')),{});
assert.equal(instance.exports.verify_bundle_validation(),6);
console.log('Six retained bundle validation/source-copy groups passed in real WASM.');
