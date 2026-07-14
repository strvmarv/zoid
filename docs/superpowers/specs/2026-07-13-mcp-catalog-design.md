# Spec 2.5 — MCP Catalog Entries (design)

**Status:** approved design, pre-plan
**Date:** 2026-07-13
**Depends on:** Spec 2 (Community Plugin Catalog), merged to `main` (`e04f123`); catalog
is live in the public `strvmarv/zoid-releases` repo (`plugins/index.json`).

## Goal

Extend the community plugin catalog to a third plugin kind: **`mcp`**. An mcp-kind
catalog entry describes a stdio MCP server (command/args/env). Installing it **merges a
server block into the user's `.mcp.json`** — additive, stdio-only, with per-name dedup —
behind a confirm gate that shows the **exact command to be run** and the **target file**.
This closes the gap the Spec 2 design explicitly deferred ("Spec 2 catalog kinds are `mode`
and `skills` only; MCP deferred to Spec 2.5").

## Non-goals (deferred / out of scope)

- **HTTP/SSE MCP transport.** zoid MCP is stdio-only (`zoid-mcp` ships only
  `StdioTransport`). An http-style server in a manifest is **rejected at validate**, mirroring
  the Spec 1 importer which drops http servers.
- **Hot-connect in-session.** Installing writes `.mcp.json` and reports a **restart hint**;
  the server connects on next startup via the existing `discover` + `spawn_connect_all` path.
  (Consistent with Spec 1 skills-install, which also reports a restart hint.) No new
  single-server connect path on `McpManager`.
- **Secret prompting / secret storage.** The catalog is public; manifests carry only
  `${VAR}` placeholders (and literal *non-secret* values). No install-time secret entry, no
  plaintext secrets written by zoid.
- **Editing / uninstalling** existing servers. Install is additive only; a name collision is
  skipped, not overwritten. Removal stays a manual `.mcp.json` edit.
- **Per-server selective install from a bundle-of-many** (that is Spec 3 territory). A single
  mcp manifest MAY declare more than one server (see §A) and they install together.

## Background: what already exists

- **`.mcp.json` is read-only today.** `zoid-mcp::config::discover(user_dir, cwd, get_env)`
  (`crates/zoid-mcp/src/config.rs:103`) reads `user_dir/mcp.json` then `cwd/.mcp.json`
  (project overrides user by name); `${VAR}` is expanded in args/env at runtime via
  `expand_vars`. **Nothing writes `.mcp.json`** — Spec 2.5 introduces the first writer.
- **Server config shape** (`config.rs:5`): `McpServerConfig { command: String, args:
  Vec<String>, env: BTreeMap<String, String> }`; wire form is
  `{ "mcpServers": { "<name>": { "command", "args", "env" } } }` (`RawServer`, `config.rs:20`).
  `command` is required; there is no `type`/`url` field, so an http-style entry fails to parse.
- **The Spec 1 importer already normalizes** a Claude `.mcp.json` into that exact string,
  dropping http servers (`crates/zoid-plugin-import/src/emit.rs`, `classify.rs`) — precedent
  for a manifest carrying a stdio server block.
- **Three gates know only `mode`/`skills`** and must learn `mcp`:
  1. `PluginManifest::validate` (`crates/zoid-plugin/src/manifest.rs:165`) hard-rejects any
     kind other than `mode`/`skills`.
  2. `map_catalog_entries` (`crates/zoid/src/main.rs:4895`) filters the catalog overlay to
     `mode`/`skills` — this is where `mcp` is currently silently dropped.
  3. `apply_plugin_scan` kind dispatch (`main.rs:5109`) + `build_plan`
     (`crates/zoid-plugin/src/plan.rs:16`) branch mode vs skills.
- **The install Effect gate rejects all writes.** `finish_plugin_install`
  (`crates/zoid/src/plugin_install.rs:64`) rejects every `SetConfig` and any Dangerous-risk
  effect *before* touching the filesystem. mcp install must therefore use its **own** path,
  not an `Effect`.
- **The catalog confirm overlay already surfaces provenance + `[y/N]`**
  (`render_plugin_catalog_overlay`, `crates/zoid-tui/src/render.rs:1146`), a reusable pattern.
- **The invisible-overlay bug class is now compiler-guarded** (`layout.rs::compute` uses an
  exhaustive `match Overlay`, fixed 2026-07-14, `c526229`), so new confirm content cannot
  silently fail to draw.

## Architecture

Four parts, mirroring the Spec 2 seams:

- **(A) Manifest** — a new `kind = ["mcp"]` manifest with an inline `[mcp.servers.<name>]`
  table and **no** `[source]`/`[mode]`.
- **(B) `.mcp.json` writer** — a new additive `merge_server` in `zoid-mcp`, sitting next to
  the existing reader.
- **(C) Install path** — a dedicated mcp branch in the bin that skips the upstream-tree fetch
  and writes `.mcp.json` after confirm.
- **(D) Surfacing** — mcp rows in the `:plugin` overlay + a kind-aware confirm sub-state
  (command/env/target) + `:plugin list` shows mcp entries.

### A. Manifest shape — `kind = ["mcp"]`

An mcp manifest has `[plugin]`, **no `[source]`** (an MCP server is not a repo tree — nothing
to fetch), **no `[mode]`**, and an inline server map:

```toml
[plugin]
id = "github"
schema = 1
kind = ["mcp"]
name = "GitHub MCP"
description = "GitHub repos/issues/PRs over MCP."
license = "MIT"

# A map (mirrors the .mcp.json `mcpServers` object). A manifest MAY declare more
# than one related server; they install together.
[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "${GITHUB_TOKEN}" }   # ${VAR} placeholders only — never a literal secret
```

New types in `crates/zoid-plugin/src/manifest.rs`:

```rust
pub struct McpServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}
// On PluginManifest:
pub mcp: Option<McpManifest>,       // Some for kind=mcp
// where
pub struct McpManifest { pub servers: BTreeMap<String, McpServerSpec> }
```

`validate()` gains an `mcp` arm:
- **Accepts** `kind = ["mcp"]`.
- **Requires** a non-empty `[mcp.servers]`; each server requires a non-empty `command`.
- **Forbids** `[source]` and `[mode]` on an mcp manifest (they are meaningless — fail loud
  rather than silently ignore).
- **Rejects** any server that looks http-style. Since the parse shape has only
  `command`/`args`/`env`, an http entry with `url`/`type` fails at deserialize; validate adds
  a belt-and-suspenders check that `command` is present. (No new fields are added for http.)

The existing `mode`/`skills` validate arms are unchanged.

### B. `.mcp.json` writer — `zoid-mcp::config`

A new writer lives next to the reader (same file, shared `RawFile`/`RawServer` shapes):

```rust
pub enum MergeOutcome { Inserted, SkippedExisting }

/// Additively merge one named stdio server into the file at `path`.
/// - Reads the existing file (or treats a missing file as empty `{ "mcpServers": {} }`).
/// - If `name` already exists: return `SkippedExisting`, write nothing.
/// - Else insert and write the whole object back, preserving all other servers.
/// - A malformed existing file is an error (do NOT overwrite unparseable config).
pub fn merge_server(
    path: &Path,
    name: &str,
    spec: &RawServer,          // command/args/env; ${VAR} written verbatim
) -> anyhow::Result<MergeOutcome>;
```

- Serialization is pretty JSON (2-space indent, trailing newline) via `serde_json`. Env is
  written **verbatim** (placeholders preserved); `merge_server` never expands `${VAR}` — that
  is runtime `discover`'s job.
- **Sibling + unknown-key preservation via `serde_json::Value` read-modify-write:** parse the
  existing file into a `serde_json::Value`, ensure a `mcpServers` object exists, check/insert
  `mcpServers[name]`, reserialize the whole `Value`. This preserves every other server **and**
  any unrecognized top-level keys, without needing a typed struct that enumerates them. A
  non-object root, or a non-object `mcpServers`, is a malformed-file error (abort, do not
  overwrite).
- Missing parent directory (user config dir) is created before write.

Writing multiple servers = `merge_server` called once per server; outcomes aggregated.

### C. Install path — dedicated mcp branch

mcp-kind **never** enters the upstream-tree fetch or the Effect gate:

1. **Discovery load** (unchanged): the `:plugin` overlay fetches `index.json`;
   `map_catalog_entries` now **includes `mcp`** rows (badge `[mcp]`).
2. **Enter on an mcp row** → the manifest details (command/env) are **not** in `index.json`,
   so entering confirm **fetches `<id>.toml` on demand** (async, unauthenticated raw), parses +
   validates it, and populates the confirm sub-state with: each server's `command` + `args`,
   its `env` keys (each flagged `⚠ not set` when the corresponding `${VAR}` is absent from the
   current environment), and the target toggle. A fetch/parse/validate failure shows an error
   in the confirm and writes nothing. (mode/skills rows keep using the already-loaded index
   data; only mcp needs the on-demand fetch.)
3. **Confirm** shows `[p] project (cwd/.mcp.json)` / `[u] user (<user_dir>/mcp.json)` (default
   project), the exact command(s), and env warnings, then `Install this MCP server? [y/N]`.
4. **`y`** → `install_mcp_server(app, manifest, target)`: for each server in
   `[mcp.servers]`, call `merge_server(target_path, name, spec)`; aggregate outcomes; report
   `✓ wrote '<name>'` / `ℹ '<name>' already configured — left unchanged` per server, plus a
   single restart hint. `n`/`Esc` → back to the list.

Bundled resolution still works: a future bundled mcp id would resolve via `bundled_manifest`
without a catalog fetch; the catalog path is the general case.

### D. Surfacing UX

- **`:plugin` overlay** — mcp rows appear alongside mode/skills, badge `[mcp]`. The confirm
  **sub-state is kind-aware**:
  - mode/skills: existing provenance card (repo @ short-sha, kind, license) — unchanged.
  - mcp: command card — server name(s), `command args`, env keys with `⚠ not set` warnings,
    and the `[p]/[u]` target toggle. `p`/`u` toggle the target in Confirm mode; `y` writes.
- **`:plugin list`** — mcp entries print like the others: `id  [mcp]  description`.

The confirm state (`PluginCatalogState` / its Confirm mode) carries optional mcp details:
```rust
// carried only when the selected row is kind=mcp and its manifest has been fetched
struct McpConfirm {
    servers: Vec<(String /*name*/, String /*command+args display*/, Vec<EnvWarn>)>,
    target: McpTarget,   // Project | User
}
enum EnvWarn { Set(String), Unset(String) }   // key + whether ${VAR} resolves now
```
`zoid-tui` stays free of `zoid`/`zoid-mcp` types — the bin maps the fetched manifest into
these plain-string/enum shapes before storing them on the overlay state (same dependency
hygiene as `McpStatusRow`/`PluginCatalogRow`).

## Data flow (mcp install)

```
:plugin  ──▶ fetch index.json ──▶ map_catalog_entries (mode|skills|mcp)
   Enter on [mcp] row
        └─▶ async fetch <id>.toml ─▶ parse_manifest + validate(kind=mcp)
                └─▶ map to McpConfirm (command/args, env warnings, target) ─▶ Confirm sub-state
   [p]/[u] toggle target ; y
        └─▶ install_mcp_server(manifest, target)
                └─▶ for each server: merge_server(target .mcp.json, name, spec)
                        ├─ Inserted        ─▶ "✓ wrote '<name>'"
                        └─ SkippedExisting ─▶ "ℹ '<name>' already configured — left unchanged"
                └─▶ restart hint
```

No tree fetch, no plan build, no Effect gate on this path.

## Trust & safety

- Every catalog entry is a **maintainer-reviewed PR** to the public repo (the human trust
  gate), same as Spec 2.
- The confirm **always shows the exact command + args + env keys + the target file** before
  any write. Installing an MCP server means zoid will later **spawn that command** — the
  confirm is the informed-consent point.
- **No secrets in the catalog:** manifests carry `${VAR}` placeholders; zoid writes them
  verbatim and never prompts for or persists secret values. Unset `${VAR}`s are surfaced as
  non-blocking warnings.
- **Additive, non-destructive:** a name collision is skipped, never overwritten; a malformed
  target file aborts the write rather than clobbering it. Other servers are always preserved.
- **stdio-only** is enforced at validate; http servers never reach the writer.

## Error handling

| Situation | Behavior |
|---|---|
| Name already in target `.mcp.json` | `SkippedExisting` — report "left unchanged", write nothing |
| Target file is malformed JSON | Abort write, report error; never overwrite unparseable config |
| Target file missing | Treat as empty; create file (and parent dir) on write |
| `${VAR}` unset at confirm | Non-blocking `⚠ not set` warning; write proceeds |
| Manifest `<id>.toml` 404 / parse / validate fail on Enter | Error shown in confirm; nothing written |
| http/sse or missing `command` in manifest | Rejected at `validate` (stdio-only) |
| `[mcp.servers]` empty | Rejected at `validate` |
| `[source]`/`[mode]` present on mcp manifest | Rejected at `validate` |

## Testing

- **Manifest parse/validate** (`zoid-plugin`): parse `[mcp.servers.<name>]` map (command/args/
  env); validate accepts mcp with ≥1 server; rejects empty servers, rejects `[source]`/`[mode]`
  on mcp, rejects a server missing `command`. mode/skills validate unchanged (regression).
- **`merge_server`** (`zoid-mcp`): insert into a missing file (creates it, one server);
  insert alongside existing siblings (siblings preserved, order deterministic); skip on name
  collision (`SkippedExisting`, bytes unchanged); malformed existing file → error, file
  untouched; env `${VAR}` written verbatim.
- **Confirm state machine** (`zoid-tui`): mcp row Enter → Confirm carries McpConfirm; `p`/`u`
  toggle target; env-warn rows reflect set/unset; `y` emits the install action; `n`/`Esc`
  back to list. mode/skills confirm path unchanged.
- **Catalog filter** (`zoid`): `map_catalog_entries` now yields mcp rows with `[mcp]` badge;
  a synthetic index with mode+skills+mcp surfaces all three.
- **Fixtures:** a sample `github.toml` (mcp kind) + a fixture `.mcp.json` with a pre-existing
  server (for the collision test).
- Network fetch itself is not unit-tested (mirrors `github_fetch`/catalog).

## Repos touched

- **`zoid` (private):**
  - `crates/zoid-plugin/src/manifest.rs` — `[mcp]` parse, `McpServerSpec`/`McpManifest`,
    validate `mcp` arm.
  - `crates/zoid-mcp/src/config.rs` — `merge_server` + `MergeOutcome` (first `.mcp.json` writer).
  - `crates/zoid/src/main.rs` — `map_catalog_entries` includes mcp; on-demand manifest fetch
    at confirm; `install_mcp_server`; kind-aware `CatalogConfirmYes`.
  - `crates/zoid-tui/src/{state,render,route}.rs` — kind-aware Confirm sub-state, target
    toggle, mcp confirm render.
- **`zoid-releases` (public):** a first real `mcp` manifest (`plugins/<id>.toml`) once the
  feature ships; CI regenerates `index.json` (the `kind` array already flows through
  `gen_index.py` unchanged). **No private internals leak into this repo.**

## Open follow-ups (not in Spec 2.5)

- Hot-connect a just-installed server without restart (needs an `McpManager` single-server
  connect path).
- Uninstall / edit an MCP server from within zoid.
- HTTP/SSE MCP transport (a real `HttpTransport` behind the existing trait) — Spec 3.
- Per-server selective install from large multi-server bundles — Spec 3.
