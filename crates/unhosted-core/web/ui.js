// unhosted — local web UI
// Talks to the daemon HTTP API on the same origin:
//   GET  /health     liveness
//   GET  /v1/status  connection details
//   POST /v1/run     streaming text/plain inference

const $ = (sel) => document.querySelector(sel);

// ---------------------------------------------------------------- auth bootstrap
//
// The daemon requires either a paired-peer signature OR a local bearer
// token for sensitive endpoints when bound to a non-loopback address.
// The UI never sees the peer-signed path; it just attaches the bearer
// on every `/v1/*` fetch.
//
// Token sources, in order:
//   1. `?t=<token>` query string — set when the user opens the URL the
//      daemon prints on startup for phone access. We stash it in
//      localStorage and strip it from the URL bar so it doesn't leak
//      into screenshots / shares.
//   2. localStorage — set by step 1 on a previous visit.
//   3. `GET /v1/auth/token` — only succeeds from loopback. On the
//      desktop shell / a browser on the same machine this is how the
//      first-time-ever flow gets its token without the user typing it.

const API_TOKEN_KEY = "unhosted-api-token";

(function bootstrapToken() {
  try {
    const params = new URLSearchParams(window.location.search);
    const fromUrl = params.get("t");
    if (fromUrl) {
      localStorage.setItem(API_TOKEN_KEY, fromUrl);
      params.delete("t");
      const cleanQuery = params.toString();
      const cleanUrl =
        window.location.pathname + (cleanQuery ? "?" + cleanQuery : "") + window.location.hash;
      window.history.replaceState({}, document.title, cleanUrl);
    }
  } catch (e) { /* localStorage / history may be unavailable */ }
})();

function getApiToken() {
  try { return localStorage.getItem(API_TOKEN_KEY); } catch (e) { return null; }
}

async function tryFetchLoopbackToken() {
  // Only attempt once per page load. If we're on the desktop shell or a
  // browser on the same host, this succeeds and primes the cache.
  try {
    const r = await fetch("/v1/auth/token", { cache: "no-store" });
    if (r.ok) {
      const j = await r.json();
      if (j && j.token) {
        localStorage.setItem(API_TOKEN_KEY, j.token);
        return j.token;
      }
    }
  } catch (e) { /* network down or non-loopback — fine, will retry on next reload */ }
  return null;
}

// Monkey-patch fetch for same-origin /v1/* calls to attach the bearer.
// Bare fetch() elsewhere (cross-origin, static assets) is untouched.
(function patchFetch() {
  const orig = window.fetch.bind(window);
  window.fetch = function (input, init) {
    try {
      const url = typeof input === "string" ? input : input.url;
      const isApi = url && (url.startsWith("/v1/") || url.startsWith(window.location.origin + "/v1/"));
      const isTokenFetch = url && url.includes("/v1/auth/token");
      if (isApi && !isTokenFetch) {
        const token = getApiToken();
        if (token) {
          init = init || {};
          const headers = new Headers(init.headers || {});
          if (!headers.has("Authorization")) {
            headers.set("Authorization", "Bearer " + token);
          }
          init.headers = headers;
        }
      }
    } catch (e) { /* fall through to original fetch */ }
    return orig(input, init);
  };
})();

// Kick off the loopback-token primer in the background. If it succeeds,
// subsequent fetches will pick it up; if it fails (we're on a phone),
// we either already have a token from `?t=` or the user will see 401s
// and the empty-state will hint at the URL to reopen.
if (!getApiToken()) {
  tryFetchLoopbackToken();
}

// ---------------------------------------------------------------- theme toggle

const THEME_KEY = "unhosted-theme";
const THEME_GLYPHS = { auto: "◐", dark: "☾", light: "☀" };
const THEME_LABELS = { auto: "theme · auto", dark: "theme · dark", light: "theme · light" };

(function initThemeToggle() {
  const btn = document.getElementById("theme-toggle");
  if (!btn) return;
  paintTheme(btn);
  btn.addEventListener("click", () => {
    const current = readTheme();
    const next = current === "auto" ? "dark" : current === "dark" ? "light" : "auto";
    if (next === "auto") {
      delete document.documentElement.dataset.theme;
      safeRemove(THEME_KEY);
    } else {
      document.documentElement.dataset.theme = next;
      safeSet(THEME_KEY, next);
    }
    paintTheme(btn);
  });
})();

function readTheme() {
  let stored = null;
  try { stored = localStorage.getItem(THEME_KEY); } catch (e) {}
  return stored === "dark" || stored === "light" ? stored : "auto";
}
function paintTheme(btn) {
  const t = readTheme();
  const glyph = btn.querySelector(".glyph");
  if (glyph) glyph.textContent = THEME_GLYPHS[t];
  btn.title = THEME_LABELS[t];
  btn.setAttribute("aria-label", THEME_LABELS[t]);
}
function safeSet(k, v) { try { localStorage.setItem(k, v); } catch (e) {} }
function safeRemove(k) { try { localStorage.removeItem(k); } catch (e) {} }

// ---------------------------------------------------------------- icons
//
// Inline SVG set. Tiny, theme-aware (stroke=currentColor), and shared
// across the UI. Use icon("name") to get an SVG string; place it inside
// any element and the surrounding color will paint it.

const ICONS = {
  plus:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><line x1="8" y1="3" x2="8" y2="13"/><line x1="3" y1="8" x2="13" y2="8"/></svg>',
  trash:   '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 4h10"/><path d="M5 4V3a1.5 1.5 0 0 1 1.5-1.5h3A1.5 1.5 0 0 1 11 3v1"/><path d="M4.5 4l.5 9a1.5 1.5 0 0 0 1.5 1.5h3A1.5 1.5 0 0 0 11 13l.5-9"/><line x1="7" y1="7" x2="7" y2="12"/><line x1="9" y1="7" x2="9" y2="12"/></svg>',
  send:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2L7 9"/><path d="M14 2L9.5 14.5L7 9L1.5 6.5L14 2Z"/></svg>',
  copy:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="5" y="5" width="9" height="9" rx="1.5"/><path d="M3 11V3.5A1.5 1.5 0 0 1 4.5 2H11"/></svg>',
  check:   '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 8.5L6.5 12L13 4.5"/></svg>',
  x:       '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><line x1="4" y1="4" x2="12" y2="12"/><line x1="12" y1="4" x2="4" y2="12"/></svg>',
  edit:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M11 2.5L13.5 5L5 13.5H2.5V11L11 2.5Z"/></svg>',
  device:  '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="2.5" y="3" width="11" height="8" rx="1"/><line x1="5.5" y1="14" x2="10.5" y2="14"/><line x1="8" y1="11" x2="8" y2="14"/></svg>',
  qr:      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" aria-hidden="true"><rect x="2" y="2" width="4.5" height="4.5"/><rect x="9.5" y="2" width="4.5" height="4.5"/><rect x="2" y="9.5" width="4.5" height="4.5"/><rect x="9.5" y="9.5" width="2" height="2"/><rect x="12.5" y="12.5" width="1.5" height="1.5"/></svg>',
  link:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M7 9.5L9 7.5"/><path d="M9.5 4.5L11 3a2.5 2.5 0 0 1 3.5 3.5L13 8"/><path d="M6.5 11.5L5 13a2.5 2.5 0 0 1-3.5-3.5L3 8"/></svg>',
  unlink:  '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9.5 4.5L11 3a2.5 2.5 0 0 1 3.5 3.5L13 8"/><path d="M6.5 11.5L5 13a2.5 2.5 0 0 1-3.5-3.5L3 8"/><line x1="2" y1="2" x2="14" y2="14"/></svg>',
  chat:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.5 4A1.5 1.5 0 0 1 4 2.5h8A1.5 1.5 0 0 1 13.5 4v6A1.5 1.5 0 0 1 12 11.5H6L3 14.5V11.5A1.5 1.5 0 0 1 1.5 10V4"/></svg>',
  network: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="8" cy="3" r="1.5"/><circle cx="3" cy="12" r="1.5"/><circle cx="13" cy="12" r="1.5"/><line x1="8" y1="4.5" x2="3" y2="10.5"/><line x1="8" y1="4.5" x2="13" y2="10.5"/></svg>',
  globe:   '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" aria-hidden="true"><circle cx="8" cy="8" r="6"/><ellipse cx="8" cy="8" rx="3" ry="6"/><line x1="2" y1="8" x2="14" y2="8"/></svg>',
  shield:  '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 1.5L2.5 4v5c0 3 2.5 5 5.5 5.5C11 14 13.5 12 13.5 9V4L8 1.5z"/></svg>',
  sun:     '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" aria-hidden="true"><circle cx="8" cy="8" r="3"/><line x1="8" y1="1.5" x2="8" y2="3"/><line x1="8" y1="13" x2="8" y2="14.5"/><line x1="1.5" y1="8" x2="3" y2="8"/><line x1="13" y1="8" x2="14.5" y2="8"/><line x1="3.5" y1="3.5" x2="4.5" y2="4.5"/><line x1="11.5" y1="11.5" x2="12.5" y2="12.5"/><line x1="3.5" y1="12.5" x2="4.5" y2="11.5"/><line x1="11.5" y1="4.5" x2="12.5" y2="3.5"/></svg>',
  moon:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M13 9.5A6 6 0 0 1 6.5 3a4.5 4.5 0 1 0 6.5 6.5z"/></svg>',
  auto:    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="8" cy="8" r="5.5"/><path d="M8 2.5v11" stroke-dasharray="0"/><path d="M8 2.5a5.5 5.5 0 0 1 0 11z" fill="currentColor"/></svg>',
  refresh: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 4.5h4v4"/><path d="M2 8a6 6 0 0 1 10-4.5L14 5"/><path d="M14 11.5h-4v-4"/><path d="M14 8a6 6 0 0 1-10 4.5L2 11"/></svg>',
  brand:   '<svg viewBox="0 0 100 100" fill="none" stroke="currentColor" stroke-width="3" aria-hidden="true"><circle cx="50" cy="50" r="44" stroke-dasharray="2 6"/><circle cx="50" cy="50" r="28"/><circle cx="50" cy="50" r="12" fill="currentColor" stroke="none"/></svg>',
};

