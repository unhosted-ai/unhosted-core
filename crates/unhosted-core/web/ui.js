// unhosted — local web UI
// Talks to the same daemon HTTP API the CLI uses:
//   GET  /health      → liveness check
//   POST /v1/run      → streaming text/plain inference

const $ = (sel) => document.querySelector(sel);

const els = {
  composer: $("#composer"),
  prompt: $("#prompt"),
  send: $("#send"),
  conversation: $("#conversation"),
  empty: $("#empty-state"),
  meta: $("#composer-meta"),
  statusDot: $(".status .dot"),
  statusLabel: $("#status-label"),
  main: document.querySelector(".app-main"),
};

let streaming = false;

// ---------------------------------------------------------------- status

async function pollStatus() {
  try {
    const r = await fetch("/health", { cache: "no-store" });
    if (r.ok) {
      setStatus("ok", "node ready");
    } else {
      setStatus("warn", `node returned ${r.status}`);
    }
  } catch (e) {
    setStatus("err", "node unreachable");
  }
}

function setStatus(state, label) {
  els.statusDot.dataset.state = state;
  els.statusLabel.textContent = label;
}

pollStatus();
setInterval(pollStatus, 15000);

// ---------------------------------------------------------------- composer behavior

function autoresize() {
  els.prompt.style.height = "auto";
  els.prompt.style.height = Math.min(els.prompt.scrollHeight, 200) + "px";
  els.send.disabled = streaming || els.prompt.value.trim().length === 0;
}

els.prompt.addEventListener("input", autoresize);
els.prompt.addEventListener("keydown", (e) => {
  // Enter sends. Shift+Enter inserts newline.
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

autoresize();

// ---------------------------------------------------------------- submit + stream

els.composer.addEventListener("submit", async (e) => {
  e.preventDefault();
  const prompt = els.prompt.value.trim();
  if (!prompt || streaming) return;

  els.empty?.remove();

  appendMessage("user", prompt);
  els.prompt.value = "";
  autoresize();

  const assistant = appendMessage("assistant", "");
  assistant.classList.add("streaming");

  streaming = true;
  els.send.disabled = true;
  els.meta.innerHTML = '<span class="info">streaming…</span>';

  try {
    const servedBy = await streamPrompt(prompt, (chunk) => {
      const bodyEl = assistant.querySelector(".body");
      bodyEl.textContent += chunk;
      // keep scrolled near the bottom while tokens arrive
      els.main.scrollTop = els.main.scrollHeight;
    });
    if (servedBy) annotateServedBy(assistant, servedBy);
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

  if (!resp.ok) {
    throw new Error(`node returned ${resp.status} ${resp.statusText}`);
  }
  if (!resp.body) {
    throw new Error("streaming not supported by this browser");
  }

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

function annotateServedBy(msgEl, servedBy) {
  const tag = document.createElement("div");
  tag.className = "served-by";
  if (servedBy.startsWith("peer:")) {
    const name = servedBy.slice("peer:".length);
    tag.innerHTML = `served by <span class="peer">peer · ${escapeHtml(name)}</span>`;
  } else {
    tag.textContent = `served by ${servedBy}`;
  }
  msgEl.append(tag);
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
  els.main.scrollTop = els.main.scrollHeight;
  return msg;
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
