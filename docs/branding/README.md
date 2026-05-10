# docs/branding/ — site logos

Logos and brand assets bundled with the GitHub Pages site. Self-contained on purpose: when this folder is uploaded as a Pages artifact it ships with everything `index.html` references. Mirrors the curated kit at [`/branding`](../../branding) at root, with the variants the live site actually uses.

## What's here

Source SVGs (the canonical versions — edit these):

```
docs/branding/
├── logo.svg                 trust-gradient mark, currentColor
├── logo-light.svg           mark on light backgrounds
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

Generated rasters (do not edit by hand — regenerate via the script):

```
├── og-image.png / .jpg          1200×630 OpenGraph social card
├── github-social.png / .jpg     1280×640 GitHub social preview
├── x-banner.png / .jpg          1500×500 X / Twitter header
├── apple-touch-icon.png         180×180 iOS home-screen icon
├── favicon-32/64/128/256/512.png  multi-size favicon raster set
├── logo.png                     mark @ 512 height (transparent bg)
├── logo-light.png               mark in dark stroke @ 512 height
├── logo-dark.png                mark in light stroke @ 512 height
├── stacked-mark.png             secondary mark @ 512 height
├── wordmark.png                 wordmark @ 1024 width
├── lockup.png                   horizontal lockup @ 1200 width
├── lockup-stacked.png           stacked lockup @ 800 width
├── logo-on-cream.jpg            logo on #F5F5F0 (no transparency surfaces)
└── logo-on-dark.jpg             logo on #0A0A0A (no transparency surfaces)
```

## Which to use where

| Surface | File |
|---|---|
| GitHub repo social preview (Settings → Social preview) | `github-social.png` |
| Twitter / X post card | `og-image.jpg` |
| LinkedIn post image | `og-image.jpg` |
| X / Twitter profile header | `x-banner.jpg` |
| Slack / Discord workspace icon | `logo-on-dark.jpg` or `logo-on-cream.jpg` |
| Notion / Confluence page header | `lockup.png` |
| iOS home screen | `apple-touch-icon.png` |
| Browser tab | `favicon.svg` (modern), `favicon-32.png` (legacy) |
| App / PWA icon | `favicon-512.png` |
| Anywhere with native SVG support | the matching `.svg` |

JPG variants exist for surfaces that strip alpha or compress aggressively (Twitter cards, LinkedIn, email). PNG for everywhere else. SVG is always canonical.

## Regenerate the rasters

When you change an SVG, regenerate its raster siblings:

```bash
brew install librsvg     # one-time, provides rsvg-convert
bash scripts/build-rasters.sh
```

The script lives at [`/scripts/build-rasters.sh`](../../scripts/build-rasters.sh). Output is deterministic — re-running with no SVG changes produces byte-identical files.

## How the site uses these

`index.html` references `branding/favicon.svg` for the tab icon and `branding/og-image.svg` for `og:image`. The trust-gradient mark inside the bento cards is inlined as SVG (so it can pick up `currentColor` and react to theme).

## Updating

When you change a brand asset, update both this folder and the canonical [`/branding`](../../branding). They diverge by design (sizes, formats can differ), but the visual identity must stay consistent. See [`/BRAND.md`](../../BRAND.md) for the rules.
