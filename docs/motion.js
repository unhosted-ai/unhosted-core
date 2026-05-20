// unhosted — motion script
//
// Uses Motion (https://motion.dev), the vanilla-JS library by the
// same author as Framer Motion. Same API mental model; no React /
// build step required.
//
// Progressive enhancement: if this script fails to load or JS is
// disabled, every element stays visible — we never hide content via
// CSS that depends on JS to reveal it. See style.css `.motion-armed`
// rules. `prefers-reduced-motion: reduce` short-circuits to a single
// reveal pass with no animation.
//
// What's animated and why (top-of-file index so a reader can find
// the bit they care about without scrolling):
//
//   1. Wordmark         — per-character entrance: blur + Y-offset
//                         with a tight stagger. The wordmark is the
//                         brand; the page-load moment should carry
//                         that weight.
//   2. Hero lede        — title, badges, sub fade up in sequence.
//   3. Hero install     — the curl one-liner gets a brief blinking
//                         caret on first reveal. Visually signals
//                         "this is a CLI tool" before you read it.
//   4. Trust-radius     — three concentric rings draw themselves
//                         from the center outward via
//                         stroke-dashoffset; the filled center disc
//                         scales in last with its label.
//   5. Sticky nav       — once you scroll past the wordmark, a thin
//                         fixed nav slides down with the brand mark
//                         + same links + same CTA. Backdrop-blurred.
//   6. Scroll reveals   — cards / stories / sections rise into view
//                         on `inView`. Distance and direction vary
//                         per element type so the whole page doesn't
//                         move identically — bento cards rise from
//                         alternating sides; storylines slide in
//                         from the left; section h2s fade with no
//                         translate at all (anchor for the eye).
//   7. Quickstart rail  — fill bar sweeps 0 → 100%, dots light in
//                         sequence, step cards stagger up.

import {
  animate,
  inView,
  stagger,
} from "https://cdn.jsdelivr.net/npm/motion@12/+esm";

// Safety net: if the entrance throws or stalls for any reason
// (Motion CDN unreachable, parse error inside this module, a
// missing selector), the motion-armed CSS would leave the wordmark
// + lede invisible. Arm a deadline; if the entrance hasn't disarmed
// it by then, force-reveal everything.
const REVEAL_DEADLINE_MS = 1600;
const safetyTimer = setTimeout(() => {
  document.documentElement.classList.remove("motion-armed");
  document.querySelectorAll(".wordmark").forEach((el) => {
    el.style.visibility = "visible";
  });
}, REVEAL_DEADLINE_MS);

const prefersReduced = window.matchMedia(
  "(prefers-reduced-motion: reduce)",
).matches;

const root = document.documentElement;
root.classList.add("motion-armed");

// Soft ease-out used everywhere. Single constant so a tweak in one
// place re-paces the whole site at once.
const EASE = [0.22, 1, 0.36, 1];

try {
  if (prefersReduced) {
    splitWordmark();           // still split so layout matches, no anim
    revealAll();
    initStickyNav();
    initPageNav();
  } else {
    splitWordmark();
    runEntrance();
    runHeroInstallCaret();
    runScrollIn();
    runTrustRadius();
    runQuickstartRail();
    initStickyNav();
    initPageNav();
  }
  clearTimeout(safetyTimer);
} catch (e) {
  // Any throw in entrance/setup → fall back to "show everything".
  // The safety timer will fire shortly anyway; we just speed it up.
  clearTimeout(safetyTimer);
  root.classList.remove("motion-armed");
  document.querySelectorAll(".wordmark").forEach((el) => {
    el.style.visibility = "visible";
  });
  try { initStickyNav(); } catch (_) {}
  try { initPageNav(); } catch (_) {}
  throw e;
}

// ─── wordmark split ──────────────────────────────────────────────
// Replace the H1's plain text with one <span> per character so each
// can animate independently. The H1 keeps its aria-label for screen
// readers, so we mark the per-char spans aria-hidden and the visual
// effect is purely cosmetic.
function splitWordmark() {
  const h1 = document.querySelector(".wordmark");
  if (!h1 || h1.dataset.split === "1") return;
  const text = h1.textContent || "";
  h1.textContent = "";
  for (const ch of text) {
    const span = document.createElement("span");
    span.className = "wm-char";
    span.textContent = ch;
    span.setAttribute("aria-hidden", "true");
    h1.appendChild(span);
  }
  h1.dataset.split = "1";
}