function icon(name) { return ICONS[name] || ""; }

// ---------------------------------------------------------------- chat grouping
//
// Chats are kept newest-first in store.chats. The sidebar groups them
// into time buckets to make a long list scannable. The buckets are
// computed from the chat's most-recent activity, not its creation time
// — a week-old conversation that got a new message today is "today".

function chatActivityTs(chat) {
  // Most recent activity timestamp. Falls back to createdAt for old
  // chats persisted before we tracked message timestamps.
  const last = chat.messages && chat.messages.length > 0 ? chat.messages[chat.messages.length - 1] : null;
  return (last && last.ts) || chat.updatedAt || chat.createdAt || 0;
}

function startOfDay(ts) {
  const d = new Date(ts);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function chatGroup(chat) {
  const now = Date.now();
  const todayStart = startOfDay(now);
  const yesterdayStart = todayStart - 24 * 60 * 60 * 1000;
  const weekStart = todayStart - 6 * 24 * 60 * 60 * 1000;
  const monthStart = todayStart - 29 * 24 * 60 * 60 * 1000;
  const ts = chatActivityTs(chat);
  if (ts >= todayStart) return { key: "today", label: "today", rank: 0 };
  if (ts >= yesterdayStart) return { key: "yesterday", label: "yesterday", rank: 1 };
  if (ts >= weekStart) return { key: "week", label: "earlier this week", rank: 2 };
  if (ts >= monthStart) return { key: "month", label: "earlier this month", rank: 3 };
  return { key: "older", label: "older", rank: 4 };
}

// ---------------------------------------------------------------- elements

const els = {
  composer: $("#composer"),
  prompt: $("#prompt"),
  send: $("#send"),
  stop: $("#stop"),
  conversation: $("#conversation"),
  empty: $("#empty-state"),
  meta: $("#composer-meta"),
  statusDot: $("#status-dot"),
  statusLabel: $("#status-label"),
  scroll: $("#scroll"),
  topic: $("#topic-label"),
  connModel: $("#conn-model"),
  connUpstream: $("#conn-upstream"),
  connNode: $("#conn-node"),
  peersBlock: $("#peers-block"),
  peerList: $("#peer-list"),
  newChat: $("#new-chat"),
  clearChats: $("#clear-chats"),
  chatList: $("#chat-list"),
  discoveredSection: $("#discovered-section"),
  discoveredList: $("#discovered-list"),
  tunnelToggle: $("#tunnel-toggle"),
  tunnelLabel: $("#tunnel-toggle-label"),
  tunnelStatus: $("#tunnel-status-line"),
  tunnelProgress: $("#tunnel-progress"),
  tunnelProgressBar: $("#tunnel-progress-bar"),
  tunnelLink: $("#tunnel-link"),
  tunnelUrl: $("#tunnel-url"),
  tunnelCopy: $("#tunnel-copy"),
  tunnelWarn: $("#tunnel-warn"),
  phoneQrCanvas: $("#phone-qr-canvas"),
  phoneQrHint:   $("#phone-qr-hint"),
  phoneSection:  $("#phone-section"),
  developerOpen: $("#developer-open"),
  developerModal: $("#developer-modal"),
  developerModalClose: $("#developer-modal-close"),
  devBaseUrl: $("#dev-base-url"),
  devToken: $("#dev-token"),
  devTunnelNote: $("#dev-tunnel-note"),
  devTunnelUrl: $("#dev-tunnel-url"),
  devSnippetCode: $("#dev-snippet-code"),
  devSnippetCopy: $("#dev-snippet-copy"),
  memorySection: $("#memory-section"),
  memoryToggle: $("#memory-toggle"),
  memoryToggleLabel: $("#memory-toggle-label"),
  memoryStatus: $("#memory-status-line"),
  memoryManage: $("#memory-manage"),
  memoryModal: $("#memory-modal"),
  memoryModalClose: $("#memory-modal-close"),
  memoryList: $("#memory-list"),
  memoryClearAll: $("#memory-clear-all"),
  memoryAddInput: $("#memory-add-input"),
  memoryAddSubmit: $("#memory-add-submit"),
};

let streaming = false;
let currentAbort = null;

function setSendMode(mode) {
  // mode: "send" | "stop"
  if (mode === "stop") {
    els.send.hidden = true;
    els.stop.hidden = false;
  } else {
    els.send.hidden = false;
    els.stop.hidden = true;
  }
}

// ---------------------------------------------------------------- chat store
//
// The canonical store lives on the daemon at /v1/chats. Every device
// paired to this daemon (laptop browser, phone PWA over LAN, …) sees
// the same conversation list. The browser keeps an in-memory mirror
// so rendering stays synchronous; mutations write through to the
// daemon (`putChat`), and we re-fetch on tab-visibility to pick up
// changes another device made.

const LEGACY_STORE_KEY = "unhosted-chats";
const MIGRATED_KEY = "unhosted-chats-migrated";
const ACTIVE_KEY = "unhosted-active-id";
const MAX_CHATS = 50;

// activeId is local UI state — each device remembers which chat *it*
// had open. Not part of the synced history.
const store = { activeId: safeGetActive(), chats: [] };

function safeGetActive() {
  try { return localStorage.getItem(ACTIVE_KEY); } catch (e) { return null; }
}
function setActiveId(id) {
  store.activeId = id;
  try {
    if (id) localStorage.setItem(ACTIVE_KEY, id);
    else localStorage.removeItem(ACTIVE_KEY);
  } catch (e) {}
}

async function fetchChats() {
  const r = await fetch("/v1/chats", { cache: "no-store" });
  if (!r.ok) throw new Error("fetch /v1/chats " + r.status);
  const j = await r.json();
  return j.chats || [];
}

async function putChat(chat) {
  try {
    const r = await fetch(`/v1/chats/${encodeURIComponent(chat.id)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(chat),
    });
    if (!r.ok) console.warn("save chat failed", r.status);
  } catch (e) { console.warn("save chat error", e); }
}

async function deleteChatRemote(id) {
  try {
    await fetch(`/v1/chats/${encodeURIComponent(id)}`, { method: "DELETE" });
  } catch (e) { console.warn("delete chat error", e); }
}

async function clearChatsRemote() {
  try {
    await fetch("/v1/chats", { method: "DELETE" });
  } catch (e) { console.warn("clear chats error", e); }
}

async function bootstrapChats() {
  try {
    store.chats = await fetchChats();
  } catch (e) {
    console.warn("chats bootstrap failed; running with empty list", e);
    store.chats = [];
  }

  // One-time migration: a returning user from the localStorage era has
  // their old chats sitting in localStorage but nothing on the daemon
  // yet. Upload them so phones / paired devices see the same history.
  let migrated = false;
  try { migrated = localStorage.getItem(MIGRATED_KEY) === "1"; } catch (e) {}
  if (!migrated && store.chats.length === 0) {
    let raw = null;
    try { raw = localStorage.getItem(LEGACY_STORE_KEY); } catch (e) {}
    if (raw) {
      try {
        const parsed = JSON.parse(raw);
        const legacy = (parsed && parsed.chats) || [];
        if (legacy.length > 0) {
          for (const c of legacy) await putChat(c);
          store.chats = await fetchChats();
          if (parsed.activeId && !store.activeId) setActiveId(parsed.activeId);
          console.info(`migrated ${legacy.length} chats from localStorage to daemon`);
        }
      } catch (e) { console.warn("legacy migration failed", e); }
    }
    try { localStorage.setItem(MIGRATED_KEY, "1"); } catch (e) {}
  }

  // Reconcile activeId: clear if it points at a chat that no longer exists.
  if (store.activeId && !store.chats.find((c) => c.id === store.activeId)) {
    setActiveId(null);
  }
}

// Pull fresh state from the daemon. Used on tab-visibility so a chat
// edited on another device shows up when you switch back. Skip while
// streaming on this device so we don't trample the in-progress message.
async function refreshChatsFromServer() {
  if (streaming) return;
  try {
    const fresh = await fetchChats();
    store.chats = fresh;
    if (store.activeId && !fresh.find((c) => c.id === store.activeId)) {
      setActiveId(fresh[0]?.id || null);
    }
    renderChatList();
    renderActiveChat();
  } catch (e) { /* keep showing what we have */ }
}

function newChatId() {
  return "c_" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
}

function activeChat() {
  return store.chats.find((c) => c.id === store.activeId) || null;
}

function ensureActiveChat() {
  let chat = activeChat();
  if (!chat) {
    chat = { id: newChatId(), title: "new chat", createdAt: Date.now(), messages: [] };
    store.chats.unshift(chat);
    setActiveId(chat.id);
    if (store.chats.length > MAX_CHATS) store.chats.length = MAX_CHATS;
    putChat(chat);
  }
  return chat;
}

function startNewChat() {
  // Only create a fresh entry if the current chat actually has messages —
  // otherwise just reuse the empty one (avoids piling up blank chats).
  const current = activeChat();
  if (current && current.messages.length === 0) {
    renderActiveChat();
    return;
  }
  const chat = { id: newChatId(), title: "new chat", createdAt: Date.now(), messages: [] };
  store.chats.unshift(chat);
  setActiveId(chat.id);
  if (store.chats.length > MAX_CHATS) store.chats.length = MAX_CHATS;
  putChat(chat);
  renderChatList();
  renderActiveChat();
  els.prompt.focus();
}

function switchToChat(id) {
  if (!store.chats.some((c) => c.id === id)) return;
  setActiveId(id);
  renderChatList();
  renderActiveChat();
}

// In-app confirm dialog. window.confirm() returns false in our WebView
// without ever rendering anything — the native dialog isn't honored — so
// every confirm-gated action (delete chat, clear chats) was silently aborted.
// This Promise-based helper uses the #confirm-modal markup instead, which
// works the same in any browser or WebView.
const confirmEls = {
  modal: $("#confirm-modal"),
  title: $("#confirm-title"),
  message: $("#confirm-message"),
  ok: $("#confirm-ok"),
  cancel: $("#confirm-cancel"),
};

function confirmDialog({ title = "are you sure?", message = "", confirmLabel = "ok", danger = false } = {}) {
  return new Promise((resolve) => {
    if (!confirmEls.modal) { resolve(window.confirm(message || title)); return; }
    confirmEls.title.textContent = title;
    confirmEls.message.textContent = message;
    confirmEls.ok.textContent = confirmLabel;
    confirmEls.ok.classList.toggle("btn-danger", !!danger);
    confirmEls.ok.classList.toggle("btn-primary", !danger);
    confirmEls.modal.hidden = false;
    const cleanup = () => {
      confirmEls.modal.hidden = true;
      confirmEls.ok.removeEventListener("click", onOk);
      confirmEls.cancel.removeEventListener("click", onCancel);
      confirmEls.modal.removeEventListener("click", onBackdrop);
      document.removeEventListener("keydown", onKey);
    };
    const onOk = () => { cleanup(); resolve(true); };
    const onCancel = () => { cleanup(); resolve(false); };
    const onBackdrop = (e) => { if (e.target === confirmEls.modal) onCancel(); };
    const onKey = (e) => {
      if (e.key === "Escape") onCancel();
      else if (e.key === "Enter") onOk();
    };
    confirmEls.ok.addEventListener("click", onOk);
    confirmEls.cancel.addEventListener("click", onCancel);
    confirmEls.modal.addEventListener("click", onBackdrop);
    document.addEventListener("keydown", onKey);
    setTimeout(() => confirmEls.cancel.focus(), 0);
  });
}

async function deleteChat(id) {
  const idx = store.chats.findIndex((c) => c.id === id);
  if (idx < 0) return;
  const chat = store.chats[idx];
  const label = chat.title && chat.title !== "new chat" ? `"${truncate(chat.title, 32)}"` : "this chat";
  const ok = await confirmDialog({
    title: "delete chat?",
    message: `delete ${label}? this can't be undone.`,
    confirmLabel: "delete",
    danger: true,
  });
  if (!ok) return;
  store.chats.splice(idx, 1);
  if (store.activeId === id) {
    setActiveId(store.chats.length > 0 ? store.chats[0].id : null);
  }
  deleteChatRemote(id);
  renderChatList();
  renderActiveChat();
}

// ---------------------------------------------------------------- rendering

function renderChatList() {
  els.chatList.innerHTML = "";
  if (els.clearChats) els.clearChats.hidden = store.chats.length === 0;
  if (store.chats.length === 0) {
    const li = document.createElement("li");
    li.className = "chat-item empty";
    li.textContent = "no chats yet";
    els.chatList.append(li);
    return;
  }

  // Group by recency. store.chats is already newest-first, so we walk
  // in order and emit a group header each time the bucket changes.
  let currentGroup = null;
  for (const chat of store.chats) {
    const group = chatGroup(chat);
    if (currentGroup !== group.key) {
      currentGroup = group.key;
      const header = document.createElement("li");
      header.className = "chat-group-head";
      header.textContent = group.label;
      els.chatList.append(header);
    }
    els.chatList.append(buildChatItem(chat));
  }
}

function buildChatItem(chat) {
  const li = document.createElement("li");
  li.className = "chat-item" + (chat.id === store.activeId ? " active" : "");
  li.dataset.chatId = chat.id;

  // Left: brand glyph + title, fills the row, switches chat on click.
  const button = document.createElement("button");
  button.type = "button";
  button.className = "chat-item-main";
  button.innerHTML =
    '<span class="chat-icon" aria-hidden="true">' + icon("chat") + "</span>" +
    '<span class="chat-title"></span>';
  button.querySelector(".chat-title").textContent = chat.title || "new chat";
  button.addEventListener("click", () => switchToChat(chat.id));
  li.append(button);

  // Right: trash icon, hover-revealed, deletes the chat after confirm.
  const del = document.createElement("button");
  del.type = "button";
  del.className = "chat-item-del";
  del.title = "delete chat";
  del.setAttribute("aria-label", "delete chat");
  del.innerHTML = icon("trash");
  del.addEventListener("click", (e) => {
    e.stopPropagation();
    deleteChat(chat.id);
  });
  li.append(del);

  return li;
}

function renderActiveChat() {
  const chat = activeChat();
  els.conversation.innerHTML = "";
  if (!chat || chat.messages.length === 0) {
    if (els.empty) els.empty.style.display = "";
    els.topic.textContent = "new chat";
    return;
  }
  if (els.empty) els.empty.style.display = "none";
  els.topic.textContent = truncate(chat.title || chat.messages[0].text, 48);
  for (const msg of chat.messages) {
    renderMessage(msg);
  }
  els.scroll.scrollTop = els.scroll.scrollHeight;
}

const ASSISTANT_MARK_SVG = `<svg class="mark" viewBox="0 0 100 100" fill="none" stroke="currentColor" stroke-width="7" aria-hidden="true">
  <circle cx="50" cy="50" r="44" stroke-dasharray="2 6"/>
  <circle cx="50" cy="50" r="28"/>
  <circle cx="50" cy="50" r="12" fill="currentColor" stroke="none"/>
</svg>`;

function renderMessage(msg) {
  const node = document.createElement("article");
  node.className = `msg ${msg.role}`;

  const who = document.createElement("div");
  who.className = "who";
  if (msg.role === "assistant") {
    who.innerHTML = `${ASSISTANT_MARK_SVG}<span>unhosted</span>`;
  } else {
    who.innerHTML = `<span class="dot"></span><span>you</span>`;
  }

  const body = document.createElement("div");
  body.className = "body";
  body.textContent = msg.text;

  node.append(who, body);

  if (msg.role === "assistant" && msg.stats) {
    node.append(buildStats(msg.stats));
  }

  els.conversation.append(node);
  return node;
}

function buildStats(stats) {
  const el = document.createElement("div");
  el.className = "stats";
  let servedHtml;
  if (stats.servedBy && stats.servedBy.startsWith("peer:")) {
    const name = stats.servedBy.slice("peer:".length);
    servedHtml = `<span class="served-peer">served by peer · ${escapeHtml(name)}</span>`;
  } else if (stats.servedBy) {
    servedHtml = `served by ${stats.servedBy}`;
  } else {
    servedHtml = "served by local";
  }
  el.innerHTML = `
    <span>${servedHtml}</span>
    <span>~${stats.tokens} tok</span>
    <span>${stats.seconds.toFixed(1)} s</span>
    <span>~${stats.tokPerSec} tok/s</span>
  `;
  return el;
}

// ---------------------------------------------------------------- status panel

async function refreshStatus() {
  try {
    const r = await fetch("/v1/status", { cache: "no-store" });
    if (!r.ok) throw new Error(`${r.status}`);
    renderStatus(await r.json());
  } catch (e) {
    setStatusDot("err", "node unreachable");
    els.connModel.textContent = "—";
    els.connUpstream.textContent = "—";
    els.connNode.textContent = "—";
  }
}

// Gates the share UI (tunnel toggle, QR panel, developer panel) based on
// whether the daemon can actually serve chat. There's no point handing
// out a public URL when the underlying LLM is unreachable. Called from
// renderStatus on every poll so it stays in sync as backends come up.
let shareGatedReason = null;
function setShareGated(gated, ctx = {}) {
  const reason = gated
    ? (ctx.discovered && ctx.discovered.length > 0
        ? "no local LLM — pair a peer on your network first"
        : "no LLM detected — start ollama, llama-server, or lm studio to enable sharing")
    : null;
  // Only mutate DOM when the gate-state or reason actually changed; avoids
  // hammering the layout on every 8s status poll.
  if (reason === shareGatedReason) return;
  shareGatedReason = reason;
  if (els.tunnelToggle) els.tunnelToggle.disabled = !!gated;
  if (els.tunnelStatus && gated) {
    els.tunnelStatus.textContent = reason;
    els.tunnelStatus.dataset.state = "gated";
  }
  if (els.tunnelLink) els.tunnelLink.hidden = gated || els.tunnelLink.hidden;
  if (els.tunnelWarn) els.tunnelWarn.hidden = gated || els.tunnelWarn.hidden;
  if (els.tunnelProgress) els.tunnelProgress.hidden = gated || els.tunnelProgress.hidden;
  if (els.phoneSection) els.phoneSection.hidden = gated;
  const developerSection = document.getElementById("developer-section");
  if (developerSection) developerSection.hidden = gated;
}

function renderStatus(s) {
  // Compute LLM readiness from status. The share/tunnel UI gates on
  // this — there's no point handing out a public URL to a daemon that
  // can't actually serve chat. Routes:
  //   1. configured upstream is reachable          → ready, local
  //   2. another known backend is reachable        → ready, will auto-route
  //   3. at least one paired peer is live          → ready, will proxy
  //   4. otherwise                                 → NOT ready, hide share
  const localReady = !!s.upstream.reachable;
  const altReady = (s.upstream.backends || []).some((b) => b.reachable);
  const peerReady = (s.peers || []).some((p) => p.live || p.trusted);
  const llmReady = localReady || altReady || peerReady;
  setShareGated(!llmReady, { localReady, altReady, peerReady, discovered: s.discovered });

  if (s.upstream.reachable) {
    setStatusDot("ok", `node ready · v${s.node.version}`);
    els.connModel.textContent = s.upstream.model || "(model not reported)";
    els.connUpstream.textContent = s.upstream.url.replace(/^https?:\/\//, "");
  } else {
    // Configured upstream is down. If a different backend is alive on
    // its default port, surface that — the daemon will auto-route to
    // it on the next request, but the user should *see* that's why
    // chat suddenly works again.
    const alt = (s.upstream.backends || []).find((b) => b.reachable);
    if (alt) {
      setStatusDot("ok", `${alt.name} reachable · auto-routing to ${alt.url.replace(/^https?:\/\//, "")}`);
      els.connModel.textContent = `(via ${alt.name})`;
      els.connUpstream.textContent = alt.url.replace(/^https?:\/\//, "");
    } else {
      setStatusDot("warn", "no runtime — start llama-server, ollama, or lm studio");
      els.connModel.textContent = "no model loaded";
      els.connUpstream.textContent = s.upstream.url.replace(/^https?:\/\//, "");
    }
  }
  els.connNode.textContent = s.node.addr;

  if (s.peers && s.peers.length > 0) {
    els.peersBlock.hidden = false;
    els.peerList.innerHTML = "";
    for (const peer of s.peers) {
      const li = document.createElement("li");

      const left = document.createElement("div");
      // Flex column that can SHRINK below its content width — without
      // `min-width: 0` (set on `.peer-info` in css) a long IPv6 address
      // pushes the whole row wider than the sidebar and the `unpair`
      // button clips off the right edge.
      left.className = "peer-info";

      const nameRow = document.createElement("span");
      nameRow.className = "pname";
      const nameText = document.createElement("span");
      nameText.className = "pname-text";
      nameText.textContent = peer.name;
      // Full hostname on hover — when truncated, the user still needs
      // to be able to read it (especially helpful when two devices
      // share a prefix).
      nameText.title = peer.name;
      nameRow.appendChild(nameText);
      // Trust badge: red dot + "trusted" for paired-with-pubkey peers,
      // muted "lan" for unauthenticated LAN peers.
      const badge = document.createElement("span");
      badge.className = peer.trusted ? "peer-badge trusted" : "peer-badge lan";
      badge.textContent = peer.trusted ? "trusted" : "lan";
      nameRow.appendChild(badge);

      const addr = document.createElement("span");
      addr.className = "paddr";
      addr.textContent = peer.addr;
      // IPv6 link-local addresses are long. Truncate visually but
      // keep the full string available via title for copy/inspection.
      addr.title = peer.addr;
      left.append(nameRow, addr);

      const unpair = document.createElement("button");
      unpair.className = "unpair";
      unpair.title = `unpair ${peer.name}`;
      unpair.setAttribute("aria-label", `unpair ${peer.name}`);
      unpair.innerHTML = icon("unlink") + '<span class="unpair-label">unpair</span>';
      unpair.addEventListener("click", () => unpairPeer(peer.name));

      li.append(left, unpair);
      els.peerList.append(li);
    }
  } else {
    els.peersBlock.hidden = true;
  }

  // discovered (unpaired) peers
  if (els.discoveredSection) {
    const list = s.discovered || [];
    if (list.length === 0) {
      els.discoveredSection.hidden = true;
      els.discoveredList.innerHTML = "";
    } else {
      els.discoveredSection.hidden = false;
      els.discoveredList.innerHTML = "";
      for (const d of list) {
        const li = document.createElement("li");

        const left = document.createElement("div");
        left.className = "dname";
        const name = document.createElement("span");
        name.className = "name";
        name.textContent = d.name;
        const addr = document.createElement("span");
        addr.className = "addr";
        addr.textContent = d.addr;
        left.append(name, addr);

        const pair = document.createElement("button");
        pair.className = "pair";
        pair.title = `pair with ${d.name}`;
        pair.setAttribute("aria-label", `pair with ${d.name}`);
        pair.innerHTML = icon("link") + '<span class="pair-label">pair</span>';
        pair.addEventListener("click", () => pairPeer(d));

        li.append(left, pair);
        els.discoveredList.append(li);
      }
    }
  }
}

async function pairPeer(d) {
  try {
    const r = await fetch("/v1/peers", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: d.name, addr: d.addr }),
    });
    if (!r.ok) throw new Error(`${r.status}`);
    await refreshStatus();
  } catch (e) {
    console.error("pair failed", e);
    alert("pair failed: " + (e && e.message ? e.message : "unknown"));
  }
}

async function unpairPeer(name) {
  try {
    const r = await fetch(`/v1/peers/${encodeURIComponent(name)}`, { method: "DELETE" });
    if (!r.ok) throw new Error(`${r.status}`);
    await refreshStatus();
  } catch (e) {
    console.error("unpair failed", e);
  }
}

function setStatusDot(state, label) {
  els.statusDot.dataset.state = state;
  els.statusLabel.textContent = label;
}

refreshStatus();
setInterval(refreshStatus, 15000);

// ---------------------------------------------------------------- composer

function autoresize() {
  els.prompt.style.height = "auto";
  els.prompt.style.height = Math.min(els.prompt.scrollHeight, 180) + "px";
  els.send.disabled = streaming || els.prompt.value.trim().length === 0;
}

els.prompt.addEventListener("input", autoresize);
els.prompt.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    if (!els.send.disabled) els.composer.requestSubmit();
  }
});

