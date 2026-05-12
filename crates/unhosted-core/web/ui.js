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

// ---------------------------------------------------------------- elements

const els = {
  composer: $("#composer"),
  prompt: $("#prompt"),
  send: $("#send"),
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
  chatList: $("#chat-list"),
  discoveredSection: $("#discovered-section"),
  discoveredList: $("#discovered-list"),
};

let streaming = false;

// ---------------------------------------------------------------- chat store

const STORE_KEY = "unhosted-chats";
const MAX_CHATS = 50;

const store = loadStore();

function loadStore() {
  let raw = null;
  try { raw = localStorage.getItem(STORE_KEY); } catch (e) {}
  if (!raw) return { activeId: null, chats: [] };
  try {
    const parsed = JSON.parse(raw);
    if (!parsed.chats) return { activeId: null, chats: [] };
    return parsed;
  } catch (e) {
    return { activeId: null, chats: [] };
  }
}

function saveStore() { safeSet(STORE_KEY, JSON.stringify(store)); }

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
    store.activeId = chat.id;
    if (store.chats.length > MAX_CHATS) store.chats.length = MAX_CHATS;
    saveStore();
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
  store.activeId = chat.id;
  if (store.chats.length > MAX_CHATS) store.chats.length = MAX_CHATS;
  saveStore();
  renderChatList();
  renderActiveChat();
  els.prompt.focus();
}

function switchToChat(id) {
  if (!store.chats.some((c) => c.id === id)) return;
  store.activeId = id;
  saveStore();
  renderChatList();
  renderActiveChat();
}

// ---------------------------------------------------------------- rendering

function renderChatList() {
  els.chatList.innerHTML = "";
  if (store.chats.length === 0) {
    const li = document.createElement("li");
    li.className = "chat-item empty";
    li.textContent = "no chats yet";
    els.chatList.append(li);
    return;
  }
  for (const chat of store.chats) {
    const li = document.createElement("li");
    li.className = "chat-item" + (chat.id === store.activeId ? " active" : "");
    li.dataset.chatId = chat.id;
    li.textContent = chat.title || "new chat";
    li.addEventListener("click", () => switchToChat(chat.id));
    els.chatList.append(li);
  }
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

function renderStatus(s) {
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
      left.style.display = "flex";
      left.style.flexDirection = "column";

      const nameRow = document.createElement("span");
      nameRow.className = "pname";
      const nameText = document.createElement("span");
      nameText.textContent = peer.name;
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
      left.append(nameRow, addr);

      const unpair = document.createElement("button");
      unpair.className = "unpair";
      unpair.textContent = "unpair";
      unpair.title = `unpair ${peer.name}`;
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
        pair.textContent = "pair";
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
autoresize();

// ---------------------------------------------------------------- submit

els.composer.addEventListener("submit", async (e) => {
  e.preventDefault();
  const prompt = els.prompt.value.trim();
  if (!prompt || streaming) return;

  const chat = ensureActiveChat();
  const userMsg = { role: "user", text: prompt };
  chat.messages.push(userMsg);
  if (chat.messages.length === 1) {
    chat.title = truncate(prompt, 48);
    els.topic.textContent = chat.title;
  }
  saveStore();
  renderChatList();

  if (els.empty) els.empty.style.display = "none";
  renderMessage(userMsg);

  els.prompt.value = "";
  autoresize();

  const assistantMsg = { role: "assistant", text: "" };
  chat.messages.push(assistantMsg);
  const assistantNode = renderMessage(assistantMsg);
  assistantNode.classList.add("streaming");

  streaming = true;
  els.send.disabled = true;
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
    });
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
  } finally {
    assistantNode.classList.remove("streaming");
    streaming = false;
    els.meta.innerHTML = '<span class="hint">enter to send</span>';
    saveStore();
    autoresize();
    els.prompt.focus();
  }
});

async function streamPrompt(prompt, onChunk) {
  const resp = await fetch("/v1/run", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ prompt, max_tokens: 512 }),
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
  pairEls.copyBtn.addEventListener("click", async () => {
    const text = pairEls.offerUri.textContent || "";
    try {
      await navigator.clipboard.writeText(text);
      pairEls.copyBtn.classList.add("copy-success");
      const orig = pairEls.copyBtn.textContent;
      pairEls.copyBtn.textContent = "✓ copied";
      setTimeout(() => {
        pairEls.copyBtn.classList.remove("copy-success");
        pairEls.copyBtn.textContent = orig;
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

// ---------------------------------------------------------------- boot

renderChatList();
renderActiveChat();
