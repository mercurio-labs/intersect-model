import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const path = resolve(process.argv[2] || 'target/wasm32-unknown-unknown/debug/intersect_semantic_bindings_tests.wasm');
const module = await WebAssembly.compile(await readFile(path));
assert.deepEqual(WebAssembly.Module.imports(module), [], 'Bindings require no JavaScript/Java host imports');
const instance = await WebAssembly.instantiate(module, {});
assert.equal(instance.exports.intersect_bindings_qualification(), 0, 'Checked static/dynamic Rcore bindings failed');
console.log('F10–F12 checked contracts, generated/manual Rust functions and model operations, pinned sessions, and typed collections/options passed in WASM.');
