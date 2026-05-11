// unhosted — local web UI
// Talks to the daemon HTTP API on the same origin:
//   GET  /health     liveness
//   GET  /v1/status  connection details
//   POST /v1/run     streaming text/plain inference

const $ = (sel) => document.querySelector(sel);

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

function renderMessage(msg) {
  const node = document.createElement("article");
  node.className = `msg ${msg.role}`;

  const who = document.createElement("div");
  who.className = "who";
  who.innerHTML = `<span class="dot"></span><span>${msg.role === "user" ? "you" : "unhosted"}</span>`;

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
    setStatusDot("warn", "upstream offline — start `llama-server`");
    els.connModel.textContent = "no model loaded";
    els.connUpstream.textContent = s.upstream.url.replace(/^https?:\/\//, "");
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
      const name = document.createElement("span");
      name.className = "pname";
      name.textContent = peer.name;
      const addr = document.createElement("span");
      addr.className = "paddr";
      addr.textContent = peer.addr;
      left.append(name, addr);

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
    assistantMsg.text += `\n[error: ${err && err.message ? err.message : "request failed"}]`;
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

  if (!resp.ok) throw new Error(`node returned ${resp.status} ${resp.statusText}`);
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

// ---------------------------------------------------------------- helpers

function showError(node, err) {
  const bodyEl = node.querySelector(".body");
  const banner = document.createElement("div");
  banner.className = "error-banner";
  banner.innerHTML =
    "<strong>error:</strong> " +
    (err && err.message ? escapeHtml(err.message) : "request failed") +
    ". is <code>llama-server</code> running and reachable from the daemon?";
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

// ---------------------------------------------------------------- boot

renderChatList();
renderActiveChat();
