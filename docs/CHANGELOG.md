# Changelog

> **Internal engineering changelog.** This file is the detailed development
> record and is **not** published. It lives under `docs/` specifically so
> cargo-dist does not detect it as a root changelog. Customer-facing release
> notes (what ships to the public releases repo) live in the root
> `RELEASES.md`.

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
