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
  // Duplicated values inside the expandable "show request flow"
  // pipeline. Filled alongside the compact rows so users who open
  // the pipeline see the same data.
  connModelPipe: $("#conn-model-pipe"),
  connUpstreamPipe: $("#conn-upstream-pipe"),
  connNodePipe: $("#conn-node-pipe"),
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
  appUpdateSection: $("#app-update-section"),
  appUpdateStatus: $("#app-update-status-line"),
  appUpdateCheck: $("#app-update-check"),
  appUpdateCheckLabel: $("#app-update-check-label"),
  appUpdateUnavailable: $("#app-update-unavailable"),
  voiceListenToggle: $("#voice-listen-toggle"),
  voiceListenLabel: $("#voice-listen-label"),
  voiceRunTranscript: $("#voice-run-transcript"),
  voiceTranscriptInput: $("#voice-transcript-input"),
  voiceAutoRun: $("#voice-auto-run"),
  voiceAutoSpeak: $("#voice-auto-speak"),
  voiceAutoSaveNeed: $("#voice-auto-save-need"),
  voiceWakeMode: $("#voice-wake-mode"),
  voiceWakeWordInput: $("#voice-wake-word-input"),
  voiceRoutingMode: $("#voice-routing-mode"),
  voiceRoutingPreview: $("#voice-routing-preview"),
  voiceSaveNeed: $("#voice-save-need"),
  voiceExtractNeed: $("#voice-extract-need"),
  voiceNeedPreview: $("#voice-need-preview"),
  voiceConsentGoogle: $("#voice-consent-google"),
  voiceConsentNotion: $("#voice-consent-notion"),
  voiceConsentSlack: $("#voice-consent-slack"),
  voiceConnectGoogle: $("#voice-connect-google"),
  voiceConnectNotion: $("#voice-connect-notion"),
  voiceConnectSlack: $("#voice-connect-slack"),
  voiceDisconnectGoogle: $("#voice-disconnect-google"),
  voiceDisconnectNotion: $("#voice-disconnect-notion"),
  voiceDisconnectSlack: $("#voice-disconnect-slack"),
  voiceConnectorsStatus: $("#voice-connectors-status"),
  voiceClearTranscript: $("#voice-clear-transcript"),
  voiceStatus: $("#voice-status-line"),
  bugReportFooter: $("#bug-report-footer"),
  bugReportOpen: $("#bug-report-open"),
  bugReportCopyDiagnostics: $("#bug-report-copy-diagnostics"),
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
  vramSection: $("#vram-pool-section"),
  vramStatus: $("#vram-pool-status-line"),
  vramDetails: $("#vram-pool-details"),
  vramModal: $("#vram-pool-modal"),
  vramModalClose: $("#vram-pool-modal-close"),
  vramLlamaPath: $("#vram-llama-path"),
  vramRpcFlag: $("#vram-rpc-flag"),
  vramRpcPath: $("#vram-rpc-path"),
  vramReady: $("#vram-ready"),
  vramHint: $("#vram-hint"),
  vramControls: $("#vram-pool-controls"),
  vramModelInput: $("#vram-pool-model-input"),
  vramStart: $("#vram-pool-start"),
  vramStop: $("#vram-pool-stop"),
  vramEndpointRow: $("#vram-pool-endpoint-row"),
  vramEndpoint: $("#vram-pool-endpoint"),
  vramPeersBlock: $("#vram-pool-peers"),
  vramPeersList: $("#vram-pool-peers-list"),
};

// Track which paired peers the user has selected as layer hosts
// for the next `start pool` click. Mutated by checkbox events;
// read by `startVramPool`. Persisted in localStorage so the
// selection survives reloads but not daemon restarts (which
// would invalidate the peer names anyway).
const VRAM_POOL_PEER_SELECTION_KEY = "unhosted-vram-pool-selected-peers";
function loadSelectedPeers() {
  try {
    const raw = localStorage.getItem(VRAM_POOL_PEER_SELECTION_KEY);
    if (!raw) return new Set();
    return new Set(JSON.parse(raw));
  } catch (e) {
    return new Set();
  }
}
function saveSelectedPeers(set) {
  try {
    localStorage.setItem(VRAM_POOL_PEER_SELECTION_KEY, JSON.stringify([...set]));
  } catch (e) { /* full disk / private mode — fine, just don't persist */ }
}
let vramSelectedPeers = loadSelectedPeers();

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
    syncPipelineFields();
  }
}

// Mirror the compact connection-row values into the expandable
// pipeline siblings. Called at the end of renderStatus so the
// expandable "show request flow" view stays in sync with the
// always-visible key-value list.
function syncPipelineFields() {
  if (els.connModelPipe) els.connModelPipe.textContent = els.connModel?.textContent || "—";
  if (els.connUpstreamPipe) els.connUpstreamPipe.textContent = els.connUpstream?.textContent || "—";
  if (els.connNodePipe) els.connNodePipe.textContent = els.connNode?.textContent || "—";
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
  syncPipelineFields();

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

  // VRAM-pool capability — populated from /v1/status.vram_pool every poll
  // so the user sees a `brew install`-induced capability flip without
  // restarting the daemon.
  if (els.vramStatus) {
    renderVramPool(s.vram_pool);
    renderVramPoolPeers(s.peers || []);
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

  // Agent mode: fork the submit path. The agent runtime is
  // non-streaming in slice 1 — POST returns the full step trace,
  // which we render into the conversation in one pass.
  if (agentModeOn()) {
    els.prompt.value = "";
    autoresize();
    await submitAgentRun(prompt);
    return;
  }

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
    handleVoiceAssistantReply(assistantMsg.text);
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
    // Auto-open the phone-section the moment the tunnel is live —
    // that's the only time the QR is actually useful. Respects the
    // user's choice afterward (we only force-open once per state
    // transition into "running", not on every re-render).
    if (els.phoneSection && !els.phoneSection.dataset.autoOpenedFor || els.phoneSection.dataset.autoOpenedFor !== s.url) {
      if (els.phoneSection) {
        els.phoneSection.open = true;
        els.phoneSection.dataset.autoOpenedFor = s.url;
      }
    }
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
    if (els.phoneSection) delete els.phoneSection.dataset.autoOpenedFor;
  } else {
    els.tunnelLabel.textContent = "enable";
    els.tunnelStatus.textContent = "off — your daemon is only reachable on this network.";
    els.tunnelStatus.dataset.state = "idle";
    els.tunnelLink.hidden = true;
    els.tunnelWarn.hidden = true;
    if (els.tunnelProgress) els.tunnelProgress.hidden = true;
    renderPhoneQr(null);
    if (els.phoneSection) delete els.phoneSection.dataset.autoOpenedFor;
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

// ---------------------------------------------------------------- voice assistant
// OpenJarvis-style local voice loop: microphone -> transcript -> run prompt,
// plus optional spoken assistant replies and explicit "save as need" memory.
const VOICE_PREFS_KEY = "unhosted-voice-assistant-prefs";
const VOICE_CONNECTOR_INFO = {
  google: {
    label: "google calendar",
    oauthUrl: "https://accounts.google.com/o/oauth2/v2/auth",
  },
  notion: {
    label: "notion",
    oauthUrl: "https://api.notion.com/v1/oauth/authorize",
  },
  slack: {
    label: "slack",
    oauthUrl: "https://slack.com/oauth/v2/authorize",
  },
};
const voiceState = {
  recognition: null,
  listening: false,
  pendingSpeak: false,
  manualStop: false,
  lastError: null,
  wakeCooldownUntil: 0,
};

function readVoicePrefs() {
  try {
    const raw = localStorage.getItem(VOICE_PREFS_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch (e) {
    return {};
  }
}

function writeVoicePrefs() {
  try {
    localStorage.setItem(
      VOICE_PREFS_KEY,
      JSON.stringify({
        autoRun: !!els.voiceAutoRun?.checked,
        autoSpeak: !!els.voiceAutoSpeak?.checked,
        autoSaveNeed: !!els.voiceAutoSaveNeed?.checked,
        wakeMode: !!els.voiceWakeMode?.checked,
        wakeWord: (els.voiceWakeWordInput?.value || "jarvis").trim() || "jarvis",
        routingMode: (els.voiceRoutingMode?.value || "auto").trim() || "auto",
      }),
    );
  } catch (e) {
    // Storage can fail in private mode — non-fatal.
  }
}

function setVoiceStatus(text, state = "idle") {
  if (!els.voiceStatus) return;
  els.voiceStatus.textContent = text;
  els.voiceStatus.dataset.state = state;
}

function speechRecognitionCtor() {
  return window.SpeechRecognition || window.webkitSpeechRecognition || null;
}

function setVoiceListeningUi(listening) {
  voiceState.listening = listening;
  if (els.voiceListenLabel) {
    els.voiceListenLabel.textContent = listening ? "stop listening" : "start listening";
  }
  if (els.voiceListenToggle) {
    els.voiceListenToggle.dataset.state = listening ? "running" : "idle";
  }
}

function transcriptText() {
  return (els.voiceTranscriptInput?.value || "").trim();
}

function wakeWord() {
  const w = (els.voiceWakeWordInput?.value || "jarvis").trim().toLowerCase();
  return w || "jarvis";
}

function connectorRefs() {
  return {
    google: {
      consent: els.voiceConsentGoogle,
      connect: els.voiceConnectGoogle,
      disconnect: els.voiceDisconnectGoogle,
    },
    notion: {
      consent: els.voiceConsentNotion,
      connect: els.voiceConnectNotion,
      disconnect: els.voiceDisconnectNotion,
    },
    slack: {
      consent: els.voiceConsentSlack,
      connect: els.voiceConnectSlack,
      disconnect: els.voiceDisconnectSlack,
    },
  };
}

function applyConnectorLedger(ledger) {
  const map = (ledger && ledger.connectors) || {};
  const refs = connectorRefs();
  const connected = [];
  for (const [name, ui] of Object.entries(refs)) {
    const s = map[name] || { enabled: false, connected: false, has_token: false };
    if (ui.consent) ui.consent.checked = !!s.enabled;
    if (ui.disconnect) ui.disconnect.disabled = !s.connected;
    if (s.connected) connected.push(VOICE_CONNECTOR_INFO[name].label + (s.has_token ? " (token)" : ""));
  }
  if (els.voiceConnectorsStatus) {
    els.voiceConnectorsStatus.textContent = connected.length
      ? `connectors: ${connected.join(" · ")}`
      : "connectors: none connected";
  }
}

async function fetchConnectorLedger() {
  const r = await fetch("/v1/connectors/consent", { cache: "no-store" });
  if (!r.ok) throw new Error(`consent load failed: ${r.status}`);
  return await r.json();
}

async function refreshConnectorStatus() {
  try {
    const ledger = await fetchConnectorLedger();
    applyConnectorLedger(ledger);
  } catch (e) {
    if (els.voiceConnectorsStatus) {
      els.voiceConnectorsStatus.textContent = "connectors: unavailable";
    }
  }
}

async function setConnectorConsent(name, enabled) {
  const r = await fetch("/v1/connectors/consent", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ connector: name, enabled }),
  });
  if (!r.ok) throw new Error(`consent save failed: ${r.status}`);
  const ledger = await r.json();
  applyConnectorLedger(ledger);
}

async function connectConnector(name) {
  const meta = VOICE_CONNECTOR_INFO[name];
  if (!meta) return;
  const url = `${meta.oauthUrl}?response_type=code&scope=openid&state=unhosted_${name}`;
  try {
    window.open(url, "_blank", "noopener");
  } catch (_) {}
  const r = await fetch("/v1/connectors/oauth/connect", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      connector: name,
      scopes: ["openid"],
      // OAuth callback wiring lands in a dedicated connector backend.
      // For now we persist consent + connection state server-side.
    }),
  });
  if (!r.ok) throw new Error(`connect failed: ${r.status}`);
  applyConnectorLedger(await r.json());
  notify(`${meta.label} consent granted`, { level: "success", duration: 2400 });
}