function revealAll() {
  const all = document.querySelectorAll(
    ".motion-armed .wordmark, .motion-armed .wm-char, " +
      ".motion-armed .lede h2, .motion-armed .lede .sub, " +
      ".motion-armed .lede .badges, .motion-armed .hardware-list li, " +
      ".motion-armed .nav a, .motion-armed .bento .card, .motion-armed .story, " +
      ".motion-armed .next-list li, .motion-armed .steps > li, .motion-armed .mode, " +
      ".motion-armed .doc-head .kicker, .motion-armed .doc-head h1, .motion-armed .doc-head .lede",
  );
  all.forEach((el) => {
    el.style.opacity = "1";
    el.style.transform = "none";
    el.style.filter = "none";
  });
}

// ─── entrance (top-of-page, load-time) ───────────────────────────
function runEntrance() {
  // Per-character wordmark reveal. Each letter starts compressed
  // toward the center with a slight blur + extra vertical drop,
  // then expands to its natural letter-spacing as it settles.
  // The compound effect: the wordmark looks like it "unfolds"
  // outward from the center while each glyph rises into place.
  //
  // Why this is more theatrical than a plain fade:
  //   - letter-spacing animates from -0.18em (cramped) to the
  //     natural -0.05em, giving the wordmark a kinetic feeling
  //     of *opening* as it lands.
  //   - per-character y-offset is now 28px (was 14) for more
  //     drama on the brand moment.
  //   - blur is 10px (was 8), so each char emerges from softness.
  //   - stagger is 55ms (was 40) — letters land slow enough to
  //     read as choreographed, not flicker.
  //   - duration is 0.85s (was 0.6) — gives the eye time to
  //     register the unfold.
  //
  // After entrance, kickWordmarkLife() takes over with subtle
  // forever-loops (breathing, hover lift) so the wordmark isn't
  // static dead pixels on a scrolled page.
  animate(
    ".wordmark",
    { letterSpacing: ["-0.18em", "-0.05em"] },
    { duration: 0.95, delay: 0.05, ease: EASE },
  );
  safeAnimate(
    ".wm-char",
    {
      opacity: [0, 1],
      transform: [
        "translateY(28px) scale(0.92)",
        "translateY(0) scale(1)",
      ],
      filter: ["blur(10px)", "blur(0)"],
    },
    { duration: 0.85, delay: stagger(0.055, { start: 0.08 }), ease: EASE },
  );
  // Schedule the "alive" loops to start once the entrance settles.
  setTimeout(kickWordmarkLife, 1100);

  safeAnimate(
    ".lede h2",
    { opacity: [0, 1], transform: ["translateY(16px)", "translateY(0)"] },
    { duration: 0.55, delay: 0.32, ease: EASE },
  );

  safeAnimate(
    ".lede .badges",
    { opacity: [0, 1], transform: ["translateY(16px)", "translateY(0)"] },
    { duration: 0.55, delay: 0.48, ease: EASE },
  );

  safeAnimate(
    ".lede .sub",
    { opacity: [0, 1], transform: ["translateY(16px)", "translateY(0)"] },
    { duration: 0.55, delay: 0.56, ease: EASE },
  );

  safeAnimate(
    ".hardware-list li",
    { opacity: [0, 1], transform: ["translateX(8px)", "translateX(0)"] },
    { duration: 0.45, delay: stagger(0.05, { start: 0.7 }), ease: EASE },
  );

  safeAnimate(
    ".nav a",
    { opacity: [0, 1] },
    { duration: 0.4, delay: stagger(0.05, { start: 0.7 }), ease: EASE },
  );

  // /docs.html head (selector is conditional; safeAnimate no-ops if
  // the element doesn't exist on the current page).
  safeAnimate(
    ".doc-head .kicker, .doc-head h1, .doc-head .lede",
    { opacity: [0, 1], transform: ["translateY(10px)", "translateY(0)"] },
    { duration: 0.5, delay: stagger(0.1), ease: EASE },
  );
}

