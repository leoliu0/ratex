// TeXres Online: projects in IndexedDB, CodeMirror editor, pdf.js preview,
// and the TeXres WebAssembly compiler in a worker.
import { unzipSync, zipSync } from 'fflate';
import * as store from './store.js';
import { Editor } from './editor.js';
import { PdfView } from './pdfview.js';
import { parseSynctex, forwardSearch, inverseSearch } from './synctex.js';
import { parseDiagnostics } from './diagnostics.js';

const config = JSON.parse(document.getElementById('texres-config').textContent);
config.base = new URL('.', location.href).href;

const TEXT_EXTENSIONS = new Set([
  'tex', 'ltx', 'sty', 'cls', 'clo', 'bib', 'bst', 'bbx', 'cbx', 'lbx', 'dbx', 'cfg', 'def', 'fd',
  'dtx', 'ins', 'txt', 'md', 'lua', 'mp', 'csv', 'dat', 'json', 'xml', 'tikz', 'pgf', 'asy', 'svg',
]);
const IMAGE_TYPES = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', svg: 'image/svg+xml' };
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const $ = (id) => document.getElementById(id);
const extension = (path) => path.slice(path.lastIndexOf('.') + 1).toLowerCase();
const isText = (path) => TEXT_EXTENSIONS.has(extension(path));
const formatBytes = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.ceil(n / 1e3)} kB`);

const state = {
  project: null,
  /** path -> {bytes, rev} */
  files: new Map(),
  open: null,
  selected: null,
  collapsed: new Set(),
  sync: null,
  syncEntry: null,
  compiling: false,
  pending: false,
  nextRev: 1,
};
// Read by the end-to-end test; mirrors what the status bar shows.
window.texres = { ready: false, compiles: 0, last: null };

// ---------------------------------------------------------------- worker --

let worker;
let workerReady;
let nextId = 1;
const waiting = new Map();

function startWorker() {
  worker = new Worker(new URL(config.worker, config.base), { type: 'module' });
  worker.onmessage = ({ data }) => {
    const entry = waiting.get(data.id);
    if (!entry) return;
    waiting.delete(data.id);
    if (data.ok) entry.resolve(data.result);
    else entry.reject(Object.assign(new Error(data.error), { fatal: data.fatal }));
  };
  worker.onerror = (event) => setStatus(`Compiler worker failed: ${event.message}`);
  workerReady = call({ type: 'init', config: { ...config } });
  workerReady.then((stats) => {
    window.texres.ready = true;
    window.texres.init = stats;
    setStatus(`Compiler ready in ${(stats.ms / 1000).toFixed(1)} s `
      + `(${formatBytes(stats.network)} downloaded, ${formatBytes(stats.cache)} cached)`);
  }, (error) => setStatus(`Compiler failed to load: ${error.message}`));
}

function call(message) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    worker.postMessage({ ...message, id });
  });
}

// ------------------------------------------------------------ components --

const editor = new Editor($('editor'), {
  onChange: (path, text) => {
    const file = state.files.get(path);
    if (!file) return;
    file.bytes = encoder.encode(text);
    file.rev = state.nextRev++;
    scheduleSave(path);
    scheduleAutoCompile();
  },
  onSyncRequest: () => syncToPdf(),
});

const pdfView = new PdfView($('pdf'), {
  workerSrc: new URL(config.pdfWorker, config.base).href,
  onInverseSearch: (page, x, y) => {
    if (!state.sync) return;
    const target = inverseSearch(state.sync, page, x, y);
    if (!target || !state.files.has(target.path)) return;
    openFile(target.path);
    editor.goToLine(target.line);
    window.texres.lastInverse = target;
  },
});

// ---------------------------------------------------------------- status --

function setStatus(text) {
  $('status').textContent = text;
}

// ------------------------------------------------------------- projects --

const TEMPLATE = String.raw`\documentclass{article}
\usepackage{amsmath}
\title{Untitled}
\author{}
\begin{document}
\maketitle

Hello, \LaTeX! $e^{i\pi} + 1 = 0$.

