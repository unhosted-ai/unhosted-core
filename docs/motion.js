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
