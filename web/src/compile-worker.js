// Runs the TeXres WebAssembly compiler off the main thread. The module holds
// the engine, the pdfLaTeX format and the package index; package chunks and
// the XeLaTeX/LuaLaTeX formats are fetched on demand and kept in the Cache API.
import init, { TexSession, setChunkLoader, setFormatLoader } from 'texres-wasm';

const CACHE = 'texres-assets-v1';

let config;
let manifest;
let session;
/** Files the session currently holds: path -> revision. */
let sessionFiles = new Map();
/** Compressed chunk bytes by index, for the lifetime of the worker. */
const chunks = new Map();
const formats = new Map();
/** Per-compile accounting, reset by `compile`. */
let used;
let fetched;
let failed;

const chunkUrl = (index) => new URL(`${config.chunkBase}${manifest.chunks[index]}.bin`, config.base).href;

async function cache() {
  return caches.open(CACHE);
}

/** Bytes of a content-addressed asset: Cache API first, else the network. */
async function cachedBytes(url, stats) {
  const store = await cache();
  let response = await store.match(url);
  if (response) {
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (stats) stats.cache += bytes.length;
    return bytes;
  }
  response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  await store.put(url, new Response(bytes));
  if (stats) stats.network += bytes.length;
  return bytes;
}

/** The compiler asks synchronously; a miss blocks on a synchronous request. */
function syncFetch(url) {
  const request = new XMLHttpRequest();
  request.open('GET', url, false);
  request.responseType = 'arraybuffer';
  try {
    request.send();
  } catch {
    return undefined;
  }
  return request.status === 200 ? new Uint8Array(request.response) : undefined;
}

function loadChunk(index) {
  used.add(index);
  let bytes = chunks.get(index);
  if (bytes) return bytes;
  if (index >= manifest.chunks.length) return undefined;
  bytes = syncFetch(chunkUrl(index));
  if (!bytes) {
    failed.add(`package chunk ${index}`);
    return undefined;
  }
  chunks.set(index, bytes);
  fetched.set(chunkUrl(index), bytes);
  return bytes;
}

function loadFormat(engine) {
  let bytes = formats.get(engine);
  if (bytes) return bytes;
  const entry = manifest.formats[engine];
  if (!entry) return undefined;
  const url = new URL(entry.url, config.base).href;
  bytes = syncFetch(url);
  if (!bytes) {
    failed.add(`${engine} format`);
    return undefined;
  }
  formats.set(engine, bytes);
  fetched.set(url, bytes);
  return bytes;
}

/** Fetch predicted chunks and formats concurrently before the compiler blocks. */
async function prefetch(indices, engines, stats) {
  const queue = [...new Set(indices)].filter((i) => i < manifest.chunks.length && !chunks.has(i));
  const work = async () => {
    for (let index = queue.pop(); index !== undefined; index = queue.pop()) {
      try {
        chunks.set(index, await cachedBytes(chunkUrl(index), stats));
      } catch {
        // The compiler retries a missing chunk synchronously.
      }
    }
  };
  const formatJobs = engines
    .filter((engine) => manifest.formats[engine] && !formats.has(engine))
    .map(async (engine) => {
      try {
        formats.set(engine, await cachedBytes(new URL(manifest.formats[engine].url, config.base).href, stats));
      } catch {
        // Likewise retried on demand.
      }
    });
  await Promise.all([...Array.from({ length: 12 }, work), ...formatJobs]);
}

async function start(message) {
  config = message.config;
  const stats = { network: 0, cache: 0 };
  const started = performance.now();
  const [wasm, manifestBytes] = await Promise.all([
    cachedBytes(new URL(config.wasm, config.base).href, stats),
    cachedBytes(new URL(config.packages, config.base).href, stats),
  ]);
  manifest = JSON.parse(new TextDecoder().decode(manifestBytes));
  await init({ module_or_path: await WebAssembly.compile(wasm) });
  setChunkLoader(loadChunk);
  setFormatLoader(loadFormat);
  session = new TexSession();
  // Remove superseded modules and manifests; chunks are content-addressed.
  const store = await cache();
  const current = new Set([config.wasm, config.packages].map((u) => new URL(u, config.base).href));
  for (const request of await store.keys()) {
    if (/\/(tex_bg|packages)-[0-9a-f]+\.(wasm|json)$/.test(request.url) && !current.has(request.url)) {
      await store.delete(request);
    }
  }
  return { ...stats, ms: performance.now() - started, chunkCount: manifest.chunks.length };
}

