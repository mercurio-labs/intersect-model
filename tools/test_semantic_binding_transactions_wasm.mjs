import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve, join } from 'node:path';
const [wasmPath, goldenPath, outputPath] = process.argv.slice(2).map(p => resolve(p));
assert.ok(wasmPath && goldenPath && outputPath);
await mkdir(outputPath, { recursive: false });
const bytes = await readFile(wasmPath), goldenBytes = await readFile(goldenPath);
const native = JSON.parse(goldenBytes);
const firstNative = JSON.parse(await readFile(new URL('../fixtures/rcore/bindings/transactions/native-observations.json', import.meta.url)));
assert.deepEqual(native, firstNative, 'Fresh native report must equal the frozen first-native complete observations');
const module = await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), []);
const hash = data => createHash('sha256').update(data).digest('hex');
const modelValue = (model, id) => model.elements.find(element => element.id === id)?.values.value;
assert.equal(native.cases.length, 10);
const names = native.cases.map(row => row.case);
assert.equal(new Set(names).size, names.length);
assert.deepEqual(names, ['commit', 'semantic', 'result', 'postcondition', 'exhaustion', 'arguments', 'cancel_before_body', 'structural_commit', 'language_commit', 'rust_semantic_after_rhai']);
for (const row of native.cases) {
  assert.equal(row.observed.rust_result, 44);
  assert.deepEqual(modelValue(row.before, 'meter'), {kind:'int64',value:'21'});
  assert.deepEqual(modelValue(row.observed.pending, 'meter'), {kind:'int64',value:'22'});
  assert.deepEqual(modelValue(row.observed.pending, 'pending'), {kind:'int64',value:'7'});
  assert.ok(row.binding_work > 0);
  assert.equal(row.observed.pending.elements.find(e=>e.id==='pending').resource,'memory:pending-transaction');
  assert.equal(row.before.resources['memory:pending-transaction'],undefined);
  assert.ok(row.observed.pending.resources['memory:pending-transaction']);
  if (row.case === 'rust_semantic_after_rhai') {
    assert.equal(row.observed.rhai_result,44);
    assert.deepEqual(row.observed.rust_error,{kind:'Semantic',message:'fixture native rejection after pending query'});
    assert.equal(row.transaction.Err,'expected native semantic failure');
  }
  if (row.case === 'structural_commit') {
    assert.equal(row.observed.edit_callback_return,44);
    assert.equal(row.observed.before_structural_validation.elements.length,3);
    const guard=row.observed.before_structural_validation.elements.find(e=>e.id==='missing_required');
    assert.ok(guard);assert.equal(guard.resource,'memory:transaction');assert.equal(guard.values.required,null);
    assert.equal(row.transaction.Err,`Multiplicity violation: ${guard.class}.required`);
  }
  if (row.case === 'commit') {
    assert.deepEqual(row.transaction, {Ok:44});
    assert.equal(row.observed.rhai_result,44);
    assert.equal(row.after.elements.length,2);
    assert.deepEqual(modelValue(row.after,'meter'),{kind:'int64',value:'22'});
    assert.deepEqual(modelValue(row.after,'pending'),{kind:'int64',value:'7'});
  } else {
    assert.equal(typeof row.transaction.Err,'string');
    assert.deepEqual(row.after,row.before,`${row.case}: complete model and resources must roll back`);
    assert.equal(row.after.resources['memory:pending-transaction'],undefined);
    assert.ok(row.recovery_after_rollback);
    assert.deepEqual(modelValue(row.recovery_after_rollback,'meter'),{kind:'int64',value:'23'});
    assert.equal(row.recovery_after_rollback.elements.length,1);
    assert.equal(row.recovery_after_rollback.resources['memory:pending-transaction'],undefined);
  }
}
const runs=[];
for(let index=0;index<2;index++) {
  const instance=await WebAssembly.instantiate(module,{});
  const ptrLen=instance.exports.intersect_transaction_qualification_report();
  const pointer=Number(ptrLen & 0xffffffffn),length=Number(ptrLen >> 32n);
  assert.ok(pointer>0 && length>0);
  const raw=Buffer.from(new Uint8Array(instance.exports.memory.buffer,pointer,length));
  await writeFile(join(outputPath,`fresh-${index}.json`),raw,{flag:'wx'});
  const report=JSON.parse(raw);
  assert.ok(report.Ok,JSON.stringify(report));
  assert.deepEqual(report.Ok,native,'All transaction observations must agree with native execution');
  runs.push({fresh_instance:index,bytes:raw.length,sha256:hash(raw),cases:report.Ok.cases.length});
}
const shared=await WebAssembly.instantiate(module,{});
assert.equal(shared.exports.intersect_bindings_qualification(),0);
const result={imports:[],wasm:{path:wasmPath,bytes:bytes.length,sha256:hash(bytes)},native:{path:goldenPath,bytes:goldenBytes.length,sha256:hash(goldenBytes)},runs,existing_shared_qualification:0};
await writeFile(join(outputPath,'receipt.json'),JSON.stringify(result,null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:native.cases.length,fresh_instances:runs.length,existing_shared_qualification:0,wasm_sha256:result.wasm.sha256}));