// ─── hero install caret ──────────────────────────────────────────
// A brief blinking caret next to the curl command on first reveal —
// just enough to signal "this is a terminal prompt" before you read
// the URL. The caret element is appended dynamically (not in HTML)
// because it's purely decorative and we don't want it in the
// no-JS fallback.
function runHeroInstallCaret() {
  const codeEl = document.querySelector(".hero-install-cmd code");
  if (!codeEl) return;
  const caret = document.createElement("span");
  caret.className = "hero-install-caret";
  caret.setAttribute("aria-hidden", "true");
  codeEl.appendChild(caret);
  // Animate it in, blink for ~3 s, then leave it static (avoid the
  // "this page never stops blinking at me" trap).
  animate(
    caret,
    { opacity: [0, 1, 1, 0, 1, 0, 1] },
    { duration: 2.8, delay: 1.0, times: [0, 0.1, 0.3, 0.45, 0.6, 0.75, 1] },
  );
}

// ─── scroll-in reveals ───────────────────────────────────────────
// Distinct motion profiles per element type. The point is that
// scrolling down feels textured, not like one global fade-up. We
// keep amplitudes small (≤ 24 px) — a tasteful scroll, not a
// parallax demo.
function runScrollIn() {
  // Bento cards alternate which side they enter from — odd cards
  // slide from the left, even from the right. Cheap to do, very
  // visible.
  document.querySelectorAll(".bento .card").forEach((el, i) => {
    const dx = i % 2 === 0 ? -10 : 10;
    inView(
      el,
      () => {
        animate(
          el,
          {
            opacity: [0, 1],
            transform: [
              `translate(${dx}px, 12px)`,
              "translate(0, 0)",
            ],
          },
          { duration: 0.55, ease: EASE },
        );
      },
      { amount: 0.15 },
    );
  });

  // Story cards (milestones) slide in from the left — they're a
  // timeline; the motion should read as "moving along".
  document.querySelectorAll(".story").forEach((el) => {
    inView(
      el,
      () => {
        animate(
          el,
          { opacity: [0, 1], transform: ["translateX(-16px)", "translateX(0)"] },
          { duration: 0.6, ease: EASE },
        );
      },
      { amount: 0.2 },
    );
  });

  // Step cards (.steps > li, .next-list li) — simple rise.
  document.querySelectorAll(".next-list li, .steps > li, .mode").forEach((el) => {
    inView(
      el,
      () => {
        animate(
          el,
          { opacity: [0, 1], transform: ["translateY(14px)", "translateY(0)"] },
          { duration: 0.5, ease: EASE },
        );
      },
      { amount: 0.15 },
    );
  });

  // Section h2s: pure fade, no translate. The h2 is the anchor for
  // the eye when you scroll into a new section, so we don't move it.
  document.querySelectorAll(".doc section h2").forEach((el) => {
    inView(
      el,
      () => {
        animate(el, { opacity: [0, 1] }, { duration: 0.6, ease: EASE });
      },
      { amount: 0.3 },
    );
  });
}