function preloadSet(name) {
  return manifest.preload?.[name] ?? [];
}

/** Engines whose format and packages a compile is expected to need. */
function predictedEngines(engine, files, entry) {
  if (engine !== 'auto') return [engine];
  const source = files.find((f) => f.path === entry);
  const text = source ? new TextDecoder().decode(source.bytes) : '';
  const preamble = text.split('\\begin{document}')[0];
  if (/\\directlua|\{(luatexja|luacode|luatextra)\}/.test(preamble)) return ['lualatex'];
  if (/% *!TeX +(TS-)?program *= *lualatex/i.test(preamble)) return ['lualatex'];
  if (/% *!TeX +(TS-)?program *= *xelatex/i.test(preamble)) return ['xelatex'];
  if (/\{(fontspec|xeCJK|ctex|unicode-math|polyglossia)\}|\{ctex(art|book|rep|beamer)\}/.test(preamble)) return ['xelatex'];
  return ['pdflatex'];
}

async function compile(message) {
  const { files, entry, engine, epoch, hints } = message;
  const stats = { network: 0, cache: 0 };
  used = new Set();
  fetched = new Map();
  failed = new Set();
  const started = performance.now();

  const engines = predictedEngines(engine, files, entry);
  const usesBiblatex = files.some((f) => /\.(tex|sty|cls)$/.test(f.path)
    && new TextDecoder().decode(f.bytes).includes('biblatex'));
  const predicted = [...(hints ?? []), ...engines.flatMap(preloadSet)];
  if (usesBiblatex) predicted.push(...preloadSet('biblatex'));
  await prefetch(predicted, engines.filter((e) => e !== 'pdflatex'), stats);
  const prefetchMs = performance.now() - started;

  const next = new Map(files.map((f) => [f.path, f.rev]));
  for (const path of sessionFiles.keys()) {
    if (!next.has(path)) session.removeFile(path);
  }
  for (const file of files) {
    if (sessionFiles.get(file.path) !== file.rev) session.addFile(file.path, file.bytes);
  }
  sessionFiles = next;
  session.setEpoch(epoch ?? undefined);

  const compileStarted = performance.now();
  const result = session.compileWith(entry, engine, true);
  const compileMs = performance.now() - compileStarted;
  const fileNames = result.fileNames;
  const job = entry.replace(/^.*\//, '').replace(/\.[^.]*$/, '');
  const dir = entry.includes('/') ? entry.replace(/\/[^/]*$/, '/') : '';
  const synctexGz = result.file(`${dir}${job}.synctex.gz`);
  const output = {
    status: result.status,
    engine: result.engine,
    pdf: result.pdf,
    log: result.log,
    diagnostics: result.diagnostics,
    passes: result.passes,
    bibtexRuns: result.bibtexRuns,
    biberRuns: result.biberRuns,
    fileNames,
    synctexGz,
  };
  result.free();
  for (const bytes of fetched.values()) stats.network += bytes.length;
  // Persist on-demand downloads after the compile so the next visit is local.
  const store = await cache();
  await Promise.all([...fetched].map(([url, bytes]) => store.put(url, new Response(bytes))));
  return {
    ...output,
    usedChunks: [...used].sort((a, b) => a - b),
    missing: [...failed],
    stats: {
      ...stats,
      onDemand: fetched.size,
      prefetchMs,
      compileMs,
      totalMs: performance.now() - started,
    },
  };
}

self.onmessage = async (event) => {
  const message = event.data;
  try {
    const result = message.type === 'init' ? await start(message) : await compile(message);
    const transfer = [result.pdf?.buffer, result.synctexGz?.buffer].filter(Boolean);
    self.postMessage({ id: message.id, ok: true, result }, transfer);
  } catch (error) {
    // A trap leaves the module unusable: the page replaces this worker.
    const fatal = error instanceof WebAssembly.RuntimeError;
    self.postMessage({ id: message.id, ok: false, fatal, error: String(error?.stack ?? error) });
  }
};
