// Assemble the static site (see scripts/build-web.sh):
//   node web/build.mjs --dist DIR --wasm DIR --assets FILE
// --wasm is wasm-bindgen's `--target web` output of the `lazy-assets` build and
// --assets the summary written by scripts/web_assets.py into the same --dist.
import { build } from 'esbuild';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { makePng } from './test/png.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const { values: args } = parseArgs({
  options: { dist: { type: 'string' }, wasm: { type: 'string' }, assets: { type: 'string' } },
});
for (const name of ['dist', 'wasm', 'assets']) {
  if (!args[name]) throw new Error(`missing --${name}`);
}
const dist = path.resolve(args.dist);
const wasmDir = path.resolve(args.wasm);
const assetsOut = path.join(dist, 'assets');
fs.mkdirSync(assetsOut, { recursive: true });

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex').slice(0, 16);
const written = new Set();

/** Write `bytes` as assets/<stem>-<hash><ext>; returns the site-relative URL. */
function emit(stem, ext, bytes) {
  const name = `${stem}-${hash(bytes)}${ext}`;
  fs.writeFileSync(path.join(assetsOut, name), bytes);
  written.add(name);
  return `assets/${name}`;
}

async function bundle(entry, options = {}) {
  const result = await build({
    entryPoints: [path.join(here, 'src', entry)],
    bundle: true,
    format: 'esm',
    minify: true,
    write: false,
    target: 'es2022',
    legalComments: 'eof',
    logLevel: 'warning',
    ...options,
  });
  return result.outputFiles[0].contents;
}

// ------------------------------------------------- package preload sets --
// Chunks a few typical documents read, recorded by running the same module in
// Node. The worker fetches the matching set concurrently before compiling.
const assets = JSON.parse(fs.readFileSync(args.assets, 'utf8'));

async function recordPreloadSets() {
  const glue = await import(pathToFileURL(path.join(wasmDir, 'tex.js')).href);
  glue.initSync({ module: fs.readFileSync(path.join(wasmDir, 'tex_bg.wasm')) });
  let used = new Set();
  glue.setChunkLoader((index) => {
    used.add(index);
    const name = assets.chunks[index];
    return name ? fs.readFileSync(path.join(dist, 'packages', `${name}.bin`)) : undefined;
  });
  glue.setFormatLoader((engine) => {
    const entry = assets.formats[engine];
    return entry ? fs.readFileSync(path.join(dist, entry.url)) : undefined;
  });
  const png = makePng(8, 8, [40, 120, 200]);
  const preamble = String.raw`\usepackage{amsmath,amssymb,amsthm}\usepackage{graphicx}\usepackage{geometry}
\usepackage{xcolor}\usepackage{booktabs}\usepackage{hyperref}`;
  const body = String.raw`\section{Intro}\label{s}Text \emph{em} \textbf{bf} $x^2+\alpha$ \ref{s}.
\begin{equation}\int_0^1 f\,dx\end{equation}\includegraphics[width=1cm]{fig.png}`;
  const docs = {
    pdflatex: [`\\documentclass{article}${preamble}\\begin{document}${body}\\end{document}`, 'pdflatex'],
    biblatex: [String.raw`\documentclass{article}${preamble}\usepackage{biblatex}\addbibresource{refs.bib}
\begin{document}${body}\cite{k}\printbibliography\end{document}`, 'pdflatex'],
    xelatex: [String.raw`\documentclass{article}\usepackage{fontspec}\usepackage{amsmath}\usepackage{graphicx}
\usepackage{hyperref}\begin{document}${body}\end{document}`, 'xelatex'],
  };
  const sets = {};
  for (const [name, [source, engine]] of Object.entries(docs)) {
    used = new Set();
    const session = new glue.TexSession();
    session.setEpoch(1700000000);
    session.addFile('main.tex', new TextEncoder().encode(source));
    session.addFile('fig.png', png);
    session.addFile('refs.bib', new TextEncoder().encode('@book{k, author={A. Author}, title={T}, year={2024}, publisher={P}}'));
    const started = performance.now();
    const result = session.compileWith('main.tex', engine, true);
    if (result.status !== 0) {
      throw new Error(`preload document ${name} failed:\n${result.diagnostics}\n${result.log.slice(-4000)}`);
    }
    result.free();
    session.free();
    sets[name] = [...used].sort((a, b) => a - b);
    const bytes = sets[name].reduce((sum, i) => sum + assets.sizes[i], 0);
    console.error(`preload ${name}: ${sets[name].length} chunks, ${(bytes / 1e6).toFixed(2)} MB, `
      + `compiled in ${((performance.now() - started) / 1000).toFixed(1)} s`);
  }
  // biblatex lists only what it adds to the pdfLaTeX set.
  const base = new Set(sets.pdflatex);
  sets.biblatex = sets.biblatex.filter((i) => !base.has(i));
  return sets;
}

