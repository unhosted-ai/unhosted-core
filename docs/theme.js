// Theme toggle — cycles auto → dark → light on every click.
// Persists choice in localStorage; an inline bootstrap script in <head>
// applies the stored theme before paint so there's no flash on load.

const KEY = "unhosted-theme";
const GLYPHS = { auto: "◐", dark: "☾", light: "☀" };
const LABELS = { auto: "theme · auto", dark: "theme · dark", light: "theme · light" };

const btn = document.getElementById("theme-toggle");
if (btn) {
  paint();
  btn.addEventListener("click", () => {
    const current = currentTheme();
    const next = current === "auto" ? "dark" : current === "dark" ? "light" : "auto";
    setTheme(next);
    paint();
  });
}

function currentTheme() {
  const stored = safeRead();
  if (stored === "dark" || stored === "light") return stored;
  return "auto";
}

function setTheme(theme) {
  if (theme === "auto") {
    delete document.documentElement.dataset.theme;
    safeWrite(null);
  } else {
    document.documentElement.dataset.theme = theme;
    safeWrite(theme);
  }
}

function paint() {
  if (!btn) return;
  const t = currentTheme();
  const glyph = btn.querySelector(".glyph");
  if (glyph) glyph.textContent = GLYPHS[t];
  btn.title = LABELS[t];
  btn.setAttribute("aria-label", LABELS[t]);
}

function safeRead() {
  try {
    return localStorage.getItem(KEY);
  } catch (e) {
    return null;
  }
}

function safeWrite(value) {
  try {
    if (value === null) localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, value);
  } catch (e) {}
}
