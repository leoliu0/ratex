// Exercise the browser-target JavaScript glue without requiring an HTTP server.
// The CI browser smoke test separately loads examples/wasm/smoke.html.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { initSync, TexSession } from '../target/wasm/web/tex.js';

const wasm = fs.readFileSync(new URL('../target/wasm/web/tex_bg.wasm', import.meta.url));
initSync({ module: wasm });
const encoder = new TextEncoder();
const session = new TexSession();
session.setEpoch(1700000000);
session.addFile('main.tex', encoder.encode(String.raw`\documentclass{article}\usepackage{amsmath}\begin{document}Browser glue $x^2$.\end{document}`));
const result = session.compile('main.tex');
assert.equal(result.status, 0, result.diagnostics + '\n' + result.log);
assert.equal(new TextDecoder().decode(result.pdf.slice(0, 5)), '%PDF-');
assert.ok(result.fileNames.includes('main.aux'));
console.log(`Wasm web glue: PDF ${result.pdf.length} bytes, ${result.passes} passes`);
result.free();
session.free();
