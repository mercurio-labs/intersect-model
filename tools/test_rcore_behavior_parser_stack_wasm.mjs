// Same complete parser documents in fresh WASM with an explicit one-MiB stack.
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const root=fileURLToPath(new URL('../',import.meta.url));
const built=spawnSync('cargo',['rustc','-p','intersect-model-acceptance','--example','rcore_behavior_parser_stack_probe','--target','wasm32-unknown-unknown','--locked','--','-C','link-arg=-zstack-size=1048576'],{cwd:root,stdio:'inherit'});
assert.equal(built.status,0,built.error?.message);
const target=resolve(root,process.env.CARGO_TARGET_DIR||'target');
const module=await WebAssembly.compile(readFileSync(resolve(target,'wasm32-unknown-unknown/debug/examples/rcore_behavior_parser_stack_probe.wasm')));
assert.deepEqual(WebAssembly.Module.imports(module),[]);
const instance=await WebAssembly.instantiate(module,{});
assert.equal(instance.exports.verify_rcore_behavior_parser_stack(),8);
console.log('Eight complete pre-repair parser documents match in fresh zero-import WASM with explicit 1048576-byte stack; full S09 remains open.');
