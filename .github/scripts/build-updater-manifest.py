#!/usr/bin/env python3
"""Assemble the Tauri updater manifest (`latest.json`) from a directory
of release artifacts.

Tauri's updater plugin polls a URL (configured in tauri.conf.json) that
must return JSON of this shape:

    {
      "version": "0.0.8",
      "notes": "See CHANGELOG.md for what's in this release.",
      "pub_date": "2026-05-12T00:00:00Z",
      "platforms": {
        "darwin-aarch64": {
          "url": "https://.../unhosted-aarch64-apple-darwin.app.tar.gz",
          "signature": "..."
        },
        "linux-x86_64": { ... },
        "windows-x86_64": { ... }
      }
    }

This script reads `<dist>/*.sig` files (produced by Tauri's bundler when
TAURI_SIGNING_PRIVATE_KEY is set), pairs each `.sig` with its asset, and
emits `<dist>/latest.json`.

If no `.sig` files are present (e.g. the signing key secret is unset),
the script exits 0 without writing anything — the updater simply won't
have an endpoint to hit until you set the secret and re-release.

Usage:
    build-updater-manifest.py <tag> <dist-dir> <repo>
        tag      e.g. "v0.0.8"
        dist-dir directory containing release artifacts + .sig files
        repo     e.g. "unhosted-ai/unhosted-core" — used to build asset URLs
"""

import datetime
import json
import os
import sys
from pathlib import Path


# Map an asset filename to the Tauri platform key. ORDER MATTERS: more
# specific arch matchers run first so e.g. `unhosted_0.0.15_aarch64.AppImage`
# doesn't get snagged by linux-x86_64's generic `.AppImage` needle (which
# is what happened in v0.0.15 — the manifest had linux-x86_64 pointing at
# the arm64 AppImage and no linux-aarch64 entry at all). Each needle has
# to be specific enough to its arch that it can't accidentally match the
# other one: that's why linux-aarch64 lists every flavor of "aarch64" or
# "arm64" you might see in a Tauri bundle filename, and linux-x86_64's
# needles all explicitly name x86_64 / amd64 / x64.
PLATFORM_KEYS = [
    ("darwin-aarch64", ["aarch64-apple-darwin"]),
    ("darwin-x86_64", ["x86_64-apple-darwin"]),
    ("linux-aarch64", ["aarch64-unknown-linux", "aarch64.AppImage", "arm64.AppImage", "arm64.deb"]),
    ("linux-x86_64", ["x86_64-unknown-linux", "amd64.AppImage", "amd64.deb", "x86_64.AppImage"]),
    ("windows-x86_64", ["x86_64-pc-windows", ".msi", "x64-setup", "x64_en-US"]),
]


def classify(name: str) -> str | None:
    """Return the Tauri platform key for an artifact filename, or None."""
    for key, needles in PLATFORM_KEYS:
        if any(n in name for n in needles):
            return key
    return None


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__)
        return 2

    tag = sys.argv[1]
    dist = Path(sys.argv[2])
    repo = sys.argv[3]

    sig_files = sorted(dist.glob("*.sig"))
    if not sig_files:
        # Quiet exit — release.yml continues without a manifest.
        return 0

    platforms: dict[str, dict[str, str]] = {}
    for sig_path in sig_files:
        # An asset's signature lives at "<asset>.sig". Strip the suffix
        # to recover the asset filename.
        asset_name = sig_path.name[:-4] if sig_path.name.endswith(".sig") else sig_path.stem
        asset_path = dist / asset_name
        if not asset_path.exists():
            # Sig file orphaned from its asset — skip rather than guess.
            continue
        key = classify(asset_name)
        if not key:
            continue
        # We pick the first asset per platform; if you have both a .dmg
        # and a .app.tar.gz, prefer the one the Tauri docs recommend.
        if key in platforms:
            # Prefer .app.tar.gz over .dmg for darwin (smaller, no mount)
            # and .AppImage over .deb for linux (no install step needed).
            preferred = (".app.tar.gz", ".AppImage", ".msi")
            if not any(asset_name.endswith(p) for p in preferred):
                continue
        sig = sig_path.read_text().strip()
        url = f"https://github.com/{repo}/releases/download/{tag}/{asset_name}"
        platforms[key] = {"url": url, "signature": sig}

    if not platforms:
        return 0

    manifest = {
        "version": tag.lstrip("v"),
        "notes": "See CHANGELOG.md for what's in this release.",
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
        "platforms": platforms,
    }

    out = dist / "latest.json"
    out.write_text(json.dumps(manifest, indent=2))
    print(f"wrote {out} with {len(platforms)} platforms: {sorted(platforms)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
