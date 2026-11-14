import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
const root=fileURLToPath(new URL('../',import.meta.url));
const built=spawnSync('cargo',['build','-p','intersect-model','--example','converted_query_probe','--target','wasm32-unknown-unknown','--locked'],{cwd:root,stdio:'inherit'});
if(built.status!==0)process.exit(built.status||1);
const target=process.env.CARGO_TARGET_DIR||resolve(root,'target');
const module=await WebAssembly.compile(readFileSync(resolve(target,'wasm32-unknown-unknown/debug/examples/converted_query_probe.wasm')));
assert.deepEqual(WebAssembly.Module.imports(module),[]);
for(let i=0;i<2;i++){const instance=await WebAssembly.instantiate(module,{});assert.equal(instance.exports.verify_converted_query_columns(),4);}
console.log('Four shared exact nominal-query column groups passed in each of two fresh zero-import WASM instances. Converted columns only; arbitrary nominal comparisons, ordering, generic/applied values and broader qualification remain open.');