async function disconnectConnector(name) {
  const meta = VOICE_CONNECTOR_INFO[name];
  if (!meta) return;
  const r = await fetch("/v1/connectors/oauth/disconnect", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ connector: name }),
  });
  if (!r.ok) throw new Error(`disconnect failed: ${r.status}`);
  applyConnectorLedger(await r.json());
  notify(`${meta.label} consent revoked`, { level: "info", duration: 2200 });
}

function complexityRiskProfile(text) {
  const s = String(text || "").trim();
  const lower = s.toLowerCase();
  const words = s ? s.split(/\s+/).length : 0;
  let complexityScore = 0;
  let riskScore = 0;

  if (words >= 40) complexityScore += 2;
  else if (words >= 18) complexityScore += 1;

  if (/(plan|compare|analy[sz]e|research|multi[- ]step|step[- ]by[- ]step|route|architecture|design)/.test(lower)) {
    complexityScore += 2;
  }
  if (/(summari[sz]e|quick|short|single|one[- ]line)/.test(lower)) {
    complexityScore -= 1;
  }

  if (/(password|token|secret|credential|ssn|passport|medical|bank|payment|transfer|wire|delete|drop table|sudo|rm -rf|oauth)/.test(lower)) {
    riskScore += 2;
  }
  if (/(public|publish|share externally|send to|post to)/.test(lower)) {
    riskScore += 1;
  }

  const complexity = complexityScore >= 3 ? "high" : complexityScore >= 1 ? "medium" : "low";
  const risk = riskScore >= 3 ? "high" : riskScore >= 1 ? "medium" : "low";

  let suggestedRoute = "chat";
  if (complexity === "high" && risk !== "high") suggestedRoute = "agent";
  if (risk === "high") suggestedRoute = "safe";

  return { complexity, risk, suggestedRoute };
}

function renderRoutingPreview(text) {
  const p = complexityRiskProfile(text);
  if (!els.voiceRoutingPreview) return p;
  els.voiceRoutingPreview.textContent =
    `route preview: ${p.suggestedRoute} · complexity ${p.complexity} · risk ${p.risk}`;
  return p;
}

function routingMode() {
  return (els.voiceRoutingMode?.value || "auto").trim() || "auto";
}

function resolveRoute(profile) {
  const mode = routingMode();
  if (mode === "chat") return "chat";
  if (mode === "agent") return "agent";
  if (mode === "safe") {
    return profile.risk === "high" ? "blocked" : "chat";
  }
  if (profile.risk === "high") return "blocked";
  return profile.suggestedRoute;
}

function extractNeedFromText(text) {
  const raw = String(text || "").trim();
  const lower = raw.toLowerCase();
  const out = { category: "preference", summary: raw };

  const nameMatch = raw.match(/(?:call me|my name is)\s+([a-z0-9 _-]{2,40})/i);
  if (nameMatch) {
    out.category = "profile";
    out.summary = `Preferred name: ${nameMatch[1].trim()}`;
    return out;
  }
  const langMatch = raw.match(/(?:respond|answer|write)\s+in\s+([a-z][a-z\- ]{1,24})/i);
  if (langMatch) {
    out.category = "language";
    out.summary = `Response language: ${langMatch[1].trim()}`;
    return out;
  }
  if (/(?:prefer|i prefer|please|always|never|do not|don't|only)/i.test(raw)) {
    out.category = "style";
    out.summary = raw;
    return out;
  }
  if (/rust|python|javascript|typescript|go|java|c\+\+/i.test(lower)) {
    out.category = "tech";
    out.summary = raw;
    return out;
  }
  return out;
}

function renderNeedPreview(need) {
  if (!els.voiceNeedPreview) return;
  if (!need || !need.summary) {
    els.voiceNeedPreview.textContent = "need preview: —";
    return;
  }
  els.voiceNeedPreview.textContent = `need preview [${need.category}]: ${need.summary}`;
}

function speakAssistantText(text) {
  const synth = window.speechSynthesis;
  if (!synth) {
    notify("speech output is not supported in this browser", { level: "error", duration: 3000 });
    setVoiceStatus("speech output unsupported in this browser", "error");
    return;
  }
  const spoken = String(text || "").trim();
  if (!spoken) return;
  const utter = new SpeechSynthesisUtterance(spoken);
  utter.rate = 1;
  utter.onstart = () => setVoiceStatus("speaking assistant reply…", "running");
  utter.onend = () => setVoiceStatus("reply spoken", "idle");
  utter.onerror = () => setVoiceStatus("speech output failed", "error");
  synth.cancel();
  synth.speak(utter);
}

function handleVoiceAssistantReply(text) {
  if (!voiceState.pendingSpeak) return;
  voiceState.pendingSpeak = false;
  if (!els.voiceAutoSpeak?.checked) return;
  // Keep spoken output clean: strip bracketed transport markers.
  const cleaned = String(text || "")
    .replace(/\[(stopped|error:[^\]]+)\]/gi, "")
    .trim();
  if (!cleaned) return;
  speakAssistantText(cleaned.slice(0, 1200));
}

async function saveNeedFromText(text) {
  const trimmed = String(text || "").trim();
  if (!trimmed) {
    notify("nothing to save", { level: "info", duration: 2200 });
    return false;
  }
  const need = extractNeedFromText(trimmed);
  renderNeedPreview(need);
  const saved = await addMemory(`user need [${need.category}]: ${need.summary}`, null);
  if (saved) {
    notify("saved to private memory", { level: "success", duration: 2200 });
    setVoiceStatus("saved as a durable user need", "idle");
    return true;
  }
  notify("save need failed", { level: "error", duration: 2600 });
  setVoiceStatus("save failed", "error");
  return false;
}

async function runVoiceTranscript({ auto = false } = {}) {
  const text = transcriptText();
  if (!text) {
    notify("record or type a transcript first", { level: "info", duration: 2200 });
    return;
  }
  if (agentModeOn()) {
    notify("voice run currently targets chat mode. switch chat/agent toggle to chat", {
      level: "info",
      duration: 3500,
    });
    return;
  }
  if (streaming) {
    notify("assistant is already running", { level: "info", duration: 2200 });
    return;
  }
  const profile = renderRoutingPreview(text);
  const route = resolveRoute(profile);
  if (route === "blocked") {
    notify("blocked by safe routing: high-risk command needs manual review", {
      level: "error",
      duration: 3800,
    });
    setVoiceStatus("high-risk request blocked by routing policy", "error");
    return;
  }
  if (els.voiceAutoSaveNeed?.checked) {
    await saveNeedFromText(text);
  }
  if (route === "agent") {
    voiceState.pendingSpeak = false;
    await submitAgentRun(text);
    setVoiceStatus(auto ? "heard you — running agent…" : "running agent from transcript…", "running");
  } else {
    voiceState.pendingSpeak = !!els.voiceAutoSpeak?.checked;
    els.prompt.value = text;
    autoresize();
    els.composer.requestSubmit();
    setVoiceStatus(auto ? "heard you — running assistant…" : "running assistant from transcript…", "running");
  }
}

function stopVoiceListening() {
  voiceState.manualStop = true;
  if (voiceState.recognition) {
    try {
      voiceState.recognition.stop();
    } catch (_) {}
  }
}

