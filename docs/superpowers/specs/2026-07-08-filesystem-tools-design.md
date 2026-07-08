# Filesystem tools — design

> **Status:** design (settled, ready for implementation planning). Supersedes the
> minimal file tools described inline in `crates/zoid-tools/src/{read,write,edit,search}.rs`
> and the P1b tools plan (`docs/superpowers/plans/2026-06-29-p1b-tools-and-tool-calling.md`).
> Implementation should follow this document and update it.

## Goal

Give the zoid agent a **mature, first-class filesystem toolset** so it rarely
needs to fall back to `shell` for routine file work, with ergonomics that match
what models already expect from Claude Code. Three drivers, all pointing the
same way:

1. **Harden/expand** the existing tools, which are deliberately minimal.
2. **Reduce shell reliance** — directory listing, glob, and regex search
   currently require `shell`, which is opaque and routes through the approval
   path.
3. **Claude Code parity** — match CC tool names and semantics so model behavior
   (especially line-referenced edits) transfers cleanly.

### Guiding constraint

zoid's default provider is **glm-5.2:cloud** (Ollama), a smaller model than the
frontier models Claude Code targets. Tool-calling reliability degrades as the
tool count and schema complexity grow. So the design keeps the FS surface tight
(six tools, not a dozen), folds multi-edit into `Edit` rather than adding a
seventh tool, and keeps every parameter schema simple and CC-shaped.

## Scope

Rename and redesign the four current FS tools into a six-tool Claude-Code-parity
set, in one coordinated pass (no aliases — see Migration):

| New tool | Replaces      | Kind  |
|----------|---------------|-------|
| `Read`   | `read_file`   | Local |
| `Write`  | `write_file`  | Local |
| `Edit`   | `edit_file`   | Local |
| `Grep`   | `search`      | Local |
| `Glob`   | *(new)*       | Local |
| `LS`     | *(new)*       | Local |

### Out of scope (v1)

- **Path jailing / sandboxing.** The current tools deliberately do not jail
  paths (`lib.rs:2-3`, `lib.rs:154-155` — "safe by human presence"). Changing
  that is a separate security decision; this design preserves the existing
  stance and does not add validation.
- **Binary / image reads.** Tools remain UTF-8 text only; non-UTF-8 fails with a
  clear error, as today.
- **A separate `MultiEdit` tool.** Multi-edit is folded into `Edit` via an
  `edits[]` array.

## Current state (the seam)

File operations already have dedicated, non-shell handlers (confirmed inventory):

- **Tool trait** (`crates/zoid-tools/src/lib.rs:64`): `name()`, `spec() -> ToolSpec`,
  `run(&Value, &Path) -> ToolOutput`, `kind() -> ToolKind`.
- **`ToolKind`** (`lib.rs:53`): `Local | Emitting | Interactive | Mcp`.
- **Registry** (`lib.rs:76`/`91`): a compiled-in `Vec<Box<dyn Tool>>`, not a
  dynamic registry.
- **Dispatch**: `run_tool` (`lib.rs:138`) linear-finds by name and calls
  `t.run()`; Local tools run via the default arm in `agent.rs:1422` inside
  `spawn_blocking` with hard-stop cancellation.
- **Schema to provider**: each `spec()` carries raw JSON Schema; `tool_specs()`
  (`agent.rs:169`) maps the registry into `CompletionRequest.tools`.
- Existing behavior being replaced:
  - `read_file` (`read.rs`): whole-file `read_to_string`, no offset/limit/cap.
  - `write_file` (`write.rs`): unconditional overwrite.
  - `edit_file` (`edit.rs`): single unambiguous `old`→`new` replace.
  - `search` (`search.rs`): literal substring walk, `MAX_RESULTS = 200`, skips
    hidden + `target`/`node_modules`, no symlink following.

The new tools reuse this entire mechanism — no new dispatch, kind, or gate
machinery. They are all `Local` and flow through `run_tool` unchanged.

## Tool specifications

Output convention: all listing/reading tools have a hard ceiling and, on hitting
it, append a **truncation notice** telling the model how to get the rest. This
is the context-safety backbone — it makes the tools context-aware rather than
context-bombs.

### `Read` — `{ path, offset?, limit? }`

- Reads a UTF-8 text file. `offset` (1-indexed start line) and `limit` (line
  count) page through large files.
- Output is `cat -n`-style: 1-indexed line number + tab + line text. This
  matches Claude Code so the model's line references align with `Edit`.
- Default cap ~2000 lines (and a byte ceiling, e.g. ~256 KB); exceeding it
  truncates and appends: *"… truncated; continue with offset=N"*.
- Non-UTF-8 → error. Missing file → error.

### `Write` — `{ path, content }`