document.querySelectorAll(".chip[data-suggest]").forEach((btn) => {
  btn.addEventListener("click", () => {
    els.prompt.value = btn.dataset.suggest;
    autoresize();
    els.prompt.focus();
  });
});

els.newChat.addEventListener("click", startNewChat);

if (els.clearChats) {
  els.clearChats.addEventListener("click", async () => {
    if (store.chats.length === 0) return;
    const n = store.chats.length;
    const ok = await confirmDialog({
      title: "clear all chats?",
      message: `clear all ${n} chat${n === 1 ? "" : "s"}? this can't be undone.`,
      confirmLabel: "clear all",
      danger: true,
    });
    if (!ok) return;
    store.chats = [];
    setActiveId(null);
    clearChatsRemote();
    renderChatList();
    renderActiveChat();
  });
}
autoresize();

// ---------------------------------------------------------------- submit

els.composer.addEventListener("submit", async (e) => {
  e.preventDefault();
  const prompt = els.prompt.value.trim();
  if (!prompt || streaming) return;

  const chat = ensureActiveChat();
  const now = Date.now();
  const userMsg = { role: "user", text: prompt, ts: now };
  chat.messages.push(userMsg);
  chat.updatedAt = now;
  if (chat.messages.length === 1) {
    chat.title = truncate(prompt, 48);
    els.topic.textContent = chat.title;
  }
  // Move the active chat to the top of the list — the recency groups
  // ("today" etc.) only mean something if the list reflects activity
  // order, not creation order.
  const idx = store.chats.findIndex((c) => c.id === chat.id);
  if (idx > 0) {
    store.chats.splice(idx, 1);
    store.chats.unshift(chat);
  }
  // Snapshot the chat now (title + user msg). The full save with the
  // assistant reply happens after streaming completes.
  putChat(chat);
  renderChatList();

  if (els.empty) els.empty.style.display = "none";
  renderMessage(userMsg);

  els.prompt.value = "";
  autoresize();

  const assistantMsg = { role: "assistant", text: "", ts: Date.now() };
  chat.messages.push(assistantMsg);
  const assistantNode = renderMessage(assistantMsg);
  assistantNode.classList.add("streaming");

  streaming = true;
  currentAbort = new AbortController();
  setSendMode("stop");
  els.meta.innerHTML = '<span class="info">streaming…</span>';

  const startedAt = performance.now();
  let bytes = 0;

  try {
    const servedBy = await streamPrompt(prompt, (chunk) => {
      assistantMsg.text += chunk;
      const bodyEl = assistantNode.querySelector(".body");
      bodyEl.textContent = assistantMsg.text;
      bytes += chunk.length;
      els.scroll.scrollTop = els.scroll.scrollHeight;
    }, currentAbort.signal);
    const elapsedMs = performance.now() - startedAt;
    const stats = {
      servedBy,
      tokens: Math.max(1, Math.round(bytes / 4)),
      seconds: elapsedMs / 1000,
    };
    stats.tokPerSec = stats.seconds > 0 ? (stats.tokens / stats.seconds).toFixed(1) : "—";
    assistantMsg.stats = stats;
    assistantNode.append(buildStats(stats));
  } catch (err) {
    if (err && (err.name === "AbortError" || err.aborted)) {
      // User pressed stop — keep whatever streamed so far, mark it.
      assistantMsg.text += assistantMsg.text ? "\n[stopped]" : "[stopped]";
      const bodyEl = assistantNode.querySelector(".body");
      bodyEl.textContent = assistantMsg.text;
    } else {
      showError(assistantNode, err);
      // Compact inline summary for the saved transcript — humans never see
      // the JSON body, just a single legible line. The rich banner lives
      // inside the DOM (see showError) and isn't persisted.
      const info = err && err.info;
      if (info && info.kind === "upstream_offline") {
        assistantMsg.text += "\n[no model runtime is running — start llama-server, ollama, or lm studio]";
      } else {
        assistantMsg.text += `\n[error: ${err && err.message ? err.message : "request failed"}]`;
      }
    }
  } finally {
    assistantNode.classList.remove("streaming");
    streaming = false;
    currentAbort = null;
    setSendMode("send");
    els.meta.innerHTML = '<span class="hint">enter to send</span>';
    chat.updatedAt = Date.now();
    putChat(chat);
    autoresize();
    els.prompt.focus();
  }
});

