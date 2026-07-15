# Changelog

> **Internal engineering changelog.** This file is the detailed development
> record and is **not** published. It lives under `docs/` specifically so
> cargo-dist does not detect it as a root changelog. Customer-facing release
> notes (what ships to the public releases repo) live in the root
> `RELEASES.md`.

## 0.5.0

Plugin distribution lands: a deterministic Claude-plugin converter (Spec 1), a
community catalog served from the public `zoid-releases` repo (Spec 2), MCP
catalog entries + the first `.mcp.json` writer (Spec 2.5), and a runtime
mouse-capture toggle. ~68 commits since v0.4.0; only ~42 touch `crates/` (the
rest are specs/plans and marketing-site work that does not ship in the binary).

Claude-plugin importer (Spec 1). Spec:
2026-07-13-claude-plugin-importer-design.md.
- New `zoid-plugin-import` bin crate (`publish = false`, maintainer-only — it is
  how catalog manifests are authored, never an end-user surface): parses Claude
  `marketplace.json` / `plugin.json`, pure capability classification
  (`classify.rs`), emits a validated `plugin.toml` + normalized `mcp.json`
  (`emit.rs`), github tree/blob fetch + `git ls-remote` sha resolve, bulk/repo
  front-ends, golden round-trip tests.
- Hardening: `git ls-remote` argv-injection guard, error on truncated tree,
  TOML-escape of plugin descriptions (backslash/control chars), sha-slice panic
  guard, all emit fields escaped.
- Plugin generalization: `zoid-plugin` manifest gains a `skills` kind (no mode
  overlay) and optional `[mode] body_intro`/`body_outro`; the mode body is now
  manifest-driven with a generic name/repo default (Superpowers is no longer
  special-cased in the body text).

Skills-kind installs.
- `install_skills_plan` lands packs in per-pack private dirs
  (`<config_dir>/skills/<plugin-id>/<skill>/SKILL.md`) with their own
  `.zoid-plugin.json` sidecar, so installing pack B cannot delete pack A
  (regression-tested); the skill scanner (`skill_import.rs`) now discovers both
  `<root>/<skill>/SKILL.md` and `<root>/<pack>/<skill>/SKILL.md`.
- `--mode` / `--skills` override at the `:plugin install` dispatch
  (`parse_plugin_install_args`, last flag wins); `--skills` clears the mode
  recipe so no `mode.md` is written.
- Honest skills-install status: reports `(<n> skills). Restart zoid to load
  them.` instead of the mode-activation messaging (skills packs are built into a
  registry at startup and cannot hot-reload).

Community plugin catalog (Spec 2). Spec: 2026-07-13-plugin-catalog-design.md.
- New `crates/zoid/src/catalog.rs`: index types + pure parse, raw
  unauthenticated `raw.githubusercontent.com` URL builders
  (`catalog_index_url`/`catalog_manifest_url`), and a 24h TTL cache with
  injected clock/env/fetcher (testable seam) plus stale-cache fallback on
  network failure. `store_and_parse` parses before writing, so a malformed
  `index.json` never clobbers a good cache.
- `Overlay::PluginCatalog` browse + provenance confirm gate (`repo@sha`, kind,
  license) built from already-loaded index data, so the card is instant and
  nothing is fetched or written pre-consent. Bundled ids still resolve locally
  first and skip the catalog. Catalog ids resolve to a manifest carried through
  `PluginScan`.
- `contrib/zoid-releases-catalog/`: the publishing kit (`gen_index.py`,
  `catalog-index.yml` CI workflow, seed manifest, contributor README) copied
  into the public repo; `index.json` is CI-generated there from `plugins/*.toml`
  and must not be hand-edited. The local seed is a seed only — it drifts from
  the live index by design.
