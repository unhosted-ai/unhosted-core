# docs/branding/ — site logos

Logos and brand assets bundled with the GitHub Pages site. Self-contained on purpose: when this folder is uploaded as a Pages artifact it ships with everything `index.html` references. Mirrors the curated kit at [`/branding`](../../branding) at root, with the variants the live site actually uses.

## What's here

```
docs/branding/
├── logo.svg                 trust-gradient mark, currentColor
├── logo-light.svg           mark on light backgrounds (for embedding)
├── logo-dark.svg            mark on dark backgrounds
├── wordmark.svg             wordmark only
├── lockup.svg               horizontal: mark + wordmark
├── lockup-stacked.svg       stacked: mark above wordmark above tagline
├── stacked-mark.svg         secondary 2x2 outlined mark (Kernel-style)
├── favicon.svg              site favicon (used by index.html)
├── favicon-light.svg        favicon variant
├── favicon-dark.svg         favicon variant
├── apple-touch-icon.svg     180x180 for iOS home screen
├── og-image.svg             1200x630 OpenGraph / Twitter card
├── github-social.svg        1280x640 GitHub social preview
└── x-banner.svg             1500x500 X / Twitter header
```

## How the site uses these

`index.html` references `branding/favicon.svg` for the tab icon and `branding/og-image.svg` for `og:image`. The trust-gradient mark inside the bento cards is inlined as SVG (so it can pick up `currentColor` and react to theme).

## Updating

When you change a brand asset, update both this folder and the canonical [`/branding`](../../branding). They diverge by design (sizes, formats can differ), but the visual identity must stay consistent. See [`/BRAND.md`](../../BRAND.md) for the rules.