els.stop.addEventListener("click", () => {
  if (currentAbort) {
    try { currentAbort.abort(); } catch (e) {}
  }
});

async function streamPrompt(prompt, onChunk, signal) {
  const resp = await fetch("/v1/run", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ prompt, max_tokens: 512 }),
    signal,
  });

  if (!resp.ok) {
    // The daemon returns a structured JSON body when the upstream
    // (llama-server / ollama / lm studio) is offline. Parse it and
    // throw an Error whose message + .info tell the UI what to render.
    const err = await readStructuredError(resp);
    throw err;
  }
  if (!resp.body) throw new Error("streaming not supported by this browser");

  const servedBy = resp.headers.get("x-unhosted-served-by");

  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    onChunk(decoder.decode(value, { stream: true }));
  }
  return servedBy;
}

// Reads either a structured JSON error (the daemon's upstream-offline
// shape) or a plain text/HTML response and returns an Error decorated
// with .info — the renderer uses this to show a friendly banner.
async function readStructuredError(resp) {
  const errorKind = resp.headers.get("x-unhosted-error");
  const contentType = resp.headers.get("content-type") || "";
  if (contentType.includes("application/json")) {
    try {
      const body = await resp.json();
      const e = body && body.error;
      if (e) {
        const err = new Error(e.message || "request failed");
        err.info = {
          kind: e.type || errorKind || "error",
          configured: e.configured || null,
          checked: e.checked || [],
          hint: e.hint || null,
          status: resp.status,
        };
        return err;
      }
    } catch (_) { /* fall through to status-line error */ }
  }
  const err = new Error(`node returned ${resp.status} ${resp.statusText || ""}`.trim());
  err.info = { kind: errorKind || "http_error", status: resp.status };
  return err;
}

