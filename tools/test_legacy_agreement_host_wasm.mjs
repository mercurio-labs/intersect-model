// Preserve the release example bytes outside Cargo caches and pass that exact path.
// Both fresh zero-import instances exercise the same public Rust host consumers.
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
const path = process.argv[2];
assert.ok(path, 'Pass the preserved legacy_agreement_host_probe.wasm path');
const bytes = readFileSync(path);
const sha256 = createHash('sha256').update(bytes).digest('hex');
const module = await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), []);
const results = [];
for (let fresh = 0; fresh < 2; fresh++) {
  const instance = await WebAssembly.instantiate(module, {});
  const groups = instance.exports.verify_legacy_agreement_host_consumers();
  assert.equal(groups, 5);
  results.push({ fresh, groups });
}
console.log(JSON.stringify({ path, sha256, imports: [], results,
  scope: 'public compatibility and explicit retained agreement host consumers; full S09 remains open' }, null, 2));