- Creates or unconditionally overwrites a UTF-8 file. Returns bytes written.
  Unchanged behavior under a parity name.

### `Edit` — single or multi

- Single: `{ path, old_string, new_string, replace_all? }`.
- Multi: `{ path, edits: [ { old_string, new_string, replace_all? }, … ] }`.
- Each edit preserves today's unambiguous-match rule: `old_string` must occur
  exactly once (unless `replace_all`), else the edit errors (`not found` /
  `ambiguous`).
- Multi-edit is **atomic**: edits apply sequentially to in-memory content; if
  any fails, the file is left untouched and an error is returned. A single write
  commits the result.

### `Grep` — `{ pattern, path?, glob?, type?, -A?, -B?, -C?, output_mode?, -i?, multiline? }`

- Regex search (via the `regex` crate) over files under `path` (default cwd).
- `glob` (include filter, e.g. `*.rs`) / `type` narrow the file set; the
  existing hidden + `target`/`node_modules` skip list and no-symlink cycle guard
  are retained.
- `output_mode`: `files_with_matches` (default, protects context) | `content`
  (line-numbered hits, honoring `-A/-B/-C` context) | `count`.
- `-i` case-insensitive; `multiline` allows `.`/patterns to span lines.
- Retains the 200-match ceiling with a truncation notice.
- **v1 cut (fast-follow):** `-A/-B/-C` context lines, `multiline`, and the
  `type` filter are deferred; v1 ships `pattern`/`path`/`glob`/`-i`/`output_mode`.
  `glob` covers the common file-filtering need. See the implementation plan.

### `Glob` — `{ pattern, path? }`

- Filename pattern matching (e.g. `**/*.rs`) under `path` (default cwd), results
  sorted by modification time (newest first), capped with a truncation notice.

### `LS` — `{ path, ignore? }`

- Lists directory entries with type (file/dir/symlink) and size. `ignore` is an
  optional list of glob patterns to omit. Respects the same skip list as `Grep`.
  Capped with a truncation notice.

## Cross-cutting

### Approval-gate wiring

All six are file tools, not `shell`, so they remain outside the blacklist. Only
the tiered name lists in `crates/zoid-tools/src/approval.rs` change:

- Never-prompt (read-only) tier: `Read`, `Grep`, `Glob`, `LS` (replacing
  `read_file`, `search`).
- Allow-by-default tier: `Write`, `Edit` (replacing `write_file`, `edit_file`).

No behavior change — renamed cases only. The gate's `shell` inspection is
untouched.

### Errors & dispatch

All `Local`; dispatched through the existing `run_tool` → `t.run()` path inside
`spawn_blocking` with hard-stop cancellation. Errors remain `ToolOutput` error
strings with a clear, recoverable message (missing path, ambiguous edit, invalid
regex).

### Dependencies

Add to `zoid-tools`:

- `regex` — `Grep` patterns.
- A glob matcher (`globset` preferred for compiled include/exclude sets, or
  `glob`) — `Glob` and `Grep`'s `glob`/`ignore` filters. Exact crate pinned at
  plan time.

## Migration (hard rename, no aliases)

Two names for one tool measurably hurts tool selection on smaller models, and
zoid is pre-1.0, so a clean cutover beats a deprecation window. One coordinated
pass:

1. `zoid-tools`: rename modules/impls (`read.rs`→`Read`, etc.), add `glob.rs` +
   `ls.rs`, update `registry()` / `registry_with_kill()`.
2. `approval.rs`: rename the tiered tool-name cases and their tests.
3. System prompts / tool descriptions that name the old tools.
4. Projection & TUI: any hard-coded `read_file`/`write_file`/`edit_file`/`search`
   strings (e.g. tool-call rendering).
5. Tests across the workspace referencing the old names.

A pre-commit sweep — `rg 'read_file|write_file|edit_file|"search"'` — confirms
nothing stale remains.

## Testing strategy

TDD, mirroring the existing `approval.rs` test density. Per-tool unit tests in
`zoid-tools`:

- `Read`: offset/limit paging, line-number format, truncation notice on cap,
  non-UTF-8 error.
- `Grep`: regex match, `-i`, context lines, glob/type filtering, each
  `output_mode`, cap + truncation.
- `Glob`: pattern matching, mtime ordering, cap.
- `LS`: entry types/sizes, skip list, `ignore` patterns.
- `Edit`: single replace, `replace_all`, atomic multi-edit success, and a
  failing edit in a batch leaves the file **untouched**; ambiguous/not-found
  errors.
- `Write`: create + overwrite byte count.

Plus:

- A registry test asserting exactly the six FS specs are advertised with valid
  JSON Schema.
- Updated approval-gate tier tests using the new names.