// ---------------------------------------------------------------- helpers

function showError(node, err) {
  const bodyEl = node.querySelector(".body");
  const banner = document.createElement("div");
  banner.className = "error-banner";
  const info = err && err.info;

  if (info && info.kind === "upstream_offline") {
    // No backend is reachable on any known port. Give the user
    // concrete next steps, the install command, and a CTA to the
    // doctor command.
    const checkedHtml = (info.checked || [])
      .map((u) => `<code>${escapeHtml(u)}</code>`)
      .join(" · ");
    banner.classList.add("error-banner-offline");
    banner.innerHTML =
      "<strong>no model runtime is responding.</strong> " +
      "unhosted is the orchestration layer — it needs a backend running locally " +
      "(<code>llama-server</code>, <code>ollama</code>, or <code>lm studio</code>) to actually do inference.<br>" +
      "<span class=\"err-row\"><span class=\"err-label\">checked:</span> " +
      (checkedHtml || "<em>nothing reachable</em>") +
      "</span>" +
      (info.hint ? "<span class=\"err-row err-hint\">" + escapeHtml(info.hint) + "</span>" : "") +
      "<span class=\"err-actions\">" +
      "<a class=\"err-btn err-btn-primary\" href=\"https://github.com/unhosted-ai/unhosted-core#install-a-runtime\" target=\"_blank\" rel=\"noopener\">install a runtime</a> " +
      "<a class=\"err-btn\" href=\"https://github.com/unhosted-ai/unhosted-core/blob/main/README.md#whats-honest\" target=\"_blank\" rel=\"noopener\">about runtimes</a>" +
      "</span>";
  } else {
    banner.innerHTML =
      "<strong>error:</strong> " +
      (err && err.message ? escapeHtml(err.message) : "request failed") +
      ". is the daemon reachable? try <code>unhosted doctor</code> for a probe.";
  }
  bodyEl.append(banner);
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function truncate(s, n) {
  if (s.length <= n) return s;
  return s.slice(0, n - 1) + "…";
}

// ---------------------------------------------------------------- tunnel
//
// "Open to internet" toggle. Clicking it tells the daemon to spawn
// cloudflared; the daemon parses the trycloudflare URL out of stderr
// and exposes it via /v1/tunnel. The displayed URL embeds the bearer
// token as ?api_token=… so the phone's first visit auto-authenticates
// (auth bootstrap up top stores it in localStorage + strips the param).
//
// Caveat: the URL + token together grant full daemon access. The user
// is warned in the UI. The classifier in auth.rs detects cf-connecting-ip
// and forces bearer for tunneled requests so loopback bypass can't leak.

let tunnelPollTimer = null;

async function fetchTunnel() {
  try {
    const r = await fetch("/v1/tunnel", { cache: "no-store" });
    if (!r.ok) return null;
    return await r.json();
  } catch (e) { return null; }
}

async function startTunnel() {
  try {
    const r = await fetch("/v1/tunnel/start", { method: "POST" });
    return r.ok ? await r.json() : null;
  } catch (e) { return null; }
}

async function stopTunnel() {
  try {
    // The X-Unhosted-Confirm header is now required by the daemon's
    // /v1/tunnel/stop. It's a server-side guard against stale tabs
    // (or anything else running pre-confirm-dialog JS) accidentally
    // killing the tunnel. The current UI always sends this header,
    // and is the only thing the UI does so — anything that lands in
    // the stop endpoint without it gets 428'd.
    const r = await fetch("/v1/tunnel/stop", {
      method: "POST",
      headers: { "X-Unhosted-Confirm": "yes" },
    });
    return r.ok ? await r.json() : null;
  } catch (e) { return null; }
}

// Stage → (sub-text, progress %). Backend emits these in TunnelState::Starting.
const TUNNEL_STAGES = {
  spawning:   { label: "spawning cloudflared…",            pct: 20 },
  requesting: { label: "requesting tunnel from cloudflare…", pct: 55 },
  connecting: { label: "negotiating connection…",          pct: 85 },
};

// QR rendering for the "send to my phone" panel. Encodes the live tunnel
// URL + bearer token, so a phone scanning it lands on the chat already
// authenticated. Updates on every renderTunnel() call so the QR tracks
// tunnel state changes (Running -> shows code; anything else -> hint).
let lastQrUrl = null;
function renderPhoneQr(linkHref) {
  if (!els.phoneQrCanvas) return;
  if (!linkHref) {
    els.phoneQrCanvas.innerHTML =
      '<span class="phone-qr-hint" id="phone-qr-hint">enable "open to internet" first — the qr appears once your tunnel is live.</span>';
    lastQrUrl = null;
    return;
  }
  if (linkHref === lastQrUrl) return; // no-op when URL hasn't changed
  if (typeof window.qrcode !== "function") {
    // Library still loading (defer'd from CDN). Retry shortly.
    els.phoneQrCanvas.innerHTML =
      '<span class="phone-qr-hint">loading qr…</span>';
    setTimeout(() => renderPhoneQr(linkHref), 200);
    return;
  }
  try {
    // typeNumber=0 = auto-pick the smallest version that fits.
    // "M" = medium error correction (~15% recoverable), good balance.
    const qr = window.qrcode(0, "M");
    qr.addData(linkHref);
    qr.make();
    els.phoneQrCanvas.innerHTML = qr.createSvgTag({ scalable: true, margin: 0 });
    lastQrUrl = linkHref;
  } catch (e) {
    els.phoneQrCanvas.innerHTML =
      '<span class="phone-qr-hint">qr render failed — copy the url instead.</span>';
    lastQrUrl = null;
  }
}

// Track the last tunnel state we rendered so we can emit transition
// notifications (e.g. "tunnel live", "tunnel down", "url rotated").
let lastTunnelState = null;
let lastTunnelUrl = null;

function renderTunnel(s) {
  if (!s || !els.tunnelToggle) return;
  const state = s.state;
  // Transition notifications — fire once per state change, not on every poll.
  const url = s.url || null;
  let liveStateChanged = false;
  if (lastTunnelState !== null) {
    if (state === "running" && lastTunnelState !== "running") {
      notify("tunnel live — your phone can chat with this mac now", { level: "success", key: "tunnel" });
      liveStateChanged = true;
    } else if (state === "running" && url && lastTunnelUrl && url !== lastTunnelUrl) {
      notify("tunnel url rotated — re-scan the qr on your phone", { level: "info", key: "tunnel", duration: 6000 });
      liveStateChanged = true;
    } else if (state === "failed" && lastTunnelState !== "failed") {
      notify("tunnel failed: " + (s.error || "unknown"), { level: "error", key: "tunnel", duration: 6000 });
      liveStateChanged = true;
    } else if (state === "idle" && (lastTunnelState === "running" || lastTunnelState === "starting")) {
      notify("tunnel is off — your daemon is local-only", { level: "info", key: "tunnel" });
      liveStateChanged = true;
    }
  }
  lastTunnelState = state;
  lastTunnelUrl = url;
  // After a state transition that emitted a toast, schedule a quick
  // second poll so the inline UI can't drift from what the toast just
  // claimed. Belt and suspenders against any WKWebView timer throttling.
  if (liveStateChanged) {
    setTimeout(() => { refreshTunnelNow(); }, 800);
  }

  if (state === "running") {
    const token = getApiToken() || "";
    const sep = s.url.includes("?") ? "&" : "?";
    const linkHref = token ? `${s.url}${sep}api_token=${encodeURIComponent(token)}` : s.url;
    els.tunnelLabel.textContent = "stop";
    els.tunnelStatus.textContent = "live — open this on your phone, anywhere:";
    els.tunnelStatus.dataset.state = "running";
    els.tunnelUrl.textContent = linkHref;
    els.tunnelUrl.dataset.copy = linkHref;
    els.tunnelLink.hidden = false;
    els.tunnelWarn.hidden = false;
    if (els.tunnelProgress) els.tunnelProgress.hidden = true;
    renderPhoneQr(linkHref);
  } else if (state === "starting") {
    const stage = TUNNEL_STAGES[s.stage] || TUNNEL_STAGES.spawning;
    els.tunnelLabel.textContent = "starting…";
    els.tunnelStatus.textContent = stage.label;
    els.tunnelStatus.dataset.state = "starting";
    els.tunnelLink.hidden = true;
    els.tunnelWarn.hidden = true;
    if (els.tunnelProgress) {
      els.tunnelProgress.hidden = false;
      els.tunnelProgressBar.style.width = stage.pct + "%";
    }
  } else if (state === "failed") {
    els.tunnelLabel.textContent = "enable";
    els.tunnelStatus.textContent = "failed: " + (s.error || "unknown");
    els.tunnelStatus.dataset.state = "failed";
    els.tunnelLink.hidden = true;
    els.tunnelWarn.hidden = true;
    if (els.tunnelProgress) els.tunnelProgress.hidden = true;
    renderPhoneQr(null);
  } else {
    els.tunnelLabel.textContent = "enable";
    els.tunnelStatus.textContent = "off — your daemon is only reachable on this network.";
    els.tunnelStatus.dataset.state = "idle";
    els.tunnelLink.hidden = true;
    els.tunnelWarn.hidden = true;
    if (els.tunnelProgress) els.tunnelProgress.hidden = true;
    renderPhoneQr(null);
  }
}

// Tunnel state polling. We use two cadences:
//   - 1.5s "fast" while state is "starting" (progress-bar updates)
//   - 8s "slow" the rest of the time (keeps UI in sync if the daemon's
//     tunnel state changes from outside — another browser tab, CLI, etc.)
// Without the slow heartbeat, if a poll ever stopped while the UI was
// mid-frame (e.g., a transient fetch failure during a daemon restart),
// the UI would stay frozen on whatever was last rendered until the
// user clicked the toggle.
function setTunnelPolling(mode) {
  if (tunnelPollTimer) { clearInterval(tunnelPollTimer); tunnelPollTimer = null; }
  const interval = mode === "fast" ? 1500 : mode === "slow" ? 8000 : null;
  if (!interval) return;
  tunnelPollTimer = setInterval(async () => {
    const s = await fetchTunnel();
    if (s) renderTunnel(s);
    // Promote/demote cadence as state changes.
    if (s && s.state === "starting" && mode !== "fast") setTunnelPolling("fast");
    else if (s && s.state !== "starting" && mode === "fast") setTunnelPolling("slow");
  }, interval);
}

// One-shot force refresh of tunnel state. Used by visibilitychange,
// window focus, and "click the tunnel panel" — any moment the user
// gives us reason to suspect our cached UI may be stale (WKWebView
// throttles setInterval aggressively when the window isn't focused).
async function refreshTunnelNow() {
  const s = await fetchTunnel();
  if (s) renderTunnel(s);
  return s;
}

// Re-sync on window focus too — visibilitychange only fires when the
// window goes hidden/visible (e.g. tab switch in a browser). For our
// desktop WebView the user often just clicks another macOS app while
// our window is still "visible". `focus` covers that case.
window.addEventListener("focus", () => { refreshTunnelNow(); });

// Manual escape hatch: clicking the tunnel section header re-syncs
// immediately. Belt and suspenders for the rare case both polling
// and focus events failed to wake us up.
const tunnelHeader = document.querySelector("#tunnel-section > h4");
if (tunnelHeader) {
  tunnelHeader.style.cursor = "pointer";
  tunnelHeader.title = "click to refresh tunnel state";
  tunnelHeader.addEventListener("click", () => { refreshTunnelNow(); });
}

if (els.tunnelToggle) {
  els.tunnelToggle.addEventListener("click", async () => {
    // Decide based on what the user is LOOKING AT (last-rendered state),
    // not on a fresh refetch. The previous version awaited fetchTunnel()
    // before deciding, which had two failure modes that both surfaced as
    // "I click enable and nothing happens":
    //   1. fetchTunnel() returns null on transient network blips (WKWebView
    //      throttles fetches when the window is backgrounded, and an
    //      unfocused tab can also stall the async chain). null → cur is
    //      falsy → isOn is false → tries to start a tunnel that may
    //      already be running → no observable UI change.
    //   2. If the UI was showing stale "off" state but the daemon was
    //      actually "running" (e.g., user enabled the tunnel from the
    //      phone PWA), clicking the toggle that LOOKED like "enable"
    //      would pop a "turn off tunnel?" confirm dialog instead, with
    //      no preceding indication that the tunnel was actually live.
    //      The user reads it as "the dialog is confused" and dismisses,
    //      then loops.
    // Using `lastTunnelState` matches the rendered UI by construction.
    // It is `null` until the first poll lands, in which case treating
    // it as "off" (fall into the else branch) is the right default —
    // click on a fresh page = "I want to enable".
    const renderedRunning =
      lastTunnelState === "running" || lastTunnelState === "starting";

    els.tunnelToggle.disabled = true;
    try {
      if (renderedRunning) {
        // Confirm before stopping a live tunnel. Without this, a single
        // accidental tap (or a click from a stale browser tab still
        // bound to /v1/tunnel) kills the tunnel and rotates the URL,
        // breaking every share. Daemon logs revealed this was happening
        // unprompted across hours.
        const ok = await confirmDialog({
          title: "turn off tunnel?",
          message: lastTunnelState === "running"
            ? "the public url will stop working and any phone using it will lose connection."
            : "this will cancel the tunnel that's starting up.",
          confirmLabel: "turn off",
          danger: true,
        });
        if (!ok) return;
        // Optimistic UI: flip the panel to idle the instant the click
        // is committed. If the daemon roundtrip later succeeds the next
        // poll just reaffirms; if it fails the poll surfaces the actual
        // state. Either way the user sees motion right now.
        renderTunnel({ state: "idle" });
        notify("stopping tunnel…", { level: "info", key: "tunnel", duration: 3000 });
        const next = await stopTunnel();
        if (next) renderTunnel(next);
        setTunnelPolling("slow");
      } else {
        // Optimistic UI for the cold-start path. Paint "starting…" at
        // 20% before anything hits the network, so the progress bar
        // shows up the same frame the click registers. This is the
        // change that fixes the "I click enable and nothing happens"
        // complaint — even if startTunnel()'s POST hangs or returns
        // null, the user already sees the panel reacting.
        renderTunnel({ state: "starting", stage: "spawning" });
        notify("starting tunnel…", { level: "info", key: "tunnel", duration: 3000 });
        setTunnelPolling("fast");
        const next = await startTunnel();
        if (next) renderTunnel(next);
      }
    } finally {
      els.tunnelToggle.disabled = false;
    }
  });
}

if (els.tunnelCopy) {
  els.tunnelCopy.addEventListener("click", async () => {
    const url = els.tunnelUrl.dataset.copy || els.tunnelUrl.textContent;
    const labelEl = els.tunnelCopy.querySelector(".tunnel-copy-label");
    try {
      await navigator.clipboard.writeText(url);
      if (labelEl) {
        labelEl.textContent = "copied";
        setTimeout(() => { labelEl.textContent = "copy"; }, 1400);
      }
      notify("tunnel url copied to clipboard", { level: "success", duration: 2000 });
    } catch (e) {
      if (labelEl) {
        labelEl.textContent = "failed";
        setTimeout(() => { labelEl.textContent = "copy"; }, 1400);
      }
      notify("couldn't access clipboard — long-press the url to copy", { level: "error" });
    }
  });
}

// ---------------------------------------------------------------- private memory
// Opt-in RAG over past chats. Sidebar toggle persists server-side at
// `~/.config/unhosted/memory-enabled.txt`; when on, the daemon prepends
// the top-k most relevant past summaries to the system prompt on each
// chat completion. v0.0.20 ships storage + manual entry; auto-summarize
// and the embedding-based retriever land in v0.0.21.

async function fetchMemory() {
  try {
    const r = await fetch("/v1/memory", { cache: "no-store" });
    return r.ok ? await r.json() : null;
  } catch (e) { return null; }
}

async function setMemoryEnabled(enabled) {
  try {
    const r = await fetch("/v1/memory/enable", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled }),
    });
    return r.ok ? await r.json() : null;
  } catch (e) { return null; }
}

