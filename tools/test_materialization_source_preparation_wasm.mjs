// Verification-only host; all preparation assertions execute in shared Rust.
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const root=fileURLToPath(new URL('../',import.meta.url));
const build=spawnSync('cargo',['build','-p','intersect-model-acceptance','--example','materialization_source_preparation_probe','--target','wasm32-unknown-unknown','--locked'],{cwd:root,stdio:'inherit'});
assert.equal(build.status,0,build.error?.message);
const target=resolve(root,process.env.CARGO_TARGET_DIR||'target');
const {instance}=await WebAssembly.instantiate(readFileSync(resolve(target,'wasm32-unknown-unknown/debug/examples/materialization_source_preparation_probe.wasm')),{});
assert.equal(instance.exports.verify_materialization_source_preparation(),4);
console.log('Four actual materialization source groups passed in real WASM.');
