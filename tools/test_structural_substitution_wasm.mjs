// Actual production public consumers; no private snapshot instrumentation.
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const root=fileURLToPath(new URL('../',import.meta.url));
const build=spawnSync('cargo',['build','-p','intersect-model','--example','structural_substitution_probe','--target','wasm32-unknown-unknown','--locked'],{cwd:root,stdio:'inherit'});
assert.equal(build.status,0,build.error?.message);
const target=resolve(root,process.env.CARGO_TARGET_DIR||'target');
const module=await WebAssembly.compile(readFileSync(resolve(target,'wasm32-unknown-unknown/debug/examples/structural_substitution_probe.wasm')));
assert.deepEqual(WebAssembly.Module.imports(module),[]);
for(let i=0;i<2;i++){
 const instance=await WebAssembly.instantiate(module,{});
 assert.equal(instance.exports.verify_structural_substitution_consumers(),3);
}
console.log('Three public structural substitution consumer groups passed in each of two fresh zero-import WASM instances; full S09 remains open.');
