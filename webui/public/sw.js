/* EXMACHINA Service Worker：离线壳缓存（静态资源 cache-first），API/WS 永远走网络。
 * 注意：客户端可连接任意远程网关——缓存只针对当前源的静态壳，不缓存跨域 API 响应。 */
const CACHE = "exm-shell-v1";
const SHELL = ["/", "/index.html", "/manifest.webmanifest", "/icon.svg"];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches.keys().then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET") return;
  // API / WS / 资产：永远走网络（实时性与鉴权）
  if (url.pathname.startsWith("/api") || url.pathname === "/ws") return;
  // 静态资源：cache-first，命中即回；未命中回源并缓存
  e.respondWith(
    caches.match(e.request).then(
      (hit) =>
        hit ||
        fetch(e.request).then((resp) => {
          if (resp.ok && url.origin === location.origin) {
            const copy = resp.clone();
            caches.open(CACHE).then((c) => c.put(e.request, copy));
          }
          return resp;
        }),
    ),
  );
});