// ─── wordmark "alive" loops ──────────────────────────────────────
// After the entrance lands, the wordmark would otherwise sit as
// static black pixels at the top of the page — visually dead. Two
// ambient effects keep it feeling alive without being distracting:
//
//   1. Breathing scale loop. 1.0 → 1.012 → 1.0 over 5.4 s,
//      `repeat: Infinity`, `repeatType: "mirror"`. The amplitude
//      is small enough that it reads as "this is a live brand"
//      rather than "this is animated". Pauses on hover so a per-
//      character lift can take over.
//
//   2. Per-character hover lift. Mousing over a single glyph
//      raises it 6px and scales it slightly. Mousing off restores.
//      Works even after the entrance is done because the spans
//      stay in the DOM (split is synchronous in splitWordmark).
//
// Both loops respect prefers-reduced-motion via the early bail in
// the main entrypoint — if the user has reduced motion on, we
// call revealAll() and never invoke this function.
function kickWordmarkLife() {
  const h1 = document.querySelector(".wordmark");
  if (!h1) return;
  // The constant breathing loop. Stored on the element so a hover
  // can pause/resume it cleanly.
  let breathing;
  const startBreathing = () => {
    if (breathing) return;
    breathing = animate(
      h1,
      { transform: ["scale(1)", "scale(1.012)", "scale(1)"] },
      {
        duration: 5.4,
        ease: [0.45, 0, 0.55, 1],
        repeat: Infinity,
      },
    );
  };
  const stopBreathing = () => {
    if (breathing && typeof breathing.stop === "function") {
      breathing.stop();
    }
    breathing = null;
  };
  startBreathing();

  // Per-character hover lift. We use mouseenter/mouseleave on the
  // H1 with event-target inspection rather than per-char listeners
  // — fewer handlers, same UX.
  const chars = Array.from(h1.querySelectorAll(".wm-char"));
  for (const ch of chars) {
    ch.addEventListener("mouseenter", () => {
      stopBreathing();
      animate(
        ch,
        { transform: ["translateY(0) scale(1)", "translateY(-8px) scale(1.05)"] },
        { duration: 0.25, ease: [0.34, 1.56, 0.64, 1] },
      );
    });
    ch.addEventListener("mouseleave", () => {
      animate(
        ch,
        { transform: ["translateY(-8px) scale(1.05)", "translateY(0) scale(1)"] },
        { duration: 0.35, ease: EASE },
      );
    });
  }
  // When the whole wordmark loses pointer focus (cursor leaves the
  // H1), resume breathing.
  h1.addEventListener("mouseleave", () => {
    setTimeout(startBreathing, 400);
  });
}