async function addMemory(summary, chatId) {
  try {
    const r = await fetch("/v1/memory", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ summary, chat_id: chatId || null }),
    });
    return r.ok ? await r.json() : null;
  } catch (e) { return null; }
}

async function deleteMemory(id) {
  try {
    const r = await fetch(`/v1/memory/${encodeURIComponent(id)}`, { method: "DELETE" });
    return r.ok;
  } catch (e) { return false; }
}

async function clearAllMemories() {
  try {
    const r = await fetch("/v1/memory/clear", { method: "POST" });
    return r.ok;
  } catch (e) { return false; }
}

let lastMemoryEnabled = null;

function renderMemory({ enabled, entries }) {
  if (!els.memoryToggle) return;
  lastMemoryEnabled = enabled;
  els.memoryToggleLabel.textContent = enabled ? "disable" : "enable";
  if (enabled) {
    const n = entries ? entries.length : 0;
    els.memoryStatus.textContent = n === 0
      ? "on — no memories yet. save a chat to start."
      : `on — ${n} memor${n === 1 ? "y" : "ies"} stored.`;
    els.memoryStatus.dataset.state = "running";
    if (els.memoryManage) els.memoryManage.hidden = false;
  } else {
    els.memoryStatus.textContent = "off — chats are not remembered between sessions.";
    els.memoryStatus.dataset.state = "idle";
    if (els.memoryManage) els.memoryManage.hidden = true;
  }
}

function renderMemoryList(entries) {
  if (!els.memoryList) return;
  els.memoryList.innerHTML = "";
  if (!entries || entries.length === 0) {
    const li = document.createElement("li");
    li.className = "muted small";
    li.textContent =
      "no memories yet — start a chat with memory on, then save it from the chat header.";
    els.memoryList.append(li);
    if (els.memoryClearAll) els.memoryClearAll.hidden = true;
    return;
  }
  // Newest first — matches how a human thinks about "recent memory"
  // and keeps the most relevant context at the top of a long list.
  const sorted = [...entries].sort((a, b) => b.created_at - a.created_at);
  for (const e of sorted) {
    const li = document.createElement("li");
    li.className = "memory-item";

    const text = document.createElement("div");
    text.className = "memory-summary";
    text.textContent = e.summary;

    const meta = document.createElement("div");
    meta.className = "memory-meta muted small";
    const when = new Date((e.created_at || 0) * 1000).toLocaleString();
    meta.textContent = e.chat_id ? `from chat · ${when}` : `manual · ${when}`;

    const delBtn = document.createElement("button");
    delBtn.className = "memory-delete";
    delBtn.title = "delete this memory";
    delBtn.setAttribute("aria-label", "delete this memory");
    delBtn.innerHTML =
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 4h10M6 4V2.5A.5.5 0 0 1 6.5 2h3a.5.5 0 0 1 .5.5V4M5 4l.5 9a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1L11 4"/></svg>';
    delBtn.addEventListener("click", async () => {
      const ok = await confirmDialog({
        title: "delete memory?",
        message: `delete "${truncate(e.summary, 60)}"? this can't be undone.`,
        confirmLabel: "delete",
        danger: true,
      });
      if (!ok) return;
      const removed = await deleteMemory(e.id);
      if (removed) {
        notify("memory deleted", { level: "info", duration: 2000 });
        await refreshMemoryUI();
      } else {
        notify("delete failed", { level: "error" });
      }
    });

    li.append(text, meta, delBtn);
    els.memoryList.append(li);
  }
  if (els.memoryClearAll) els.memoryClearAll.hidden = false;
}

