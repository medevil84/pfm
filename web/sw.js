// PFM Service Worker
// Strategy:
//   - /api/* requests: network-first (never cache financial data)
//   - Static assets (CSS, JS, images, fonts): cache-first with versioned keys
//   - Navigation requests (HTML): network-first, fall back to cached index.html

const CACHE_NAME = 'pfm-static-v9';
const STATIC_ASSETS = [
  '/',
  '/manifest.webmanifest',
  '/icons/icon-192.png',
  '/icons/icon-512.png',
];

// Cache-first is right for assets that never change under the same URL: the
// vendored bundles, the fonts and the icons. It is wrong for our own code and
// catalogues, which have no hash in the URL: a browser that had already
// installed the worker would keep serving the previous app.js and i18n files
// for ever, and no amount of redeploying would reach it. Those go
// network-first, with the cache only as an offline fallback.
const IMMUTABLE_PREFIXES = ['/vendor/', '/icons/'];
const isImmutable = (pathname) =>
  IMMUTABLE_PREFIXES.some((p) => pathname.startsWith(p)) ||
  pathname === '/manifest.webmanifest';

// Install: pre-cache critical static assets
self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(STATIC_ASSETS))
  );
  self.skipWaiting();
});

// Activate: clean up old caches
self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys
          .filter((key) => key !== CACHE_NAME)
          .map((key) => caches.delete(key))
      )
    )
  );
  self.clients.claim();
});

// Fetch handler
self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);

  // Only handle same-origin requests.
  if (url.origin !== self.location.origin) {
    return;
  }

  // API calls: always network-first, never serve from cache.
  // Financial data must be fresh; on offline failure return a 503 JSON
  // so the client can queue the mutation.
  if (url.pathname.startsWith('/api/') || url.pathname === '/healthz' || url.pathname === '/readyz') {
    event.respondWith(handleNetworkFirst(event.request));
    return;
  }

  // Navigation requests (HTML pages): network-first with offline fallback
  // to the cached index.html (app shell).
  if (event.request.mode === 'navigate') {
    event.respondWith(handleNavigation(event.request));
    return;
  }

  // Immutable assets (vendor bundles, fonts, icons): cache-first.
  if (isImmutable(url.pathname)) {
    event.respondWith(handleCacheFirst(event.request));
    return;
  }

  // Everything else we ship ourselves (app.js, offline-queue.js, i18n
  // catalogues, CSS): network-first, so a redeploy is picked up, with the
  // cache kept up to date for offline use.
  event.respondWith(handleAppAsset(event.request));
});

// Our own assets: network-first, refreshing the cache on every success.
async function handleAppAsset(request) {
  try {
    // no-cache: il server risponde max-age=86400, senza revalidare il browser riserverebbe app.js/cataloghi vecchi.
    const response = await fetch(request, {cache: 'no-cache'});
    if (response.ok) {
      const cache = await caches.open(CACHE_NAME);
      cache.put(request, response.clone());
    }
    return response;
  } catch (err) {
    const cached = await caches.match(request);
    if (cached) {
      return cached;
    }
    return new Response('Offline', { status: 503, headers: { 'Content-Type': 'text/plain' } });
  }
}

// Network-first: try network, fall back to cache
async function handleNetworkFirst(request) {
  try {
    const response = await fetch(request);
    return response;
  } catch (err) {
    // Network failed: try cache.
    const cached = await caches.match(request);
    if (cached) {
      return cached;
    }
    // No cache entry: return a 503 JSON for API, or an opaque error for others.
    if (request.url.includes('/api/')) {
      return new Response(JSON.stringify({ error: 'offline' }), {
        status: 503,
        headers: { 'Content-Type': 'application/json' },
      });
    }
    return new Response('Offline', { status: 503, headers: { 'Content-Type': 'text/plain' } });
  }
}

// Navigation: network-first with offline index.html fallback
async function handleNavigation(request) {
  try {
    // no-cache: senza, il browser può riusare per un giorno l'index.html in
    // cache HTTP e dopo un aggiornamento non compaiono né i nuovi controlli né il nuovo CSS.
    const response = await fetch(request, {cache: 'no-cache'});
    // Cache successful navigation responses for offline use.
    if (response.ok) {
      const cache = await caches.open(CACHE_NAME);
      cache.put(request, response.clone());
    }
    return response;
  } catch (err) {
    const cached = await caches.match('/');
    if (cached) {
      return cached;
    }
    return new Response('Offline - no cached page available', {
      status: 503,
      headers: { 'Content-Type': 'text/plain' },
    });
  }
}

// Cache-first: serve from cache, update in background
async function handleCacheFirst(request) {
  const cached = await caches.match(request);
  if (cached) {
    return cached;
  }
  try {
    const response = await fetch(request);
    if (response.ok) {
      const cache = await caches.open(CACHE_NAME);
      cache.put(request, response.clone());
    }
    return response;
  } catch (err) {
    return new Response('Offline', { status: 503, headers: { 'Content-Type': 'text/plain' } });
  }
}