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
- model files (separate download)
- the desktop wrapper (`unhosted-desktop`) — not yet shipped in releases; plan to add once we have icons and code signing sorted

Both are tracked as gaps in the next milestone (`unhosted pull` for models, bundled `llama.cpp` later).