const preload = await recordPreloadSets();
const packages = emit('packages', '.json', Buffer.from(JSON.stringify({ ...assets, preload })));

// --------------------------------------------------------------- bundles --
const wasm = emit('tex_bg', '.wasm', fs.readFileSync(path.join(wasmDir, 'tex_bg.wasm')));
const worker = emit('compile-worker', '.js', await bundle('compile-worker.js', {
  alias: { 'texres-wasm': path.join(wasmDir, 'tex.js') },
}));
const main = emit('main', '.js', await bundle('main.js'));
// Fonts referenced by the stylesheet land next to it under content-hashed names.
const css = await build({
  entryPoints: [path.join(here, 'src', 'style.css')],
  bundle: true,
  minify: true,
  write: false,
  outdir: assetsOut,
  loader: { '.woff2': 'file' },
  assetNames: '[name]-[hash]',
  entryNames: 'style',
  logLevel: 'warning',
});
const fonts = [];
for (const file of css.outputFiles.filter((f) => f.path.endsWith('.woff2'))) {
  fs.writeFileSync(file.path, file.contents);
  written.add(path.basename(file.path));
  fonts.push(`assets/${path.basename(file.path)}`);
}
const style = emit('style', '.css', css.outputFiles.find((f) => f.path.endsWith('.css')).contents);
const pdfWorker = emit('pdf.worker', '.js',
  fs.readFileSync(path.join(here, 'node_modules/pdfjs-dist/build/pdf.worker.min.mjs')));

const buildId = hash(Buffer.from([packages, wasm, worker, main, style, pdfWorker, ...fonts].join('\n')));
const siteConfig = { build: buildId, wasm, worker, pdfWorker, packages, chunkBase: 'packages/' };
const html = fs.readFileSync(path.join(here, 'index.html'), 'utf8')
  .replace('%STYLE%', style)
  .replace('%MAIN%', main)
  .replace('%CONFIG%', JSON.stringify(siteConfig).replace(/</g, '\\u003c'));
fs.writeFileSync(path.join(dist, 'index.html'), html);
const shell = ['./', main, style, worker, pdfWorker, ...fonts];
fs.writeFileSync(path.join(dist, 'sw.js'), fs.readFileSync(path.join(here, 'src/sw.js'), 'utf8')
  .replace('%BUILD%', buildId)
  .replace('%SHELL%', JSON.stringify(shell)));
for (const name of fs.readdirSync(assetsOut)) {
  if (!written.has(name)) fs.rmSync(path.join(assetsOut, name));
}
// Legal notices for the bundled third-party code.
fs.copyFileSync(path.join(here, 'node_modules/pdfjs-dist/LICENSE'), path.join(dist, 'LICENSE-pdfjs.txt'));
fs.copyFileSync(path.join(here, 'fonts/OFL.txt'), path.join(dist, 'LICENSE-fonts.txt'));
console.error(`site: ${dist} (build ${buildId})`);
