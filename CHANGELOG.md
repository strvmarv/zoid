# Changelog

All notable changes to zoid are documented here. Each `## X.Y.Z` section
matches a released version tag; `cargo-dist` parses the section matching the
tag being released and uses it as that release's announcement notes.

## 0.9.2

This is a bridge release. zoid is now open source at
[strvmarv/zoid](https://github.com/strvmarv/zoid) under MIT OR Apache-2.0.
This is the last release on `strvmarv/zoid-releases`; future releases live on
`strvmarv/zoid`. After installing this update, `zoid update` will
automatically check the new repo going forward.

## 0.9.1

Subagent dispatch hardening, worktree branch-deletion fix, Windows home-dir
fix, unified logging, Ollama thinking detection, top bar redesign. 44
commits since v0.9.0.

Subagent dispatch hardening (spec:
2026-08-04-subagent-dispatch-hardening-design.md; plan:
2026-08-04-subagent-dispatch-hardening.md).
- `zoid/src/agent.rs`: duplicate-dispatch guard keyed on `(agent, task)`
  that rejects identical `dispatch_subagent` calls before the pool-capacity
  check. A turn-scoped `dispatched_this_turn: bool` latch gates post-dispatch
  free-text narration; a 60-token budget cap trips runaway narration, guarded
  by `!tool_call_seen_this_sub_turn` so a compliant ack+dispatch+follow-up is
  never falsely capped. Reuses the existing `aborted`/`stream_task.abort()`
  cleanup path. `SYSTEM_PROMPT` gets an English-language directive.
- Reviewed by gilfoyle (per-task + whole-branch); two blockers (dead prune,
  false-positive cap) found and fixed in the plan before implementation.

Worktree branch-deletion fix (bug:
2026-08-04-exit-worktree-branch-deletion.md).
- `zoid/src/worktree.rs`: `branch_has_unmerged_commits` used
  `graph_descendant_of(branch, head)` which returns false when main advanced
  while the worktree was active (HEAD diverged from the branch). Replaced
  with a `merge_base(branch, head) != branch` check that handles both cases.
  Diagnostic OIDs in `compute_worktree_switch` moved before `remove_worktree`
  so they're not captured after the branch is already deleted.

Windows home-dir fix.
- `zoid/src/main.rs`: all five path resolvers (`resolve_db_path`,
  `resolve_config_dir`, `resolve_cache_dir`, `resolve_secret_key_path`,
  `uninstall_targets`) fell back to `env("HOME").unwrap_or_default()` which
  is `""` on Windows when HOME is unset — so `.local/share`, `.config`, and
  `.cache` resolved against the CWD. Added a `home_dir()` helper that checks
  `HOME` then `USERPROFILE` and used it in all six resolution sites.

Unified logging with TTL purge (spec:
2026-07-30-unified-logging-design.md; plan:
2026-07-30-unified-logging.md).
- `zoid-core/src/store.rs`: `logs` table in the event store with
  `write_log`/`purge_logs` methods and `Cmd::WriteLog`/`Cmd::PurgeLogs`
  session commands.
- `zoid/src/main.rs`: boot-time purge + ring-buffer flush to the logs table.
- `zoid/src/obs.rs`: logs ring buffer + `FieldGrab` fields collector for
  structured tracing output.

Ollama thinking capability detection (spec:
2026-07-28-ollama-thinking-capability.md).
- `zoid-provider/src/ollama.rs`: `fetch_model_info` reads thinking capability
  from `/api/show`; `parse_ollama_thinking` parses the capabilities.
- `zoid-provider/src/lib.rs`: `resolve_thinking` uses provider-aware default
  for ollama-local; `request_body` trusts the `resolve_thinking` gate for the
  `think` field.

Top bar chip rearrangement (spec:
2026-07-28-top-bar-chip-spec.md).
- `zoid-tui/src/render.rs`: title line rewritten with SELECT+YOLO chips and
  centered version; SELECT pill removed from the bottom status bar.
- `zoid-tui/src/state.rs`: `yolo` field added to `ShellState`, synced to
  `app.yolo` for the renderer.

Local model seed data.
- `zoid-model/src/registry.rs`: curated local model seed data.
- `zoid-core/src/store.rs`: `local_models` db table + curated seed step.
- `zoid/src/main.rs`: seeds the `local_models` table at boot.

AGENTS.md reminder.
- `zoid/src/agent.rs`: `SYSTEM_PROMPT` now directs the agent to read
  `AGENTS.md` before touching anything.

## 0.9.0

First-run onboarding wizard, token-budgeted turn protection, Ollama context
tracking fix, worktree unmerged-commits fix, TUI drawer polish. 40 commits
since v0.8.0.

First-run LLM connection wizard (spec:
2026-07-10-onboarding-llm-connection-wizard-design.md; plan:
2026-07-10-onboarding-llm-connection-wizard.md).
- `zoid-core/src/config.rs`: compiled default for `provider` becomes empty
  string (was `"ollama"`) — the "unconfigured" sentinel.
- `zoid-model/src/registry.rs`: new `key_url: Option<&'static str>` field on
  `ProviderEntry` (the key-acquisition URL shown in the API-key step).
- `zoid-tui/src/render.rs`: new `Overlay::Onboarding` variant + full-screen
  `render_onboarding` wizard view (2–3 step linear flow: Provider → API key →
  Model; step 3 only if provider has >1 registry model).
- `zoid-tui/src/route.rs`: onboarding key routing, Action variants, paste
  target for API-key entry.
- `zoid/src/main.rs`: `wizard_needed` gate predicate
  (`first_time_user && (provider empty || (provider requires key && no key
  found))`, `ollama-local` exempt); boot-time overlay open + state seed; config
  write-back through existing `set_in_toml` + `SecretStore::set` +
  `select_provider` re-selection (no new write paths).
- `fix(onboarding)`: friendly provider name in step-2 prompt; literal glyphs
  replaced with token constants in render.

Token-budgeted turn protection (spec:
2026-07-27-token-budgeted-turn-protection-design.md; plan:
2026-07-27-token-budgeted-turn-protection.md).
- `zoid-core/src/eviction.rs`: `compute_protection` replaces the fixed
  `recent_n` count with a three-layer policy — (1) hard floor of 1 (current
  turn always protected), (2) `min_protected_turns` (default 3) minimum count
  (quality backstop; soft band never overrides), (3) `protection_pct` of
  `low_water` (default 15%) budget ceiling extending protection to additional
  recent turns when cheap. Capacity backstop shrinks `min_protected_turns`
  toward 1 if the protected floor would exceed
  `capacity − CAPACITY_SAFETY_MARGIN`.
- `zoid-core/src/config.rs`: `min_protected_turns` + `protection_pct` fields
  with layered merge. `recent_n` kept as deprecated back-compat alias for
  `min_protected_turns`.
- `zoid/src/main.rs`: wires `min_protected_turns` + `protection_pct` into
  runtime `EvictionPolicy`.
- `crates/zoid-tui/src/config_view.rs`: protected turns + protection % rows
  in the Economy section.
- `fix(eviction)`: per-turn token estimates scaled to match band units
  (calibration mismatch fix).

Ollama context tracking fix (spec:
2026-08-03-ollama-context-tracking-design.md; plan:
2026-08-03-ollama-context-tracking.md).
- `zoid-provider/src/ollama.rs`: on cache-hit turns, Ollama's `done` frame
  reports `prompt_eval_count` as only the uncached tail (tokens actually
  *evaluated*, not tokens served from warm KV cache), so `input_tokens` was a
  tiny fraction of the real prompt. Provider-side reconstruction now
  reconstructs the full prompt size on cache-hit turns (`n3` reconstruction)
  so `ctx_used` / the status bar reflects reality. Cache-miss turns unchanged.
- Tests: deep cache-hit + eviction self-correction tests added.

Worktree unmerged-commits fix.
- `zoid/src/main.rs`: `branch_has_unmerged_commits` resolves main's HEAD (not
  the worktree's HEAD) when checking for unmerged commits on exit, so commits
  on the worktree branch that haven't merged to main are correctly detected
  and the branch ref is retained.

TUI drawer polish.
- `zoid-tui/src/render.rs`: subagents drawer auto-collapses when empty; tasks
  list grows without cap; tasks and subagents drawers swapped in the rail.

## 0.8.0

Eviction master switch + diff line background highlighting. 14 commits
since v0.7.3.

Eviction master switch (spec:
2026-07-28-eviction-master-switch-design.md; plan:
2026-07-28-eviction-master-switch.md).
- `zoid-core/src/config.rs`: new `EvictionConfig.enabled: bool` (default
  true via manual `Default` impl), flowing through `PartialEviction`,
  `Provenance` (new `eviction_enabled` field), and `merge`. Same pattern
  as `wake.enabled`, `companion.enabled`, `thinking.enabled`.
- `crates/zoid-tui/src/config_view.rs`: new Bool row at top of Economy
  section for the `eviction` toggle.
- `zoid/src/main.rs`: new `ConfigToggle` arm (write) and `current_write`
  arm (read-back); `ZOID_EVICTION_ENABLED` env var mirrors
  `ZOID_COMPANION_ENABLED`; `EvictionPolicy.enabled` now reads
  `app.config.eviction.enabled`, replacing the implicit
  `compact_threshold_pct > 0` derivation. Eviction is decoupled from
  compaction.
- `crates/zoid-tui/tests/shell_snapshot.rs` + `main.rs` test fixture:
  all `Provenance` struct literal sites updated for the new field.
- Back-compat blast radius: users who set `compact_threshold_pct = 0`
  had eviction implicitly off; with the new switch defaulting on,
  eviction is silently re-enabled on upgrade. Call out in
  `RELEASES.md`. `compact_threshold_pct = 0` still disables compaction.

Diff line background highlighting (spec:
2026-07-28-diff-background-highlighting-design.md; plan:
2026-07-28-diff-background-highlighting.md).
- `crates/zoid-tui/src/tokens.rs`: new `ADDED_BG`/`REMOVED_BG` color
  constants (distinct from `CHAT_BG`).
- `crates/zoid-tui/src/chat.rs`: add/del diff lines get a full-row
  background band across the gutter, padded to terminal width via
  `saturating_sub`; context lines keep `bg = None` (no visible band).
  New named `GUTTER_W = 12` const.
- Tests: `gutter_width_matches_format_string` (locks `GUTTER_W` to the
  literal), `diff_highlight_band_fills_to_width`, `diff_highlight_clamps_
  when_too_wide`, plus context-line bg and foreground-color assertions.
  Structural span selection (no fragile substring probing).

## 0.7.3

Wake scheduling discipline: prompt hardening + runtime per-note deduplication.
3 commits since v0.7.2.

Wake scheduling prompt hardening (spec:
2026-07-28-wake-scheduling-discipline-design.md).
- `zoid-tools/src/wake.rs`: tool description restructured to include
  "Schedule exactly ONE wake per event" and "Duplicate wakes for the same
  note are rejected." Guard assertions verify both substrings.
- `zoid/src/agent.rs`: `SYSTEM_PROMPT` gains wake discipline sentence so
  `wrap_reassertion` reinforces it. `system_prompt_reinforces_no_poll` test
  extended with wake assertions.
- `zoid/src/agent.rs`: `schedule_wake` tool result now includes nudge
  ("do not schedule additional wakes for the same event... cancel it with
  cancel_wake if you no longer need it").

Runtime per-note deduplication.
- `zoid/src/main.rs`: `handle_schedule_wake` now rejects a new wake if a
  pending wake with the same `note` already exists. Returns an error
  message that tells the model what to do instead ("cancel it first" /
  "wait for it to fire"). `handle_schedule_wake_rejects_duplicate_note`
  test added. The global `WAKE_MAX_PENDING` (16) cap remains as backstop.

## 0.7.2

Projection parallelization + subagent no-poll prompt hardening. 9 commits
since v0.7.1.

Projection cache parallelization (`eaa311c`).
- `zoid/src/main.rs`: `ProjectionCache::refresh` now runs its 5 independent
  O(n) passes (conversation, context_window, churn_timeline, tasks,
  token_ledger) concurrently as scoped threads via `std::thread::scope`.
  Wall-clock from sum(passes) to max(passes). Zero new dependencies
  (std::thread::scope, stable since Rust 1.63).

Subagent no-poll prompt hardening (spec:
2026-07-27-subagent-no-poll-prompt-hardening-design.md).
- `zoid-tools/src/subagent_dispatch.rs`: tool description restructured to
  lead with "Fire-and-forget" framing; rule first, mechanism after.
  Regression-guard assertions verify the description starts with
  "Fire-and-forget" and names `list_subagents` as do-not-call.
- `zoid/src/agent.rs`: `SYSTEM_PROMPT` gains a fire-and-forget sentence so
  `wrap_reassertion` periodically reinforces the no-poll rule.
  `system_prompt_reinforces_no_poll` unit test added.
- `zoid/src/agent.rs`: `dispatch_subagent` tool result changed from bare
  JSON `{"subagent_id": "..."}` to JSON + em-dash + positive directive
  ("do NOT call list_subagents... End your turn now"). Test extended with
  two new assertions.
- `zoid/src/agent.rs`: `format_subagent_list` helper extracted to module
  level (after `fire_subagent_kill`). Agent-loop `list_subagents` arm
  rewired to call it. Soft no-poll reminder appended when subagents are
  running (data + reminder, not a refusal). Test rewritten to call the
  helper directly — exercises real code instead of a duplicated
  reconstruction. Reminder present non-empty / absent empty assertions.

## 0.7.1

Windows keyboard double-fire fix.
- On Windows, crossterm emits both `KeyEventKind::Press` and
  `KeyEventKind::Release` for every keypress (Unix only emits `Press`
  unless keyboard-enhancement flags are enabled). `route_key`
  (`crates/zoid-tui/src/route.rs`) matched on `key.code` alone, never
  checking `key.kind`, so every key event was processed twice — arrow
  keys moved the palette/overlay selection 2 rows, Enter double-fired,
  Esc closed immediately, typed characters doubled. Added a
  `key.kind != KeyEventKind::Press → Action::Noop` guard at the top of
  `route_key` (the single chokepoint for all TUI key routing) and in
  the startup session picker (`crates/zoid/src/main.rs`), which handles
  keys directly. No-op on Unix.

## 0.7.0

Concurrent subagent execution, companion browser view, peek popup removal,
incremental projection, TUI perf/stability fixes, and picker UX. ~55
commits since v0.6.0.

Concurrent subagent pool (spec:
2026-07-25-concurrent-subagent-execution-design.md).
- `zoid/src/agent.rs`: subagents now run in a configurable pool
  (`subagent.max_concurrent`, default 3) with queue overflow. Each
  subagent gets its own SQLite store. `dispatch_subagent` tool
  description documents the new concurrent-execution semantics.
- `zoid-core/src/config.rs`: `max_concurrent` config field + layered
  merge. `SubagentQueued` variant for overflow feedback.
- `zoid-tools/src/subagent_dispatch.rs`: updated tool description.
- Per-result delegation wake — dropped the `is_empty` gate that was
  dropping wakeups when other results were still in-flight
  (`b9d5bc3`). `try_send` used at the 7-field send site.
- Session takeover kills in-flight subagents (`4b41105`).
- Test coverage: edge-case + cancellation tests for the pool
  (`fbbaf54`, `cfff74a`).

Companion server (spec:
2026-07-25-palette-cleanup-companion-default-design.md).
- `zoid-core/src/config.rs`: `companion.enabled` field (default false)
  + `ZOID_COMPANION_ENABLED` env var. `zoid-tui/src/config_view.rs`:
  settings overlay row + live toggle.
- `zoid/src/main.rs`: boot OR for companion, settings row wiring.

Peek popup removal + incremental projection (spec:
2026-07-26-peek-removal-and-incremental-projection-design.md).
- Removed `PeekState`/`PeekContent` from `ShellState` (`a6d78cb`),
  `PeekHit`/`PeekKind` types + `peek_hits` fn from `zoid-tui::chat`
  (`b38d3ac`), peek action variants + routing (`dc1a3e1`), peek rect
  from `ShellLayout` (`59aba76`), and peek overlay/cache/handlers/click
  logic (`ec14236`).
- `zoid/src/main.rs`: `apply_streaming` replaced with tiered
  `apply_event` + dirty-flag economy refresh (`65c502d`). Each
  streaming event is applied incrementally with a churn-dirty flag
  instead of reprocessing the full transcript per frame. Economy
  refresh is triggered only when the dirty flag is set.
- `churn_dirty` flag correction in `apply_event` (`49fb618`).
- Test coverage: tier classification, edge cases, dirty-flag refresh
  (`ec38c6b`).

TUI perf & stability.
- `zoid/src/main.rs`: biased `select!` so `ui_rx` is never starved by
  the motion tick (`974b5cc`). Split motion tick — 30fps for streaming,
  5fps for subagent-only (`a90b95b`). Peek cache recompute skipped on
  body cache hit (`5659dfc`).
- `zoid-tui/src/palette.rs`: removed `delegate` and `drawer` from
  `:stage1` palette (`74d07b9`). Animated subagent glyph in subagents
  drawer (`580111b`).
- Removed unused `PeekCache` width/scroll fields (`35993e9`).
- Diagnostic tracing added and removed during investigation
  (`ef7f691`, `c104b4c`, `ec490ed`).

Picker UX (spec:
2026-07-26-picker-create-new-at-top-design.md).
- "Create new" moved to the top of the startup picker
  (`fca4db2`). Scroll handling so the row stays visible (`295ea25`).
- `pick_choice` boundary flipped (`8d6e376`). Stale scroll-offset tests
  + doc comments updated (`452f469`).

Bug fixes.
- Esc cancellation (`ba5f76a`).
- Default idle/hard timeouts bumped to 900s/1800s (`c784a41`).

Not yet shipped.
- ProjectionCache::refresh parallelization — spec/plan revised for
  `std::thread::scope` (`2decd35`); not yet implemented. TODO:
  lazy-load body cache.
- Aggressive context eviction investigation (TODO docs: `b2c9ee7`,
  `b3b65e3`).

## 0.6.0

Local model support, agent profiles, peek popups, session delete,
relevance-rescued eviction, and context-management hardening. ~145
commits since v0.5.0.

Agent profiles (spec: 2026-07-23-agents-as-entity-design.md).
- `AgentRegistry` in `zoid-core` discovers `agent.md` files from
  configured `[agents] source_dirs`. Each file carries a name,
  description, system prompt, and tool-set mode. `parse_agent_md`
  extracts the frontmatter + markdown body.
- `list_agents` tool — surfaces available agent profiles to the model.
- `dispatch_subagent` gains an `agent` parameter — the model names a
  profile, and zoid resolves it to the profile's system prompt + tool
  set instead of the default subagent profile.
- TUI: subagents drawer shows the agent profile name (not the raw ID).

Peek popups.
- Click any tool-call line or delegated-chip in the conversation to open
  a scrollable popup showing the full tool output or delegation summary.
  `PeekState` / `PeekContent` on `ShellState`; `peek_hits` in
  `zoid-tui::chat` maps click coordinates to peek targets. Esc/click-away
  dismisses.

Session delete.
- `SessionHandle::delete_session` actor command + `EventStore::delete_session`
  (transactional: events, FTS index, embeddings all removed together).
- Startup picker: `Delete` key arms an inline confirm; `y` confirms,
  `n`/`Esc` cancels. Live sessions can't be deleted (guarded).
- `SessionDelete`/`ConfirmYes`/`ConfirmNo` actions wired into
  `route_sessions_key`.

Relevance-rescued eviction (spec: ACM eviction-weight eval).
- `GoalContext` in `zoid-core::eviction` — embeds the recent goal text
  and computes cosine similarity against candidate turn embeddings to
  prefer evicting turns least relevant to the current task.
- `rescue_weight` config in `[eviction]` — controls the balance between
  relevance and recency (0 = pure recency, default 0.3).
- `RecencyScorer` + `plan_evictions` use the goal context to re-rank
  eviction candidates.
- TUI: eviction chip at Normal zoom (shows what was dropped); breakdown
  at Detail zoom (span, token estimate, topic hint).
- `ChatMsg::Evicted` variant in the projection + all match arms;
  `build_request_with_thinking` filters evicted messages from the
  provider request.

Local Ollama (`ollama-local` provider). Spec:
2026-07-25-local-model-evaluation-design.md.
- `ZOID_NUM_CTX` env var + `OllamaProvider::with_num_ctx` builder +
  `options.num_ctx` in the native `/api/chat` request body. A local daemon
  applies its own (small) default and silently truncates rather than
  erroring, so the client must request a window explicitly. Cloud path is
  byte-identical (`num_ctx = None` → no `options` key).
- `[economy] num_ctx` in `config.toml` — no env var needed. Precedence:
  `ZOID_NUM_CTX` env (back-compat) > `[economy] num_ctx` > default 32768.
- `fetch_model_info` clamps the reported context window to the requested
  `num_ctx` so the economy view reflects the real limit.

Context overflow protection. Plan:
2026-07-25-context-overflow-protection.md.
- `plan_compactions_for_overflow` in `zoid-core::compaction` — like
  `plan_compactions` but driven by a hard ceiling (the model's actual
  context window) rather than a soft threshold. Compacts the largest
  uncompacted tool results first, using real `compact_tool_output`
  summaries. Ports File-item handling from `plan_compactions`.
- Hard-ceiling pass at the end of `preflight_gate` — after the existing
  soft-threshold compaction + eviction, if the estimate still exceeds
  the model's context window, force-compacts via
  `plan_compactions_for_overflow`. Uses `TurnConfig.context_window` (the
  live-fetched value from `ModelInfoFetched` / `ctx_ceiling`), not the
  static `model_info` table's conservative default.
- `read` tool default limit lowered from 2000 to 500 lines.

Context budget hint for small-context models.
- When `ctx_ceiling < 64K`, appends a context-efficiency section to the
  system prompt: prefer `grep`/`glob` before reading, use `limit`/`offset`,
  stop early, use `recall` for compacted content.

Max tokens for thinking-capable models.
- `ThinkingMode::Off` now checks `model_info(model).thinking` — if the
  model supports thinking, `max_tokens` is bumped from 4096 to 8192.

UI improvements.
- Thinking badge replaces the thinking marker line at Normal zoom.
- Average TPS (rolling per-turn) in the session widget.
- Width-aware truncation: tool-call summaries and first-line previews
  cap to the available conversation width, not a fixed 120 columns.
- System prompt expanded with environment context (git branch, worktree,
  session info).

Test-suite performance. Spec/Plan:
2026-07-25-test-suite-performance-design.md.
- `cargo nextest` adopted as the release gate (`AGENTS.md:46`).
- `[profile.test.package.zoid-core] opt-level = 1` — surgical override.
- `economy_integration` fixtures shrunk from 2000 to 100 lines.
- Results: cargo test 139.6s → 95.2s (-32%), nextest 108.3s → 77.5s (-28%).

Workspace version inheritance.
- `zoid-mcp`, `zoid-embed`, `zoid-testkit` now use `version.workspace = true`.

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
