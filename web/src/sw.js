// Offline shell: the page, scripts, fonts and the pdf.js worker. The compiler worker
// caches the WebAssembly module, formats and package chunks itself.
const BUILD = '%BUILD%';
const SHELL = %SHELL%;
const CACHE = `texres-shell-${BUILD}`;

self.addEventListener('install', (event) => {
  // The page has just downloaded these content-addressed files; take them
  // from the HTTP cache instead of fetching them a second time.
  const requests = SHELL.map((path) => new Request(path, { cache: path === './' ? 'default' : 'force-cache' }));
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(requests)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    for (const name of await caches.keys()) {
      if (name.startsWith('texres-shell-') && name !== CACHE) await caches.delete(name);
    }
    await self.clients.claim();
  })());
});

const shellUrls = new Set(SHELL.map((path) => new URL(path, self.registration.scope).href));

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  url.search = '';
  if (request.mode === 'navigate') {
    // Network first so a new build is picked up; the cached page works offline.
    event.respondWith(fetch(request).then((response) => {
      if (response.ok) {
        const copy = response.clone();
        caches.open(CACHE).then((cache) => cache.put(new URL('./', self.registration.scope).href, copy));
      }
      return response;
    }).catch(async () => (await caches.match(new URL('./', self.registration.scope).href)) ?? Response.error()));
    return;
  }
  if (shellUrls.has(url.href)) {
    event.respondWith(caches.match(url.href).then((hit) => hit ?? fetch(request)));
  }
});