\end{document}
`;

async function refreshProjectList() {
  const projects = await store.listProjects();
  const select = $('project');
  select.replaceChildren(...projects.map((p) => new Option(p.name, p.id, false, p.id === state.project?.id)));
  return projects;
}

async function loadProject(id) {
  const project = await store.getProject(id);
  if (!project) return;
  state.project = project;
  localStorage.setItem('texres-online:last-project', id);
  state.files = new Map((await store.listFiles(id)).map((f) => [f.path, { bytes: new Uint8Array(f.bytes), rev: state.nextRev++ }]));
  state.open = null;
  state.selected = null;
  state.sync = null;
  editor.states.clear();
  editor.close();
  $('engine').value = project.engine;
  $('auto-compile').checked = project.autoCompile;
  $('epoch').value = project.epoch ?? '';
  await refreshProjectList();
  renderTree();
  const first = state.files.has(project.main) ? project.main : [...state.files.keys()].find(isText);
  if (first) openFile(first);
  pdfView.clear();
  showDiagnostics([], '', '');
  const output = await store.getOutput(id);
  if (output?.pdf) {
    await pdfView.show(new Uint8Array(output.pdf));
    if (output.synctex) {
      state.sync = parseSynctex(output.synctex, output.entry);
      state.syncEntry = output.entry;
    }
    showDiagnostics(parseDiagnostics(output.diagnostics), output.diagnostics, output.log);
    setStatus('Showing the last build of this project.');
  }
}

async function updateProject(changes) {
  Object.assign(state.project, changes, { modified: Date.now() });
  await store.putProject(state.project);
}

async function newProject() {
  const name = prompt('Project name', 'Untitled project');
  if (!name) return;
  const project = await store.createProject(name, [{ path: 'main.tex', bytes: encoder.encode(TEMPLATE) }], 'main.tex');
  await loadProject(project.id);
}

async function renameProject() {
  const name = prompt('Project name', state.project.name);
  if (!name) return;
  await updateProject({ name });
  await refreshProjectList();
}

async function deleteProject() {
  if (!confirm(`Delete project "${state.project.name}" and all its files?`)) return;
  await store.deleteProject(state.project.id);
  const projects = await store.listProjects();
  if (projects.length) await loadProject(projects[0].id);
  else await newProjectFromTemplate();
}

async function newProjectFromTemplate() {
  const project = await store.createProject('Untitled project', [{ path: 'main.tex', bytes: encoder.encode(TEMPLATE) }], 'main.tex');
  await loadProject(project.id);
}

// ----------------------------------------------------------------- files --

function normalizePath(path) {
  const parts = [];
  for (const part of path.replace(/\\/g, '/').split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') throw new Error('paths must stay inside the project');
    parts.push(part);
  }
  if (!parts.length) throw new Error('empty path');
  return parts.join('/');
}

function folderOf(path) {
  return path.includes('/') ? path.slice(0, path.lastIndexOf('/') + 1) : '';
}

/** The folder new files go into: the selected folder, or the selected file's. */
function currentFolder() {
  const selected = state.selected;
  if (!selected) return '';
  return state.files.has(selected) ? folderOf(selected) : `${selected}/`;
}

async function writeFile(path, bytes) {
  state.files.set(path, { bytes, rev: state.nextRev++ });
  await store.putFile(state.project.id, path, bytes);
  await updateProject({});
}

const saveTimers = new Map();
function scheduleSave(path) {
  clearTimeout(saveTimers.get(path));
  saveTimers.set(path, setTimeout(async () => {
    saveTimers.delete(path);
    const file = state.files.get(path);
    if (file) {
      await store.putFile(state.project.id, path, file.bytes);
      await updateProject({});
    }
  }, 300));
}

async function flushSaves() {
  for (const [path, timer] of saveTimers) {
    clearTimeout(timer);
    saveTimers.delete(path);
    const file = state.files.get(path);
    if (file) await store.putFile(state.project.id, path, file.bytes);
  }
}

function openFile(path) {
  state.open = path;
  state.selected = path;
  const file = state.files.get(path);
  const viewer = $('binary-view');
  if (isText(path)) {
    viewer.hidden = true;
    $('editor').hidden = false;
    editor.open(path, decoder.decode(file.bytes));
  } else {
    editor.close();
    $('editor').hidden = true;
    viewer.hidden = false;
    viewer.replaceChildren();
    const type = IMAGE_TYPES[extension(path)];
    if (type) {
      const image = document.createElement('img');
      image.src = URL.createObjectURL(new Blob([file.bytes], { type }));
      image.alt = path;
      viewer.append(image);
    }
    const caption = document.createElement('p');
    caption.textContent = `${path} — ${formatBytes(file.bytes.length)}`;
    viewer.append(caption);
  }
  $('open-path').textContent = path;
  renderTree();
}

async function newFile() {
  const input = prompt('New file path', `${currentFolder()}untitled.tex`);
  if (!input) return;
  let path;
  try {
    path = normalizePath(input);
  } catch (error) {
    alert(error.message);
    return;
  }
  if (state.files.has(path)) {
    alert(`${path} already exists`);
    return;
  }
  await writeFile(path, new Uint8Array());
  openFile(path);
}

async function uploadFiles(fileList) {
  const folder = currentFolder();
  let last;
  for (const file of fileList) {
    const path = normalizePath(folder + file.name);
    await writeFile(path, new Uint8Array(await file.arrayBuffer()));
    editor.forget(path);
    last = path;
  }
  if (last) openFile(last);
  scheduleAutoCompile();
}

async function renameSelected() {
  const from = state.selected;
  if (!from) return;
  const input = prompt('Rename to', from);
  if (!input) return;
  let to;
  try {
    to = normalizePath(input);
  } catch (error) {
    alert(error.message);
    return;
  }
  if (to === from) return;
  const moves = state.files.has(from)
    ? [[from, to]]
    : [...state.files.keys()].filter((p) => p.startsWith(`${from}/`)).map((p) => [p, to + p.slice(from.length)]);
  for (const [, target] of moves) {
    if (state.files.has(target)) {
      alert(`${target} already exists`);
      return;
    }
  }
  await flushSaves();
  for (const [source, target] of moves) {
    state.files.set(target, state.files.get(source));
    state.files.delete(source);
    await store.renameFile(state.project.id, source, target);
    editor.rename(source, target);
    if (state.open === source) state.open = target;
    if (state.project.main === source) await updateProject({ main: target });
  }
  state.selected = to;
  if (state.open) $('open-path').textContent = state.open;
  await updateProject({});
  renderTree();
}

async function deleteSelected() {
  const target = state.selected;
  if (!target) return;
  const paths = state.files.has(target)
    ? [target]
    : [...state.files.keys()].filter((p) => p.startsWith(`${target}/`));
  if (!paths.length || !confirm(`Delete ${paths.length === 1 ? paths[0] : `${target}/ (${paths.length} files)`}?`)) return;
  for (const path of paths) {
    clearTimeout(saveTimers.get(path));
    saveTimers.delete(path);
    state.files.delete(path);
    editor.forget(path);
    await store.deleteFile(state.project.id, path);
    if (state.open === path) {
      state.open = null;
      $('open-path').textContent = '';
      $('binary-view').hidden = true;
      $('editor').hidden = false;
    }
  }
  state.selected = null;
  await updateProject({});
  renderTree();
}

async function setMain() {
  const path = state.selected;
  if (!path || !state.files.has(path) || extension(path) !== 'tex') return;
  await updateProject({ main: path });
  renderTree();
}

function renderTree() {
  const root = { folders: new Map(), files: [] };
  for (const path of [...state.files.keys()].sort()) {
    const parts = path.split('/');
    let node = root;
    for (const part of parts.slice(0, -1)) {
      if (!node.folders.has(part)) node.folders.set(part, { folders: new Map(), files: [] });
      node = node.folders.get(part);
    }
    node.files.push(path);
  }
  const list = (node, prefix) => {
    const ul = document.createElement('ul');
    for (const [name, child] of node.folders) {
      const path = prefix + name;
      const li = document.createElement('li');
      const row = document.createElement('div');
      row.className = 'tree-row folder';
      row.dataset.path = path;
      row.textContent = `${state.collapsed.has(path) ? '▸' : '▾'} ${name}/`;
      if (state.selected === path) row.classList.add('selected');
      row.onclick = () => {
        state.selected = path;
        if (state.collapsed.has(path)) state.collapsed.delete(path);
        else state.collapsed.add(path);
        renderTree();
      };
      li.append(row);
      if (!state.collapsed.has(path)) li.append(list(child, `${path}/`));
      ul.append(li);
    }
    for (const path of node.files) {
      const li = document.createElement('li');
      const row = document.createElement('div');
      row.className = 'tree-row file';
      row.dataset.path = path;
      row.textContent = path.slice(prefix.length);
      if (path === state.project.main) row.classList.add('main');
      if (path === state.open) row.classList.add('open');
      if (path === state.selected) row.classList.add('selected');
      row.onclick = () => openFile(path);
      li.append(row);
      ul.append(li);
    }
    return ul;
  };
  $('tree').replaceChildren(list(root, ''));
}

// ------------------------------------------------------------------- zip --

async function importZip(file) {
  const entries = unzipSync(new Uint8Array(await file.arrayBuffer()));
  let files = Object.entries(entries)
    .filter(([name]) => !name.endsWith('/') && !name.startsWith('__MACOSX/') && !/(^|\/)\.DS_Store$/.test(name))
    .map(([name, bytes]) => ({ path: name, bytes }));
  // Strip one top-level folder shared by every file.
  const top = files[0]?.path.split('/')[0];
  if (top && files.every((f) => f.path.startsWith(`${top}/`))) {
    files = files.map((f) => ({ ...f, path: f.path.slice(top.length + 1) }));
  }
  files = files.map((f) => ({ ...f, path: normalizePath(f.path) }));
  if (!files.length) {
    alert('The archive holds no files.');
    return;
  }
  const tex = files.filter((f) => extension(f.path) === 'tex');
  const main = tex.find((f) => f.path === 'main.tex')
    ?? tex.find((f) => /\\documentclass/.test(decoder.decode(f.bytes)))
    ?? tex[0] ?? files[0];
  const project = await store.createProject(file.name.replace(/\.zip$/i, ''), files, main.path);
  await loadProject(project.id);
}

async function exportZip() {
  await flushSaves();
  const entries = {};
  for (const [path, file] of state.files) entries[path] = file.bytes;
  const blob = new Blob([zipSync(entries, { level: 6 })], { type: 'application/zip' });
  const link = document.createElement('a');
  link.href = URL.createObjectURL(blob);
  link.download = `${state.project.name.replace(/[\\/:*?"<>|]+/g, '_') || 'project'}.zip`;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(link.href), 10_000);
}

// --------------------------------------------------------------- compile --

let autoTimer;
function scheduleAutoCompile() {
  if (!state.project?.autoCompile) return;
  clearTimeout(autoTimer);
  autoTimer = setTimeout(() => compile(), 1200);
}

async function gunzipText(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'));
  return new Response(stream).text();
}

async function compile() {
  if (!state.project) return;
  if (state.compiling) {
    state.pending = true;
    return;
  }
  const entry = state.project.main;
  if (!state.files.has(entry)) {
    setStatus(`Main file ${entry} does not exist; select a .tex file and choose “Set main”.`);
    return;
  }
  state.compiling = true;
  state.pending = false;
  document.body.dataset.state = 'compiling';
  $('compile').disabled = true;
  setStatus('Compiling…');
  const project = state.project;
  try {
    await workerReady;
    const files = [...state.files].map(([path, file]) => ({ path, bytes: file.bytes, rev: file.rev }));
    const started = performance.now();
    const result = await call({
      type: 'compile',
      files,
      entry,
      engine: project.engine,
      epoch: project.epoch,
      hints: project.chunkHints,
    });
    const elapsed = performance.now() - started;
    if (state.project !== project) return;
    const synctex = result.synctexGz ? await gunzipText(result.synctexGz) : null;
    const diagnostics = parseDiagnostics(result.diagnostics);
    showDiagnostics(diagnostics, result.diagnostics, result.log);
    const missing = result.missing.length ? ` Could not download: ${result.missing.join(', ')}.` : '';
    if (result.status === 0) {
      await pdfView.show(result.pdf);
      state.sync = synctex ? parseSynctex(synctex, entry) : null;
      state.syncEntry = entry;
      await store.putOutput({
        project: project.id, entry, pdf: result.pdf, synctex,
        diagnostics: result.diagnostics, log: result.log,
      });
    }
    const runs = [`${result.passes} pass${result.passes === 1 ? '' : 'es'}`];
    if (result.bibtexRuns) runs.push(`BibTeX ×${result.bibtexRuns}`);
    if (result.biberRuns) runs.push(`Biber ×${result.biberRuns}`);
    const fetched = result.stats.network ? `, downloaded ${formatBytes(result.stats.network)}` : '';
    setStatus(`${result.status === 0 ? 'Compiled' : 'Failed'} with ${result.engine} in `
      + `${(elapsed / 1000).toFixed(1)} s (${runs.join(', ')}${fetched}).${missing}`);
    // The compiler keeps recently decoded chunks between builds, so one
    // build reports only what it newly read: keep the union as the hint.
    const hints = new Set([...(project.chunkHints ?? []), ...result.usedChunks]);
    await updateProject({ chunkHints: [...hints].sort((a, b) => a - b) });
    window.texres.compiles++;
    window.texres.last = {
      status: result.status,
      engine: result.engine,
      passes: result.passes,
      biberRuns: result.biberRuns,
      bibtexRuns: result.bibtexRuns,
      pdf: result.pdf,
      stats: result.stats,
      elapsedMs: elapsed,
      usedChunks: result.usedChunks.length,
      diagnostics: diagnostics.length,
    };
  } catch (error) {
    setStatus(`Compiler error: ${error.message.split('\n')[0]}`);
    showDiagnostics([], error.message, '');
    if (error.fatal) {
      worker.terminate();
      startWorker();
    }
  } finally {
    state.compiling = false;
    document.body.dataset.state = 'idle';
    $('compile').disabled = false;
    if (state.pending) compile();
  }
}

function showDiagnostics(items, raw, log) {
  const list = $('diagnostics');
  list.replaceChildren();
  for (const item of items) {
    const li = document.createElement('li');
    li.className = `diagnostic ${item.severity}`;
    const where = item.path ? `${item.path}:${item.line}` : '';
    li.textContent = `${item.severity}: ${item.message}${where ? ` — ${where}` : ''}`;
    if (item.path && state.files.has(item.path)) {
      li.classList.add('linked');
      li.dataset.path = item.path;
      li.dataset.line = String(item.line);
      li.onclick = () => {
        openFile(item.path);
        editor.goToLine(item.line, item.column);
      };
    }
    list.append(li);
  }
  if (!items.length && raw.trim()) {
    const li = document.createElement('li');
    li.className = 'diagnostic raw';
    li.textContent = raw.trim();
    list.append(li);
  }
  const errors = items.filter((i) => i.severity === 'error').length;
  $('diagnostics-tab').textContent = `Problems (${errors} error${errors === 1 ? '' : 's'}, ${items.length - errors} warning${items.length - errors === 1 ? '' : 's'})`;
  $('log').textContent = log;
}

function syncToPdf() {
  if (!state.sync || !state.open) return;
  const target = forwardSearch(state.sync, state.open, editor.cursorLine());
  if (!target) {
    setStatus(`No SyncTeX record for ${state.open}:${editor.cursorLine()}.`);
    return;
  }
  pdfView.highlight(target);
  window.texres.lastForward = target;
}

// ------------------------------------------------------------------ wire --

function on(id, event, handler) {
  $(id).addEventListener(event, (e) => {
    Promise.resolve(handler(e)).catch((error) => setStatus(`Error: ${error.message}`));
  });
}

on('project', 'change', (e) => loadProject(e.target.value));
on('new-project', 'click', newProject);
on('rename-project', 'click', renameProject);
on('delete-project', 'click', deleteProject);
on('new-file', 'click', newFile);
on('upload', 'change', async (e) => {
  await uploadFiles(e.target.files);
  e.target.value = '';
});
on('rename-file', 'click', renameSelected);
on('delete-file', 'click', deleteSelected);
on('set-main', 'click', setMain);
on('import-zip', 'change', async (e) => {
  if (e.target.files[0]) await importZip(e.target.files[0]);
  e.target.value = '';
});
on('export-zip', 'click', exportZip);
on('compile', 'click', compile);
on('sync-forward', 'click', syncToPdf);
on('engine', 'change', (e) => updateProject({ engine: e.target.value }));
on('auto-compile', 'change', (e) => updateProject({ autoCompile: e.target.checked }));
on('epoch', 'change', (e) => {
  const value = e.target.value.trim();
  return updateProject({ epoch: value === '' ? null : Math.max(0, Math.floor(Number(value))) });
});
on('zoom-in', 'click', () => pdfView.setZoom(pdfView.zoom * 1.2));
on('zoom-out', 'click', () => pdfView.setZoom(pdfView.zoom / 1.2));
for (const tab of document.querySelectorAll('[data-panel]')) {
  tab.addEventListener('click', () => {
    for (const other of document.querySelectorAll('[data-panel]')) {
      other.classList.toggle('active', other === tab);
      $(other.dataset.panel).hidden = other !== tab;
    }
  });
}
window.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === 's') {
    e.preventDefault();
    compile();
  }
});
window.addEventListener('beforeunload', () => {
  for (const [path] of saveTimers) {
    const file = state.files.get(path);
    if (file) store.putFile(state.project.id, path, file.bytes);
  }
});

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('sw.js').catch(() => {});
}

startWorker();
const projects = await refreshProjectList();
const last = localStorage.getItem('texres-online:last-project');
if (projects.some((p) => p.id === last)) await loadProject(last);
else if (projects.length) await loadProject(projects[0].id);
else await newProjectFromTemplate();
