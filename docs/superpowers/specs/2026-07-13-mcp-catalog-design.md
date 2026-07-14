# Spec 2.5 — MCP Catalog Entries (design)

**Status:** approved design (gilfoyle-revised), pre-plan
**Date:** 2026-07-13
**Depends on:** Spec 2 (Community Plugin Catalog), merged to `main` (`e04f123`); catalog
is live in the public `strvmarv/zoid-releases` repo (`plugins/index.json`).

## Goal

Extend the community plugin catalog to a third plugin kind: **`mcp`**. An mcp-kind
catalog entry describes **exactly one** stdio MCP server (command/args/env). Installing it
**merges that server block into a `.mcp.json`** — additive, stdio-only, skip-on-name-collision —
behind a confirm gate that shows the **exact command to be run** and the **target file**.
This closes the gap Spec 2 explicitly deferred ("Spec 2 catalog kinds are `mode` and `skills`
only; MCP deferred to Spec 2.5").

## Non-goals (deferred / out of scope)

- **HTTP/SSE MCP transport.** zoid MCP is stdio-only (`zoid-mcp` ships only `StdioTransport`).
  An http-style server in a manifest is **rejected at validate**, mirroring the Spec 1 importer
  which drops http servers. Deferred to Spec 3 (a real `HttpTransport` behind the existing trait).
- **Multiple servers per manifest / per-server selective install.** A 2.5 mcp manifest declares
  **exactly one** server. A plugin needing two servers ships two manifests. Multi-server bundles
  and selective install land together in Spec 3 (they share the aggregation + partial-success
  machinery this spec deliberately avoids).
- **Hot-connect in-session.** Installing writes `.mcp.json` and reports a **restart hint**; the
  server connects on next startup via the existing `discover` + `spawn_connect_all` path
  (consistent with Spec 1 skills-install). No new single-server connect path on `McpManager`.
- **Secret prompting / secret storage.** The catalog is public; manifests carry only `${VAR}`
  placeholders (and literal *non-secret* values). No install-time secret entry, no plaintext
  secrets written by zoid.
- **Editing / uninstalling** existing servers. Install is additive only; a name collision is
  skipped, not overwritten. Removal stays a manual `.mcp.json` edit.

## Background: what already exists (verified against the code)

- **`.mcp.json` is read-only today.** `zoid-mcp::config` has `parse`/`discover`/`expand` and
  **no writer**. `discover(user_dir, cwd, get_env)` (`config.rs:103`) reads `user_dir/mcp.json`
  then `cwd/.mcp.json` (project overrides user by name, `config.rs:108-114`); `${VAR}` is expanded
  in args/env at runtime via `expand_vars` (`config.rs:48`). A malformed file is silently dropped
  on read (`config.rs:81`). **Spec 2.5 introduces the first writer.**
- **Server config shape** (`config.rs:5`, `:20`): `McpServerConfig { command, args, env }`; wire
  form `{ "mcpServers": { "<name>": { "command", "args", "env" } } }`. `command` is required and
  there is no `type`/`url` field, so an http-style entry fails to deserialize.
- **The Spec 1 importer already normalizes** a Claude `.mcp.json` into that exact string, dropping
  http servers (`crates/zoid-plugin-import/src/{emit.rs,classify.rs}`) — precedent for a manifest
  carrying a stdio server block.
- **Three gates know only `mode`/`skills`** and must learn `mcp`:
  1. `PluginManifest::validate` (`crates/zoid-plugin/src/manifest.rs:165`) hard-rejects any kind
     other than `mode`/`skills`.
  2. `map_catalog_entries` (`crates/zoid/src/main.rs:4895`) filters the catalog overlay to
     `mode`/`skills` — where `mcp` is currently silently dropped.
  3. `apply_plugin_scan` kind dispatch (`main.rs:5109`) + `build_plan` (`plan.rs:16`).
- **The install Effect gate is reject-only for config writes.** `finish_plugin_install`
  (`plugin_install.rs:64-75`) rejects every `SetConfig` and any Dangerous-risk effect *before* any
  filesystem write. (It does **not** block file materialization — `materialize` still writes skill
  files; it blocks *config-mutating* effects.) mcp install must therefore use its **own** path, not
  an `Effect`.
- **The catalog confirm is synchronous today.** `enter_confirm()` (`state.rs:177`) flips
  `CatalogMode::List → Confirm` and Confirm renders from **data already in the row** (index.json
  provenance); no fetch happens at Enter. The manifest fetch happens **after** `CatalogConfirmYes`
  (`main.rs:4542-4553`), which sets `overlay=None` and clears state, *then* spawns → terminal
  `AgentUpdate::PluginScan`. Because the overlay is already closed, Spec 2 has no
  "overlay mutated mid-fetch" race. **Spec 2.5's confirm-time fetch reintroduces one and must
  handle it explicitly (see §C/C1).**
- `CatalogMode` is `{ List, Confirm }` (`state.rs:131`) and is rendered by an **exhaustive
  `match CatalogMode`** in `render_plugin_catalog_overlay` (`render.rs:1188`) — a new sub-state must
  add a real variant there (the `layout.rs` exhaustive `match Overlay` guards only the *palette
  region*, not sub-state render arms; `PluginCatalog` already reserves its region).

## Architecture

Five parts, mirroring the Spec 2 seams:

- **(A) Manifest** — `kind = ["mcp"]` (exclusive) with one inline `[mcp.servers.<name>]` table,
  **no** `[source]`/`[mode]`.
- **(B) `.mcp.json` writer** — a new additive, **atomic**, order-preserving `merge_server` in
  `zoid-mcp`, next to the reader.
- **(C) Confirm-time fetch machine** — a dedicated `AgentUpdate` carrier, a `ConfirmLoading`
  sub-state, and a selected-id match so the confirm always shows the manifest the user is looking at.
- **(D) Install path** — a dedicated mcp branch that skips the tree fetch and the Effect gate and
  writes `.mcp.json` after confirm.
- **(E) Surfacing** — mcp rows in `:plugin` + kind-aware confirm + `:plugin list`.

### A. Manifest shape — `kind = ["mcp"]` (exclusive, one server)

```toml
[plugin]
id = "github"
schema = 1
kind = ["mcp"]                     # EXACTLY ["mcp"] — mutually exclusive with mode/skills
name = "GitHub MCP"
description = "GitHub repos/issues/PRs over MCP."
license = "MIT"

# Exactly one server. The table key is the server name written into .mcp.json.
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
pub struct McpManifest { pub servers: BTreeMap<String, McpServerSpec> }  // len == 1 (validated)
// On PluginManifest:
pub mcp: Option<McpManifest>,       // Some for kind=mcp
```

The server map is kept (rather than a flattened `[mcp] name = "..."`) so the TOML reads naturally
(`[mcp.servers.github]`) and round-trips with the importer's normalized `mcpServers` output.

`validate()` gains an `mcp` arm:
- **Requires `kind == ["mcp"]` exactly.** A mixed `["mcp","skills"]`/`["mcp","mode"]` is rejected
  (mcp is not composable with the tree-materializing kinds; the dispatch can only route one way).
- **Requires exactly one** `[mcp.servers.<name>]`; the server requires a non-empty `command`.
- **Forbids** `[source]` and `[mode]` (meaningless for mcp — fail loud, don't silently ignore).
- **stdio-only:** the parse shape has only `command`/`args`/`env`, so an http entry (`url`/`type`)
  fails at deserialize; validate adds a belt-and-suspenders non-empty-`command` check.

The existing `mode`/`skills` validate arms are unchanged (regression-tested).

### B. `.mcp.json` writer — `zoid-mcp::config`

```rust
pub enum MergeOutcome { Inserted, SkippedExisting }

/// Additively merge one named stdio server into the file at `path`. Atomic, order-preserving.
pub fn merge_server(path: &Path, name: &str, spec: &RawServer) -> anyhow::Result<MergeOutcome>;
```

- **Read-modify-write over `serde_json::Value`:** parse the existing file into a `Value`, ensure a
  `mcpServers` object exists, check `mcpServers[name]`:
  - present → return `SkippedExisting`, **write nothing**;
  - absent → insert `spec`, reserialize the whole `Value`.
  This preserves every other server **and** any unrecognized top-level keys without a typed struct
  that enumerates them. A non-object root, or a non-object `mcpServers`, is a **malformed-file
  error** (abort; never overwrite).
- **Order-preserving:** enable the `serde_json/preserve_order` feature (workspace-wide) so `Value`
  is `IndexMap`-backed and insertion order — not alphabetical — is kept. Without it the first merge
  rewrites a hand-maintained file in alphabetical order (whole-file diff on a tracked file). *The
  plan verifies no existing test depends on `serde_json::Value` key ordering before enabling it.*
- **Atomic write:** serialize to a temp file **in the same directory**, then `rename` over `path`.
  A crash mid-write leaves the original intact rather than a truncated file that `discover` would
  silently drop (taking every sibling server offline). Bounds concurrent writers to last-writer-wins.
- **Env verbatim:** `${VAR}` placeholders are written unchanged; `merge_server` never expands them
  (that is runtime `discover`'s job).
- Missing parent directory (user config dir) and missing target file are created (empty →
  `{ "mcpServers": { ... } }`). Serialization is pretty JSON (2-space indent, trailing newline).

### C. Confirm-time fetch machine (C1)

`index.json` carries only id/name/kind/description/source — **not** the command/env an mcp confirm
must show. So entering the confirm on an mcp row fetches the full `<id>.toml` **into a still-open
overlay**, which needs three things Spec 2 didn't:

1. **A loading sub-state.** `CatalogMode` gains a real variant:
   `{ List, ConfirmLoading, Confirm }`. `render_plugin_catalog_overlay`'s exhaustive
   `match CatalogMode` (`render.rs:1188`) gets a `ConfirmLoading` arm ("fetching manifest…") and the
   `Confirm` arm renders either the fetched `McpConfirm` card or a `[fetch failed: <err>]` terminal.
   A missing arm is a compile error, per the existing exhaustive match.
2. **A carrier update.** New `AgentUpdate::McpManifestFetched { id: String, res: Result<McpConfirm,
   String> }` (mapped by the bin — see §E for the plain-string shape). `PluginScan` stays
   install-terminal; it is not reused to populate a confirm.
3. **A selected-id match.** On `Enter` over an mcp row, set `mode = ConfirmLoading`, record the
   selected `id`, and spawn the fetch. When `McpManifestFetched { id, res }` arrives, apply it **only
   if** `overlay == PluginCatalog` **and** `mode == ConfirmLoading` **and** `id == currently
   selected row id`; otherwise **drop it** (the user navigated away). This prevents a slow fetch for
   server A from populating a confirm the user has moved to server B — the consent-integrity guard.
   `Esc` during `ConfirmLoading` → `back_to_list` (`mode = List`); the late result is then dropped by
   the mode/id guard.

mode/skills rows are unaffected: their `Enter` stays synchronous (`List → Confirm`, no fetch), using
the already-loaded index provenance.

### D. Install path — dedicated mcp branch (M1)

mcp-kind **never** enters the upstream-tree fetch or the Effect gate:

1. **Discovery** (unchanged): `:plugin` fetches `index.json`; `map_catalog_entries` now **includes
   `mcp`** rows (badge `[mcp]`).
2. **Enter** on an mcp row → the §C fetch machine populates `Confirm` with the `McpConfirm` card.
3. **Confirm** shows target `[u] user (<user_dir>/mcp.json)` / `[p] project (cwd/.mcp.json)`
   (**default user** — see Trust & safety), the exact `command args`, env keys (each `⚠ not set` when
   its `${VAR}` is unset), and `Install this MCP server? [y/N]`. `p`/`u` toggle target in Confirm mode.
4. **`y`** → `install_mcp_server(app, carried_manifest, target)`: call `merge_server(target_path,
   name, spec)` for the one server; report `✓ wrote '<name>'` or `ℹ '<name>' already configured —
   left unchanged`, plus a restart hint. **It uses the manifest already carried in the confirm state
   — it must NOT re-enter `install_plugin`/the id path**, whose `PluginRef::Id → Catalog` branch
   re-fetches `<id>.toml` and *requires* `[source]` (`main.rs:5027-5030`), which an mcp manifest
   lacks (would error). `n`/`Esc` → back to the list.

No tree fetch, no plan build, no Effect gate on this path.

### E. Surfacing UX + dependency hygiene

- **`:plugin` overlay** — mcp rows alongside mode/skills, badge `[mcp]`. Kind-aware confirm:
  mode/skills keep the provenance card (repo @ short-sha, kind, license); mcp shows the command card.
- **`:plugin list`** — mcp entries print `id  [mcp]  description`.
- **`zoid-tui` stays free of `zoid`/`zoid-mcp` types.** The bin maps the fetched `PluginManifest`
  into plain shapes before storing them on overlay state (same hygiene as `McpStatusRow`
  `state.rs:65` / `PluginCatalogRow`):
  ```rust
  struct McpConfirm {
      server_name: String,
      command_display: String,       // "npx -y @modelcontextprotocol/server-github"
      env: Vec<EnvWarn>,
      target: McpTarget,             // User (default) | Project
  }
  enum EnvWarn { Set(String), Unset(String) }   // key; Unset only for a real ${VAR} placeholder
  enum McpTarget { User, Project }
  ```
  **EnvWarn placeholder rule:** a value is a candidate for `Unset` **only if it contains a `${VAR}`
  reference**; a plain literal is never flagged. For a `${VAR}` reference, `Unset` iff that variable
  is absent from the current environment (a value with literal text *and* an embedded `${VAR}` still
  resolves via `expand_vars` at runtime and is flagged on the var).

## Data flow (mcp install)

```
:plugin  ─▶ fetch index.json ─▶ map_catalog_entries (mode|skills|mcp)
  Enter on [mcp] row (id=X)
    └─▶ mode=ConfirmLoading, remember id=X ─▶ spawn: fetch <id>.toml ─▶ parse + validate(kind=mcp)
          └─▶ AgentUpdate::McpManifestFetched { id:X, res: Ok(McpConfirm) | Err }
                └─▶ apply IFF overlay==PluginCatalog && mode==ConfirmLoading && selected==X
                      ├─ Ok  ─▶ mode=Confirm, show command card (default target=User)
                      └─ Err ─▶ Confirm shows "[fetch failed: …]"
  [u]/[p] toggle target ; y
    └─▶ install_mcp_server(carried_manifest, target)   // NOT install_plugin
          └─▶ merge_server(target .mcp.json, name, spec)   // atomic, order-preserving
                ├─ Inserted        ─▶ "✓ wrote '<name>'"  + restart hint
                └─ SkippedExisting ─▶ "ℹ '<name>' already configured — left unchanged"
```

## Trust & safety

- Every catalog entry is a **maintainer-reviewed PR** to the public repo (the human trust gate).
- The confirm **always shows the exact command + args + env keys + the target file** before any
  write — installing an mcp server means zoid will **spawn that command at next startup**, so the
  confirm is the informed-consent point. The §C id-match guarantees the card matches the selected row.
- **Default target = user** (`<user_dir>/mcp.json`): private, and imposes on no one. The **project**
  option (`cwd/.mcp.json`) writes into a conventionally *tracked* file that every collaborator's zoid
  would `discover` and spawn on next start — so the confirm labels project explicitly as "your repo's
  tracked `.mcp.json` (shared with collaborators)". The installer chooses per-install; the safe
  option is the default.
- **No secrets in the catalog:** manifests carry `${VAR}` placeholders; zoid writes them verbatim and
  never prompts for or persists secret values. Unset `${VAR}`s are non-blocking warnings.
- **Additive, non-destructive, atomic:** a name collision is skipped (never overwritten); a malformed
  target aborts the write; the write is temp-file + `rename` so a crash can't truncate the file;
  siblings and unknown keys are preserved in original order.
- **stdio-only** enforced at validate; http never reaches the writer.

## Error handling

| Situation | Behavior |
|---|---|
| Name already in target `.mcp.json` | `SkippedExisting` — report "left unchanged", write nothing |
| Target file is malformed JSON (non-object root / non-object `mcpServers`) | Abort write, report; never overwrite |
| Target file / parent dir missing | Treat as empty; create dir + file on write |
| Crash mid-write | Temp-file + `rename` → original intact |
| `${VAR}` unset at confirm | Non-blocking `⚠ not set`; write proceeds |
| Manifest `<id>.toml` 404 / parse / validate fail on Enter | `Confirm` shows `[fetch failed: …]`; nothing written |
| Fetch result arrives after user navigated away / closed | Dropped by mode+id guard |
| `kind` mixes mcp with mode/skills | Rejected at `validate` |
| `[mcp.servers]` empty or has >1 server | Rejected at `validate` |
| `[source]`/`[mode]` present on mcp manifest | Rejected at `validate` |
| http/sse or missing `command` | Rejected at `validate` (stdio-only) |

## Testing

- **Manifest parse/validate** (`zoid-plugin`): parse one `[mcp.servers.<name>]` (command/args/env);
  validate accepts `["mcp"]` with one server; **rejects** mixed `["mcp","skills"]`, empty servers,
  >1 server, `[source]`/`[mode]` on mcp, a server missing `command`. mode/skills validate unchanged.
- **`merge_server`** (`zoid-mcp`): insert into missing file (creates dir+file, one server); insert
  alongside siblings (siblings preserved **in original order** — assert exact bytes, catching a
  reordering regression); skip on collision (`SkippedExisting`, bytes byte-identical); malformed
  existing → error, file untouched; `${VAR}` written verbatim; atomicity via a temp-file assertion
  (no partial artifact left on the happy path).
- **Confirm state machine** (`zoid-tui`): mcp Enter → `ConfirmLoading`; `McpManifestFetched` Ok →
  `Confirm` with `McpConfirm`; Err → fetch-failed render; a fetched result for a non-selected id is
  dropped; `u`/`p` toggle target; env-warn reflects set/unset **and** literal-vs-placeholder; `y`
  emits the mcp install action; `n`/`Esc` back to list. mode/skills confirm path unchanged.
- **Catalog filter** (`zoid`): a synthetic index with mode+skills+mcp surfaces all three; mcp badge.
- **Fixtures:** a sample `github.toml` (mcp kind, one server) + a fixture `.mcp.json` with a
  pre-existing server (collision + sibling-preservation tests).
- Network fetch itself is not unit-tested (mirrors `github_fetch`/catalog).

## Repos touched

- **`zoid` (private):**
  - `crates/zoid-plugin/src/manifest.rs` — `[mcp]` parse, `McpServerSpec`/`McpManifest`, validate
    `mcp` arm (exclusive kind, one server).
  - `crates/zoid-mcp/src/config.rs` — `merge_server` + `MergeOutcome` (first `.mcp.json` writer;
    atomic, order-preserving); enable `serde_json/preserve_order` in `zoid-mcp/Cargo.toml`.
  - `crates/zoid/src/agent.rs` — `AgentUpdate::McpManifestFetched { id, res }`.
  - `crates/zoid/src/main.rs` — `map_catalog_entries` includes mcp; confirm-time fetch spawn +
    `apply_mcp_manifest_fetched` (id-guarded); `install_mcp_server`; kind-aware `CatalogConfirmYes`
    (must not re-enter `install_plugin`).
  - `crates/zoid-tui/src/{state,render,route}.rs` — `CatalogMode::ConfirmLoading`, `McpConfirm`/
    `McpTarget`/`EnvWarn`, target toggle, mcp confirm + loading + fetch-failed render arms.
- **`zoid-releases` (public):** a first real `mcp` manifest (`plugins/<id>.toml`) once the feature
  ships; CI regenerates `index.json` (the `kind` array already flows through `gen_index.py`
  unchanged). **No private internals leak into this repo.**

## Open follow-ups (not in Spec 2.5)

- Hot-connect a just-installed server without restart (needs an `McpManager` single-server connect).
- Uninstall / edit an MCP server from within zoid.
- HTTP/SSE MCP transport (a real `HttpTransport` behind the existing trait) — Spec 3.
- Multiple servers per manifest + per-server selective install from large bundles — Spec 3.
