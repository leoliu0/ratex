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

/** An icon from the sprite in index.html. */
function icon(name, className = '') {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  if (className) svg.setAttribute('class', className);
  const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  use.setAttribute('href', `#i-${name}`);
  svg.append(use);
  return svg;
}

const FILE_ICONS = [
  [['tex', 'ltx', 'dtx', 'ins'], 'file-tex', 'tex'],
  [['bib', 'bst', 'bbx', 'cbx', 'lbx', 'dbx'], 'book', 'bib'],
  [['png', 'jpg', 'jpeg', 'gif', 'svg', 'pdf', 'eps'], 'image', 'image'],
  [['sty', 'cls', 'clo', 'def', 'fd', 'cfg', 'lua'], 'braces', 'code'],
];
function fileIcon(path) {
  const ext = extension(path);
  const [, name, kind] = FILE_ICONS.find(([exts]) => exts.includes(ext)) ?? [null, 'file', 'other'];
  return icon(name, `ficon ${kind}`);
}

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
  setStatus('Loading the compiler…');
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
  onRender: () => updatePdfToolbar(),
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
  $('status').title = text;
}

/** The result chip next to the Compile button. */
function setCompileInfo(ok, text, detail) {
  const info = $('compile-info');
  info.hidden = false;
  info.className = `compile-info ${ok ? 'ok' : 'failed'}`;
  info.replaceChildren(icon(ok ? 'check' : 'x'), text);
  info.title = detail;
}

function updatePdfToolbar() {
  const pages = pdfView.pdf?.numPages ?? 0;
  $('page-info').textContent = pages ? `Page ${pdfView.currentPage()} / ${pages}` : 'PDF';
  $('zoom-level').textContent = `${Math.round(pdfView.zoom * 100)}%`;
  $('zoom-fit').classList.toggle('active', pdfView.fitWidth);
}