- Index entries may omit `[source]` (mcp rows carry none).
- `:plugin list` now opens the **same overlay, read-only**
  (`PluginCatalogState::loading_read_only()` → `read_only: bool`), instead of
  joining every entry into one `status_hint` line. That hint is a single `Span`
  in the status bar's `left` vec (`render.rs:395`) — embedded newlines do not
  wrap, so a listing never fit there; the overlay is the only surface that
  renders one row per entry. `read_only` gates the `Enter` key route, **both**
  confirm doors in `PluginCatalogState` (`enter_confirm` for mode/skills rows
  and `begin_confirm_loading` for mcp rows — the dispatcher branches between
  them, so gating only the former would leave mcp rows reachable), and the
  `CatalogEnterConfirm` handler itself (so a read-only listing can never spawn
  the mcp manifest fetch). Footer swaps to `↑↓ scroll · esc close`. Mouse needs
  no gate: `route_mouse` already no-ops every click while an overlay is up.
- `apply_catalog_loaded`'s status-hint fallback is deleted: both openers now
  raise the overlay *before* spawning the load, so a result arriving after the
  overlay closed is stale and dropped.
  Fixes two doc comments that described behavior the code never had
  (`spawn_catalog_load` claimed `:plugin list` "prints rows to the scrollback").
  **Design-doc conflict, unresolved:** the 2026-07-09 plan (§1811) specs
  `:plugin list` as a read-only overlay of *installed* plugins, while Spec 2
  (§169) specs it as printing the *catalog*. This change implements Spec 2's
  content on the plan's surface. A genuine no-TTY scripting surface would be a
  `zoid plugin list` CLI subcommand writing stdout — a `:` command inside the
  TUI can never be one. Deferred.

MCP catalog entries (Spec 2.5). Spec: 2026-07-13-mcp-catalog-design.md.
- `zoid-plugin`: `[mcp]` manifest kind — parse + exclusive-kind validate; a
  server with an empty `command` is rejected (stdio requires an executable).
  Exactly one stdio server per manifest; http/SSE rejected at validate
  (`zoid-mcp` ships only `StdioTransport`).
- `zoid-mcp::config::merge_server` — the **first `.mcp.json` writer** (the
  module was read-only until now): additive, atomic (temp + rename),
  order-preserving (`serde_json/preserve_order`), skip-on-name-collision.
  Unknown top-level keys survive; a malformed target aborts without
  overwriting.
- Confirm-time manifest fetch (the full command is not in the index): an
  id-guarded carrier (`AgentUpdate::McpManifestFetched`) so a slow fetch for row
  A can never populate row B's confirm card. TUI gains `ConfirmLoading` /
  `McpConfirm` states + a `u`/`p` target toggle (default user →
  `<config_dir>/mcp.json`; project → `<cwd>/.mcp.json`), rendering the exact
  command and `env: KEY = ${VAR}` lines with a `⚠ not set` marker for absent
  vars. `${VAR}` placeholders are written verbatim — no install-time secret
  entry, no plaintext secrets written.
- No hot-connect: `install_mcp_server` reports a restart hint; the server
  connects on next startup via the existing `discover` + `spawn_connect_all`.
- Fix: the catalog confirm-error pane dismisses on `y` instead of re-entering
  `install_plugin`.

Select mode (runtime mouse-capture toggle). Spec:
2026-07-13-zoid-select-mode-design.md.
- `:select` / `:mouse`, `Alt+M`, and a state-aware command-palette row toggle
  `shell.select_mode`; the run loop reconciles terminal mouse capture against it
  each iteration. While on, zoid's mouse features (click-to-copy code blocks,
  choice clicks, scroll routing) are suspended and the terminal's native
  drag-select/copy works unmodified. Not persisted — capture is always on at
  startup.
- Always-visible SELECT status pill, a purple sibling right of the mode pill
  (ON = `BRANCH` glyphs on `SELECT_BG` fill; OFF = `DIM`, no fill), so mouse
  state is never ambiguous. Style asserted by test.

Site / docs (not shipped in the binary).
- §2 reworked into a real onboarding→Superpowers-mode player; extensibility
  scene added; both scenes re-captured at v0.4.0; copy refreshed; tools-models
  picker re-seeded with OpenCode Zen. Full-bleed breakout removes frame
  scrollbars on desktop; install one-liner fit-and-centered.
- **Known drift:** `public/` still renders the v0.4.0 status bar and beta chip;
  the site is decoupled from the tag and is refreshed separately.