function startVoiceListening() {
  const Ctor = speechRecognitionCtor();
  if (!Ctor) {
    notify("speech recognition is not supported in this browser", { level: "error", duration: 3200 });
    setVoiceStatus("speech recognition unsupported", "error");
    return;
  }
  if (voiceState.listening) return;

  const rec = new Ctor();
  voiceState.recognition = rec;
  voiceState.manualStop = false;
  voiceState.lastError = null;
  rec.lang = "en-US";
  rec.continuous = !!els.voiceWakeMode?.checked;
  rec.interimResults = true;

  let stableText = "";
  rec.onstart = () => {
    setVoiceListeningUi(true);
    setVoiceStatus("listening… speak now", "running");
  };
  rec.onerror = (ev) => {
    voiceState.lastError = ev.error || "unknown";
    setVoiceStatus(`mic error: ${ev.error || "unknown"}`, "error");
    notify(`voice error: ${ev.error || "unknown"}`, { level: "error", duration: 3200 });
  };
  rec.onresult = (ev) => {
    const wakeMode = !!els.voiceWakeMode?.checked;
    let interim = "";
    for (let i = ev.resultIndex; i < ev.results.length; i += 1) {
      const t = (ev.results[i][0] && ev.results[i][0].transcript) || "";
      if (ev.results[i].isFinal) {
        stableText = `${stableText} ${t}`.trim();
        if (wakeMode) {
          const ww = wakeWord();
          const lower = t.toLowerCase();
          const idx = lower.indexOf(ww);
          if (idx >= 0 && Date.now() >= voiceState.wakeCooldownUntil) {
            const command = t.slice(idx + ww.length).replace(/^[\s,:.-]+/, "").trim();
            if (!command) {
              setVoiceStatus(`wake word heard (${ww}) — waiting for command`, "running");
            } else if (streaming) {
              setVoiceStatus("assistant busy — wake command queued in transcript", "running");
              if (els.voiceTranscriptInput) els.voiceTranscriptInput.value = command;
            } else {
              if (els.voiceTranscriptInput) els.voiceTranscriptInput.value = command;
              voiceState.wakeCooldownUntil = Date.now() + 1200;
              runVoiceTranscript({ auto: true });
            }
          }
        }
      } else {
        interim = `${interim} ${t}`.trim();
      }
    }
    if (!wakeMode && els.voiceTranscriptInput) {
      els.voiceTranscriptInput.value = `${stableText} ${interim}`.trim();
    } else if (wakeMode && !stableText && interim) {
      setVoiceStatus(`listening for wake word "${wakeWord()}"…`, "running");
    }
  };
  rec.onend = async () => {
    setVoiceListeningUi(false);
    const wakeMode = !!els.voiceWakeMode?.checked;
    if (wakeMode) {
      const fatal = voiceState.lastError === "not-allowed" || voiceState.lastError === "service-not-allowed";
      if (!voiceState.manualStop && !fatal) {
        setTimeout(() => {
          if (els.voiceWakeMode?.checked && !voiceState.manualStop) startVoiceListening();
        }, 140);
        return;
      }
    }
    const text = transcriptText();
    if (!text) {
      setVoiceStatus("no speech captured", "error");
      return;
    }
    if (els.voiceAutoRun?.checked) {
      await runVoiceTranscript({ auto: true });
    } else {
      setVoiceStatus("transcript ready — click run transcript", "idle");
    }
  };
  rec.start();
}

if (els.voiceAutoRun || els.voiceAutoSpeak || els.voiceAutoSaveNeed) {
  const prefs = readVoicePrefs();
  if (els.voiceAutoRun && typeof prefs.autoRun === "boolean") {
    els.voiceAutoRun.checked = prefs.autoRun;
  }
  if (els.voiceAutoSpeak && typeof prefs.autoSpeak === "boolean") {
    els.voiceAutoSpeak.checked = prefs.autoSpeak;
  }
  if (els.voiceAutoSaveNeed && typeof prefs.autoSaveNeed === "boolean") {
    els.voiceAutoSaveNeed.checked = prefs.autoSaveNeed;
  }
  if (els.voiceWakeMode && typeof prefs.wakeMode === "boolean") {
    els.voiceWakeMode.checked = prefs.wakeMode;
  }
  if (els.voiceWakeWordInput && typeof prefs.wakeWord === "string" && prefs.wakeWord.trim()) {
    els.voiceWakeWordInput.value = prefs.wakeWord.trim();
  }
  if (els.voiceRoutingMode && typeof prefs.routingMode === "string") {
    const allowed = new Set(["auto", "chat", "agent", "safe"]);
    if (allowed.has(prefs.routingMode)) {
      els.voiceRoutingMode.value = prefs.routingMode;
    }
  }
  [els.voiceAutoRun, els.voiceAutoSpeak, els.voiceAutoSaveNeed, els.voiceWakeMode, els.voiceWakeWordInput, els.voiceRoutingMode]
    .filter(Boolean)
    .forEach((el) => el.addEventListener("change", writeVoicePrefs));
}

if (els.voiceRoutingMode) {
  els.voiceRoutingMode.addEventListener("change", () => {
    renderRoutingPreview(transcriptText());
  });
}

if (els.voiceTranscriptInput) {
  els.voiceTranscriptInput.addEventListener("input", () => {
    renderRoutingPreview(transcriptText());
  });
}

if (els.voiceListenToggle) {
  els.voiceListenToggle.addEventListener("click", () => {
    if (voiceState.listening) stopVoiceListening();
    else startVoiceListening();
  });
}
if (els.voiceRunTranscript) {
  els.voiceRunTranscript.addEventListener("click", async () => {
    await runVoiceTranscript();
  });
}
if (els.voiceSaveNeed) {
  els.voiceSaveNeed.addEventListener("click", async () => {
    await saveNeedFromText(transcriptText());
  });
}
if (els.voiceExtractNeed) {
  els.voiceExtractNeed.addEventListener("click", async () => {
    const text = transcriptText();
    if (!text) {
      notify("record or type transcript first", { level: "info", duration: 2200 });
      return;
    }
    const need = extractNeedFromText(text);
    renderNeedPreview(need);
    await saveNeedFromText(need.summary);
  });
}
if (els.voiceClearTranscript) {
  els.voiceClearTranscript.addEventListener("click", () => {
    if (els.voiceTranscriptInput) {
      els.voiceTranscriptInput.value = "";
    }
    renderNeedPreview(null);
    renderRoutingPreview("");
    setVoiceStatus("transcript cleared", "idle");
  });
}

for (const name of Object.keys(VOICE_CONNECTOR_INFO)) {
  const refs = connectorRefs()[name];
  if (refs?.consent) {
    refs.consent.addEventListener("change", async () => {
      try {
        await setConnectorConsent(name, !!refs.consent.checked);
      } catch (e) {
        notify(`${VOICE_CONNECTOR_INFO[name].label}: ${e.message || e}`, { level: "error" });
        refreshConnectorStatus();
      }
    });
  }
  if (refs?.connect) {
    refs.connect.addEventListener("click", async () => {
      try {
        await connectConnector(name);
      } catch (e) {
        notify(`${VOICE_CONNECTOR_INFO[name].label}: ${e.message || e}`, { level: "error" });
      }
    });
  }
  if (refs?.disconnect) {
    refs.disconnect.addEventListener("click", async () => {
      try {
        await disconnectConnector(name);
      } catch (e) {
        notify(`${VOICE_CONNECTOR_INFO[name].label}: ${e.message || e}`, { level: "error" });
      }
    });
  }
}
refreshConnectorStatus();
renderRoutingPreview(transcriptText());

// Initial paint of the memory panel — runs alongside the first status
// poll so the sidebar reflects the persisted state on every page load.
refreshMemoryUI();

// ---------------------------------------------------------------- sidebar collapsibles
// Sidebar sections wrapped in <details class="sidebar-collapsible">
// remember their open/closed state per-element in localStorage so
// reloads don't reset whatever the user expanded. Keyed by element
// id so adding new collapsibles doesn't conflict.

const SIDEBAR_COLLAPSE_KEY_PREFIX = "unhosted-sidebar-open:";
for (const det of document.querySelectorAll(".sidebar-collapsible")) {
  if (!det.id) continue;
  const key = SIDEBAR_COLLAPSE_KEY_PREFIX + det.id;
  // Hydrate from storage. Default is closed (matches the HTML).
  try {
    if (localStorage.getItem(key) === "1") det.open = true;
  } catch (e) { /* private mode etc — fine */ }
  det.addEventListener("toggle", () => {
    try {
      if (det.open) localStorage.setItem(key, "1");
      else localStorage.removeItem(key);
    } catch (e) { /* ignore */ }
  });
}

// ---------------------------------------------------------------- vram-pool
// Surface for the v0.0.26 detection foundation (ADR 0009). Reports
// whether this machine has an RPC-capable llama.cpp build. v0.1.0
// orchestration commands will live on the same surface once they
// ship — the panel grows actions then, today it's read-only.

// Two pieces of state drive the panel: (1) the local-machine
// CAPABILITY probe (whether the binaries are present at all),
// from /v1/status.vram_pool, and (2) the actual POOL STATE
// (whether a pool is running, starting, etc.), from
// /v1/vram-pool. Status poll provides (1) on every tick;
// `pollVramPoolStatus` provides (2) on its own tighter cadence
// while we're in a transition. The panel re-renders from both.
let lastVramCap = null;
let lastVramPool = null;
let vramPoolPollTimer = null;

function renderVramPool(cap) {
  lastVramCap = cap || null;
  if (!els.vramStatus) return;
  if (!cap) {
    if (els.vramSection) els.vramSection.hidden = true;
    return;
  }
  if (els.vramSection) els.vramSection.hidden = false;
  if (els.vramDetails) els.vramDetails.hidden = false;
  renderVramCombined();
  // Kick off a /v1/vram-pool fetch in the background to populate
  // pool state (which the status poll doesn't carry). We'll only
  // refetch on a faster cadence if the pool is transitioning.
  refreshVramPoolStateNow();
}

