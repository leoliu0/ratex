// Projects, their files and their last build live in one IndexedDB database.
const DB_NAME = 'texres-online';
const DB_VERSION = 1;

let dbPromise;

function db() {
  dbPromise ??= new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const d = request.result;
      d.createObjectStore('projects', { keyPath: 'id' });
      const files = d.createObjectStore('files', { keyPath: ['project', 'path'] });
      files.createIndex('project', 'project');
      d.createObjectStore('outputs', { keyPath: 'project' });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return dbPromise;
}

function done(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function finished(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}

export async function listProjects() {
  const tx = (await db()).transaction('projects');
  const projects = await done(tx.objectStore('projects').getAll());
  return projects.sort((a, b) => b.modified - a.modified);
}

export async function getProject(id) {
  return done((await db()).transaction('projects').objectStore('projects').get(id));
}

export async function putProject(project) {
  const tx = (await db()).transaction('projects', 'readwrite');
  tx.objectStore('projects').put(project);
  return finished(tx);
}

/** Create a project with its files ({path, bytes}) in one transaction. */
export async function createProject(name, files, main) {
  const project = {
    id: crypto.randomUUID(),
    name,
    main,
    engine: 'auto',
    epoch: null,
    autoCompile: true,
    chunkHints: [],
    created: Date.now(),
    modified: Date.now(),
  };
  const tx = (await db()).transaction(['projects', 'files'], 'readwrite');
  tx.objectStore('projects').put(project);
  for (const file of files) {
    tx.objectStore('files').put({ project: project.id, path: file.path, bytes: file.bytes });
  }
  await finished(tx);
  return project;
}

export async function deleteProject(id) {
  const tx = (await db()).transaction(['projects', 'files', 'outputs'], 'readwrite');
  tx.objectStore('projects').delete(id);
  tx.objectStore('outputs').delete(id);
  const files = tx.objectStore('files');
  const keys = await done(files.index('project').getAllKeys(id));
  for (const key of keys) files.delete(key);
  return finished(tx);
}

export async function listFiles(project) {
  const tx = (await db()).transaction('files');
  const files = await done(tx.objectStore('files').index('project').getAll(project));
  return files.sort((a, b) => a.path.localeCompare(b.path));
}

export async function putFile(project, path, bytes) {
  const tx = (await db()).transaction('files', 'readwrite');
  tx.objectStore('files').put({ project, path, bytes });
  return finished(tx);
}

export async function deleteFile(project, path) {
  const tx = (await db()).transaction('files', 'readwrite');
  tx.objectStore('files').delete([project, path]);
  return finished(tx);
}

export async function renameFile(project, from, to) {
  const tx = (await db()).transaction('files', 'readwrite');
  const files = tx.objectStore('files');
  const file = await done(files.get([project, from]));
  if (!file) throw new Error(`no file ${from}`);
  files.delete([project, from]);
  files.put({ project, path: to, bytes: file.bytes });
  return finished(tx);
}

export async function getOutput(project) {
  return done((await db()).transaction('outputs').objectStore('outputs').get(project));
}

export async function putOutput(output) {
  const tx = (await db()).transaction('outputs', 'readwrite');
  tx.objectStore('outputs').put(output);
  return finished(tx);
}
