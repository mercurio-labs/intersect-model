// Verification-only host; shared Rust controls compare source admission and fresh jobs.
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const root=fileURLToPath(new URL('../',import.meta.url));
const build=spawnSync('cargo',['build','-p','intersect-model-acceptance','--example','rcore_enum_queries_probe','--target','wasm32-unknown-unknown','--locked'],{cwd:root,stdio:'inherit'});
assert.equal(build.status,0,build.error?.message);
const target=resolve(root,process.env.CARGO_TARGET_DIR||'target');
const module=await WebAssembly.compile(readFileSync(resolve(target,'wasm32-unknown-unknown/debug/examples/rcore_enum_queries_probe.wasm')));
assert.deepEqual(WebAssembly.Module.imports(module),[]);
const instance=await WebAssembly.instantiate(module,{});
assert.equal(instance.exports.verify_rcore_enum_queries(),3);
console.log('Three enum import/binding/typing consumer groups passed in zero-import WASM; full compiler/S09 remains open.');
