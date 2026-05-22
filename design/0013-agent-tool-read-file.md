# ADR 0013: Agent `read_file` tool

Status: Proposed (2026-05-21). Implementation lands as the slice immediately following this ADR.

## TL;DR

Add a `read_file` tool to the agent runtime's registry. The agent can read text files from a small set of operator-allow-listed roots, with a size cap, a deny-list of sensitive paths, and a refusal to follow symlinks outside the sandbox. Default config is **empty** — no roots, no reads — so the daemon out-of-the-box adds no filesystem blast radius. Operators opt-in by creating `~/.config/unhosted/agent-fs.toml`.

## Context

ADR-0012 deliberately deferred filesystem access from slice 1 of the agent runtime. The slice-1 tools (`web_fetch`, `search_memory`, `list_models`) are all read-only and constrained to the daemon's existing primitives. `read_file` is the first tool that lets the agent reach into the host's filesystem.

The blast-radius question is real:

- The daemon runs as the operator's user. Anything that user can read, `read_file` could in principle read.
- A misconfigured allow-list (e.g., `~` or `/`) lets an agent exfiltrate SSH keys, cloud credentials, `.env` files, the operator's mail, browser cookies on Linux, etc.
- An agent that calls `read_file` based on user input is one prompt-injection away from being instructed to read a path the operator never intended.

A filesystem tool exists in every serious agent framework (LangChain, AutoGen, OpenAI Assistants); the question is not whether to add one but how to scope it. This ADR picks a model that errs on the side of refusal.

## Decision

### Allow-list of roots, no globs

Operators specify zero or more **absolute** paths in `agent-fs.toml`. The tool only reads files whose canonicalized absolute path is **strictly under one of those roots**.

```toml
# ~/.config/unhosted/agent-fs.toml

# Roots the agent may read. Absolute paths only. Each root is
# treated as a closed prefix — the agent can read any file
# whose canonical path starts with `<root>/`.
allow_roots = [
    "/Users/operator/Documents/unhosted-agent-workspace",
    "/Users/operator/Projects/public-docs"
]

# Deny patterns. Applied after allow-list match — even paths
# inside an allowed root are rejected if they match any of
# these substrings. Suffix-style matching, case-insensitive.
deny_patterns = [
    ".env",
    ".env.local",
    ".env.production",
    "id_rsa",
    "id_ed25519",
    ".pem",
    ".p12",
    "credentials",
    "secrets",
    ".sqlite",
    ".db"
]

# Max bytes read per call. Larger files return only the first
# `max_bytes` with a `truncated: true` marker.
max_bytes = 524288   # 512 KiB

# Whether to follow symlinks. Default: false. With follow_symlinks
# = true, the canonicalization still keeps the result under an
# allowed root, but a symlink that resolves into an allowed root
# from outside it is permitted. Recommended: leave false.
follow_symlinks = false
```

Absent config file ⇒ tool returns `read_file not configured on this host` on every call. This matches how `lightning_cfg` and `dlp` behave: the daemon doesn't infer permissions from defaults.

### Resolution algorithm

For every `read_file({ path })` call:

1. If `path` is not absolute, reject. The model can't use `~` shorthand or relative paths — agents shouldn't depend on the daemon's CWD, and the model has no concept of "the user's home directory" except what we tell it.
2. Canonicalize the path. This resolves `.`, `..`, and (when `follow_symlinks: true`) symlinks. Canonicalization uses `std::fs::canonicalize` which calls the OS's `realpath`; if the path doesn't exist, the call returns an error and the tool returns "file not found" without leaking whether intermediate path components exist.
3. Verify the canonicalized result is **strictly under** one of the allow-list roots (also canonicalized at config load). "Strictly under" means: equal to the root **or** the next byte after the root prefix is a path separator. Prevents a path like `/Users/operator/Documents/unhosted-agent-workspace2/secret` from passing the check against root `/Users/operator/Documents/unhosted-agent-workspace`.
4. Apply deny-pattern matching. Case-insensitive substring match against the canonicalized path. Any match → reject.
5. Open the file with `std::fs::File::open` and read up to `max_bytes`. If the file is larger, set `truncated: true` in the response.
6. Refuse binary content. Detect by attempting to interpret the read bytes as UTF-8; non-UTF-8 → return `(non-text content: <byte_count> bytes)` rather than a binary blob. This keeps the model from seeing noise that can't help it reason.

### Symlink policy

Default `follow_symlinks: false`. With it off:

- The canonicalization happens, but if any path component is a symlink, `std::fs::symlink_metadata` and a per-component check refuse the read. This is a stricter posture than `realpath` alone, which silently follows symlinks.
- Rationale: a malicious symlink at `<allowed_root>/notes.txt → /etc/passwd` would otherwise pass step 3 (canonical path is `/etc/passwd`, which is *not* under the allow-list root and gets rejected by step 3 anyway). The `follow_symlinks: false` flag is belt-and-braces — it refuses **before** canonicalization, so we never even invoke `realpath` on a symlinked path.

With `follow_symlinks: true`, step 3 is the only gate. Operators who want this should be explicit about it.

### Tool surface as the model sees it

OpenAI tool definition (matching what other tools in slice 1 emit):

