// Verification only: actual public A/B/C/D consumers and complete first-native rows.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
const sha = value => createHash('sha256').update(value).digest('hex');
const [wasmPath,nativePath,outputPath] = process.argv.slice(2);
assert(wasmPath && nativePath && outputPath,'Provide immutable WASM, first native JSONL and NEW output directory');
fs.mkdirSync(outputPath,{recursive:false});
const bytes=fs.readFileSync(wasmPath),nativeBytes=fs.readFileSync(nativePath);
const decoder=new TextDecoder('utf-8',{fatal:true});
const native=decoder.decode(nativeBytes).trimEnd().split('\n').map(line=>JSON.parse(line));
assert.equal(native.length,12);
const module=await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module),[]);
for(let fresh=0;fresh<2;fresh++) {
  const {exports:w}=await WebAssembly.instantiate(module,{});
  assert.equal(w.verify_generic_source_host_consumers(),15);
}
let checked=0;
for(const mode of [0,1,2,3]) for(const quantum of [1,3,256]) {
  const {exports:w}=await WebAssembly.instantiate(module,{});
  const length=w.capture_generic_source_host_row(mode,quantum);
  assert(length>0);
  const raw=Buffer.from(new Uint8Array(w.memory.buffer,w.generic_source_host_output_buffer(),length));
  const value=JSON.parse(decoder.decode(raw));
  fs.writeFileSync(path.join(outputPath,`mode-${mode}-quantum-${quantum}.json`),raw,{flag:'wx'});
  assert.equal(value.ok,true);
  assert.deepEqual(value.row,native[checked],`complete actual public-host row mode=${mode} quantum=${quantum}`);
  checked++;
}
const receipt={wasm:{path:wasmPath,bytes:bytes.length,sha256:sha(bytes)},first_native:{path:nativePath,bytes:nativeBytes.length,sha256:sha(nativeBytes)},imports:[],fresh_qualifier_instances:2,verification_groups_each:15,fresh_row_instances:12,complete_row_comparisons:checked,scope:'Actual public A/B/C/D constructor consumers, empty registry, no internal-phase attribution, candidate only; full S09 remains open'};
fs.writeFileSync(path.join(outputPath,'receipt.json'),JSON.stringify(receipt,null,2),{flag:'wx'});
console.log(JSON.stringify(receipt));
