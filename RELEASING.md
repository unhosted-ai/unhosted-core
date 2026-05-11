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
