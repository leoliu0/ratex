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
session.addFile('main.tex', encoder.encode(String.raw`\documentclass{article}
\usepackage{fontspec}
\setmainfont{Latin Modern Roman}
\begin{document}
Browser Wasm glue: \textbf{Bold glyphs} \& \textit{Italic shapes}.
\end{document}`));
const result = session.compile('main.tex');
assert.equal(result.status, 0, result.diagnostics + '\n' + result.log);
assert.equal(new TextDecoder().decode(result.pdf.slice(0, 5)), '%PDF-');
assert.ok(result.fileNames.includes('main.aux'));
fs.writeFileSync(new URL('../target/wasm/web-hello.pdf', import.meta.url), result.pdf);
console.log(`Wasm web glue: native-font PDF ${result.pdf.length} bytes, ${result.passes} passes`);
result.free();
session.free();