function renderVramCombined() {
  if (!els.vramStatus) return;
  const cap = lastVramCap;
  const pool = lastVramPool;
  const ready = cap && cap.has_rpc_server_bin && cap.llama_server_has_rpc_flag;

  // Pool state takes precedence when something's actively
  // happening. Otherwise fall back to "capability ready / not ready".
  if (pool && pool.state === "starting") {
    const stage = (pool.stage || "spawning_local_rpc").replace(/_/g, " ");
    els.vramStatus.textContent = `starting — ${stage}…`;
    els.vramStatus.dataset.state = "starting";
    if (els.vramControls) els.vramControls.hidden = false;
    if (els.vramStart) {
      els.vramStart.hidden = true;
    }
    if (els.vramStop) els.vramStop.hidden = false;
    if (els.vramModelInput) els.vramModelInput.disabled = true;
    if (els.vramEndpointRow) els.vramEndpointRow.hidden = true;
  } else if (pool && pool.state === "running") {
    const model = (pool.plan && pool.plan.model) || "(unknown model)";
    const lh = (pool.plan && pool.plan.layer_hosts) || [];
    els.vramStatus.textContent = `running ${model} across ${lh.length} layer host${lh.length === 1 ? "" : "s"}`;
    els.vramStatus.dataset.state = "running";
    if (els.vramControls) els.vramControls.hidden = false;
    if (els.vramStart) els.vramStart.hidden = true;
    if (els.vramStop) els.vramStop.hidden = false;
    if (els.vramModelInput) els.vramModelInput.disabled = true;
    if (els.vramEndpointRow) els.vramEndpointRow.hidden = false;
    if (els.vramEndpoint) els.vramEndpoint.textContent = pool.endpoint || "—";
  } else if (pool && pool.state === "failed") {
    els.vramStatus.textContent = `failed — ${pool.error || "unknown error"}`;
    els.vramStatus.dataset.state = "failed";
    if (els.vramControls) els.vramControls.hidden = !ready;
    if (els.vramStart) els.vramStart.hidden = !ready;
    if (els.vramStop) els.vramStop.hidden = true;
    if (els.vramModelInput) els.vramModelInput.disabled = false;
    if (els.vramEndpointRow) els.vramEndpointRow.hidden = true;
  } else if (ready) {
    // Idle + ready
    els.vramStatus.textContent = "ready — pick a model and click start";
    els.vramStatus.dataset.state = "idle";
    if (els.vramControls) els.vramControls.hidden = false;
    if (els.vramStart) els.vramStart.hidden = false;
    if (els.vramStop) els.vramStop.hidden = true;
    if (els.vramModelInput) els.vramModelInput.disabled = false;
    if (els.vramEndpointRow) els.vramEndpointRow.hidden = true;
  } else if (cap && !cap.llama_server_path) {
    els.vramStatus.textContent = "no llama-server found — install llama.cpp to enable";
    els.vramStatus.dataset.state = "idle";
    if (els.vramControls) els.vramControls.hidden = true;
    if (els.vramEndpointRow) els.vramEndpointRow.hidden = true;
  } else {
    els.vramStatus.textContent =
      "llama.cpp installed, but built without -DGGML_RPC=ON — click details";
    els.vramStatus.dataset.state = "idle";
    if (els.vramControls) els.vramControls.hidden = true;
    if (els.vramEndpointRow) els.vramEndpointRow.hidden = true;
  }
}

// Build the layer-host picker. Each paired peer becomes a
// checkbox; the user-selected set drives `startVramPool`'s
// `--peers` list. Hidden when no paired peers exist (single-
// machine user — self-loopback is the only option).
//
// Hidden also while the pool is starting/running/hosting:
// changing the layer-host set mid-flight would require killing
// and re-planning, which is what the stop+restart flow already
// handles cleanly.
function renderVramPoolPeers(peers) {
  if (!els.vramPeersBlock || !els.vramPeersList) return;
  if (peers.length === 0 || (lastVramPool && lastVramPool.state !== "idle")) {
    els.vramPeersBlock.hidden = true;
    return;
  }
  els.vramPeersBlock.hidden = false;

  // Prune the selection set of peers that are no longer paired
  // (peer-unpaired between renders).
  const known = new Set(peers.map((p) => p.name));
  let pruned = false;
  for (const sel of vramSelectedPeers) {
    if (!known.has(sel)) {
      vramSelectedPeers.delete(sel);
      pruned = true;
    }
  }
  if (pruned) saveSelectedPeers(vramSelectedPeers);

  els.vramPeersList.innerHTML = "";
  for (const peer of peers) {
    const li = document.createElement("li");
    li.className = "vram-pool-peer-item";
    const label = document.createElement("label");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.dataset.peerName = peer.name;
    cb.checked = vramSelectedPeers.has(peer.name);
    cb.addEventListener("change", () => {
      if (cb.checked) {
        vramSelectedPeers.add(peer.name);
      } else {
        vramSelectedPeers.delete(peer.name);
      }
      saveSelectedPeers(vramSelectedPeers);
    });
    const nameSpan = document.createElement("span");
    nameSpan.className = "vram-pool-peer-name";
    nameSpan.textContent = peer.name;
    const trustSpan = document.createElement("span");
    trustSpan.className = peer.trusted
      ? "vram-pool-peer-badge trusted"
      : "vram-pool-peer-badge lan";
    trustSpan.textContent = peer.trusted ? "trusted" : "lan";
    label.append(cb, nameSpan, trustSpan);
    li.append(label);
    els.vramPeersList.append(li);
  }
}

async function refreshVramPoolStateNow() {
  try {
    const r = await fetch("/v1/vram-pool", { cache: "no-store" });
    if (!r.ok) return;
    const pool = await r.json();
    lastVramPool = pool;
    renderVramCombined();
    // Tight polling while transitioning, slow polling otherwise.
    if (pool.state === "starting") {
      if (!vramPoolPollTimer) {
        vramPoolPollTimer = setInterval(refreshVramPoolStateNow, 1500);
      }
    } else if (vramPoolPollTimer) {
      clearInterval(vramPoolPollTimer);
      vramPoolPollTimer = null;
    }
  } catch (e) {
    /* daemon transient — try again on next status tick */
  }
}

async function startVramPool() {
  if (!els.vramModelInput) return;
  const model = els.vramModelInput.value.trim();
  if (!model) {
    notify("paste a path to a .gguf model first", { level: "info", duration: 3000 });
    return;
  }

  // Look up addresses for selected peers from the latest status
  // snapshot. We need their daemon addr to build the LayerHost.addr
  // (peer_ip:50052) the orchestrator expects.
  const status = await fetch("/v1/status", { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : null))
    .catch(() => null);
  const peers = (status && status.peers) || [];

  const layer_hosts = [];
  for (const sel of vramSelectedPeers) {
    const peer = peers.find((p) => p.name === sel);
    if (!peer) {
      notify(`selected peer "${sel}" not found in registry`, { level: "error", duration: 4000 });
      return;
    }
    // peer.addr is "host:port" (the daemon's HTTP addr). Strip
    // the port and append 50052 for the rpc-server.
    const colon = peer.addr.lastIndexOf(":");
    const host = peer.addr.slice(0, colon);
    layer_hosts.push({ name: peer.name, addr: `${host}:50052` });
  }
  // No peers selected → self-loopback (local machine is the only
  // layer host). This matches the planner's default behavior.
  if (layer_hosts.length === 0) {
    layer_hosts.push({ name: "local", addr: "127.0.0.1:50052" });
  }

  const plan = {
    orchestrator: "local",
    layer_hosts,
    model,
  };
  els.vramStart.disabled = true;
  const layerSummary =
    layer_hosts.length === 1 && layer_hosts[0].name === "local"
      ? "self-loopback"
      : `across ${layer_hosts.length} peer${layer_hosts.length === 1 ? "" : "s"}`;
  notify(`starting vram-pool ${layerSummary} — this can take 30 s for a large model`, {
    level: "info",
    duration: 4000,
  });
  try {
    const r = await fetch("/v1/vram-pool/start", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ plan }),
    });
    if (!r.ok) {
      const body = await r.text();
      notify(`start failed: ${body.slice(0, 140)}`, { level: "error", duration: 6000 });
    }
    await refreshVramPoolStateNow();
  } catch (e) {
    notify(`start failed: ${e}`, { level: "error" });
  } finally {
    els.vramStart.disabled = false;
  }
}

async function stopVramPool() {
  const ok = await confirmDialog({
    title: "stop the pool?",
    message: "this will kill the rpc-server and llama-server processes. any in-flight chat against the pool's endpoint will fail.",
    confirmLabel: "stop",
    danger: true,
  });
  if (!ok) return;
  els.vramStop.disabled = true;
  try {
    const r = await fetch("/v1/vram-pool/stop", { method: "POST" });
    if (r.ok) notify("pool stopped", { level: "info", duration: 2000 });
  } catch (e) {
    notify(`stop failed: ${e}`, { level: "error" });
  } finally {
    els.vramStop.disabled = false;
    await refreshVramPoolStateNow();
  }
}

if (els.vramStart) {
  els.vramStart.addEventListener("click", startVramPool);
}
if (els.vramStop) {
  els.vramStop.addEventListener("click", stopVramPool);
}

function fillVramPoolModal() {
  // Snapshot from the latest status poll. We could refetch /v1/status
  // here for fresher data, but the sidebar already polls every 8 s,
  // and the values are cheap to refresh by reopening the modal.
  fetch("/v1/status", { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : null))
    .then((s) => {
      if (!s || !s.vram_pool) return;
      const cap = s.vram_pool;
      if (els.vramLlamaPath)
        els.vramLlamaPath.textContent =
          cap.llama_server_path || "(not found on PATH)";
      if (els.vramRpcFlag)
        els.vramRpcFlag.textContent = cap.llama_server_has_rpc_flag
          ? "yes"
          : "no — build lacks -DGGML_RPC=ON";
      if (els.vramRpcPath)
        els.vramRpcPath.textContent =
          cap.rpc_server_path || "(not found on PATH)";
      const ready =
        cap.has_rpc_server_bin && cap.llama_server_has_rpc_flag;
      if (els.vramReady) els.vramReady.textContent = ready ? "YES" : "no";
      if (els.vramHint) {
        if (ready) {
          els.vramHint.textContent =
            "this machine can act as both orchestrator and layer host. orchestration commands ship in v0.1.0.";
        } else if (!cap.llama_server_path) {
          els.vramHint.textContent =
            "llama-server not found on PATH. install llama.cpp via your package manager.";
        } else {
          els.vramHint.innerHTML =
            'llama.cpp is installed but was NOT built with <code>-DGGML_RPC=ON</code>. ' +
            "until upstream Homebrew lands the change, build from source with that flag, " +
            'or watch the <code>unhosted-ai/homebrew-unhosted</code> tap announcement.';
        }
      }
    })
    .catch(() => {});
}

if (els.vramDetails && els.vramModal) {
  const closeVram = () => {
    els.vramModal.hidden = true;
  };
  els.vramDetails.addEventListener("click", () => {
    els.vramModal.hidden = false;
    fillVramPoolModal();
  });
  if (els.vramModalClose)
    els.vramModalClose.addEventListener("click", closeVram);
  els.vramModal.addEventListener("click", (e) => {
    if (e.target === els.vramModal) closeVram();
  });
}

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

