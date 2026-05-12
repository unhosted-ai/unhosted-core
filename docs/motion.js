// unhosted — motion script
// Uses Motion (https://motion.dev), the vanilla-JS library by the same author
// as Framer Motion. Same API mental model; no React/build step required.
//
// Progressive enhancement: if this script fails to load or JS is disabled,
// every element stays visible — we never hide content via CSS that depends
// on JS to reveal it. See style.css `.motion-armed` rules.
//
// Respects prefers-reduced-motion: we skip all motion and just reveal.

import {
  animate,
  inView,
  stagger,
} from "https://cdn.jsdelivr.net/npm/motion@12/+esm";

const prefersReduced = window.matchMedia(
  "(prefers-reduced-motion: reduce)",
).matches;

const root = document.documentElement;
root.classList.add("motion-armed");

const EASE = [0.22, 1, 0.36, 1]; // a soft "ease-out" curve

if (prefersReduced) {
  // Just reveal everything immediately, no animation.
  revealAll();
} else {
  runEntrance();
  runScrollIn();
  runQuickstartRail();
}

function revealAll() {
  const all = document.querySelectorAll(
    ".motion-armed .wordmark, .motion-armed .lede h2, .motion-armed .lede .sub, " +
      ".motion-armed .lede .badges, .motion-armed .hardware-list li, " +
      ".motion-armed .nav a, .motion-armed .bento .card, .motion-armed .story, " +
      ".motion-armed .next-list li, .motion-armed .steps > li, .motion-armed .mode, " +
      ".motion-armed .doc-head .kicker, .motion-armed .doc-head h1, .motion-armed .doc-head .lede",
  );
  all.forEach((el) => {
    el.style.opacity = "1";
    el.style.transform = "none";
  });
}

function runEntrance() {
  // Top-of-page elements that animate immediately on load.
  // Each call is guarded so missing selectors on /docs.html don't error.
  safeAnimate(
    ".wordmark",
    { opacity: [0, 1], transform: ["translateY(-12px)", "translateY(0)"] },
    { duration: 0.7, ease: EASE },
  );

  safeAnimate(
    ".lede h2",
    { opacity: [0, 1], transform: ["translateY(16px)", "translateY(0)"] },
    { duration: 0.55, delay: 0.12, ease: EASE },
  );

  safeAnimate(
    ".lede .badges",
    { opacity: [0, 1], transform: ["translateY(16px)", "translateY(0)"] },
    { duration: 0.55, delay: 0.28, ease: EASE },
  );

  safeAnimate(
    ".lede .sub",
    { opacity: [0, 1], transform: ["translateY(16px)", "translateY(0)"] },
    { duration: 0.55, delay: 0.34, ease: EASE },
  );

  safeAnimate(
    ".hardware-list li",
    { opacity: [0, 1], transform: ["translateX(8px)", "translateX(0)"] },
    { duration: 0.45, delay: stagger(0.05, { start: 0.45 }), ease: EASE },
  );

  safeAnimate(
    ".nav a",
    { opacity: [0, 1] },
    { duration: 0.4, delay: stagger(0.05, { start: 0.55 }), ease: EASE },
  );

  // /docs.html head
  safeAnimate(
    ".doc-head .kicker, .doc-head h1, .doc-head .lede",
    { opacity: [0, 1], transform: ["translateY(10px)", "translateY(0)"] },
    { duration: 0.5, delay: stagger(0.1), ease: EASE },
  );
}

function runScrollIn() {
  // Elements that animate as they enter the viewport.
  const selectors = [
    ".bento .card",
    ".story",
    ".next-list li",
    ".steps > li",
    ".mode",
    ".doc section h2",
  ];

  selectors.forEach((sel) => {
    document.querySelectorAll(sel).forEach((el) => {
      inView(
        el,
        () => {
          animate(
            el,
            {
              opacity: [0, 1],
              transform: ["translateY(14px)", "translateY(0)"],
            },
            { duration: 0.5, ease: EASE },
          );
        },
        { amount: 0.15 },
      );
    });
  });
}

function safeAnimate(selector, keyframes, options) {
  const els = document.querySelectorAll(selector);
  if (els.length === 0) return;
  animate(els, keyframes, options);
}

/* Quickstart progression — the "sliding 1-2-3" feel.
   When .quickstart scrolls into view:
     1. the rail fill grows from 0% to 100% width over 900ms
     2. the three numbered dots pulse in sequence as the fill passes them
     3. the three step cards stagger-fade in from below.
   Each .qs-step also gets a hover handler that emphasizes the matching
   dot, so the rail stays the index of focus while you read. */
function runQuickstartRail() {
  const rail = document.querySelector(".qs-rail");
  const fill = document.querySelector("[data-qs-fill]");
  const dots = Array.from(document.querySelectorAll(".qs-rail-dot"));
  const steps = Array.from(document.querySelectorAll(".qs-step"));
  if (!rail || !fill || dots.length === 0) return;

  // Hover wiring stays available regardless of motion-reduced.
  steps.forEach((step, i) => {
    const dot = dots[i];
    if (!dot) return;
    const focus = () => dots.forEach((d, j) => d.classList.toggle("is-active", j === i));
    const blur = () => dots.forEach((d) => d.classList.remove("is-active"));
    step.addEventListener("mouseenter", focus);
    step.addEventListener("mouseleave", blur);
    step.addEventListener("focusin", focus);
    step.addEventListener("focusout", blur);
    dot.addEventListener("mouseenter", () =>
      step.scrollIntoView({ block: "nearest", behavior: "smooth" }),
    );
  });

  // Entrance animation: fill the rail, light the dots in sequence,
  // stagger the cards. Triggered on first viewport entry.
  inView(
    rail,
    () => {
      // calm fill: a single soft sweep from 0 → 100% over ~1.1 s.
      // No elastic curve, no bounce — restrained.
      animate(
        fill,
        { width: ["0%", "100%"] },
        { duration: 1.1, ease: [0.22, 1, 0.36, 1] },
      );

      // dots simply fade in as the fill reaches them. No scale-burst.
      const stops = [0, 0.5, 1];
      dots.forEach((dot, i) => {
        animate(
          dot,
          { opacity: [0.35, 1] },
          { duration: 0.35, delay: stops[i] * 1.05, ease: [0.22, 1, 0.36, 1] },
        );
      });

      // cards stagger in from below; slightly longer so the eye can
      // follow each step without feeling rushed.
      animate(
        steps,
        { opacity: [0, 1], transform: ["translateY(12px)", "translateY(0)"] },
        { duration: 0.55, delay: stagger(0.16, { start: 0.2 }), ease: [0.22, 1, 0.36, 1] },
      );
    },
    { amount: 0.3 },
  );
}