// ─── trust-radius rings ──────────────────────────────────────────
// Three concentric rings centered on (100, 100):
//   • outer "public":  r = 92, dashed stroke
//   • middle "trusted": r = 62, solid stroke
//   • inner "local":   r = 34, filled disc
// Drawn inside → out so the user reads "this is what you start with,
// this is what you extend into, this is what's outside". After the
// draw-in entrance, the diagram stays "alive" with:
//   - a slow infinite rotation on the dashed public ring (60s/rev),
//     signalling that "public" is the outermost orbit
//   - a gentle breathing pulse on the local disc (3.5s mirror loop)
//   - legend-row hover sync: hover a legend item, the matching ring
//     thickens + un-mutes; the others dim. Same in reverse — hover
//     a ring, the legend row highlights.
function runTrustRadius() {
  const svg = document.querySelector(".radius-diagram svg");
  if (!svg) return;
  const trustedCircle = svg.querySelector(".ring-trusted circle");
  const publicCircle = svg.querySelector(".ring-public circle");
  const localCircle = svg.querySelector(".ring-local circle");
  const localLabel = svg.querySelector(".ring-label.inside");
  if (!trustedCircle || !publicCircle || !localCircle) return;

  // Set up dashoffset = circumference so the stroke starts "empty"
  // and animates down to 0 ("fully drawn"). The dashed public ring
  // uses its existing stroke-dasharray (3 6) for the visible look;
  // we override only during the draw-in transition.
  // IMPORTANT: do NOT set the "empty" stroke state synchronously up
  // front. If we did and `inView` never fires (section already in
  // viewport on load, IntersectionObserver flaky in some browsers,
  // or the safety timer disarms motion-armed before this runs), the
  // rings would be stuck invisible. Set the empty state inside the
  // inView callback instead, right before the animation that resolves
  // it. The rings render as static SVG until the moment of animation.
  inView(
    svg,
    () => {
      // local disc — set initial scale, then pop in.
      localCircle.style.transformBox = "fill-box";
      localCircle.style.transformOrigin = "center";
      localCircle.style.transform = "scale(0)";
      if (localLabel) localLabel.style.opacity = "0";
      animate(
        localCircle,
        { transform: ["scale(0)", "scale(1)"] },
        { duration: 0.5, ease: EASE },
      );
      // trusted ring — set empty stroke, draw to full.
      const trustedR = parseFloat(trustedCircle.getAttribute("r")) || 0;
      const trustedC = 2 * Math.PI * trustedR;
      trustedCircle.style.strokeDasharray = String(trustedC);
      trustedCircle.style.strokeDashoffset = String(trustedC);
      animate(
        trustedCircle,
        { strokeDashoffset: [trustedC, 0] },
        { duration: 0.9, delay: 0.35, ease: EASE },
      );
      // public ring — same, then restore the dashed pattern.
      const publicR = parseFloat(publicCircle.getAttribute("r")) || 0;
      const publicC = 2 * Math.PI * publicR;
      publicCircle.style.strokeDasharray = String(publicC);
      publicCircle.style.strokeDashoffset = String(publicC);
      animate(
        publicCircle,
        { strokeDashoffset: [publicC, 0] },
        { duration: 1.1, delay: 0.7, ease: EASE },
      );
      // Restore everything to its static state after animations
      // complete — belt-and-braces. If any animate() promise rejects
      // silently, this still ensures the diagram ends up in its
      // intended visible state. Then start the "alive" ambient loops.
      setTimeout(() => {
        trustedCircle.style.strokeDasharray = "";
        trustedCircle.style.strokeDashoffset = "";
        publicCircle.style.strokeDasharray = "3 6";
        publicCircle.style.strokeDashoffset = "0";

        // Slow orbital rotation of the public (outer dashed) ring.
        // 60s/revolution — slow enough to be unconscious, fast
        // enough to be perceptible if you look. The transform-
        // origin is the SVG center (100, 100).
        publicCircle.style.transformBox = "fill-box";
        publicCircle.style.transformOrigin = "center";
        animate(
          publicCircle,
          { transform: ["rotate(0deg)", "rotate(360deg)"] },
          { duration: 60, ease: "linear", repeat: Infinity },
        );

        // Gentle breathing on the local (inner filled) disc. Scale
        // 1.0 → 1.05 → 1.0 over 3.5 s. Mirror loop so the eye
        // catches both directions; signals "this is your home".
        animate(
          localCircle,
          { transform: ["scale(1)", "scale(1.05)", "scale(1)"] },
          {
            duration: 3.5,
            ease: [0.45, 0, 0.55, 1],
            repeat: Infinity,
          },
        );

        // Legend ↔ ring hover sync. Hover a legend item, the
        // matching ring un-mutes (full opacity + stronger stroke);
        // the others dim. Same in reverse — hover a ring, the
        // legend row highlights. Implemented via CSS classes so
        // the styling lives in style.css.
        wireRadiusHoverSync(svg);
      }, 1900);
      // label fades up after the disc lands
      if (localLabel) {
        animate(
          localLabel,
          { opacity: [0, 1] },
          { duration: 0.4, delay: 0.6, ease: EASE },
        );
      }
    },
    { amount: 0.4 },
  );
}

/// Bidirectional hover sync between the radius diagram rings and the
/// legend rows. Hovering either highlights the same trust layer,
/// dims the other two, so the user can map symbol → meaning at a
/// glance. The actual styling is in style.css under
/// `.radius-diagram.is-focused-*` and `.radius-legend.is-focused-*`.
function wireRadiusHoverSync(svg) {
  const wrap = svg.closest(".radius-diagram");
  if (!wrap) return;
  const legend = wrap.querySelector(".radius-legend");
  if (!legend) return;

  const layers = ["local", "trusted", "public"];

  function focus(layer) {
    for (const l of layers) {
      wrap.classList.toggle(`is-focused-${l}`, l === layer);
    }
  }
  function clearFocus() {
    for (const l of layers) {
      wrap.classList.remove(`is-focused-${l}`);
    }
  }

  // Diagram side — each <a class="ring ring-X"> wraps its circle.
  for (const layer of layers) {
    const ring = svg.querySelector(`.ring-${layer}`);
    if (ring) {
      ring.addEventListener("mouseenter", () => focus(layer));
      ring.addEventListener("mouseleave", clearFocus);
      ring.addEventListener("focusin", () => focus(layer));
      ring.addEventListener("focusout", clearFocus);
    }
  }
  // Legend side — match by the legend-key class.
  const legendItems = legend.querySelectorAll("li");
  legendItems.forEach((li) => {
    const key = li.querySelector(".legend-key");
    if (!key) return;
    const layer = layers.find((l) => key.classList.contains(`key-${l}`));
    if (!layer) return;
    li.addEventListener("mouseenter", () => focus(layer));
    li.addEventListener("mouseleave", clearFocus);
  });
}

