# docs/ — GitHub Pages source

This folder is the source for the Unhosted landing page at **https://unhosted-ai.github.io/unhosted-core/** (or whatever GitHub Pages URL the repo ends up on).

## Enable GitHub Pages

1. Push this folder to `main`.
2. Go to repo → Settings → Pages.
3. Source: **Deploy from a branch**.
4. Branch: **main**, folder: **/docs**.
5. Save. The page goes live in ~1 minute.

For a custom domain (e.g. `unhosted.dev`), add a `CNAME` file in this folder containing the domain, then point a CNAME DNS record at `unhosted-ai.github.io`.

## Layout

- `index.html` — single-page landing in the heavy-lowercase Kernel-inspired style. Uses the trust-gradient mark inside cards and a stacked 2x2 secondary mark in the footer.
- `style.css` — vanilla CSS, no build step. Loads Boldonse and Inter from Google Fonts.
- `favicon.svg` / `og-image.svg` — page-local copies of the brand assets.

## Editing

The page has no build step. Edit HTML/CSS, push to `main`, GitHub re-publishes automatically. Test locally with any static server:

```bash
cd docs
python3 -m http.server 8080
# open http://127.0.0.1:8080
```

## Voice and visual rules

See [`../BRAND.md`](../BRAND.md). Don't add a "Sponsors" section, don't ask for stars, don't use emoji in copy, don't use the word "platform."
