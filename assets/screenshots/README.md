# Screenshots

This directory holds PNG screenshots of the running Unhosted daemon's web UI, embedded by the top-level [README](../../README.md).

## Filenames the README expects

| File | What to capture |
| --- | --- |
| `01-overview.png` | The default view after `unhosted serve`: chat composer on the right, sidebar on the left, no active conversation yet. |
| `02-chat.png` | An open chat with at least two user / assistant turns. Use the sample seeded by `scripts/screenshots.sh` (a short "what is unhosted?" exchange). |
| `03-public-mode.png` | Sidebar with the "public mode" collapsible expanded — rail checkboxes, KYC tier, blocked-countries list, save button. Other sidebar sections collapsed. |
| `04-vram-pool.png` | Sidebar with the "cluster (vram-pool)" collapsible expanded — model picker, layer-hosts list. Other sidebar sections collapsed. |

Dimensions: anything reasonable for a Mac browser window. The helper script sets Safari to 1280 × 820 (window chrome included); pick what looks good if you capture manually.

Format: PNG. Keep individual files under 500 KB — use a tool like `pngquant`, `oxipng`, or `sips -s formatOptions normal` to shrink.

## Two ways to generate

### Auto (macOS, recommended)

```bash
./scripts/screenshots.sh
```

Builds a small Swift CLI (`scripts/shotter.swift` → `target/shotter`, ~3s one-time) that uses `WKWebView.takeSnapshot()` to render each view to a PNG via WebKit's own compositor. **No Screen Recording permission required**, no focus stealing, no popping windows — the WebView is offscreen the whole time.

Requirements:

- macOS with Xcode command-line tools (`xcode-select --install` if missing).
- A built daemon at `target/debug/unhosted` or `target/release/unhosted`. `cargo build -p unhosted-cli` produces the former.

The script spins a fresh daemon on `127.0.0.1:7798` with a clean config dir, sets a meaningful public-mode policy so the sidebar has substance, then renders each shot at 2× backing scale (Retina-sharp on any Mac).

### Manual

Open the running daemon's URL in any browser, navigate to each state in the table above, and capture the window. Save with the filenames above and commit.

## Reshooting

The UI changes every release. When it changes meaningfully, rerun `scripts/screenshots.sh` and re-commit. The script overwrites existing PNGs.

## Why these aren't in CI

The shotter only builds on macOS (uses Cocoa + WebKit). A cross-platform headless renderer (Playwright/Puppeteer) is possible but adds ~300 MB of Chromium download to the dev workflow; the manual-on-release cadence is fine and the Mac shotter is a five-line Swift program shipped in `scripts/shotter.swift`.