// ─── page-nav (left-side section TOC) ────────────────────────────
// One IntersectionObserver across every section anchored by the
// nav. Whichever section has the most viewport coverage wins the
// `.is-active` class on its matching nav link. We don't pick by
// "first intersecting" because sections of very different heights
// would let a tiny section near the top steal active from a tall
// one the user is actually reading.
function initPageNav() {
  const nav = document.querySelector(".page-nav");
  if (!nav) return;
  const links = Array.from(nav.querySelectorAll("a[data-section]"));
  if (links.length === 0) return;

  // Map each section id → its link, and collect the sections that
  // actually exist on this page (the nav lists `#top` which is the
  // hero element, not a section — handle that as a separate case).
  const linkByKey = new Map();
  for (const a of links) linkByKey.set(a.dataset.section, a);

  // Resolve target elements. `top` → the hero header element; the
  // others → the <section id=…>.
  const targets = [];
  for (const a of links) {
    const key = a.dataset.section;
    const el =
      key === "top"
        ? document.querySelector("header.hero")
        : document.getElementById(key);
    if (el) targets.push({ key, el });
  }
  if (targets.length === 0) return;

  // Click handler — smooth-scroll the anchor into view. Default
  // href="#anchor" works without JS too; we just gentler the
  // animation when JS is on.
  for (const a of links) {
    a.addEventListener("click", (e) => {
      const key = a.dataset.section;
      const el =
        key === "top"
          ? document.querySelector("header.hero")
          : document.getElementById(key);
      if (!el) return;
      e.preventDefault();
      el.scrollIntoView({ behavior: "smooth", block: "start" });
      // Update active state immediately for snappy feedback;
      // observer will reconfirm shortly.
      setActive(key);
      // Update the URL hash without re-triggering a jump.
      if (history.replaceState) {
        history.replaceState(null, "", `#${key}`);
      }
    });
  }

  function setActive(key) {
    for (const a of links) {
      a.classList.toggle("is-active", a.dataset.section === key);
    }
  }

  // Track in-view ratio for each section. Whichever has the highest
  // ratio wins. We use rootMargin to bias slightly toward "section
  // whose top is near the top of the viewport" — that matches the
  // user's intuition of "I'm reading this section now".
  const ratios = new Map();
  for (const { key } of targets) ratios.set(key, 0);

  const obs = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        const key =
          e.target.id ||
          (e.target.tagName === "HEADER" ? "top" : null);
        if (!key) continue;
        ratios.set(key, e.intersectionRatio);
      }
      // Find the section with the highest ratio. Tie-break: the
      // first one in DOM order (matches reading direction).
      let best = null;
      let bestRatio = 0;
      for (const { key } of targets) {
        const r = ratios.get(key) || 0;
        if (r > bestRatio) {
          best = key;
          bestRatio = r;
        }
      }
      if (best) setActive(best);
    },
    {
      // Multiple thresholds so the observer fires often enough for
      // the active state to track scroll smoothly.
      threshold: [0, 0.1, 0.25, 0.5, 0.75, 1],
      rootMargin: "-80px 0px -40% 0px",
    },
  );
  for (const { el } of targets) obs.observe(el);

  // Seed initial state — pick whichever target is most visible on
  // first paint (usually the hero/top).
  setActive(targets[0].key);
}

