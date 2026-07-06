# Changelog

## Unreleased

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