Known gaps (specced, deliberately not shipped in 0.5.0).
- Stale-cache indicator: the catalog falls back to a stale cache offline, but
  the overlay footer is hardcoded (`↑↓ select · ↵ install · esc close`), so
  offline-with-cache is visually identical to online. Spec §Error handling asked
  for `(cached)` / last-updated.
- The MCP confirm card shows neither the resolved target paths nor the spec's
  "shared with collaborators" warning for a tracked project `.mcp.json`; the
  path only appears post-write in the status hint.
- No select-mode transient status hint (`toggle_select_mode` only flips the
  bool); the pill is the only feedback.

## 0.4.0

Largest feature release since 0.2.0: three new model providers + a large model
registry, a manifest-driven plugin system, agent-schedulable wake-ups, subagent
guardrails/cancellation/verification, worktree tooling correctness, and inline
edit diffs. ~190 commits since v0.3.2 (marketing-site/web-capture commits are
docs-only and do not ship in the binary).

Providers & models.
- New `OpenAiResponsesProvider` (`crates/zoid-provider/src/openai.rs`): request
  body builder, SSE parse, streaming accumulator, `Provider` impl.
- New Google `GeminiProvider` (`crates/zoid-provider/src/gemini.rs`):
  `request_body`, `parse_chunk`, `Provider` impl.
- New `OpenCodeZenProvider` with 4-way wire-shape routing + secret
  prettification (`zai` etc.) and bin wiring.
- `zoid-model` registry gains the opencode-zen entry and real model caps (~52
  models total across providers).
- Context-economy re-assertion: `[economy].reassert_interval_tokens` (default
  100k, 0 disables); `TurnConfig.reassert_interval` + `wrap_reassertion`; turn
  loop fires a preflight-accounted, calibration-safe re-floor; providers render
  the reassert as a trailing system message (ollama-native, openai-compat/zai).

Plugin system.
- New `zoid-plugin` crate (pure, IO-free schema + planning): `manifest`,
  `resolve`, `plan`, `effect` (with `classify_config_key`/`RiskTier` gate),
  `provenance`, `bundled`. Spec: 2026-07-09-zoid-plugin-support-design.md.
- Effectful installer core wired into the main loop via `PluginScan`:
  `:plugin install` command + command-palette row + onboarding retargeted to
  `:plugin install superpowers`. Superpowers ships as a bundled, byte-identical
  manifest (golden-guarded). Provenance sidecar `.zoid-plugin.json`. `SetConfig`
  rejected at the v1 effect gate.

Scheduled wake-ups (Spec 3 of subagent-dispatch-safety).
- `schedule_wake`/`cancel_wake` Emitting tools (main-Chat-only) with
  floor/cap/enabled guards and an i64-overflow delay cap.
- `WakeScheduled`/`WakeFired`/`WakeCancelled` events + pending-wake projection;
  `[wake] enabled` config (`WakeConfig`, layered merge + provenance);
  watch-driven watcher task with rebuild-on-load; due wakes fire by injecting a
  `UserMessage` + `WakeFired`, with catch-up on load and drain at
  `TurnComplete`.

Subagent safety, reliability & verification.
- Guardrails: `cancel_subagent` kill tool + `fire_subagent_kill` handler; Esc
  escalation kills subagents (no-turn armed confirm); the subagent observes the
  hard token during streaming and at top-of-turn so guardrails stop a parked
  subagent; `WakeTimer` timeout supervisor + `[subagent]` idle/hard timeout
  config; registry handle types + heartbeat; `in_flight` HashSet→HashMap.
- Fixes: repaired 400s + TUI corruption on dispatch; wake the idle orchestrator
  when a `DelegationResult` arrives.