// --------------------------------------------------------- public-mode
// ADR-0010 slice 2. Read/write the PeerPaymentPolicy that this node
// advertises. The policy is rail-gating only — nothing here moves
// money. The quote endpoint (slice 3) is what actually consults it.

const ALL_RAILS = [
  "lightning",
  "usdc_base",
  "usdc_solana",
  "stripe_connect",
  "apple_pay",
  "manual",
];

async function fetchPublicModePolicy() {
  try {
    const r = await fetch("/v1/public-mode/policy");
    if (!r.ok) return null;
    return await r.json();
  } catch (_) {
    return null;
  }
}

async function savePublicModePolicy(policy) {
  const r = await fetch("/v1/public-mode/policy", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(policy),
  });
  if (!r.ok) throw new Error(`save failed: ${r.status}`);
  return await r.json();
}

function renderPublicModePolicy(policy) {
  const statusLine = document.getElementById("public-mode-status-line");
  const badge = document.getElementById("public-mode-badge");
  if (!policy) {
    if (statusLine) statusLine.textContent = "could not load";
    if (badge) {
      badge.textContent = "?";
      badge.dataset.state = "closed";
    }
    return;
  }
  const rails = new Set(policy.accepted_rails || []);
  for (const cb of document.querySelectorAll('#public-mode-rails-list input[type="checkbox"]')) {
    cb.checked = rails.has(cb.dataset.rail);
  }
  const kyc = document.getElementById("public-mode-kyc");
  if (kyc) kyc.value = policy.min_kyc || "none";
  const blocked = document.getElementById("public-mode-blocked");
  if (blocked) blocked.value = (policy.blocked_countries || []).join(", ");
  if (rails.size === 0) {
    if (badge) { badge.textContent = "closed"; badge.dataset.state = "closed"; }
    if (statusLine) statusLine.textContent = "accepts nothing";
  } else {
    if (badge) { badge.textContent = "open"; badge.dataset.state = "open"; }
    if (statusLine) {
      const n = rails.size;
      statusLine.textContent = `${n} rail${n === 1 ? "" : "s"} · min kyc ${policy.min_kyc || "none"}`;
    }
  }
}

function readPublicModePolicyFromUI() {
  const accepted_rails = [];
  for (const cb of document.querySelectorAll('#public-mode-rails-list input[type="checkbox"]')) {
    if (cb.checked && ALL_RAILS.includes(cb.dataset.rail)) {
      accepted_rails.push(cb.dataset.rail);
    }
  }
  const min_kyc = document.getElementById("public-mode-kyc").value || "none";
  const blockedRaw = document.getElementById("public-mode-blocked").value || "";
  const blocked_countries = blockedRaw
    .split(/[\s,]+/)
    .map((c) => c.trim().toUpperCase())
    .filter((c) => /^[A-Z]{2}$/.test(c));
  return { accepted_rails, min_kyc, blocked_countries };
}

const publicModeSave = document.getElementById("public-mode-save");
if (publicModeSave) {
  publicModeSave.addEventListener("click", async () => {
    try {
      const policy = readPublicModePolicyFromUI();
      const saved = await savePublicModePolicy(policy);
      renderPublicModePolicy(saved);
      notify("public-mode policy saved", { level: "success", duration: 2000 });
    } catch (e) {
      notify(`save failed: ${e.message || e}`, { level: "error" });
    }
  });
}

fetchPublicModePolicy().then(renderPublicModePolicy);

async function inspectPublicModePolicy(payer) {
  const r = await fetch("/v1/public-mode/policy/inspect", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payer),
  });
  if (!r.ok) throw new Error(`inspect failed: ${r.status}`);
  return await r.json();
}

const publicModeInspectBtn = document.getElementById("public-mode-inspect-btn");
if (publicModeInspectBtn) {
  publicModeInspectBtn.addEventListener("click", async () => {
    const resultEl = document.getElementById("public-mode-inspect-result");
    try {
      const rail = document.getElementById("public-mode-inspect-rail").value;
      const kyc = document.getElementById("public-mode-inspect-kyc").value;
      let country = (document.getElementById("public-mode-inspect-country").value || "").trim().toUpperCase();
      if (!/^[A-Z]{2}$/.test(country)) {
        resultEl.textContent = "country must be a two-letter ISO code";
        resultEl.dataset.state = "error";
        return;
      }
      const out = await inspectPublicModePolicy({ rail, kyc, country });
      if (out.accepted) {
        resultEl.textContent = "✓ accepted";
        resultEl.dataset.state = "ok";
      } else {
        resultEl.textContent = `✗ ${out.reason || "rejected"}`;
        resultEl.dataset.state = "error";
      }
    } catch (e) {
      resultEl.textContent = `error: ${e.message || e}`;
      resultEl.dataset.state = "error";
    }
  });
}

// ------------------------------------------------------------- benchmark
// Sidebar panel that fires a real chat completion against the local
// node, measures wall time + token count from the upstream's usage
// field, and reports tok/sec. History is persisted to localStorage so
// the panel survives reloads. Same shape as scripts /tmp/bench.py
// from the 2026-05-20 loopback run — moves "what tok/sec am I doing
// right now" out of the terminal and into the app.

const BENCH_HISTORY_KEY = "unhosted-bench-history-v1";
const BENCH_HISTORY_MAX = 25;
const BENCH_PROMPT = "Explain quantum tunneling in three sentences.";
const BENCH_MAX_TOKENS = 200;

function loadBenchHistory() {
  try {
    const raw = localStorage.getItem(BENCH_HISTORY_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw);
    return Array.isArray(arr) ? arr : [];
  } catch (_) {
    return [];
  }
}

function saveBenchHistory(history) {
  try {
    localStorage.setItem(
      BENCH_HISTORY_KEY,
      JSON.stringify(history.slice(-BENCH_HISTORY_MAX)),
    );
  } catch (_) {
    /* localStorage full / unavailable / private mode: ignore */
  }
}

function median(xs) {
  if (xs.length === 0) return 0;
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[mid - 1] + sorted[mid]) / 2
    : sorted[mid];
}

async function runBenchmark() {
  const statusEl = document.getElementById("bench-status");
  const runBtn = document.getElementById("bench-run");
  const resultsEl = document.getElementById("bench-results");
  if (!statusEl || !runBtn || !resultsEl) return;

  runBtn.disabled = true;
  statusEl.textContent = "running… (≤ 60s)";
  const t0 = performance.now();

  let body;
  try {
    const resp = await fetch("/v1/chat/completions", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: "default",
        messages: [{ role: "user", content: BENCH_PROMPT }],
        max_tokens: BENCH_MAX_TOKENS,
        temperature: 0.0,
        stream: false,
      }),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    body = await resp.json();
  } catch (e) {
    statusEl.textContent = `error: ${e.message || e}`;
    runBtn.disabled = false;
    return;
  }

  const elapsedMs = performance.now() - t0;
  const usage = body.usage || {};
  const outTokens = usage.completion_tokens || 0;
  if (!outTokens || elapsedMs <= 0) {
    statusEl.textContent = "ran but upstream didn't report completion_tokens — can't compute tok/sec";
    runBtn.disabled = false;
    return;
  }
  const tps = (outTokens * 1000) / elapsedMs;
  const model = body.model || "(unknown)";
  // The OpenAI-compat layer doesn't surface "via" reliably; we infer
  // from the daemon's status endpoint.
  let via = "local";
  try {
    const s = await (await fetch("/v1/status")).json();
    via = s.upstream?.url || "local";
  } catch (_) {}

  const entry = {
    at: Date.now(),
    tps,
    elapsedMs,
    outTokens,
    model,
    via,
  };
  const history = loadBenchHistory();
  history.push(entry);
  saveBenchHistory(history);

  renderBench(entry, history);
  runBtn.disabled = false;
  statusEl.textContent = "done.";
}

function renderBench(entry, history) {
  const resultsEl = document.getElementById("bench-results");
  const histWrap = document.getElementById("bench-history-wrap");
  if (!resultsEl) return;

  if (entry) {
    document.getElementById("bench-last").textContent = `${entry.tps.toFixed(1)} tok/s  (${entry.outTokens} tok / ${(entry.elapsedMs / 1000).toFixed(2)}s)`;
    document.getElementById("bench-last").dataset.good = entry.tps >= 10 ? "yes" : "no";
    document.getElementById("bench-model").textContent = entry.model;
    document.getElementById("bench-via").textContent = entry.via;
  }

  const last5 = history.slice(-5).map((e) => e.tps);
  document.getElementById("bench-median").textContent =
    last5.length === 0 ? "—" : `${median(last5).toFixed(1)} tok/s  (n=${last5.length})`;

  resultsEl.hidden = history.length === 0;
  if (histWrap) histWrap.hidden = history.length < 2;

  const histList = document.getElementById("bench-history");
  if (histList) {
    histList.innerHTML = "";
    for (const e of history.slice().reverse()) {
      const li = document.createElement("li");
      const when = new Date(e.at);
      const ts = when.toLocaleTimeString();
      li.textContent = `${ts}  ${e.tps.toFixed(1)} tok/s  ${e.outTokens}tok/${(e.elapsedMs / 1000).toFixed(1)}s`;
      histList.appendChild(li);
    }
  }
}

