// Rift Online service worker. The app shell and card list are network-first, so an
// online visit always gets the latest build, and offline the cached copy still runs
// decks, sealed and solo play. Card images are immutable, so they are cache-first.
const VERSION = '__BUILD__';
const SHELL = `rift-shell-${VERSION}`;
const IMG = 'rift-img-v1';
const IMG_MAX = 600;
const PRECACHE = [
  '/', '/style.css', '/fonts.css', '/manifest.webmanifest', '/cards.json',
  '/pkg/rift_client.js', '/pkg/rift_client_bg.wasm',
  '/icons/icon-192.png', '/icons/icon-512.png',
  '/fonts/barlow-400.woff2', '/fonts/barlow-500.woff2', '/fonts/barlow-600.woff2',
  '/fonts/barlow-condensed-500.woff2', '/fonts/barlow-condensed-600.woff2', '/fonts/barlow-condensed-700.woff2',
];

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(SHELL).then((c) => c.addAll(PRECACHE)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith('rift-shell-') && k !== SHELL).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

async function networkFirst(req, key) {
  const cache = await caches.open(SHELL);
  try {
    const res = await fetch(req);
    if (res.ok) cache.put(key, res.clone());
    return res;
  } catch (err) {
    const hit = await cache.match(key);
    if (hit) return hit;
    throw err;
  }
}

async function cacheFirst(req) {
  const cache = await caches.open(IMG);
  const hit = await cache.match(req);
  if (hit) return hit;
  const res = await fetch(req);
  if (res.ok) {
    await cache.put(req, res.clone());
    const keys = await cache.keys();
    for (const k of keys.slice(0, Math.max(0, keys.length - IMG_MAX))) await cache.delete(k);
  }
  return res;
}

self.addEventListener('fetch', (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== 'GET' || url.origin !== self.location.origin) return;
  const p = url.pathname;
  // Live and account traffic never comes from a cache.
  if (p === '/ws' || p.startsWith('/api/') || p.startsWith('/auth/') || p === '/healthz') return;
  if (p === '/img') return e.respondWith(cacheFirst(req));
  // Every page (including /?room=CODE invite links) is the same app shell.
  if (req.mode === 'navigate') return e.respondWith(networkFirst(req, '/'));
  e.respondWith(networkFirst(req, url.pathname));
});
