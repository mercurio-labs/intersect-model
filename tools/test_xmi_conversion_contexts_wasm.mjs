import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
const root=fileURLToPath(new URL('../',import.meta.url));
const built=spawnSync('cargo',['build','-p','intersect-model','--features','xmi','--example','xmi_conversion_probe','--target','wasm32-unknown-unknown','--locked'],{cwd:root,stdio:'inherit'});
if(built.status!==0)process.exit(built.status||1);
const target=process.env.CARGO_TARGET_DIR||resolve(root,'target');
const module=await WebAssembly.compile(readFileSync(resolve(target,'wasm32-unknown-unknown/debug/examples/xmi_conversion_probe.wasm')));
assert.deepEqual(WebAssembly.Module.imports(module),[]);
for(let i=0;i<2;i++){
  const instance=await WebAssembly.instantiate(module,{});
  assert.equal(instance.exports.verify_xmi_conversion_contexts(),7);
}
console.log('Seven shared source-bound XMI lexical-provider groups passed in each of two fresh zero-import WASM instances. Single-package native codec only; inferred aliases, browser adoption, provider-aware resource sets and broader qualification remain open.');