const benchRunBtn = document.getElementById("bench-run");
if (benchRunBtn) {
  benchRunBtn.addEventListener("click", runBenchmark);
  // Render any persisted history on load — so users see the panel
  // populated immediately, not just after a fresh run.
  const persisted = loadBenchHistory();
  if (persisted.length > 0) {
    renderBench(persisted[persisted.length - 1], persisted);
    document.getElementById("bench-status").textContent =
      `last run ${new Date(persisted[persisted.length - 1].at).toLocaleString()}`;
  }
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

// ---------------------------------------------------------- agent mode
//
// ADR-0012 slice 3: Web UI surface for the agent runtime. Adds:
//   - A Chat / Agent toggle in the main header.
//   - A tool-allow-list bar above the composer (visible only in
//     agent mode).
//   - A forked submit path that POSTs to /v1/agents/run and renders
//     the step trace inline in the conversation.
//
// State lives in localStorage so the user's chosen mode survives
// reloads. The toggle's UI is purely cosmetic until they actually
// submit — switching modes mid-typing doesn't lose what's in the
// composer.

const AGENT_MODE_LS_KEY = "unhosted.agentMode";

function agentModeOn() {
  return document.body.classList.contains("agent-mode");
}

function setAgentMode(on) {
  document.body.classList.toggle("agent-mode", on);
  const bar = document.getElementById("agent-bar");
  if (bar) bar.hidden = !on;
  // Composer placeholder reflects the mode so the user sees the
  // affordance, not just an abstract toggle.
  if (els.prompt) {
    els.prompt.placeholder = on
      ? "give the agent a goal — it can fetch URLs, query memory, list models."
      : "ask anything. shift+enter for newline.";
  }
  // Toggle visual state on the buttons.
  document.querySelectorAll(".mode-toggle-btn").forEach((b) => {
    const active = (b.dataset.mode === "agent") === on;
    b.classList.toggle("is-active", active);
    b.setAttribute("aria-selected", active ? "true" : "false");
  });
  try {
    localStorage.setItem(AGENT_MODE_LS_KEY, on ? "1" : "0");
  } catch (_) {}
}

// Wire the toggle buttons. Each click sets agent mode to whichever
// the clicked button represents.
document.querySelectorAll(".mode-toggle-btn").forEach((b) => {
  b.addEventListener("click", () => {
    setAgentMode(b.dataset.mode === "agent");
  });
});

// Restore previous mode on load.
try {
  if (localStorage.getItem(AGENT_MODE_LS_KEY) === "1") {
    setAgentMode(true);
  }
} catch (_) {}

function getSelectedAgentTools() {
  return Array.from(
    document.querySelectorAll('.agent-tool-chip input[type="checkbox"]:checked'),
  ).map((cb) => cb.dataset.agentTool);
}

/**
 * Run an agent against the supplied goal. Posts to /v1/agents/run,
 * renders the step trace into the conversation, marks the final
 * answer with `.turn--agent-final`. Captures step records into the
 * active chat so reload preserves the trace.
 */
async function submitAgentRun(goal) {
  const chat = ensureActiveChat();
  const now = Date.now();
  const userMsg = { role: "user", text: goal, ts: now };
  chat.messages.push(userMsg);
  chat.updatedAt = now;
  if (chat.messages.length === 1) {
    chat.title = truncate(goal, 48);
    els.topic.textContent = chat.title;
  }
  // Hoist to top of recency list, same as a normal chat would.
  const idx = store.chats.findIndex((c) => c.id === chat.id);
  if (idx > 0) {
    store.chats.splice(idx, 1);
    store.chats.unshift(chat);
  }
  putChat(chat);
  renderChatList();
  if (els.empty) els.empty.style.display = "none";
  renderMessage(userMsg);

  // Show a thinking indicator immediately. The agent endpoint is
  // non-streaming in slice 1, so this is the only liveness signal
  // the user gets until the response arrives.
  const thinkingMsg = {
    role: "agent_thinking",
    text: "running agent…",
    ts: Date.now(),
  };
  const thinkingNode = renderAgentTurn(thinkingMsg);

  streaming = true;
  setSendMode("stop");
  els.meta.innerHTML = '<span class="info">agent run in progress…</span>';

  const tools = getSelectedAgentTools();
  const body = {
    goal,
    tools,
    max_steps: 8,
    max_tokens: 4096,
    max_seconds: 60,
    model: "auto",
  };

  let resp;
  try {
    const r = await fetch("/v1/agents/run", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!r.ok) {
      const txt = await r.text().catch(() => "");
      throw new Error(`daemon ${r.status}: ${txt || r.statusText}`);
    }
    resp = await r.json();
  } catch (err) {
    thinkingNode.remove();
    const errorMsg = {
      role: "agent_error",
      text: `agent run failed: ${err && err.message ? err.message : err}`,
      ts: Date.now(),
    };
    chat.messages.push(errorMsg);
    renderAgentTurn(errorMsg);
    streaming = false;
    setSendMode("send");
    els.meta.innerHTML = '<span class="hint">enter to send</span>';
    putChat(chat);
    return;
  }

  // Replace the thinking indicator with the rendered trace.
  thinkingNode.remove();

  const steps = Array.isArray(resp.steps) ? resp.steps : [];
  for (const step of steps) {
    if (step.kind === "model_message") {
      // A model turn with no content + tool_calls > 0 is a "thinking"
      // step. Show it compactly so the conversation reads as
      // "agent decided to fetch / agent decided" rather than blank
      // assistant bubbles.
      if (!step.content || step.content === "") {
        const m = {
          role: "agent_thinking",
          text: `step ${step.step}: deciding (${step.tool_calls_made} tool call${step.tool_calls_made === 1 ? "" : "s"})`,
          ts: Date.now(),
        };
        chat.messages.push(m);
        renderAgentTurn(m);
      } else {
        const m = {
          role: "assistant",
          text: step.content,
          ts: Date.now(),
          agentStep: step.step,
        };
        chat.messages.push(m);
        renderMessage(m);
      }
    } else if (step.kind === "tool_call") {
      const m = {
        role: "agent_tool",
        tool: step.tool,
        argsHash: step.args_hash,
        resultChars: step.result_chars,
        error: step.error || null,
        ts: Date.now(),
      };
      chat.messages.push(m);
      renderAgentTurn(m);
    }
  }

  // Final answer: highlight it so the user's eye lands on the
  // result, not the trace. If the last step was a model_message
  // with content, mark its bubble as final.
  const finalText = resp.final_answer || "";
  const lastConvBubble = els.conversation.lastElementChild;
  if (lastConvBubble && finalText && lastConvBubble.dataset.role === "assistant") {
    lastConvBubble.classList.add("turn--agent-final");
  } else if (finalText) {
    // No assistant bubble preceded — the run terminated on a
    // non-final-answer reason (max_steps, dlp_blocked, etc.) and
    // `final_answer` carries the system's explanation. Render it.
    const m = {
      role: "agent_error",
      text: finalText,
      ts: Date.now(),
    };
    chat.messages.push(m);
    renderAgentTurn(m);
  }

  // Stop footer indicator.
  const reasonLabel = (resp.stopped_because || "unknown").replace(/_/g, " ");
  els.meta.innerHTML = `<span class="info">agent stopped: ${escapeHtml(reasonLabel)} · ${resp.tokens_used || 0} tokens · run ${escapeHtml((resp.run_id || "").slice(0, 8))}</span>`;
  streaming = false;
  setSendMode("send");
  putChat(chat);
  els.scroll.scrollTop = els.scroll.scrollHeight;
}

/**
 * Render a non-chat agent turn (thinking indicator, tool call, or
 * error) and return the DOM node so the caller can remove or update
 * it later. Mirrors renderMessage()'s shape so the existing layout
 * works without changes.
 */
function renderAgentTurn(msg) {
  const node = document.createElement("div");
  node.className = "turn";
  node.dataset.role = msg.role;

  if (msg.role === "agent_thinking") {
    node.classList.add("turn--agent-thinking");
    node.textContent = msg.text || "thinking…";
  } else if (msg.role === "agent_tool") {
    node.classList.add("turn--agent-tool");
    if (msg.error) node.classList.add("is-error");
    const name = document.createElement("span");
    name.className = "agent-tool-name";
    name.textContent = `🔧 ${msg.tool}`;
    const meta = document.createElement("span");
    meta.className = "agent-tool-meta";
    if (msg.error) {
      meta.textContent = `error: ${truncate(String(msg.error), 120)}`;
    } else {
      meta.textContent = `args#${(msg.argsHash || "").slice(0, 8)}  ${msg.resultChars || 0} chars`;
    }
    node.append(name, meta);
  } else if (msg.role === "agent_error") {
    node.classList.add("turn--agent-tool", "is-error");
    node.textContent = msg.text || "agent error";
  }
  els.conversation.appendChild(node);
  els.scroll.scrollTop = els.scroll.scrollHeight;
  return node;
}

// ---------------------------------------------------------- settings modal
// Owns every "configuration" panel that used to clutter the sidebar
// (tunnel, phone, memory, vram-pool, public-mode, benchmark, developer).
// Opened by the gear icon in the sidebar footer; tabbed into three
// logical buckets.
const settingsEls = {
  modal: $("#settings-modal"),
  close: $("#settings-modal-close"),
  openBtn: $("#settings-open"),
  tunnelChip: $("#conn-tunnel-chip"),
  tunnelChipLabel: $("#conn-tunnel-chip-label"),
  tabs: () => Array.from(document.querySelectorAll(".settings-tab")),
  panels: () => Array.from(document.querySelectorAll(".settings-panel")),
};

function openSettingsModal(tab) {
  if (!settingsEls.modal) return;
  settingsEls.modal.hidden = false;
  if (tab) switchSettingsTab(tab);
  // Focus the close button — gives ESC + tab-to-controls instant
  // affordance without trapping focus inside the modal.
  if (settingsEls.close) {
    setTimeout(() => settingsEls.close.focus(), 50);
  }
}

function closeSettingsModal() {
  if (!settingsEls.modal) return;
  settingsEls.modal.hidden = true;
}

function switchSettingsTab(name) {
  for (const t of settingsEls.tabs()) {
    const active = t.dataset.tab === name;
    t.classList.toggle("is-active", active);
    t.setAttribute("aria-selected", active ? "true" : "false");
  }
  for (const p of settingsEls.panels()) {
    p.hidden = p.dataset.panel !== name;
  }
}

if (settingsEls.openBtn) {
  settingsEls.openBtn.addEventListener("click", () => openSettingsModal());
}
// Auto-open the settings modal on first page load so features
// (tunnel/QR, vram-pool, memory, hosting) are visible upfront.
// User can dismiss; we remember the choice so it doesn't re-open
// on every refresh.
try {
  if (!localStorage.getItem("unhosted-settings-seen")) {
    setTimeout(() => {
      openSettingsModal("network");
      localStorage.setItem("unhosted-settings-seen", "1");
    }, 0);
  }
} catch (_) { /* localStorage may be unavailable in private mode */ }
if (settingsEls.close) {
  settingsEls.close.addEventListener("click", closeSettingsModal);
}
if (settingsEls.modal) {
  // Click backdrop to close (but not when clicking inside the modal).
  settingsEls.modal.addEventListener("click", (e) => {
    if (e.target === settingsEls.modal) closeSettingsModal();
  });
}
for (const t of settingsEls.tabs()) {
  t.addEventListener("click", () => switchSettingsTab(t.dataset.tab));
}
// ESC closes the settings modal (only when it's the topmost — if pair
// modal is open over it, this shouldn't fire). Listening on the
// modal itself rather than document so it doesn't fight other ESC
// handlers.
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (settingsEls.modal && !settingsEls.modal.hidden) {
    closeSettingsModal();
  }
});