- Tool-execution verification (this release's final feature): `verify_execution`
  in `crates/zoid/src/subagent.rs` computes orphan `ToolCall`s (call id with no
  matching `ToolResult`) and tool-call count; `distill` flips `ok=false` + notes
  on orphans, and appends an advisory note (ok unchanged) when a subagent
  emitted zero tool calls. Guarded by `assembled_tools_exclude_emitting`.

Worktree tooling correctness (Spec 2 — WT-1..WT-4).
- Synchronous enter/exit switch so commits land on the worktree branch and exit
  keeps tooling alive (WT-1/WT-2); worktree-aware, change-driven git poller so
  the rail reflects the active worktree (WT-3); redundant hints dropped (WT-4).

TUI.
- Inline edit diffs: `edit`/`write` attach an ephemeral `FileDiff` to
  `ToolOutput`; capped line-diff core (`compute_file_diff`); agent forwards
  ephemeral edit diffs to a bounded in-memory TUI cache; render `+N −M` counts
  and inline last-K edit diffs; `[ui] edit_diff` toggle + `edit_diff_inline`
  (K). Adaptive table column widths (fit natural, shrink widest first).

## 0.3.2

Discoverability: an in-app keyboard-shortcuts help overlay.

Help overlay.
- New `Overlay::Help` (`crates/zoid-tui/src/help.rs`): a bordered, read-only, scrollable shortcuts reference on its own centered 84×26 rect (`HELP_RECT_W`/`HELP_RECT_H`), clamped to the conversation area by `layout::centered`, mirroring the read-only `/mcp` overlay pattern. Content is produced by a single pure `help_lines() -> Vec<Line<'static>>` builder (6 sections, 31 rows) so it stays unit-testable and edited in one place. Registered at both compiler-blind overlay seams — `layout.rs`'s rect branch and `render.rs`'s if/else dispatch — with the layout guard-test array extended to `Overlay::Help` so a missed seam fails a test instead of rendering nothing.
- Three open paths, all resetting overlay state consistently: `?` routed only in the `Focus::Conversation` arm (so a literal `?` typed into the input box is preserved), the `:help` command (`Command::OpenHelp`, parsing both `:help` and bare `help`), and a "Keyboard shortcuts…" command-palette row plus a `help` completion in the `:` direct phase. `Esc`/`q` close via `close_overlay()`, which resets `help_scroll`.
- Scroll has a single source of truth: `Action::ScrollHelp(i32)` only increments `help_scroll` (saturating at 0, no ceiling); the bin clamps it per-frame against the real rect height (`saturating_sub(2)` for the borders, reusing the frame's already-computed layout), mirroring how `conv_max_scroll` bounds conversation scroll, so it can never run past the last page.
- Empty-state hint (`Press ? (or run :help) for keyboard shortcuts`, `CHAT_ACCENT`) added to both the new-user and returning-user onboarding screens.

## 0.3.1

Fixes.
- Startup progress no longer staircases after the session picker. The picker (`main.rs` `BootPath::Picker`) calls `enable_raw_mode()` and deliberately leaves it on through launch (it re-enters the alt screen for `run()`), so `ONLCR` is off; the Reporter's bare-`\n` line endings (`writeln!`) then dropped a row without returning the carriage, indenting each subsequent step further right. Manifested only on launches that show the picker (i.e. when prior sessions exist) — first-run launches print in cooked mode and were unaffected. `crates/zoid/src/startup.rs` now emits explicit `\r\n` via a single `newline()` helper across `write_line`/`progress_done` (correct in raw mode, harmless in cooked mode). Regression test asserts no bare `\n` survives.

## 0.3.0

Second beta feature drop: a one-action Superpowers mode install, startup progress feedback, a data-removal command, and default/UX fixes.

Superpowers mode install.
- Deterministic, model-free install of the canonical `obra/superpowers` skill set as a first-class zoid mode (`crates/zoid/src/superpowers_install.rs`). Reuses the URL-import wizard's fetch + materialize; the only bespoke logic is a pinned upstream ref (frozen commit for reproducibility), a deterministic mapping (`skills/using-superpowers/SKILL.md` → generated `mode.md` overlay; every other `skills/<skill>/**` file copied verbatim with the `skills/` prefix stripped), and the generated `mode.md` body.
- Surfaced three ways: a `:mode install superpowers` palette command + row, a first-run onboarding install line, and an async install orchestrator wired into the main loop (materialize + reload without restart). Test coverage over the `SuperpowersScan` arm.

Startup progress feedback.
- TTY-gated `Reporter` (`crates/zoid/src/startup.rs`, gated via `std::io::IsTerminal`): a launch banner (`zoid vX.Y.Z`), step lines ("opening session store", "loading session", "building skills & modes", "loading semantic model"), and a live download progress readout for the first-run model-weight fetch. Pure `format_progress(downloaded, total)` helper unit-tested.
- Streaming/atomic weight download (`crates/zoid-embed/src/fetch.rs`): `ensure_weights_with_progress` streams to a `.part` sidecar with incremental sha256 and throttled (200ms) progress callbacks, then atomic-renames after verification (replaces the old buffer-all-130MB-then-verify path). New `ProgressFn` type; `load_with_progress` threads the callback through `zoid-embed`.

`zoid uninstall`.
- New `Uninstall { purge }` CLI variant (`crates/zoid/src/cli.rs`) + `crates/zoid/src/uninstall.rs`: removes the data, config, and cache dirs after a typed `uninstall` confirmation; `--purge` also removes the binary (degrades gracefully when a running exe can't remove itself). Data dir derived from the XDG base (independent of `$ZOID_DB`); a guard refuses to `remove_dir_all` any path whose final component isn't `zoid`. Testable core (`run_with_io` over `&mut dyn BufRead`/`&mut dyn Write`).

Defaults & UX fixes.
- Config defaults: `context_target` 300k, `compact_threshold_pct` 80.
- TUI: bracketed paste routed through focus/overlay precedence.
- Wizard: don't clear `app.wizard` on Reject and guard the Approve path; `render_scan` trims non-`SKILL.md` files to path+size for faster scans of large folders.
- Tools: tool names lowercased for convention consistency.

Release docs.
- Split public notes (root `RELEASES.md`) from this internal changelog (moved to `docs/`), added `AGENTS.md`, updated `docs/RELEASING.md`.

## 0.2.0

The first feature release since the distribution pipeline landed — a large batch of new capabilities for beta testing.

Active Context Management (ACM).
- Demand-paged context: tool-result compaction with band-holding eviction (a pre-flight gate plus a hysteresis controller) keeps long sessions inside the model's window instead of truncating blindly.
- Cold-tier recall: evicted content is re-admitted on demand via a session-scoped FTS/BM25 `recall` tool, so paged-out context stays reachable.
- Local semantic recall (opt-in `local-embed` feature, baked into release binaries): a pure-Rust `candle` embedder (bge-small-en-v1.5, 384-dim) indexes events in an in-memory ring, and hybrid recall fuses FTS and vector candidates via Reciprocal Rank Fusion. Weights are checksum-verified on first fetch; the whole path is gated by a cargo feature and an `[embed]` config section.

MCP client support.
- zoid is now an MCP client: tools from external MCP servers configured in `.mcp.json` (project `./.mcp.json` + user `~/.config/zoid/mcp.json`, `${VAR}`-expanded, project wins) appear alongside built-ins, namespaced `server__tool`.
- Hand-rolled JSON-RPC-over-stdio client (no SDK dependency): background connect with per-server timeouts, crash/disconnect detection, and a read-only `/mcp` status overlay. Trust-on-configure — a configured server's tools run like built-ins.

Modes & skills.
- Import any skill set as a first-class mode and switch with Shift+Tab; mode promotion plus an `Alt+`-style quick-switch.
- `SKILL.md` source adapter/importer and a URL import wizard for pulling external skill definitions.

Filesystem toolset (Claude Code parity).
- `Read` (offset/limit paging, line numbers, per-line cap), `Write`, `Edit` (`replace_all` + atomic multi-edit), `Grep` (regex + glob filter + output modes), `Glob`, and `LS`.

Reasoning & thinking modes.
- Extended-thinking controls surfaced in the TUI (thinking markers, session-drawer effort indicator) with per-provider wiring.

Escalating interrupt (Esc).
- First Esc is graceful (abandons in-flight network/MCP waits, stops new tools); a second Esc hard-stops by `SIGKILL`-ing the running `shell` command's whole process group. Every started tool call still gets a synthesized result, preserving the request/response balance.

Feedback tool.
- Built-in `submit_feedback` tool + `:feedback` command + skill that files a GitHub issue (token or browser fallback) with optional diagnostics.

Providers & operations.
- Added the `opencode-go` provider (per-model wire routing) alongside the Ollama Cloud native and typed Anthropic paths.
- Multi-instance safety: SQLite WAL + stateful sessions so concurrent instances don't corrupt state; a CWD-scoped startup session picker with `--new`/`--resume`.
- Optional `zoid-companion` localhost metrics dashboard + push-card channel; an observability/Overview page.
- Best-effort build-expiration tripwire so leaked pre-release builds stop running after 30 days (`--version`/`--help`/`zoid update` remain ungated).
- TUI/UX: VSCode-style command palette redesign (flat, search-first, inline Rename), inline question cards replacing the modal overlay, status-bar indicators, `:compact` command, empty-state onboarding, and direct-phase autocomplete. Upgraded to ratatui 0.30 (clears advisory RUSTSEC #26).

Settings redesign.
- Full-screen three-column settings (sections · fields · contextual picker) replacing the cramped card; baseline 160×40 with graceful degradation.
- Visible provider/model picker (Miller-column cascade) replacing the blind cycle; selecting a provider seeds `base_url` from the registry and jumps to model selection.
- Transport-aware provider registry: `ollama-local` / `ollama-cloud` split, `anthropic-api`. Legacy `ollama`/`anthropic` ids alias to the new canonical ids. (The `anthropic-cli`/`anthropic-sdk` `[planned]` seam entries were removed — falsified by the `spikes/cc-infer` spike, which proved `claude -p` is an agent, not an inference endpoint.)
- Live model discovery: the model picker fetches available models from the provider (Ollama `/api/tags`, Anthropic `/v1/models`), falling back to the registry list offline. Selecting a key-requiring provider prompts for the API key before fetching.
- `Alt+P` quick-switch overlay for changing provider + model mid-session.

Anthropic provider flesh-out.
- Typed internal submodule (`crates/zoid-provider/src/anthropic/{types,request,parse,cache,mod}.rs`) replacing the hand-rolled `json!` wire format. Serde structs for the Messages API request (`ContentBlock` union: Text/ToolUse/ToolResult/Thinking; `StreamEvent` tagged enum for SSE responses) make the wire format greppable, compile-checked, and maintainable without an external community crate.
- Tool-use parity with the Ollama provider: sends the `tools` array, parses `tool_use` content blocks across `content_block_start` → `input_json_delta`* → `content_block_stop` (via a per-stream `ToolUseAccumulator`), maps `tool_result` messages with the `tool_call_id → tool_name → empty` fallback chain. Flips `MODEL_CAPS` `tools: true` for `claude-sonnet-4-6` and `claude-opus-4-8`.
- Connect-phase 429 retry: bounded `MAX_RETRIES=3` with `retry-after` header + exponential backoff (`BASE_BACKOFF=500ms`) + wall-clock jitter. Mid-stream overload stays terminal.
- `anthropic-beta` header plumbing: `with_betas(Vec<String>)` builder, config/env-populated, comma-joined. Makes beta features (extended thinking, fine-grained tool streaming) opt-in without code changes.
- Extended-thinking parsing: `ThinkingDelta`/`SignatureDelta` typed and discarded from the event stream (config-gated off via `AnthropicRequest.thinking: None`; replay across compaction is a deferred follow-up).
- Prompt-cache breakpoints preserved: `place_breakpoints` converts the last plain-text message to a cacheable `Blocks([Text{ cache_control: Ephemeral1h }])`, mirroring the legacy rolling-breakpoint behavior.

## 0.1.2

Release-pipeline hardening (no functional binary changes).
- The byproduct GitHub release in the private source repo is now auto-deleted after each release (its git tag is kept); anonymous users only ever see the public releases repo with working install URLs.

## 0.1.1

Release-pipeline fix (no functional binary changes).
- Public release notes now carry the full install instructions (shell + PowerShell one-liners and the download table) with download URLs pointed at the public releases repo.
- First release to exercise the `zoid update` self-update client path (updating from 0.1.0).

## 0.1.0

First distributed release.
- Prebuilt binaries for Linux (x86_64 static musl), macOS (Apple Silicon), Windows (x86_64).
- `zoid update` — anonymous, checksum-verified self-update.
- `zoid --version` / `zoid --help`.