```json
{
  "type": "function",
  "function": {
    "name": "read_file",
    "description": "Read a UTF-8 text file from the operator's allow-listed roots. Returns the file's contents or a structured error.",
    "parameters": {
      "type": "object",
      "properties": {
        "path": {
          "type": "string",
          "description": "Absolute path. Must be under one of the operator's configured allow-list roots."
        }
      },
      "required": ["path"]
    }
  }
}
```

Note the description discloses the allow-list constraint to the model. A well-prompted model will not waste a turn trying paths outside the sandbox.

### Audit + metrics

`read_file` calls emit the existing `AuditEvent::AgentToolCall` — no new variant needed. The `args_hash` is the SHA-256 prefix of `{ "path": "..." }`, so a SIEM can correlate "same agent kept trying the same path" without the path itself leaking into the audit feed. Full path is at `tracing::debug`-level only.

The `unhosted_agent_tool_calls_total` counter already increments per tool call. No new metric series for slice 4a; aggregate-by-tool labelling is a follow-up.

### What this does not do

- **No write access.** Slice 4a is read-only. `write_file`, `append_file`, `mkdir`, `delete_file` are separate ADRs.
- **No directory listing.** The agent can read a path it knows, not enumerate what's there. `list_dir` is a separate (much smaller) ADR; deferring it keeps slice 4a focused.
- **No glob expansion.** `path` is one path, not a glob. Globs would require operators to think about traversal in addition to roots.
- **No "human in the loop" prompt.** Some agent frameworks pop a "the agent wants to read /etc/passwd, allow?" dialog. We don't. The daemon is often headless; the operator's contract is "configure the allow-list correctly once, agents stay inside it."
- **No retroactive content filtering.** If an allowed file *contains* an API key the operator forgot to deny-pattern, the model sees it. The deny-list is path-based, not content-based. Content-level DLP is what the existing `dlp` hook is for.

## Implementation slice

New module `agent_fs.rs`:

- `AgentFsConfig { allow_roots, deny_patterns, max_bytes, follow_symlinks }`
- `pub fn load() -> Result<Option<AgentFsConfig>>` — same shape as `lightning_cfg::load`.
- `pub fn read_file(cfg: &AgentFsConfig, path: &str) -> ReadFileOutcome` — does steps 1–6 above.

`ReadFileOutcome` variants:
- `Ok { content, bytes, truncated }`
- `Err { reason: ReadFileError }` — enum: `NotConfigured | NotAbsolute | OutsideAllowList | DenyPattern | SymlinkRefused | NotFound | NotUtf8 | Io(String)`

`NodeState` gains an `agent_fs: Option<Arc<AgentFsConfig>>` loaded at startup (same posture as `dlp`).

`agent.rs`:
- `known_tools()` gains `"read_file"`.
- `tool_definitions` emits the JSON definition above when the caller allow-lists `read_file`.
- `execute_tool` gains a `"read_file"` arm dispatching to `agent_fs::read_file`.
- `RunContext` gains `agent_fs: Option<Arc<AgentFsConfig>>` so `execute_tool` can read it.

Tests, all using `tempfile::TempDir`:
- Allowed read returns content.
- Read of file outside allow-list returns `OutsideAllowList`.
- Deny-pattern match (e.g., `.env`) returns `DenyPattern`.
- Path-traversal attempt (`/<root>/../etc/passwd`) returns `OutsideAllowList` after canonicalization.
- Symlink out of allow-list returns `SymlinkRefused` when `follow_symlinks: false`.
- File larger than `max_bytes` returns `truncated: true`.
- Binary file returns `NotUtf8`.
- Missing config → `NotConfigured`.

## Threat model summary

| Threat | Mitigation |
|---|---|
| Prompt injection → "read /etc/passwd" | Allow-list rejects; deny-pattern rejects; `.passwd` substring would catch it too. |
| Symlink trap | `follow_symlinks: false` default; canonicalization + strict-under check. |
| Path traversal (`../`) | Canonicalize before any check; the resolved path goes through the allow-list gate. |
| Deny-list bypass via case (e.g., `.ENV`) | Case-insensitive matching. |
| Reading a file containing secrets the operator didn't add to deny-list | The existing `dlp` hook catches the content before the model sees it (the agent's "tool result → model" path is DLP-gated, ADR-0012). |
| Operator misconfigures with `allow_roots = ["/"]` | Documented loudly in this ADR + the comment template in `agent-fs.toml`. No technical defense beyond refusing to start the daemon on a "/" root, which feels over-paternalistic. |

## Consequences

- **Pro:** The agent can grep a codebase, summarize a doc, refer to past meeting notes — all the obvious filesystem-read use cases — with a tight blast radius.
- **Pro:** Default-empty config means the security posture out-of-the-box is unchanged. Existing deployments are not silently more powerful after the upgrade.
- **Pro:** The same audit / DLP / metrics infrastructure all the other agent tools use applies here. No new operational surface.
- **Con:** Operators who want filesystem access have to learn one more config file. The cost is real but the alternative (broad-default permissions) is worse.
- **Con:** UTF-8 refusal means the agent can't read PDFs, images, or binary office docs. A future `extract_text` tool that wraps a parser is the right answer; piping raw bytes through the model is not.

## License

This ADR is AGPL-3.0-or-later, same as the rest of `unhosted-core`. The author timestamps it via the git commit that adds it.