// The compact tunnel-state chip in the connection row. Click → open
// settings on the network tab. State labels are kept in sync by
// `renderTunnel()` in the main flow — we re-read the chip's
// data-state and update its visible label whenever the tunnel state
// changes. To avoid touching renderTunnel() directly, watch the
// existing tunnel-status-line element for mutations: that line is
// already updated for every state transition.
if (settingsEls.tunnelChip) {
  settingsEls.tunnelChip.addEventListener("click", () =>
    openSettingsModal("network"),
  );
  const tunnelStatusLine = document.getElementById("tunnel-status-line");
  const privacyNote = document.getElementById("privacy-note");
  const privacyNoteText = document.getElementById("privacy-note-text");
  if (tunnelStatusLine) {
    const syncChip = () => {
      const text = (tunnelStatusLine.textContent || "").toLowerCase();
      let state = "off";
      let label = "local only";
      let privacyMsg =
        "all local — nothing leaves this machine.";
      if (text.includes("starting") || text.includes("connecting")) {
        state = "starting";
        label = "starting…";
        privacyMsg =
          "tunnel starting — about to be reachable from the public web.";
      } else if (
        text.startsWith("on ") ||
        text.includes("live") ||
        text.includes("public")
      ) {
        state = "on";
        label = "public";
        privacyMsg =
          "tunnel live — anyone with your bearer-token URL can reach this daemon.";
      }
      settingsEls.tunnelChip.dataset.state = state;
      if (settingsEls.tunnelChipLabel) {
        settingsEls.tunnelChipLabel.textContent = label;
      }
      if (privacyNote) privacyNote.dataset.state = state;
      if (privacyNoteText) privacyNoteText.textContent = privacyMsg;
    };
    const obs = new MutationObserver(syncChip);
    obs.observe(tunnelStatusLine, { childList: true, characterData: true, subtree: true });
    syncChip();
  }
}

// ---------------------------------------------------------- bug reporting
// Opens a prefilled issue template with runtime diagnostics so bug reports
// have enough context to reproduce without extra back-and-forth.
const BUG_REPORT_BASE_URL =
  "https://github.com/unhosted-ai/unhosted-core/issues/new";

async function readBugDiagnostics() {
  const diagnostics = {
    app: "unhosted web ui",
    origin: location.origin,
    path: location.pathname,
    userAgent: navigator.userAgent,
    utc: new Date().toISOString(),
  };
  try {
    const healthRes = await fetch("/health", { cache: "no-store" });
    diagnostics.healthHttp = healthRes.status;
    diagnostics.healthOk = healthRes.ok;
  } catch (_) {
    diagnostics.healthError = "request failed";
  }
  try {
    const statusRes = await fetch("/v1/status", { cache: "no-store" });
    diagnostics.statusHttp = statusRes.status;
    if (statusRes.ok) {
      const status = await statusRes.json();
      diagnostics.status = {
        mode: status.mode || null,
        model: status.model || null,
        upstream: status.upstream || null,
        daemonAddr: status.addr || null,
        peersCount: Array.isArray(status.peers) ? status.peers.length : null,
        publicMode: status.public_mode ? status.public_mode.state || "configured" : null,
      };
    }
  } catch (_) {
    diagnostics.statusError = "request failed";
  }

  return diagnostics;
}

function bugDiagnosticsMarkdown(diagnostics) {
  return [
    "## Summary",
    "Describe what you expected vs what happened.",
    "",
    "## Steps To Reproduce",
    "1. ",
    "2. ",
    "3. ",
    "",
    "## Diagnostics",
    "```json",
    JSON.stringify(diagnostics, null, 2),
    "```",
  ].join("\n");
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (_) {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand("copy");
      ta.remove();
      return true;
    } catch (__) {
      ta.remove();
      return false;
    }
  }
}

async function openBugReport() {
  const diagnostics = await readBugDiagnostics();
  const params = new URLSearchParams({
    labels: "bug",
    template: "bug_report.md",
    title: "[bug] ",
    body: bugDiagnosticsMarkdown(diagnostics),
  });
  window.open(`${BUG_REPORT_BASE_URL}?${params.toString()}`, "_blank", "noopener");
}

if (els.bugReportFooter) {
  els.bugReportFooter.addEventListener("click", async (e) => {
    // Always route through the prefilled flow even though href exists as a fallback.
    e.preventDefault();
    await openBugReport();
  });
}
if (els.bugReportOpen) {
  els.bugReportOpen.addEventListener("click", async () => {
    await openBugReport();
  });
}
if (els.bugReportCopyDiagnostics) {
  els.bugReportCopyDiagnostics.addEventListener("click", async () => {
    const diagnostics = await readBugDiagnostics();
    const ok = await copyText(bugDiagnosticsMarkdown(diagnostics));
    if (ok) {
      notify("diagnostics copied", { level: "success", duration: 2200 });
    } else {
      notify("couldn't access clipboard", { level: "error" });
    }
  });
}

// ---------------------------------------------------------- api access
// Inline copy-buttons for local base url + public tunnel url + bearer
// token. Visible in the sidebar so an MCP / OpenAI-compatible client
// can be configured without opening the settings modal.
(function wireApiAccess() {
  const $id = (s) => document.getElementById(s);
  const localBtn = $id("api-copy-local");
  const tokenBtn = $id("api-copy-token");
  const tokenPreview = $id("api-token-preview");
  const publicRow = $id("api-public-row");
  const publicBtn = $id("api-copy-public");
  const publicCode = $id("api-public-url");
  const localCode = $id("api-local-url");

  if (localCode) localCode.textContent = location.origin;

  const flashCopied = (btn) => {
    if (!btn) return;
    btn.classList.add("copied");
    setTimeout(() => btn.classList.remove("copied"), 900);
  };
  const copy = async (text, btn) => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      flashCopied(btn);
    } catch (_) {
      // Fallback for non-secure contexts (rare on loopback, but cheap).
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand("copy"); flashCopied(btn); } catch (__) {}
      ta.remove();
    }
  };

  if (localBtn) {
    localBtn.addEventListener("click", () => copy(location.origin, localBtn));
  }

  // Pre-fetch the bearer token once and cache it. /v1/auth/token only
  // succeeds from loopback, which is exactly where this page is served.
  let cachedToken = null;
  const fetchToken = async () => {
    if (cachedToken) return cachedToken;
    try {
      const r = await fetch("/v1/auth/token", { cache: "no-store" });
      if (!r.ok) return null;
      const j = await r.json();
      cachedToken = j.token || j.api_token || null;
      return cachedToken;
    } catch (_) { return null; }
  };
  fetchToken().then((t) => {
    if (t && tokenPreview) {
      tokenPreview.textContent = t.slice(0, 12) + "… (click to copy)";
    } else if (tokenPreview) {
      tokenPreview.textContent = "no token (loopback only)";
    }
  });
  if (tokenBtn) {
    tokenBtn.addEventListener("click", async () => {
      const t = await fetchToken();
      if (t) copy(t, tokenBtn);
    });
  }

  // Poll the tunnel status to surface the public url inline. Cheap
  // (only when the row is in the DOM) and decoupled from renderTunnel.
  let lastUrl = null;
  const refresh = async () => {
    if (!publicRow || !publicCode || !publicBtn) return;
    try {
      const r = await fetch("/v1/tunnel", { cache: "no-store" });
      if (!r.ok) return;
      const j = await r.json();
      if (j.state === "running" && j.url) {
        const t = await fetchToken();
        const sep = j.url.includes("?") ? "&" : "?";
        const href = t ? `${j.url}${sep}api_token=${encodeURIComponent(t)}` : j.url;
        if (href !== lastUrl) {
          lastUrl = href;
          publicCode.textContent = href;
          publicBtn.dataset.copy = href;
        }
        publicRow.hidden = false;
      } else {
        publicRow.hidden = true;
        lastUrl = null;
      }
    } catch (_) {}
  };
  if (publicBtn) {
    publicBtn.addEventListener("click", () => copy(publicBtn.dataset.copy || "", publicBtn));
  }
  refresh();
  setInterval(refresh, 5000);
})();

// ---------------------------------------------------------- desktop updater
(function wireDesktopUpdater() {
  const invoke = globalThis.__TAURI__?.core?.invoke;
  if (!els.appUpdateSection || !els.appUpdateUnavailable) return;

  if (!invoke) {
    els.appUpdateUnavailable.hidden = false;
    return;
  }

  els.appUpdateSection.hidden = false;

  const setStatus = (text) => {
    if (els.appUpdateStatus) els.appUpdateStatus.textContent = text;
  };

  const setBusy = (busy) => {
    if (!els.appUpdateCheck) return;
    els.appUpdateCheck.disabled = busy;
    if (els.appUpdateCheckLabel) {
      els.appUpdateCheckLabel.textContent = busy ? "checking…" : "check for updates";
    }
  };

  els.appUpdateCheck?.addEventListener("click", async () => {
    setBusy(true);
    setStatus("checking for signed desktop updates…");
    try {
      const res = await invoke("check_for_app_update");
      if (res?.available) {
        const latest = res.latestVersion ? `v${res.latestVersion}` : "a newer version";
        setStatus(`${latest} is available — follow the native updater prompt.`);
      } else {
        const current = res?.currentVersion ? `v${res.currentVersion}` : "this version";
        setStatus(`${current} is up to date.`);
      }
    } catch (err) {
      const msg = typeof err === "string" ? err : (err && err.message) || "update check failed";
      setStatus(`update check failed — ${msg}`);
    } finally {
      setBusy(false);
    }
  });
})();