/** Show `path` (or nothing) in the editor pane's header. */
function showOpenPath(path) {
  const element = $('open-path');
  element.replaceChildren();
  element.title = path ?? '';
  $('source').classList.toggle('no-file', !path);
  $('editor-empty').hidden = Boolean(path);
  if (!path) return;
  const dir = folderOf(path);
  if (dir) {
    const span = document.createElement('span');
    span.className = 'dir';
    span.textContent = dir;
    element.append(span);
  }
  element.append(path.slice(dir.length));
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
  showOpenPath(null);
  $('compile-info').hidden = true;
  $('engine').value = project.engine;
  $('auto-compile').checked = project.autoCompile;
  $('epoch').value = project.epoch ?? '';
  markSettings();
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
  showOpenPath(path);
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
  if (state.open) showOpenPath(state.open);
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
      showOpenPath(null);
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
  const nameOf = (text) => {
    const span = document.createElement('span');
    span.className = 'name';
    span.textContent = text;
    return span;
  };
  const list = (node, prefix, depth) => {
    const ul = document.createElement('ul');
    for (const [name, child] of node.folders) {
      const path = prefix + name;
      const collapsed = state.collapsed.has(path);
      const li = document.createElement('li');
      const row = document.createElement('div');
      row.className = `tree-row folder${collapsed ? '' : ' expanded'}`;
      row.dataset.path = path;
      row.title = `${path}/`;
      row.style.paddingLeft = `${8 + depth * 16}px`;
      row.append(icon('chevron', 'twisty'), icon(collapsed ? 'folder' : 'folder-open', 'ficon folder'), nameOf(name));
      if (state.selected === path) row.classList.add('selected');
      row.onclick = () => {
        state.selected = path;
        if (state.collapsed.has(path)) state.collapsed.delete(path);
        else state.collapsed.add(path);
        renderTree();
      };
      li.append(row);
      if (!collapsed) li.append(list(child, `${path}/`, depth + 1));
      ul.append(li);
    }
    for (const path of node.files) {
      const li = document.createElement('li');
      const row = document.createElement('div');
      row.className = 'tree-row file';
      row.dataset.path = path;
      row.title = path;
      row.style.paddingLeft = `${8 + depth * 16}px`;
      const space = document.createElement('span');
      space.className = 'twisty-space';
      row.append(space, fileIcon(path), nameOf(path.slice(prefix.length)));
      if (path === state.project.main) {
        row.classList.add('main');
        const tag = document.createElement('span');
        tag.className = 'tag';
        tag.textContent = 'main';
        tag.title = 'Compiled file';
        row.append(tag);
      }
      if (path === state.open) row.classList.add('open');
      if (path === state.selected) row.classList.add('selected');
      row.onclick = () => openFile(path);
      li.append(row);
      ul.append(li);
    }
    return ul;
  };
  if (!state.files.size) {
    const empty = document.createElement('p');
    empty.className = 'tree-empty';
    empty.textContent = 'No files. Create or upload one above.';
    $('tree').replaceChildren(empty);
    return;
  }
  $('tree').replaceChildren(list(root, '', 0));
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
    setStatus(`Main file ${entry} does not exist; select a .tex file and choose Set as main file in the Files menu.`);
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
    const summary = `${result.status === 0 ? 'Compiled' : 'Failed'} with ${result.engine} in `
      + `${(elapsed / 1000).toFixed(1)} s (${runs.join(', ')}${fetched}).${missing}`;
    setStatus(summary);
    const chip = [result.status === 0 ? `${(elapsed / 1000).toFixed(1)} s` : 'Failed'];
    if (result.stats.network) chip.push(`${formatBytes(result.stats.network)} downloaded`);
    setCompileInfo(result.status === 0, chip.join(' · '),
      `${summary}\nFinished at ${new Date().toLocaleTimeString()}.`);
    if (result.status !== 0 && diagnostics.length) showPanel('diagnostics');
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
    setCompileInfo(false, 'Error', error.message.split('\n')[0]);
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
    const message = document.createElement('span');
    message.className = 'message';
    message.textContent = item.message;
    li.append(icon(item.severity === 'error' ? 'error' : 'warning', 'sev'), message);
    if (item.path) {
      const where = document.createElement('span');
      where.className = 'where';
      where.textContent = `${item.path}:${item.line}`;
      li.append(where);
    }
    if (item.path && state.files.has(item.path)) {
      li.classList.add('linked');
      li.title = `Go to ${item.path}, line ${item.line}`;
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
  const warnings = items.length - errors;
  $('error-count').textContent = String(errors);
  $('error-count').title = `${errors} error${errors === 1 ? '' : 's'}`;
  $('error-count').hidden = !errors;
  $('warning-count').textContent = String(warnings);
  $('warning-count').title = `${warnings} warning${warnings === 1 ? '' : 's'}`;
  $('warning-count').hidden = !warnings;
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
on('engine', 'change', async (e) => {
  await updateProject({ engine: e.target.value });
  markSettings();
});
on('auto-compile', 'change', (e) => updateProject({ autoCompile: e.target.checked }));
on('epoch', 'change', async (e) => {
  const value = e.target.value.trim();
  await updateProject({ epoch: value === '' ? null : Math.max(0, Math.floor(Number(value))) });
  markSettings();
});
on('zoom-in', 'click', () => pdfView.setZoom(pdfView.zoom * 1.2));
on('zoom-out', 'click', () => pdfView.setZoom(pdfView.zoom / 1.2));
on('zoom-fit', 'click', () => pdfView.fit());
let pageFrame = 0;
$('pdf').addEventListener('scroll', () => {
  cancelAnimationFrame(pageFrame);
  pageFrame = requestAnimationFrame(updatePdfToolbar);
}, { passive: true });
// Keep "fit to width" fitted when the pane changes size.
let fitTimer;
let pdfWidth = $('pdf').clientWidth;
new ResizeObserver(() => {
  const width = $('pdf').clientWidth;
  if (width === pdfWidth) return;
  pdfWidth = width;
  clearTimeout(fitTimer);
  if (pdfView.fitWidth && pdfView.bytes) fitTimer = setTimeout(() => pdfView.fit(), 250);
}).observe($('pdf'));

// ----------------------------------------------------------------- panel --

const LAYOUT_KEY = 'texres-online:layout';
const layout = (() => {
  try {
    return JSON.parse(localStorage.getItem(LAYOUT_KEY)) ?? {};
  } catch {
    return {};
  }
})();
function applyLayout() {
  const style = document.documentElement.style;
  const set = (name, value, unit) => (value ? style.setProperty(name, `${value}${unit}`) : style.removeProperty(name));
  set('--files-w', layout.files, 'px');
  set('--preview-w', layout.preview, '%');
  set('--panel-h', layout.panel, 'px');
  document.body.classList.toggle('panel-collapsed', Boolean(layout.collapsed));
  $('panel-toggle').setAttribute('aria-expanded', String(!layout.collapsed));
  $('panel-toggle').title = layout.collapsed ? 'Show panel' : 'Hide panel';
}
const saveLayout = () => localStorage.setItem(LAYOUT_KEY, JSON.stringify(layout));
const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

function showPanel(id) {
  for (const tab of document.querySelectorAll('[data-panel]')) {
    tab.classList.toggle('active', tab.dataset.panel === id);
    $(tab.dataset.panel).hidden = tab.dataset.panel !== id;
  }
  if (layout.collapsed) {
    layout.collapsed = false;
    applyLayout();
    saveLayout();
  }
}
for (const tab of document.querySelectorAll('[data-panel]')) {
  tab.addEventListener('click', () => showPanel(tab.dataset.panel));
}
$('panel-toggle').addEventListener('click', () => {
  layout.collapsed = !layout.collapsed;
  applyLayout();
  saveLayout();
});

// Drag the 1px gutters to resize panes; double-click one to reset it.
for (const gutter of document.querySelectorAll('.gutter')) {
  const kind = gutter.dataset.resize;
  gutter.addEventListener('pointerdown', (down) => {
    if (kind === 'panel' && layout.collapsed) return;
    down.preventDefault();
    gutter.setPointerCapture(down.pointerId);
    gutter.classList.add('dragging');
    document.body.classList.add('resizing');
    document.body.classList.toggle('rows', kind === 'panel');
    const area = $('workspace').getBoundingClientRect();
    const filesWidth = $('files').getBoundingClientRect().width;
    const move = (event) => {
      if (kind === 'files') {
        layout.files = Math.round(clamp(event.clientX - area.left, 150, Math.min(420, area.width - 600)));
      } else if (kind === 'preview') {
        const max = area.width - filesWidth - 302;
        layout.preview = Number((clamp(area.right - event.clientX, 280, max) / area.width * 100).toFixed(2));
      } else {
        layout.panel = Math.round(clamp(window.innerHeight - event.clientY, 90, window.innerHeight * 0.6));
      }
      applyLayout();
    };
    const up = () => {
      gutter.removeEventListener('pointermove', move);
      gutter.removeEventListener('pointerup', up);
      gutter.removeEventListener('pointercancel', up);
      gutter.classList.remove('dragging');
      document.body.classList.remove('resizing', 'rows');
      saveLayout();
    };
    gutter.addEventListener('pointermove', move);
    gutter.addEventListener('pointerup', up);
    gutter.addEventListener('pointercancel', up);
  });
  gutter.addEventListener('dblclick', () => {
    delete layout[kind];
    applyLayout();
    saveLayout();
  });
}
applyLayout();

// ---------------------------------------------------- menus, settings, look --

/** A drop-down menu: opens from its button; a choice, Escape or a click elsewhere closes it. */
const menus = [];
function setupMenu(buttonId, menuId, onOpen = () => {}) {
  const button = $(buttonId);
  const menu = $(menuId);
  const items = () => [...menu.querySelectorAll('[role="menuitem"]')].filter((item) => !item.disabled);
  const toggle = (open) => {
    menu.hidden = !open;
    button.setAttribute('aria-expanded', String(open));
    if (open) {
      onOpen();
      items()[0]?.focus();
    }
  };
  button.addEventListener('click', () => toggle(menu.hidden));
  menu.addEventListener('click', (event) => {
    if (event.target.closest('[role="menuitem"]')) toggle(false);
  });
  menu.addEventListener('keydown', (event) => {
    const list = items();
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      list[(list.indexOf(document.activeElement) + step + list.length) % list.length]?.focus();
    } else if ((event.key === 'Enter' || event.key === ' ') && document.activeElement.tagName === 'LABEL') {
      event.preventDefault();
      document.activeElement.click();
    }
  });
  menus.push({ button, menu, close: () => toggle(false) });
}
setupMenu('project-menu-button', 'project-menu');
setupMenu('file-menu-button', 'file-menu', () => {
  const selected = state.selected;
  const isFile = Boolean(selected) && state.files.has(selected);
  $('file-menu-target').textContent = !selected ? 'Nothing selected' : isFile ? selected : `${selected}/`;
  $('rename-file').disabled = !selected;
  $('delete-file').disabled = !selected;
  $('set-main').disabled = !isFile || extension(selected) !== 'tex' || selected === state.project.main;
});

/** Mark the settings button while a setting differs from its default. */
function markSettings() {
  const { engine, epoch } = state.project;
  $('settings-button').classList.toggle('has-value', engine !== 'auto' || epoch != null);
}
function toggleSettings(open) {
  $('settings').hidden = !open;
  $('settings-button').setAttribute('aria-expanded', String(open));
  if (open) $('epoch').focus();
}
$('settings-button').addEventListener('click', () => toggleSettings($('settings').hidden));
document.addEventListener('pointerdown', (event) => {
  if (!$('settings').hidden && !event.target.closest('.popover-anchor')) toggleSettings(false);
  for (const { menu, close } of menus) {
    if (!menu.hidden && !event.target.closest('.menu-anchor')?.contains(menu)) close();
  }
});

const DESIGN_KEY = 'texres-online:design';
$('design').value = document.documentElement.dataset.design;
$('design').addEventListener('change', (e) => {
  document.documentElement.dataset.design = e.target.value;
  localStorage.setItem(DESIGN_KEY, e.target.value);
  editor.view.requestMeasure();
});

const THEME_KEY = 'texres-online:theme';
const systemDark = matchMedia('(prefers-color-scheme: dark)');
function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  const dark = theme === 'dark';
  $('theme-toggle').firstElementChild.firstElementChild.setAttribute('href', dark ? '#i-sun' : '#i-moon');
  $('theme-toggle').title = dark ? 'Switch to the light theme' : 'Switch to the dark theme';
}
setTheme(document.documentElement.dataset.theme);
$('theme-toggle').addEventListener('click', () => {
  const theme = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
  localStorage.setItem(THEME_KEY, theme);
  setTheme(theme);
});
systemDark.addEventListener('change', () => {
  if (!localStorage.getItem(THEME_KEY)) setTheme(systemDark.matches ? 'dark' : 'light');
});

if (/Mac|iPhone|iPad/.test(navigator.platform)) {
  for (const key of document.querySelectorAll('[data-kbd]')) key.textContent = key.textContent.replace('Ctrl ', '⌘');
  for (const element of document.querySelectorAll('[title*="Ctrl+"]')) element.title = element.title.replace('Ctrl+', '⌘');
}
window.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === 's') {
    e.preventDefault();
    compile();
  } else if (e.key === 'Escape' && !$('settings').hidden) {
    toggleSettings(false);
    $('settings-button').focus();
  } else if (e.key === 'Escape') {
    for (const { button, menu, close } of menus) {
      if (menu.hidden) continue;
      close();
      button.focus();
    }
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
