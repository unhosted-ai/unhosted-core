// unhosted — local web UI
// Talks to the daemon HTTP API on the same origin:
//   GET  /health     liveness
//   GET  /v1/status  connection details: node + upstream + model + peers
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
      safeRemove();
    } else {
      document.documentElement.dataset.theme = next;
      safeSet(next);
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
function safeSet(v) { try { localStorage.setItem(THEME_KEY, v); } catch (e) {} }
function safeRemove() { try { localStorage.removeItem(THEME_KEY); } catch (e) {} }

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
};

let streaming = false;
let firstUserPrompt = null;

// ---------------------------------------------------------------- status

async function refreshStatus() {
  try {
    const r = await fetch("/v1/status", { cache: "no-store" });
    if (!r.ok) throw new Error(`${r.status}`);
    const s = await r.json();
    renderStatus(s);
  } catch (e) {
    setStatusDot("err", "node unreachable");
    els.connModel.textContent = "—";
    els.connUpstream.textContent = "—";
    els.connNode.textContent = "—";
  }
}

function renderStatus(s) {
  // upstream first
  if (s.upstream.reachable) {
    setStatusDot("ok", `node ready · v${s.node.version}`);
    els.connModel.textContent = s.upstream.model || "(model not reported)";
    els.connUpstream.textContent = s.upstream.url.replace(/^https?:\/\//, "");
  } else {
    setStatusDot(
      "warn",
      "upstream offline — start `llama-server` to enable inference",
    );
    els.connModel.textContent = "no model loaded";
    els.connUpstream.textContent = s.upstream.url.replace(/^https?:\/\//, "");
  }

  els.connNode.textContent = s.node.addr;

  // peers
  if (s.peers && s.peers.length > 0) {
    els.peersBlock.hidden = false;
    els.peerList.innerHTML = "";
    for (const peer of s.peers) {
      const li = document.createElement("li");
      const name = document.createElement("span");
      name.className = "pname";
      name.textContent = peer.name;
      const addr = document.createElement("span");
      addr.className = "paddr";
      addr.textContent = peer.addr;
      li.append(name, addr);
      els.peerList.append(li);
    }
  } else {
    els.peersBlock.hidden = true;
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

els.newChat.addEventListener("click", () => {
  els.conversation.innerHTML = "";
  if (els.empty) els.empty.style.display = "";
  firstUserPrompt = null;
  els.topic.textContent = "new chat";
  els.prompt.focus();
});

autoresize();

// ---------------------------------------------------------------- submit + stream

els.composer.addEventListener("submit", async (e) => {
  e.preventDefault();
  const prompt = els.prompt.value.trim();
  if (!prompt || streaming) return;

  if (els.empty) els.empty.style.display = "none";
  if (!firstUserPrompt) {
    firstUserPrompt = prompt;
    els.topic.textContent = truncate(prompt, 48);
  }

  appendMessage("user", prompt);
  els.prompt.value = "";
  autoresize();

  const assistant = appendMessage("assistant", "");
  assistant.classList.add("streaming");

  streaming = true;
  els.send.disabled = true;
  els.meta.innerHTML = '<span class="info">streaming…</span>';

  const startedAt = performance.now();
  let bytes = 0;

  try {
    const servedBy = await streamPrompt(prompt, (chunk) => {
      const bodyEl = assistant.querySelector(".body");
      bodyEl.textContent += chunk;
      bytes += chunk.length;
      els.scroll.scrollTop = els.scroll.scrollHeight;
    });
    const elapsedMs = performance.now() - startedAt;
    annotateStats(assistant, servedBy, bytes, elapsedMs);
  } catch (err) {
    showError(assistant, err);
  } finally {
    assistant.classList.remove("streaming");
    streaming = false;
    els.meta.innerHTML = '<span class="hint">enter to send</span>';
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

// ---------------------------------------------------------------- DOM helpers

function appendMessage(role, text) {
  const msg = document.createElement("article");
  msg.className = `msg ${role}`;

  const who = document.createElement("div");
  who.className = "who";
  who.innerHTML = `<span class="dot"></span><span>${role === "user" ? "you" : "unhosted"}</span>`;

  const body = document.createElement("div");
  body.className = "body";
  body.textContent = text;

  msg.append(who, body);
  els.conversation.append(msg);
  els.scroll.scrollTop = els.scroll.scrollHeight;
  return msg;
}

function annotateStats(msgEl, servedBy, bytes, elapsedMs) {
  // Rough estimate: 1 token ≈ 4 bytes for English text in most tokenizers.
  // Good enough for a live indicator; the actual count would require parsing
  // llama-server's response headers.
  const approxTokens = Math.max(1, Math.round(bytes / 4));
  const seconds = elapsedMs / 1000;
  const tokPerSec = seconds > 0 ? (approxTokens / seconds).toFixed(1) : "—";

  const stats = document.createElement("div");
  stats.className = "stats";

  let servedHtml;
  if (servedBy && servedBy.startsWith("peer:")) {
    const name = servedBy.slice("peer:".length);
    servedHtml = `<span class="served-peer">served by peer · ${escapeHtml(name)}</span>`;
  } else if (servedBy) {
    servedHtml = `served by ${servedBy}`;
  } else {
    servedHtml = `served by local`;
  }

  stats.innerHTML = `
    <span>${servedHtml}</span>
    <span>~${approxTokens} tok</span>
    <span>${seconds.toFixed(1)} s</span>
    <span>~${tokPerSec} tok/s</span>
  `;
  msgEl.append(stats);
}

function showError(assistantEl, err) {
  const bodyEl = assistantEl.querySelector(".body");
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
