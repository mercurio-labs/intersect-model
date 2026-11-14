// Production public consumers only. Build into a NEW dedicated target, preserve
// the resulting bytes outside Cargo caches, and pass that immutable path here.
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
const path=process.argv[2];
assert.ok(path, 'Pass the preserved checked_normalization_probe.wasm path');
const bytes=readFileSync(path);
const sha256=createHash('sha256').update(bytes).digest('hex');
const module=await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), []);
const results=[];
for (let fresh=0;fresh<2;fresh++) {
  const instance=await WebAssembly.instantiate(module, {});
  const groups=instance.exports.verify_checked_normalization_consumers();
  assert.equal(groups,3);
  results.push({fresh,groups});
}
console.log(JSON.stringify({path,sha256,imports:[],results,scope:'production public consumers; full S09 remains open'},null,2));
