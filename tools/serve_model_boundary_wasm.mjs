// Local browser runner for the same model-only WASM export used by the Node probe.
// Only these two test endpoints are served; repository files are not exposed.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';

const path = resolve(process.argv[2] || 'target/model-only/wasm32-unknown-unknown/debug/intersect_model_only_consumer.wasm');
const bytes = await readFile(path);
const digest = createHash('sha256').update(bytes).digest('hex');
const html = `<!doctype html><meta charset="utf-8">
<title>Model-only WASM browser controls</title>
<h1>Model-only WASM browser controls</h1>
<p>Source getters, construction, reload, rejection and caller budgets.</p>
<button id="run">Run controls</button>
<pre id="result">Ready</pre>
<script type="module">
const button = document.querySelector('#run');
const result = document.querySelector('#result');
button.addEventListener('click', async () => {
  button.disabled = true;
  result.textContent = 'Running';
  try {
    const response = await fetch('/model.wasm');
    if (!response.ok) throw new Error('WASM fetch failed: ' + response.status);
    const module = await WebAssembly.compile(await response.arrayBuffer());
    const imports = WebAssembly.Module.imports(module);
    if (imports.length !== 0) throw new Error('Unexpected host imports');
    const instance = await WebAssembly.instantiate(module, {});
    const code = instance.exports.verify_model_boundary();
    if (code !== 0) throw new Error('Control export returned ' + code);
    result.dataset.status = 'passed';
    result.textContent = JSON.stringify({status:'passed', export:'verify_model_boundary', code, imports, wasmSha256:'${digest}'}, null, 2);
  } catch (error) {
    result.dataset.status = 'failed';
    result.textContent = String(error);
  } finally { button.disabled = false; }
});
</script>`;
const server = createServer((request, response) => {
  if (request.url === '/') {
    response.writeHead(200, {'Content-Type':'text/html; charset=utf-8', 'Cache-Control':'no-store'});
    response.end(html);
  } else if (request.url === '/model.wasm') {
    response.writeHead(200, {'Content-Type':'application/wasm', 'Cache-Control':'no-store'});
    response.end(bytes);
  } else {
    response.writeHead(404);
    response.end();
  }
});
server.listen(0, '127.0.0.1', () => {
  const address = server.address();
  if (address && typeof address === 'object') {
    console.log(`Model-only browser controls: http://127.0.0.1:${address.port}/`);
    console.log(`WASM SHA-256: ${digest}`);
  }
});