async function refreshMemoryUI() {
  const s = await fetchMemory();
  if (!s) return;
  renderMemory(s);
  renderMemoryList(s.entries);
}

if (els.memoryToggle) {
  els.memoryToggle.addEventListener("click", async () => {
    const next = !lastMemoryEnabled;
    els.memoryToggle.disabled = true;
    // Optimistic UI: paint the new state immediately, then reconcile
    // with the server response. Matches the tunnel toggle pattern.
    renderMemory({ enabled: next, entries: [] });
    notify(next ? "memory on — chats can now be remembered" : "memory off", {
      level: "info",
      key: "memory",
      duration: 2500,
    });
    const resp = await setMemoryEnabled(next);
    if (resp === null) {
      notify("couldn't save memory setting", { level: "error", key: "memory" });
    }
    await refreshMemoryUI();
    els.memoryToggle.disabled = false;
  });
}

if (els.memoryManage && els.memoryModal) {
  const closeMemory = () => { els.memoryModal.hidden = true; };
  els.memoryManage.addEventListener("click", async () => {
    els.memoryModal.hidden = false;
    await refreshMemoryUI();
  });
  if (els.memoryModalClose) els.memoryModalClose.addEventListener("click", closeMemory);
  els.memoryModal.addEventListener("click", (e) => {
    if (e.target === els.memoryModal) closeMemory();
  });
}

if (els.memoryAddSubmit) {
  els.memoryAddSubmit.addEventListener("click", async () => {
    if (!els.memoryAddInput) return;
    const text = els.memoryAddInput.value.trim();
    if (text.length === 0) {
      notify("enter a note first", { level: "info", duration: 2000 });
      return;
    }
    els.memoryAddSubmit.disabled = true;
    const added = await addMemory(text, null);
    els.memoryAddSubmit.disabled = false;
    if (added) {
      els.memoryAddInput.value = "";
      notify("memory saved", { level: "success", duration: 2000 });
      await refreshMemoryUI();
    } else {
      notify("save failed", { level: "error" });
    }
  });
}

if (els.memoryClearAll) {
  els.memoryClearAll.addEventListener("click", async () => {
    const ok = await confirmDialog({
      title: "clear every memory?",
      message: "deletes all stored summaries. the toggle stays on so new chats can still be remembered.",
      confirmLabel: "clear all",
      danger: true,
    });
    if (!ok) return;
    const cleared = await clearAllMemories();
    if (cleared) {
      notify("all memories cleared", { level: "info", duration: 2500 });
      await refreshMemoryUI();
    } else {
      notify("clear failed", { level: "error" });
    }
  });
}

// Initial paint of the memory panel — runs alongside the first status
// poll so the sidebar reflects the persisted state on every page load.
refreshMemoryUI();

// ---------------------------------------------------------------- pair modal

const pairEls = {
  modal: $("#pair-modal"),
  close: $("#pair-modal-close"),
  title: $("#pair-modal-title"),
  viewIdentity: $("#pair-view-identity"),
  viewOffer: $("#pair-view-offer"),
  viewAccept: $("#pair-view-accept"),
  myName: $("#pair-my-name"),
  myPubkey: $("#pair-my-pubkey"),
  myAddr: $("#pair-my-addr"),
  code: $("#pair-code"),
  codeInput: $("#pair-code-input"),
  offerUri: $("#pair-offer-uri"),
  offerTtl: $("#pair-offer-ttl"),
  offerReach: $("#pair-offer-reach"),
  copyBtn: $("#pair-copy-btn"),
  acceptInput: $("#pair-accept-input"),
  acceptSubmit: $("#pair-accept-submit"),
  acceptUriSubmit: $("#pair-accept-uri-submit"),
  acceptMsg: $("#pair-accept-msg"),
  showOfferBtn: $("#pair-show-offer"),
  acceptOfferBtn: $("#pair-accept-offer"),
};

let pairTickInterval = null;

function openPairModal(mode) {
  if (!pairEls.modal) return;
  pairEls.modal.hidden = false;
  pairEls.viewOffer.hidden = mode !== "offer";
  pairEls.viewAccept.hidden = mode !== "accept";
  pairEls.viewIdentity.hidden = false; // always show identity at top

  pairEls.title.textContent =
    mode === "offer" ? "show my offer" : "accept an offer";

  // Fill identity.
  fetch("/v1/identity")
    .then((r) => r.json())
    .then((d) => {
      pairEls.myName.textContent = d.name || "—";
      pairEls.myPubkey.textContent = d.pubkey || "—";
      pairEls.myAddr.textContent = d.addr || "—";
    })
    .catch(() => {});

  if (mode === "offer") {
    requestOffer();
  } else {
    pairEls.acceptInput.value = "";
    pairEls.acceptMsg.textContent = "";
    pairEls.acceptInput.focus();
  }
}

function closePairModal() {
  pairEls.modal.hidden = true;
  if (pairTickInterval) {
    clearInterval(pairTickInterval);
    pairTickInterval = null;
  }
}

async function requestOffer() {
  if (pairEls.code) pairEls.code.textContent = "····";
  pairEls.offerUri.textContent = "—";
  pairEls.offerReach.textContent = "";
  if (pairTickInterval) {
    clearInterval(pairTickInterval);
    pairTickInterval = null;
  }

  // Prefer short-code path (needs a relay). Fall back to long URI if relay
  // isn't configured (HTTP 412 PRECONDITION_FAILED).
  let usedShort = false;
  try {
    const sr = await fetch("/v1/pair/short-offer", { method: "POST" });
    if (sr.ok) {
      const d = await sr.json();
      pairEls.code.textContent = d.code;
      let ttl = d.expires_in_seconds;
      pairEls.offerTtl.textContent = ttl;
      pairTickInterval = setInterval(() => {
        ttl -= 1;
        pairEls.offerTtl.textContent = Math.max(0, ttl);
        if (ttl <= 0) {
          clearInterval(pairTickInterval);
          pairTickInterval = null;
          pairEls.code.textContent = "····";
        }
      }, 1000);
      pairEls.offerReach.textContent =
        "✓ share the 4 letters. the other device types them in.";
      pairEls.offerReach.style.color = "var(--ok)";
      usedShort = true;
    } else if (sr.status !== 412) {
      throw new Error(`short HTTP ${sr.status}`);
    }
  } catch (e) {
    /* fall through */
  }

  // Always fetch the long URI too — for the "or share a long link" fallback.
  try {
    const lr = await fetch("/v1/pair/offer", { method: "POST" });
    if (!lr.ok) throw new Error(`HTTP ${lr.status}`);
    const d = await lr.json();
    pairEls.offerUri.textContent = d.offer;
    if (!usedShort) {
      // No relay → no short code; long URI is the primary share.
      pairEls.code.textContent = "—";
      let ttl = d.expires_in_seconds;
      pairEls.offerTtl.textContent = ttl;
      pairTickInterval = setInterval(() => {
        ttl -= 1;
        pairEls.offerTtl.textContent = Math.max(0, ttl);
        if (ttl <= 0) {
          clearInterval(pairTickInterval);
          pairTickInterval = null;
          pairEls.offerUri.textContent = "expired — close and reopen.";
        }
      }, 1000);
      if (d.reachability === "lan") {
        pairEls.offerReach.textContent =
          "no relay configured → no short code. share the long link. add --relay to your daemon for codes.";
        pairEls.offerReach.style.color = "var(--mute)";
      } else if (d.reachability === "loopback_only") {
        pairEls.offerReach.textContent =
          "⚠ only works on this machine. restart with --addr 0.0.0.0:7777 (lan) or --relay (internet).";
        pairEls.offerReach.style.color = "var(--err)";
      }
    }
  } catch (e) {
    if (!usedShort) pairEls.offerUri.textContent = "failed: " + (e.message || "unknown");
  }
}

async function acceptCode() {
  const code = (pairEls.codeInput?.value || "").trim();
  if (code.length < 3) {
    pairEls.acceptMsg.textContent = "type the 4-letter code from the other device.";
    return;
  }
  pairEls.acceptSubmit.disabled = true;
  pairEls.acceptMsg.textContent = "pairing…";
  try {
    const r = await fetch("/v1/pair/use-code", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code }),
    });
    if (!r.ok) {
      const text = await r.text();
      throw new Error(text || `HTTP ${r.status}`);
    }
    const d = await r.json();
    pairEls.acceptMsg.textContent = `paired with ${d.name}.`;
    pairEls.codeInput.value = "";
    await refreshStatus();
    setTimeout(closePairModal, 1500);
  } catch (e) {
    pairEls.acceptMsg.textContent = "failed: " + (e.message || "unknown");
  } finally {
    pairEls.acceptSubmit.disabled = false;
  }
}

async function acceptOffer() {
  const offer = pairEls.acceptInput.value.trim();
  if (!offer.startsWith("unhosted://pair?")) {
    pairEls.acceptMsg.textContent = "looks invalid — expected 'unhosted://pair?…'";
    return;
  }
  pairEls.acceptSubmit.disabled = true;
  pairEls.acceptMsg.textContent = "pairing…";
  try {
    const r = await fetch("/v1/pair/connect", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ offer }),
    });
    if (!r.ok) {
      const text = await r.text();
      throw new Error(text || `HTTP ${r.status}`);
    }
    const d = await r.json();
    pairEls.acceptMsg.textContent = `paired with ${d.name} (${d.addr}).`;
    pairEls.acceptInput.value = "";
    await refreshStatus(); // sidebar's peers section updates
    setTimeout(closePairModal, 1500);
  } catch (e) {
    pairEls.acceptMsg.textContent = "failed: " + (e.message || "unknown");
  } finally {
    pairEls.acceptSubmit.disabled = false;
  }
}

