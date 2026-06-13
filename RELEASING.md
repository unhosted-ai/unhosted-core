# Releasing

How to cut a new release. Short.

## Cut a release

1. Update `CHANGELOG.md` — move the `[Unreleased]` section under a new dated heading.
2. Bump versions if needed in `Cargo.toml` (`[workspace.package].version`).
3. Commit on `main`:
   ```
   git commit -am "Release v0.0.2"
   git push origin main
   ```
4. Tag and push:
   ```
   git tag v0.0.2
   git push origin v0.0.2
   ```
5. GitHub Actions (`.github/workflows/release.yml`) picks up the tag, builds for:
   - `aarch64-apple-darwin` (Apple Silicon)
   - `x86_64-apple-darwin` (Intel Mac)
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu` (Raspberry Pi 5, ARM servers)
6. Workflow creates a GitHub release with the tarballs + SHA-256 sums attached.

Confirm at https://github.com/unhosted-ai/unhosted-core/releases — should take 5–10 minutes from tag push.

## What users see

Once the release exists, anyone can install with:

```
curl -fsSL https://raw.githubusercontent.com/unhosted-ai/unhosted-core/main/scripts/install.sh | sh
```

The script auto-detects platform, pulls the right tarball from the latest release, and installs to `/usr/local/bin/unhosted`.

To pin a version: `UNHOSTED_VERSION=v0.0.2 curl ... | sh`.
To install elsewhere: `UNHOSTED_INSTALL_DIR=~/.local/bin curl ... | sh`.

## Manual release

If the workflow misfires, you can also trigger it from the Actions tab:

- Actions → Release → Run workflow → enter the tag you want to release against an existing branch.

## Local macOS desktop release pipeline

For local macOS desktop packaging (app + dmg), use the single orchestrator:

```bash
bash scripts/release-macos.sh
```

This runs a deterministic pipeline:

- build `dist/unhosted.app`
- build `dist/unhosted.dmg`
- stage versioned artifacts + SHA-256 checksums in `dist/release-macos/`

Optional signing + notarization (mirrors a production release flow):

```bash
UNHOSTED_VERSION=v0.0.2 \
UNHOSTED_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
UNHOSTED_NOTARY_PROFILE="AC_PASSWORD" \
bash scripts/release-macos.sh
```

By default, `scripts/release-macos.sh` refuses to run from a dirty
working tree to avoid accidentally shipping uncommitted changes. For
local smoke tests only, override with:

```bash
UNHOSTED_ALLOW_DIRTY=1 bash scripts/release-macos.sh
```

Expected outputs:

- `dist/release-macos/unhosted-macos-app-<target>.tar.gz`
- `dist/release-macos/unhosted-macos-<version>-<target>.dmg`
- corresponding `.sha256` files

Without signing variables, the script still builds unsigned artifacts for local testing.

## Cross-compile locally from macOS

The CI workflow is the golden path. But sometimes you want to build a
Linux or Windows artifact on this Mac — to smoke-test a fix before
tagging, or to hand someone a binary directly.

`zig` as a cross-linker is the cleanest setup. One install covers both
Linux (musl + gnu) and Windows (gnu) targets:

```bash
brew install zig                                    # ~50 MB
cargo install --locked cargo-zigbuild              # ~10 MB
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-musl
rustup target add x86_64-pc-windows-gnu
```

CLI cross-compile (works cleanly — pure Rust + reqwest with rustls):

```bash
cargo zigbuild --release -p unhosted-cli --target x86_64-unknown-linux-musl
cargo zigbuild --release -p unhosted-cli --target x86_64-pc-windows-gnu
```

Then use the bundle scripts with the right target:

```bash
UNHOSTED_TARGET=x86_64-unknown-linux-musl bash scripts/bundle-linux.sh
# Windows .zip — run from a Windows box or matching cross-environment.
```

**Caveat on the desktop binary.** `unhosted-desktop` pulls in Tauri,
which links against webkit2gtk on Linux and WebView2 on Windows. Those
have C/system dependencies that don't cross-compile cleanly from
macOS. For desktop builds, lean on CI or build natively on a Linux
container / Windows box. CLI-only cross-compile is the realistic
local-from-Mac story.

## What's not in the binary

- `llama.cpp` (separate install)
- model files (use `unhosted pull <name>` after install)
- the desktop wrapper (`unhosted-desktop`) — not yet shipped in releases; plan to add once we have icons and code signing sorted

## Docker image

Pushing to `main` or tagging `v*.*.*` also triggers `.github/workflows/docker.yml`, which builds a multi-arch (amd64 + arm64) image and publishes to GitHub Container Registry:

- `ghcr.io/unhosted-ai/unhosted:latest` (latest main)
- `ghcr.io/unhosted-ai/unhosted:0.0.3` and `:0.0` (per-tag)

Typical use:

```bash
docker run --rm -p 7777:7777 \
  -v ~/.cache/unhosted:/root/.cache/unhosted \
  -v ~/.config/unhosted:/root/.config/unhosted \
  -e UNHOSTED_LLAMA_SERVER_URL=http://host.docker.internal:8080 \
  ghcr.io/unhosted-ai/unhosted:latest
```

The image is daemon-only — `llama-server` and models stay on the host.

### What the GHCR package page shows

The Dockerfile and `docker/metadata-action` together set OCI annotations that make the GitHub Packages page render with:

- **Title / description** — from `org.opencontainers.image.title` / `description`
- **Source** — auto-linked to this repo via `org.opencontainers.image.source`
- **License** — `AGPL-3.0-or-later` via `org.opencontainers.image.licenses`
- **README** — pulled from the repo's README.md once the source label resolves
- **Provenance + SBOM** — attached on every build, visible under "Artifact metadata"

After the first successful publish, set the package to **public** in:
https://github.com/orgs/unhosted-ai/packages/container/unhosted/settings

Once public, `docker pull ghcr.io/unhosted-ai/unhosted:latest` works without auth.
