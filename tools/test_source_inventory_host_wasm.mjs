// Verification only. Compare actual shared Rust WASM output with frozen FIRST native rows.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const wasmPath = process.argv[2], nativePath = process.argv[3];
assert(wasmPath && nativePath, 'Provide immutable WASM and frozen first native JSONL paths');
const bytes = fs.readFileSync(wasmPath), nativeBytes = fs.readFileSync(nativePath);
const decoder = new TextDecoder('utf-8', { fatal: true });
const native = decoder.decode(nativeBytes).trimEnd().split('\n').map(line => JSON.parse(line));
assert.equal(native.length, 9);
const module = await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), []);
const groups = [];
for (let fresh = 0; fresh < 2; fresh++) {
  const { exports: w } = await WebAssembly.instantiate(module, {});
  assert.equal(w.verify_source_inventory_host_consumers(), 10);
  groups.push({ fresh, legacy_groups: 5, inventory_groups: 5 });
}
let checked = 0;
for (const mode of [0, 1, 2]) {
  for (const quantum of [1, 3, 256]) {
    const { exports: w } = await WebAssembly.instantiate(module, {});
    const length = w.capture_source_inventory_host_row(mode, quantum);
    assert(length > 0);
    const value = JSON.parse(decoder.decode(new Uint8Array(w.memory.buffer, w.source_inventory_host_output_buffer(), length)));
    assert.equal(value.ok, true);
    assert.deepEqual(value.row, native[checked], `complete native/WASM row mode=${mode} quantum=${quantum}`);
    checked++;
  }
}
console.log(JSON.stringify({ wasm: wasmPath, wasm_sha256: sha(bytes), first_native_rows: nativePath, first_native_sha256: sha(nativeBytes), imports: [], groups, complete_row_comparisons: checked, fresh_instances: 11, scope: 'Actual public compatibility/current-B/C host consumers. Empty registry; no provider-bearing stale-every-poll or full S09 award.' }));