// ─── sticky nav ──────────────────────────────────────────────────
// Once the wordmark has scrolled out of view, a thin fixed-position
// nav slides down. Backdrop-blurred so it reads as "above the
// content" without being opaque. The trigger is the bottom of the
// .wordmark-band — when that's above the viewport top, the sticky
// nav is on. We use IntersectionObserver on a sentinel element so
// there's no scroll-listener overhead.
function initStickyNav() {
  const wordmark = document.querySelector(".wordmark-band");
  const sticky = document.querySelector(".sticky-nav");
  if (!wordmark || !sticky) return;

  // Sentinel: a 1px-tall div placed immediately *after* the
  // wordmark band. When it leaves the top of the viewport, we know
  // the wordmark is offscreen.
  const sentinel = document.createElement("div");
  sentinel.style.position = "absolute";
  sentinel.style.left = "0";
  sentinel.style.right = "0";
  sentinel.style.height = "1px";
  sentinel.style.pointerEvents = "none";
  wordmark.after(sentinel);

  const obs = new IntersectionObserver(
    (entries) => {
      const e = entries[0];
      // Sentinel visible = wordmark band still on screen ⇒ sticky off.
      // Sentinel offscreen above ⇒ sticky on.
      const past = !e.isIntersecting && e.boundingClientRect.top < 0;
      sticky.classList.toggle("is-visible", past);
    },
    { threshold: 0, rootMargin: "-1px 0px 0px 0px" },
  );
  obs.observe(sentinel);
}

// ─── quickstart rail ─────────────────────────────────────────────
// Calm fill: a single soft sweep from 0 → 100% over ~1.1 s; dots
// fade in as the fill passes them; step cards stagger up. Plus the
// existing hover/click wiring that drives the horizontal carousel.
function runQuickstartRail() {
  const rail = document.querySelector(".qs-rail");
  const fill = document.querySelector("[data-qs-fill]");
  const dots = Array.from(document.querySelectorAll(".qs-rail-dot"));
  const steps = Array.from(document.querySelectorAll(".qs-step"));
  if (!rail || !fill || dots.length === 0) {
    // The rail markup was removed in the 2-step quickstart redesign;
    // step cards still need their entrance.
    if (steps.length) {
      steps.forEach((step, i) => {
        inView(
          step,
          () => {
            animate(
              step,
              { opacity: [0, 1], transform: ["translateY(12px)", "translateY(0)"] },
              { duration: 0.55, delay: i * 0.08, ease: EASE },
            );
          },
          { amount: 0.2 },
        );
      });
    }
    return;
  }

  const railSteps = document.querySelector(".qs-steps");
  steps.forEach((step, i) => {
    const dot = dots[i];
    if (!dot) return;
    const focus = () =>
      dots.forEach((d, j) => d.classList.toggle("is-active", j === i));
    const blur = () => dots.forEach((d) => d.classList.remove("is-active"));
    step.addEventListener("mouseenter", focus);
    step.addEventListener("mouseleave", blur);
    step.addEventListener("focusin", focus);
    step.addEventListener("focusout", blur);
    dot.addEventListener("click", (e) => {
      e.preventDefault();
      step.scrollIntoView({ block: "nearest", inline: "start", behavior: "smooth" });
      focus();
    });
  });

  if (railSteps && "IntersectionObserver" in window) {
    const obs = new IntersectionObserver(
      (entries) => {
        let bestIdx = -1;
        let bestRatio = 0;
        entries.forEach((entry) => {
          if (entry.isIntersecting && entry.intersectionRatio > bestRatio) {
            const idx = steps.indexOf(entry.target);
            if (idx >= 0) {
              bestIdx = idx;
              bestRatio = entry.intersectionRatio;
            }
          }
        });
        if (bestIdx >= 0) {
          dots.forEach((d, j) => d.classList.toggle("is-active", j === bestIdx));
        }
      },
      { root: railSteps, threshold: [0.4, 0.6, 0.8] },
    );
    steps.forEach((step) => obs.observe(step));
  }

  inView(
    rail,
    () => {
      animate(fill, { width: ["0%", "100%"] }, { duration: 1.1, ease: EASE });
      const stops = [0, 0.5, 1];
      dots.forEach((dot, i) => {
        animate(
          dot,
          { opacity: [0.35, 1] },
          { duration: 0.35, delay: stops[i] * 1.05, ease: EASE },
        );
      });
      animate(
        steps,
        { opacity: [0, 1], transform: ["translateY(12px)", "translateY(0)"] },
        { duration: 0.55, delay: stagger(0.16, { start: 0.2 }), ease: EASE },
      );
    },
    { amount: 0.3 },
  );
}

function safeAnimate(selector, keyframes, options) {
  const els = document.querySelectorAll(selector);
  if (els.length === 0) return;
  animate(els, keyframes, options);
}
