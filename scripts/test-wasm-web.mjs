// Exercise the browser-target JavaScript glue without requiring an HTTP server.
// The CI browser smoke test separately loads examples/wasm/smoke.html.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

// Match scripts/build-libs.sh, which writes below $CARGO_TARGET_DIR when set.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const web = path.join(path.resolve(root, process.env.CARGO_TARGET_DIR || 'target'), 'wasm', 'web');
const { initSync, TexSession } = await import(pathToFileURL(path.join(web, 'tex.js')).href);

const wasm = fs.readFileSync(path.join(web, 'tex_bg.wasm'));
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
fs.writeFileSync(path.join(web, '..', 'web-hello.pdf'), result.pdf);
console.log(`Wasm web glue: native-font PDF ${result.pdf.length} bytes, ${result.passes} passes`);
result.free();
session.free();
