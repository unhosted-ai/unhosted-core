// Service worker for the unhosted web UI.
//
// Exists for two reasons:
//   1. Chrome on Android requires a fetch-handling service worker
//      before it offers "Install app" — without this file the PWA
//      can only be bookmarked, not installed.
//   2. Offline shell: the chat UI loads from cache when the phone
//      briefly loses the tunnel/LAN, instead of a browser error
//      page. API calls still fail visibly (the UI shows its own
//      offline state) — we never fake daemon responses.
//
// Strategy: network-first for everything, falling back to a cached
// copy of the static shell. /v1/* and /health are NEVER cached —
// chat turns, SSE streams, and status polls must hit the daemon.

const CACHE = "unhosted-shell-v1";
const SHELL = [
  "/",
  "/ui.css",
  "/ui.js",
  "/manifest.json",
  "/favicon.svg",
  "/app-icon.svg",
  "/icon-192.png",
  "/icon-512.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((c) => c.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  // Same-origin GETs only; the daemon API and any cross-origin
  // request pass straight through to the network untouched.
  if (event.request.method !== "GET" || url.origin !== location.origin) return;
  if (url.pathname.startsWith("/v1/") || url.pathname === "/health" || url.pathname === "/metrics") {
    return;
  }
  event.respondWith(
    fetch(event.request)
      .then((resp) => {
        // Keep the shell cache fresh on every successful load.
        if (resp.ok) {
          const copy = resp.clone();
          caches.open(CACHE).then((c) => c.put(event.request, copy));
        }
        return resp;
      })
      .catch(() =>
        caches.match(event.request).then((hit) => hit || caches.match("/")),
      ),
  );
});