if (pairEls.showOfferBtn) {
  pairEls.showOfferBtn.addEventListener("click", () => openPairModal("offer"));
}
if (pairEls.acceptOfferBtn) {
  pairEls.acceptOfferBtn.addEventListener("click", () => openPairModal("accept"));
}
if (pairEls.close) pairEls.close.addEventListener("click", closePairModal);
if (pairEls.modal) {
  pairEls.modal.addEventListener("click", (e) => {
    if (e.target === pairEls.modal) closePairModal();
  });
}
if (pairEls.acceptSubmit) pairEls.acceptSubmit.addEventListener("click", acceptCode);
if (pairEls.acceptUriSubmit) pairEls.acceptUriSubmit.addEventListener("click", acceptOffer);
if (pairEls.codeInput) {
  pairEls.codeInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") acceptCode();
  });
}
if (pairEls.copyBtn) {
  // Cache the original innerHTML once so the success-flash can restore
  // it exactly — we now have an icon + label, so swapping textContent
  // would destroy the SVG.
  const copyBtnOrig = pairEls.copyBtn.innerHTML;
  pairEls.copyBtn.addEventListener("click", async () => {
    const text = pairEls.offerUri.textContent || "";
    try {
      await navigator.clipboard.writeText(text);
      pairEls.copyBtn.classList.add("copy-success");
      pairEls.copyBtn.innerHTML = icon("check") + '<span>copied</span>';
      setTimeout(() => {
        pairEls.copyBtn.classList.remove("copy-success");
        pairEls.copyBtn.innerHTML = copyBtnOrig;
      }, 1200);
    } catch (e) {
      // fallback: select the code element
      const range = document.createRange();
      range.selectNodeContents(pairEls.offerUri);
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
    }
  });
}
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && pairEls.modal && !pairEls.modal.hidden) {
    closePairModal();
  }
});

// ---------------------------------------------------------------- toast notifications
//
// Non-blocking status feedback. Called from state-change points (tunnel
// went live, URL rotated, copy succeeded/failed, etc.) so the user has a
// running narrative of what the daemon is doing instead of staring at
// silent UI.
//
// notify(message, { level, duration, key }):
//   level    "info" (default) | "success" | "error"
//   duration ms before auto-dismiss (default 4000; 0 to require manual)
//   key      stable id — re-firing with the same key replaces the
//            existing toast (so "tunnel live" doesn't pile up on every
//            poll). Omit for one-shot toasts.
const toastStack = document.getElementById("toast-stack");
const liveToasts = new Map(); // key -> {el, timer}

function notify(message, { level = "info", duration = 4000, key = null } = {}) {
  if (!toastStack) return;
  if (key && liveToasts.has(key)) {
    // refresh in place
    const existing = liveToasts.get(key);
    existing.el.textContent = message;
    existing.el.dataset.level = level;
    if (existing.timer) clearTimeout(existing.timer);
    existing.timer = duration ? setTimeout(() => dismissToast(key, existing.el), duration) : null;
    return;
  }
  const el = document.createElement("div");
  el.className = "toast";
  el.dataset.level = level;
  el.textContent = message;
  toastStack.appendChild(el);
  const localKey = key || `k${Date.now()}_${Math.random()}`;
  const timer = duration ? setTimeout(() => dismissToast(localKey, el), duration) : null;
  liveToasts.set(localKey, { el, timer });
  el.addEventListener("click", () => dismissToast(localKey, el));
}
function dismissToast(key, el) {
  if (!el || !el.parentNode) { liveToasts.delete(key); return; }
  el.classList.add("toast-leaving");
  setTimeout(() => {
    if (el.parentNode) el.parentNode.removeChild(el);
    liveToasts.delete(key);
  }, 180);
}

// ---------------------------------------------------------------- developer modal
//
// "for developers" panel. The Unhosted daemon already speaks an
// OpenAI-compatible API on /v1/* — this modal just makes it discoverable:
// shows the user their endpoint + token, plus copy-pasteable curl /
// Python / JavaScript snippets so they can plug their local daemon into
// any app without reading source.

function devSnippet(lang, baseUrl, token) {
  const tokenDisplay = token || "<your-token>";
  if (lang === "curl") {
    return `curl ${baseUrl}/v1/chat/completions \\
  -H "Authorization: Bearer ${tokenDisplay}" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "local",
    "messages": [{"role": "user", "content": "hello"}],
    "stream": true
  }'`;
  }
  if (lang === "python") {
    return `# pip install openai
from openai import OpenAI

client = OpenAI(
    base_url="${baseUrl}/v1",
    api_key="${tokenDisplay}",
)

stream = client.chat.completions.create(
    model="local",
    messages=[{"role": "user", "content": "hello"}],
    stream=True,
)
for chunk in stream:
    print(chunk.choices[0].delta.content or "", end="", flush=True)`;
  }
  // javascript
  return `const r = await fetch("${baseUrl}/v1/chat/completions", {
  method: "POST",
  headers: {
    "Authorization": "Bearer ${tokenDisplay}",
    "Content-Type": "application/json",
  },
  body: JSON.stringify({
    model: "local",
    messages: [{ role: "user", content: "hello" }],
    stream: false,
  }),
});
const j = await r.json();
console.log(j.choices[0].message.content);`;
}

let currentDevTab = "curl";

async function populateDeveloperModal() {
  const baseUrl = `${location.protocol}//${location.host}`;
  const token = getApiToken() || "";
  els.devBaseUrl.textContent = baseUrl;
  els.devBaseUrl.dataset.copy = baseUrl;
  els.devToken.textContent = token || "(loopback only — no token needed)";
  els.devToken.dataset.copy = token;
  // If a tunnel is running, show the public URL so users know to swap in
  // the public origin when calling from a remote app.
  const t = await fetchTunnel();
  if (t && t.state === "running" && t.url) {
    els.devTunnelUrl.textContent = t.url;
    els.devTunnelNote.hidden = false;
  } else {
    els.devTunnelNote.hidden = true;
  }
  renderDevSnippet(baseUrl, token);
}

function renderDevSnippet(baseUrl, token) {
  els.devSnippetCode.textContent = devSnippet(currentDevTab, baseUrl, token);
}

function openDeveloperModal() {
  if (!els.developerModal) return;
  els.developerModal.hidden = false;
  populateDeveloperModal();
}
function closeDeveloperModal() {
  if (!els.developerModal) return;
  els.developerModal.hidden = true;
}

if (els.developerOpen) els.developerOpen.addEventListener("click", openDeveloperModal);
if (els.developerModalClose) els.developerModalClose.addEventListener("click", closeDeveloperModal);
if (els.developerModal) {
  els.developerModal.addEventListener("click", (e) => {
    if (e.target === els.developerModal) closeDeveloperModal();
  });
}
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && els.developerModal && !els.developerModal.hidden) {
    closeDeveloperModal();
  }
});

// Tab switching inside the developer modal.
document.querySelectorAll(".dev-tab").forEach((btn) => {
  btn.addEventListener("click", () => {
    currentDevTab = btn.dataset.tab;
    document.querySelectorAll(".dev-tab").forEach((b) => b.classList.toggle("active", b === btn));
    renderDevSnippet(`${location.protocol}//${location.host}`, getApiToken() || "");
  });
});

// Copy buttons inside the developer modal. Re-uses the data-copy-target
// → element id convention so we don't reinvent clipboard plumbing.
document.querySelectorAll("[data-copy-target]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const targetEl = document.getElementById(btn.dataset.copyTarget);
    if (!targetEl) return;
    const text = targetEl.dataset.copy ?? targetEl.textContent;
    try {
      await navigator.clipboard.writeText(text);
      const old = btn.querySelector("span")?.textContent;
      if (old) {
        btn.querySelector("span").textContent = "copied";
        setTimeout(() => { btn.querySelector("span").textContent = old; }, 1200);
      }
    } catch (e) { /* clipboard denied; user can still hand-select */ }
  });
});

if (els.devSnippetCopy) {
  els.devSnippetCopy.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(els.devSnippetCode.textContent);
      const old = els.devSnippetCopy.textContent;
      els.devSnippetCopy.textContent = "copied";
      setTimeout(() => { els.devSnippetCopy.textContent = old; }, 1200);
      notify("snippet copied — paste it into your code", { level: "success", duration: 2000 });
    } catch (e) {
      notify("couldn't access clipboard", { level: "error" });
    }
  });
}

// ---------------------------------------------------------------- boot

// Render synchronously first (empty list, while the daemon answers)
// so the UI shows something instead of flashing nothing. Then swap
// to the real state once the fetch resolves.
renderChatList();
renderActiveChat();

bootstrapChats().then(() => {
  renderChatList();
  renderActiveChat();
});

fetchTunnel().then((s) => {
  renderTunnel(s);
  // Always have *some* poll running so the UI re-syncs if the daemon's
  // state changes from another tab/CLI. Fast cadence while starting,
  // slow heartbeat otherwise.
  setTunnelPolling(s && s.state === "starting" ? "fast" : "slow");
});

// Cross-device sync: when this tab comes back to the foreground, pull
// fresh state so a chat edited on another paired device (phone PWA,
// other browser) shows up. Cheap GET, skipped while we're mid-stream.
// Also re-syncs the tunnel state, which can drift if the WebView's
// poll timer paused while backgrounded.
document.addEventListener("visibilitychange", () => {
  if (document.hidden) return;
  refreshChatsFromServer();
  fetchTunnel().then((s) => { if (s) renderTunnel(s); });
});