// ---------------------------------------------------------- model manager
// LM Studio-style library (settings -> compute -> models): curated
// catalog with one-click downloads, installed list with load/switch/
// delete, and a supervised llama-server runtime the daemon spawns on
// the upstream port. Polls /v1/models/manage while the settings modal
// is open so progress bars and runtime transitions render live.
(function wireModelManager() {
  const els = {
    section: document.getElementById("models-section"),
    dirPath: document.getElementById("models-dir-path"),
    statusLine: document.getElementById("models-status-line"),
    runtimeActions: document.getElementById("models-runtime-actions"),
    unloadBtn: document.getElementById("models-unload"),
    installedWrap: document.getElementById("models-installed-wrap"),
    installedList: document.getElementById("models-installed-list"),
    catalogList: document.getElementById("models-catalog-list"),
    customUrl: document.getElementById("models-custom-url"),
    customBtn: document.getElementById("models-custom-download"),
  };
  if (!els.section) return;

  let snapshot = null;
  let pollTimer = null;

  function humanBytes(n) {
    if (!n || n <= 0) return "—";
    if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
    if (n >= 1e6) return `${(n / 1e6).toFixed(0)} MB`;
    return `${Math.max(1, Math.round(n / 1e3))} KB`;
  }

  async function api(path, opts) {
    const res = await fetch(path, opts);
    let body = null;
    try { body = await res.json(); } catch (_) { /* non-JSON error */ }
    if (!res.ok) {
      throw new Error((body && body.error) || `HTTP ${res.status}`);
    }
    return body;
  }

  function activeFile() {
    const r = snapshot?.runtime;
    if (!r) return null;
    if (r.state === "running" || r.state === "starting") return r.file;
    return null;
  }

  function renderStatus() {
    const r = snapshot.runtime;
    const d = snapshot.download;
    let text = "";
    let state = "idle";
    if (!snapshot.llama_server_found) {
      text = "llama-server not installed — run `brew install llama.cpp`, then models load with one click.";
      state = "error";
    } else if (r.state === "running") {
      text = `serving ${r.file} on :${r.port}`;
      state = "running";
    } else if (r.state === "starting") {
      text = `loading ${r.file}… (large models can take a minute)`;
      state = "running";
    } else if (r.state === "failed") {
      text = `runtime: ${r.error}`;
      state = "error";
    } else if (d.state === "downloading") {
      const pct = d.bytes_total > 0 ? ` ${(100 * d.bytes_done / d.bytes_total).toFixed(0)}%` : "";
      text = `downloading ${d.file}…${pct}`;
      state = "running";
    } else if (d.state === "failed") {
      text = `download failed: ${d.error}`;
      state = "error";
    } else if (snapshot.installed.length === 0) {
      text = "no local models yet — grab one below to get a private chat running.";
    } else {
      text = "pick a model from the library to start serving.";
    }
    els.statusLine.textContent = text;
    els.statusLine.dataset.state = state;
    els.runtimeActions.hidden = !(r.state === "running" || r.state === "starting");
  }

  function modelRow({ name, sub, buttons, active, progressPct }) {
    const li = document.createElement("li");
    if (active) li.classList.add("model-active");
    const meta = document.createElement("div");
    meta.className = "model-meta";
    const nameEl = document.createElement("div");
    nameEl.className = "model-name";
    nameEl.textContent = name;
    const subEl = document.createElement("div");
    subEl.className = "model-sub";
    subEl.textContent = sub;
    meta.appendChild(nameEl);
    meta.appendChild(subEl);
    if (progressPct != null) {
      const wrap = document.createElement("div");
      wrap.className = "model-progress";
      const bar = document.createElement("div");
      bar.className = "model-progress-bar";
      bar.style.width = `${progressPct}%`;
      wrap.appendChild(bar);
      meta.appendChild(wrap);
    }
    const actions = document.createElement("div");
    actions.className = "model-actions";
    for (const b of buttons) actions.appendChild(b);
    li.appendChild(meta);
    li.appendChild(actions);
    return li;
  }

  function actionBtn(label, { danger = false, disabled = false } = {}) {
    const btn = document.createElement("button");
    btn.className = danger ? "model-btn model-btn-danger" : "model-btn";
    btn.textContent = label;
    btn.disabled = disabled;
    return btn;
  }

  function render() {
    if (!snapshot) return;
    if (els.dirPath) els.dirPath.textContent = snapshot.models_dir;
    renderStatus();

    const act = activeFile();
    const dl = snapshot.download;

    // Library (installed files).
    els.installedWrap.hidden = snapshot.installed.length === 0;
    els.installedList.replaceChildren();
    for (const m of snapshot.installed) {
      const isActive = m.file === act;
      const loadBtn = actionBtn(isActive ? "loaded" : "load", {
        disabled: isActive || !snapshot.llama_server_found,
      });
      loadBtn.addEventListener("click", async () => {
        loadBtn.disabled = true;
        try {
          await api("/v1/models/load", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ file: m.file }),
          });
          notify(`loading ${m.file}`, { level: "success", duration: 2400 });
        } catch (e) {
          notify(`load failed: ${e.message}`, { level: "error" });
        }
        refresh();
      });
      const delBtn = actionBtn("delete", { danger: true, disabled: isActive });
      delBtn.addEventListener("click", async () => {
        if (!confirm(`delete ${m.file} from disk?`)) return;
        try {
          await api(`/v1/models/file/${encodeURIComponent(m.file)}`, { method: "DELETE" });
          notify(`${m.file} deleted`, { level: "success", duration: 2200 });
        } catch (e) {
          notify(`delete failed: ${e.message}`, { level: "error" });
        }
        refresh();
      });
      els.installedList.appendChild(
        modelRow({
          name: m.file,
          sub: humanBytes(m.size_bytes),
          buttons: [loadBtn, delBtn],
          active: isActive,
        }),
      );
    }

    // Catalog (downloadable).
    els.catalogList.replaceChildren();
    for (const c of snapshot.catalog) {
      const isDownloading = dl.state === "downloading" && dl.file === c.file;
      let buttons;
      let progressPct = null;
      if (c.installed) {
        buttons = [actionBtn("installed", { disabled: true })];
      } else if (isDownloading) {
        progressPct = dl.bytes_total > 0 ? Math.min(100, (100 * dl.bytes_done) / dl.bytes_total) : 0;
        const cancelBtn = actionBtn("cancel", { danger: true });
        cancelBtn.addEventListener("click", async () => {
          try {
            await api("/v1/models/download/cancel", { method: "POST" });
          } catch (e) {
            notify(`cancel failed: ${e.message}`, { level: "error" });
          }
          refresh();
        });
        buttons = [cancelBtn];
      } else {
        const dlBtn = actionBtn("download", {
          disabled: dl.state === "downloading",
        });
        dlBtn.addEventListener("click", async () => {
          dlBtn.disabled = true;
          try {
            await api("/v1/models/download", {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify({ id: c.id }),
            });
            notify(`downloading ${c.name}`, { level: "success", duration: 2400 });
          } catch (e) {
            notify(`download failed: ${e.message}`, { level: "error" });
          }
          refresh();
        });
        buttons = [dlBtn];
      }
      els.catalogList.appendChild(
        modelRow({
          name: c.name,
          sub: `${humanBytes(c.size_bytes)} · ${c.blurb}`,
          buttons,
          progressPct,
        }),
      );
    }
  }

  async function refresh() {
    try {
      snapshot = await api("/v1/models/manage");
      render();
    } catch (e) {
      if (els.statusLine) {
        els.statusLine.textContent = `models: ${e.message}`;
        els.statusLine.dataset.state = "error";
      }
    }
  }

  if (els.unloadBtn) {
    els.unloadBtn.addEventListener("click", async () => {
      try {
        await api("/v1/models/unload", { method: "POST" });
        notify("model ejected", { level: "success", duration: 2200 });
      } catch (e) {
        notify(`eject failed: ${e.message}`, { level: "error" });
      }
      refresh();
    });
  }

  if (els.customBtn) {
    els.customBtn.addEventListener("click", async () => {
      const url = (els.customUrl?.value || "").trim();
      if (!url) {
        notify("paste a huggingface .gguf url first", { level: "info", duration: 2400 });
        return;
      }
      try {
        await api("/v1/models/download", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ url }),
        });
        notify("download started", { level: "success", duration: 2200 });
        if (els.customUrl) els.customUrl.value = "";
      } catch (e) {
        notify(`download failed: ${e.message}`, { level: "error" });
      }
      refresh();
    });
  }

  // Poll fast while the settings modal is open (progress bars), slow
  // otherwise (catches runtime death even with the modal closed).
  function schedule() {
    clearInterval(pollTimer);
    const settingsOpen = !document.getElementById("settings-modal")?.hidden;
    pollTimer = setInterval(refresh, settingsOpen ? 2500 : 15000);
  }
  const settingsModal = document.getElementById("settings-modal");
  if (settingsModal) {
    new MutationObserver(schedule).observe(settingsModal, {
      attributes: true,
      attributeFilter: ["hidden"],
    });
  }
  refresh();
  schedule();
})();

// ---------------------------------------------------------- service worker
// Registers the offline-shell worker (sw.js). Required for Chrome
// Android's "Install app" prompt; on iOS the apple-touch-icon +
// manifest cover Add to Home Screen. Daemon API paths are excluded
// from caching inside the worker itself. Browsers only allow SWs on
// secure contexts (https or loopback) — register() would just reject
// elsewhere, so the guard is for log cleanliness, not security.
(function registerServiceWorker() {
  if (!("serviceWorker" in navigator)) return;
  const secure =
    location.protocol === "https:" ||
    location.hostname === "127.0.0.1" ||
    location.hostname === "localhost";
  if (!secure) return;
  navigator.serviceWorker.register("/sw.js").catch((e) => {
    console.warn("service worker registration failed", e);
  });
})();
