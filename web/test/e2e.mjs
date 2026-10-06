// End-to-end check of the built site in headless Chromium:
//   node web/test/e2e.mjs --dist DIR --texres BIN [--chromium PATH]
// Serves DIR, builds a biblatex article with a figure through the UI, and
// compares each PDF with native `texres` output for the same files. Prints
// network bytes per phase (counted by the server) and compile times.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright-core';
import { makePng } from './png.mjs';

const { values: args } = parseArgs({
  options: {
    dist: { type: 'string' },
    texres: { type: 'string' },
    chromium: { type: 'string', default: '/usr/bin/chromium' },
  },
});
if (!args.dist || !args.texres) throw new Error('usage: e2e.mjs --dist DIR --texres BIN');
const dist = path.resolve(args.dist);
const EPOCH = 1700000000;

// ------------------------------------------------------------- server --
const TYPES = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css',
  '.json': 'application/json', '.wasm': 'application/wasm', '.bin': 'application/octet-stream',
  '.txt': 'text/plain',
};
const traffic = { bytes: 0, requests: 0, byKind: {} };
function resetTraffic() {
  traffic.bytes = 0;
  traffic.requests = 0;
  traffic.byKind = {};
}
const server = http.createServer((request, response) => {
  const url = new URL(request.url, 'http://localhost');
  let file = path.join(dist, decodeURIComponent(url.pathname));
  if (!file.startsWith(dist)) {
    response.writeHead(403).end();
    return;
  }
  if (fs.existsSync(file) && fs.statSync(file).isDirectory()) file = path.join(file, 'index.html');
  if (!fs.existsSync(file)) {
    response.writeHead(404).end();
    return;
  }
  const body = fs.readFileSync(file);
  const kind = url.pathname.startsWith('/packages/') ? 'packages'
    : url.pathname.startsWith('/formats/') ? 'formats'
      : url.pathname.endsWith('.wasm') ? 'wasm' : 'shell';
  traffic.bytes += body.length;
  traffic.requests++;
  traffic.byKind[kind] = (traffic.byKind[kind] ?? 0) + body.length;
  response.writeHead(200, { 'Content-Type': TYPES[path.extname(file)] ?? 'application/octet-stream' });
  response.end(body);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}/`;
const snapshot = () => ({ bytes: traffic.bytes, requests: traffic.requests, byKind: { ...traffic.byKind } });
const mb = (n) => `${(n / 1e6).toFixed(2)} MB`;

// ------------------------------------------------------------ fixtures --
const MAIN = String.raw`\documentclass{article}
\usepackage{amsmath}
\usepackage{graphicx}
\usepackage[backend=biber,style=authoryear]{biblatex}
\addbibresource{refs.bib}
\title{A Test Article}
\author{TeXres Online}
\begin{document}
\maketitle
\section{Introduction}\label{sec:intro}
This article cites \textcite{knuth1984} and \parencite{lamport1994}.
See Figure~\ref{fig:box} in Section~\ref{sec:intro}.
\begin{equation}
  \int_0^1 x^2\,dx = \frac{1}{3}.
\end{equation}
\begin{figure}[h]
  \centering
  \includegraphics[width=3cm]{figures/box.png}
  \caption{A blue box.}\label{fig:box}
\end{figure}
\printbibliography
\end{document}
`;
const REFS = String.raw`@book{knuth1984,
  author = {Donald E. Knuth},
  title = {The {\TeX}book},
  publisher = {Addison-Wesley},
  year = {1984}
}
@book{lamport1994,
  author = {Leslie Lamport},
  title = {{\LaTeX}: A Document Preparation System},
  publisher = {Addison-Wesley},
  edition = {2},
  year = {1994}
}
`;
const EDITED_LINE = 'An added sentence after editing.';
const PNG = makePng(16, 16, [40, 90, 200]);
const XETEX = String.raw`\documentclass{article}
\usepackage{fontspec}
\begin{document}
Unicode text: café, naïve, Ωμέγα.
\end{document}
`;

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'texres-online-e2e-'));
function nativePdf(name, files) {
  const dir = path.join(work, name);
  for (const [file, bytes] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    fs.writeFileSync(path.join(dir, file), bytes);
  }
  execFileSync(args.texres, ['--silent', 'main.tex'], {
    cwd: dir,
    env: { ...process.env, SOURCE_DATE_EPOCH: String(EPOCH), FORCE_SOURCE_DATE: '1', TEX_RS_CACHE_DIR: path.join(work, 'cache') },
    stdio: ['ignore', 'ignore', 'inherit'],
  });
  return fs.readFileSync(path.join(dir, 'main.pdf'));
}

// ------------------------------------------------------------- browser --
const profile = path.join(work, 'profile');
const context = await chromium.launchPersistentContext(profile, {
  executablePath: args.chromium,
  headless: true,
  acceptDownloads: true,
  viewport: { width: 1600, height: 1000 },
});
const page = context.pages()[0] ?? await context.newPage();
const answers = [];
page.on('dialog', (dialog) => {
  const answer = answers.shift();
  if (answer === undefined) throw new Error(`unexpected dialog: ${dialog.message()}`);
  return answer === true ? dialog.accept() : dialog.accept(answer);
});
page.on('console', (message) => {
  if (message.type() === 'error') console.error(`[page] ${message.text()}`);
});
page.on('pageerror', (error) => console.error(`[page] ${error.message}`));

const report = {};
const ready = () => page.waitForFunction(() => window.texres?.ready, null, { timeout: 120_000 });
async function compileAndWait(trigger) {
  const before = await page.evaluate(() => window.texres.compiles);
  await trigger();
  await page.waitForFunction((n) => window.texres.compiles > n && document.body.dataset.state === 'idle', before, { timeout: 300_000 });
  return page.evaluate(() => {
    const { pdf, ...rest } = window.texres.last;
    return { ...rest, pdf: Array.from(pdf) };
  });
}
async function setEditorText(text) {
  await page.click('.cm-content');
  await page.keyboard.press('Control+A');
  await page.keyboard.insertText(text);
}
async function goToLine(line) {
  await page.click('.cm-content');
  await page.keyboard.press('Control+Home');
  for (let i = 1; i < line; i++) await page.keyboard.press('ArrowDown');
}
const editorLine = () => page.evaluate(() => {
  const active = document.querySelector('.cm-activeLineGutter');
  return Number(active?.textContent);
});

// First visit: everything comes from the network.
resetTraffic();
await page.goto(origin);
await ready();
report.firstLoad = { ...snapshot(), init: await page.evaluate(() => window.texres.init) };

// Create the project through the UI.
answers.push('E2E Article');
await page.click('#new-project');
await page.waitForFunction(() => document.querySelector('#project option:checked')?.textContent === 'E2E Article');
await page.uncheck('#auto-compile');
await page.click('#settings-button');
await page.fill('#epoch', String(EPOCH));
await page.dispatchEvent('#epoch', 'change');
await setEditorText(MAIN);
answers.push('refs.bib');
await page.click('#new-file');
await page.waitForSelector('.tree-row[data-path="refs.bib"]');
await setEditorText(REFS);
await page.setInputFiles('#upload', { name: 'box.png', mimeType: 'image/png', buffer: PNG });
await page.waitForSelector('.tree-row[data-path="box.png"]');
answers.push('figures/box.png');
await page.click('.tree-row[data-path="box.png"]');
await page.click('#rename-file');
await page.waitForSelector('.tree-row[data-path="figures/box.png"]');
await page.click('.tree-row[data-path="main.tex"]');

// First compile: package chunks are fetched on demand.
resetTraffic();
const first = await compileAndWait(() => page.click('#compile'));
assert.equal(first.status, 0, await page.textContent('#diagnostics'));
assert.equal(first.biberRuns, 1);
const nativeFirst = nativePdf('article', { 'main.tex': MAIN, 'refs.bib': REFS, 'figures/box.png': PNG });
assert.ok(Buffer.from(first.pdf).equals(nativeFirst), 'first PDF differs from native texres');
report.firstCompile = { ...snapshot(), elapsedMs: first.elapsedMs, stats: first.stats, passes: first.passes, chunks: first.usedChunks, pdfBytes: first.pdf.length };

// Edit with auto-compile on: the pause triggers a rebuild.
await page.check('#auto-compile');
const edited = MAIN.replace('\\printbibliography', `${EDITED_LINE}\n\\printbibliography`);
const second = await compileAndWait(async () => {
  await goToLine(MAIN.split('\n').findIndex((l) => l.startsWith('\\printbibliography')) + 1);
  await page.keyboard.insertText(`${EDITED_LINE}\n`);
});
assert.equal(second.status, 0);
assert.ok(Buffer.from(second.pdf).equals(nativePdf('edited', { 'main.tex': edited, 'refs.bib': REFS, 'figures/box.png': PNG })),
  'edited PDF differs from native texres');
report.editCompile = { elapsedMs: second.elapsedMs, stats: second.stats, passes: second.passes };
await page.uncheck('#auto-compile');

// SyncTeX: source -> PDF, then double-click the mark -> source.
const sectionLine = edited.split('\n').findIndex((l) => l.startsWith('\\section')) + 1;
await goToLine(sectionLine);
await page.click('#sync-forward');
await page.waitForSelector('.sync-mark');
const forward = await page.evaluate(() => window.texres.lastForward);
assert.equal(forward.page, 1);
const box = await page.locator('.sync-mark').boundingBox();
await page.click('.tree-row[data-path="refs.bib"]');
await page.mouse.dblclick(box.x + box.width / 2, box.y + box.height / 2);
await page.waitForFunction(() => window.texres.lastInverse);
const inverse = await page.evaluate(() => window.texres.lastInverse);
assert.deepEqual(inverse, { path: 'main.tex', line: sectionLine });
assert.equal(await editorLine(), sectionLine);
report.synctex = { forward, inverse };

// A TeX error is listed with its location; clicking it jumps to the line.
const broken = edited.replace(EDITED_LINE, `${EDITED_LINE} \\undefinedmacro`);
await setEditorText(broken);
const failed = await compileAndWait(() => page.click('#compile'));
assert.notEqual(failed.status, 0);
const errorLine = broken.split('\n').findIndex((l) => l.includes('\\undefinedmacro')) + 1;
await goToLine(1);
await page.click(`.diagnostic.linked[data-line="${errorLine}"]`);
assert.equal(await editorLine(), errorLine);
await setEditorText(edited);
const fixed = await compileAndWait(() => page.click('#compile'));
assert.equal(fixed.status, 0);

// Zip export, then import as a second project with the same files.
const [download] = await Promise.all([page.waitForEvent('download'), page.click('#export-zip')]);
const zipPath = path.join(work, 'export.zip');
await download.saveAs(zipPath);
await page.setInputFiles('#import-zip', zipPath);
await page.waitForFunction(() => document.querySelector('#project option:checked')?.textContent === 'export');
const imported = await page.$$eval('.tree-row.file', (rows) => rows.map((r) => r.dataset.path).sort());
assert.deepEqual(imported, ['figures/box.png', 'main.tex', 'refs.bib']);

// XeLaTeX through automatic engine selection (fontspec).
answers.push('XeLaTeX');
await page.click('#new-project');
await page.waitForFunction(() => document.querySelector('#project option:checked')?.textContent === 'XeLaTeX');
await page.uncheck('#auto-compile');
await page.click('#settings-button');
await page.fill('#epoch', String(EPOCH));
await page.dispatchEvent('#epoch', 'change');
await setEditorText(XETEX);
resetTraffic();
const xe = await compileAndWait(() => page.click('#compile'));
assert.equal(xe.status, 0, await page.textContent('#diagnostics'));
assert.equal(xe.engine, 'xelatex');
assert.ok(Buffer.from(xe.pdf).equals(nativePdf('xetex', { 'main.tex': XETEX })), 'XeLaTeX PDF differs from native texres');
report.xelatexCompile = { ...snapshot(), elapsedMs: xe.elapsedMs, stats: xe.stats };

// Second visit: the project persists and assets come from the caches.
await page.selectOption('#project', { label: 'E2E Article' });
await page.waitForFunction(() => document.querySelector('.tree-row[data-path="figures/box.png"]'));
resetTraffic();
await page.reload();
await ready();
report.secondLoad = { ...snapshot(), init: await page.evaluate(() => window.texres.init) };
assert.equal(await page.textContent('#project option:checked'), 'E2E Article');
assert.deepEqual(await page.$$eval('.tree-row.file', (rows) => rows.map((r) => r.dataset.path).sort()),
  ['figures/box.png', 'main.tex', 'refs.bib']);
await page.waitForFunction(() => Number(document.querySelector('#pdf').dataset.pages) >= 1);
resetTraffic();
const again = await compileAndWait(() => page.click('#compile'));
assert.equal(again.status, 0);
assert.ok(Buffer.from(again.pdf).equals(Buffer.from(fixed.pdf)));
report.secondLoadCompile = { ...snapshot(), elapsedMs: again.elapsedMs, stats: again.stats };

await context.close();
server.close();
fs.rmSync(work, { recursive: true, force: true });

console.log(JSON.stringify(report, null, 2));
console.log([
  `first load: ${mb(report.firstLoad.bytes)} in ${report.firstLoad.requests} requests`,
  `first compile (article + figure + biblatex/biber): ${mb(report.firstCompile.bytes)} fetched, ${(report.firstCompile.elapsedMs / 1000).toFixed(2)} s`,
  `edit + auto-compile: ${(report.editCompile.elapsedMs / 1000).toFixed(2)} s`,
  `XeLaTeX compile: ${mb(report.xelatexCompile.bytes)} fetched, ${(report.xelatexCompile.elapsedMs / 1000).toFixed(2)} s`,
  `second load: ${mb(report.secondLoad.bytes)} in ${report.secondLoad.requests} requests`,
  `compile after reload: ${mb(report.secondLoadCompile.bytes)} fetched, ${(report.secondLoadCompile.elapsedMs / 1000).toFixed(2)} s`,
  'PASS: PDFs equal native texres; SyncTeX, diagnostics, zip and persistence work',
].join('\n'));
