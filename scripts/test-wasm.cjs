// Run after scripts/build-libs.sh wasm. Uses the shipped Node.js bindings.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const { TexSession } = require(path.join(root, 'target/wasm/nodejs/tex.js'));
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const session = new TexSession();
session.setEpoch(1700000000);
session.addFile('main.tex', encoder.encode(String.raw`\documentclass{article}
\usepackage{fontspec}
\setmainfont{Latin Modern Roman}
\begin{document}
Native font selection in libtex C ABI: \textbf{Bold glyphs} and \textit{italic shapes}.
\end{document}
`));
let result = session.compile('main.tex');
assert.equal(result.status, 0, result.diagnostics + '\n' + result.log);
assert.ok(decoder.decode(result.pdf.slice(0, 5)) === '%PDF-');
assert.ok(result.passes >= 2);
assert.ok(result.fileNames.includes('main.aux'));
fs.writeFileSync(path.join(root, 'target/wasm/hello.pdf'), result.pdf);
const native = path.join(root, 'target/libtex/hello.pdf');
if (fs.existsSync(native)) assert.deepEqual(Buffer.from(result.pdf), fs.readFileSync(native));
console.log(`Wasm Node.js: native-font PDF ${result.pdf.length} bytes, ${result.passes} passes`);
result.free();

session.addFile('main.tex', encoder.encode(String.raw`\documentclass{article}\usepackage{amsmath}\begin{document}\input{parts/body}\bibliographystyle{plain}\bibliography{refs}\end{document}`));
session.addFile('parts/body.tex', encoder.encode(String.raw`Hello $x^2$. Citation~\cite{paper}.`));
session.addFile('refs.bib', encoder.encode('@article{paper,author={Ada Lovelace},title={Library Test},journal={Testing},year={2024}}'));
result = session.compile('main.tex');
assert.equal(result.status, 0, result.diagnostics + '\n' + result.log);
assert.ok(result.bibtexRuns > 0);
assert.match(decoder.decode(result.file('main.bbl')), /Lovelace/);
console.log(`Wasm Node.js: nested inputs and BibTeX, ${result.passes} passes`);
result.free();
session.addFile('main.tex', encoder.encode(String.raw`\documentclass{article}
\usepackage{fontspec}
\setmainfont{Latin Modern Roman}
\setsansfont{Latin Modern Sans}
\setmonofont{Latin Modern Mono}
\begin{document}
Roman font. {\sffamily Sans font.} {\ttfamily Monospace font.}
\end{document}`));
result = session.compile('main.tex');
assert.equal(result.status, 0, result.diagnostics + '\n' + result.log);
console.log(`Wasm Node.js: multi-family fontspec styles, ${result.passes} passes`);
result.free();

session.addFile('main.tex', encoder.encode(String.raw`\documentclass{article}\usepackage{fontspec}\setmainfont{NonexistentPhantomFont}\begin{document}Fail\end{document}`));
result = session.compile('main.tex');
assert.equal(result.status, 1);
assert.equal(result.pdf.length, 0);
console.log('Wasm Node.js: missing native font error handled');
result.free();


session.addFile('main.tex', encoder.encode(String.raw`\documentclass{article}\begin{document}\input{missing-file}\end{document}`));
result = session.compile('main.tex');
assert.equal(result.status, 1);
assert.equal(result.pdf.length, 0);
assert.match(result.diagnostics, /missing-file/);
result.free();
assert.throws(() => session.addFile('../escape.tex', new Uint8Array()));
session.free();
console.log('Wasm Node.js: errors, ownership, and isolation passed');
