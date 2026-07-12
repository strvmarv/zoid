# OpenCode Zen provider — full-parity subscription, four-way per-model wire routing

Date: 2026-07-09
Status: Design (approved in brainstorm; spec research complete 2026-07-10; **re-validated 2026-07-11** — see §0)
Extends: `2026-07-05-opencode-go-provider-design.md` (registry shape, provider picker, key gate), `2026-07-03-settings-redesign-design.md`

## 0. Re-validation (2026-07-11)

The design was re-validated against the codebase after the reassertion + thinking-mode features landed. **No architectural changes** — the 4-way routing, shared key, model catalog, and Secrets prettification all hold. Three mechanical drifts are corrected inline (marked `[RV]`):

1. **`CompletionRequest.reassert` field** (`[RV]`, added by commit `b95ce65`, post-spec). Every `CompletionRequest { … }` literal in the implementation plan must include `reassert: None`. The two new generic leaves render reassert as a no-op for v1 (pass `None`); a follow-up can wire trailing-text placement if needed. See §4.3/§4.4 notes.

2. **Thinking mode is now model-driven via `ThinkingWireShape`.** The original spec hardcoded `reasoning_params` for OpenAI Responses. The codebase now drives thinking rendering from `model_info(model).thinking_wire` (`None`/`Anthropic`/`DeepSeek`/`OpenAI`). The OpenAI Responses leaf consults `thinking_wire == OpenAI` (matching `openai_compat.rs`'s pattern) rather than a leaf-local map. Gemini has **no** `ThinkingWireShape` variant — per brainstorm decision (Option A), Gemini thinking is **leaf-local**: `MODEL_CAPS` sets `thinking: Toggle, thinking_wire: None`, and `google_gemini.rs` renders `generationConfig.thinkingConfig.includeThoughts` from `req.thinking != Off`. This mirrors how `ollama.rs` handles its own thinking param without a dedicated wire-shape variant. See §4.3/§4.4.

3. **Anthropic provider refactored to a directory module** (`anthropic/mod.rs` + `request.rs`/`parse.rs`/`types.rs`/`cache.rs`). The import `crate::anthropic::AnthropicProvider` still resolves correctly — no code change, just a reference update. The `AnthropicProvider` gains `with_betas` (not used by Zen v1).

## 1. Problem

OpenCode Zen is OpenCode's premium LLM subscription, billed separately from OpenCode Go, reachable at `https://opencode.ai/zen`. A user with an OpenCode Go API key should be able to select `opencode-zen` in zoid's provider picker, pick any Zen model, and have zoid stream completions + tool-calls against the Zen endpoint — with no changes to existing provider behavior.

The key wrinkle (carried over from the Go design doc, which explicitly deferred Zen): Zen routes its models across **four wire shapes**, and the wire shape is a property of the **(offering × model)** pair, not the model alone:

| Wire shape | Zen models | Endpoint |
|---|---|---|
| OpenAI Chat Completions | deepseek-v4-*, glm-5.*, grok-4.5, grok-build-0.1, minimax-m*, kimi-k2.*, big-pickle, hy3-free, mimo-v2.5-free, north-mini-code-free, nemotron-3-ultra-free (19 models) | `POST {base}/v1/chat/completions` |
| Anthropic Messages | claude-fable-5, claude-opus-4-5..4-8, claude-sonnet-4-5..4-6/5, claude-haiku-4-5, qwen3.5-plus..3.7-max (16 models) | `POST {base}/v1/messages` |
| OpenAI Responses | gpt-5..5.5, gpt-*-codex, gpt-5.4-mini/nano (17 models) | `POST {base}/v1/responses` |
| Google Gemini | gemini-3-flash, gemini-3.1-pro, gemini-3.5-flash (3 models) | `POST {base}/v1/models/<model>:streamGenerateContent?alt=sse` |

zoid already has clients for the first two (the shared `OpenAICompatProvider` and `AnthropicProvider` leaves, used by Go). It has **no client** for OpenAI Responses or Google Gemini. So this slice adds two new generic transport leaves alongside a dedicated `OpenCodeZenProvider` that owns per-model routing across all four.

### Billing isolation vs. key friction

Zen and Go are billed separately but the same API key authorizes both. The chosen trade-off (brainstorm, option B): **one shared env var (`OPENCODE_GO_API_KEY`) backs both providers — least friction.** Isolation lives in the registry entry / picker row / provider struct, not the credential gate. This is a deliberate, documented relaxation from full isolation: there is no independent "is Zen configured?" gate, so a single key unlocks both billing pools. The spec calls this out so it stays a conscious decision rather than an oversight. If independent keying is ever needed, a future spec can introduce `OPENCODE_ZEN_API_KEY` with optional Go-fallback without changing the routing architecture.

### Secrets section label prettification

The config screen's Secrets section currently shows each secret row's label as the literal env-var name (`OPENCODE_GO_API_KEY`, `OLLAMA_API_KEY`, `ANTHROPIC_API_KEY`). That label doubles as the secret-store key: the edit commit flow does `secret_store.set(label, &buffer)` (main.rs ~line 3718), and `refresh_config_sections` looks up `status(name)` by that same string. So today the display label *is* the key.

This slice prettifies the **displayed** labels to friendly names — `opencode`, `ollama`, `anthropic` — while the underlying secret-store keys (`OPENCODE_GO_API_KEY`, `OLLAMA_API_KEY`, `ANTHROPIC_API_KEY`) stay unchanged. This requires decoupling the display label from the key:

- `FieldRow` (`zoid-tui/src/config_view.rs`) gains an optional `secret_key: Option<&'static str>`. When present, the row renders `label` (the friendly name) but the secret edit flow and `status()` lookup use `secret_key`.
- `build_sections`' Secrets section sets `label: "opencode"` / `"ollama"` / `"anthropic"` with `secret_key: Some("OPENCODE_GO_API_KEY")` / `Some("OLLAMA_API_KEY")` / `Some("ANTHROPIC_API_KEY")`.
- The commit flow (`Action::ConfigFieldCommit` arm at main.rs ~3714) and `field_target` resolve the secret-store key from the row's `secret_key` (falling back to `label` for any future secret row that doesn't set it), not from `label` directly. `current_config_field` is extended to carry `secret_key` (or the commit flow re-fetches the row from `config_sections`).
- `refresh_config_sections` (`key_status` array, main.rs ~3140) keeps passing the real env-var names to `status()` — only the rendered label changes.

The provider picker rows (`opencode · go` → stays; `opencode · zen` → new) are **unaffected** — this change is scoped to the Secrets section only. Backend id `opencode-go`, family, and env var `OPENCODE_GO_API_KEY` are unchanged — no stored-config migration, no `canonical_id` churn.

## 2. Goals / Non-goals

### In scope (this spec → one plan)
- A dedicated `opencode-zen` provider: one registry entry, family `opencode-zen`, one picker row (`opencode · zen`). Wire routing per model is invisible to the user.
- Two new generic transport leaves:
  - `OpenAIResponsesProvider` (`openai_responses.rs`) — OpenAI Responses API (`/v1/responses`, SSE, `response.*` events, function-call via `response.function_call_arguments.delta/.done`, reasoning summaries, usage on `response.completed`).
  - `GoogleGeminiProvider` (`google_gemini.rs`) — Google Gemini (`POST {base}/v1/models/<model>:streamGenerateContent?alt=sse`, `candidates[].content.parts[]` with `text`/`functionCall`/`thought`, `usageMetadata`).
- An `OpenCodeZenProvider` (`opencode_zen.rs`) that delegates `stream()`/`list_models()` to one of four sub-clients based on a static per-model wire-shape map (`ZEN_MODELS`).
- One new `ProviderEntry` in `zoid-model` (`opencode-zen`, family `opencode-zen`, transport `Http`, default base_url `https://opencode.ai/zen`).
- New `MODEL_CAPS` entries for each Zen model id (placeholder caps — see §6).
- Prettify the Secrets section display labels (`OPENCODE_GO_API_KEY`→`opencode`, `OLLAMA_API_KEY`→`ollama`, `ANTHROPIC_API_KEY`→`anthropic`) by decoupling display label from secret-store key via a new `FieldRow::secret_key` field. Underlying env-var names unchanged.
- Bin wiring: `key_env_for`, `select_provider`, `provider_for_id` gain `opencode-zen` arms reading `OPENCODE_GO_API_KEY`. No new settings field (shared key).
- Tests for all new logic; offline `TcpListener` stubs for streaming/routing (no live-endpoint CI), matching the existing `ollama.rs`/`anthropic.rs`/`openai_compat.rs`/`opencode_go.rs` stance.

### Out of scope (separate future specs)
- **Independent Zen keying (`OPENCODE_ZEN_API_KEY`).** The shared-key trade-off is accepted here; the seams are named so an independent key (with optional Go-fallback) slots in cleanly later if billing isolation ever needs to be enforced at the credential layer.
- **Zen usage-limit / budget tracking.** Zen's quota limits surface as HTTP 429, handled by existing error surfacing. Economy is tokens-only; dollar-budget tracking is separate.
- **Live-endpoint / network integration tests.** All tests run offline against stubbed `TcpListener` servers.
- **Non-essential Responses/Gemini event types.** Audio, web_search, file_search, mcp, code_interpreter, image_gen, refusal, computer-use, and similar tool events are parsed-but-ignored (logged at trace), not surfaced as new `ProviderEvent` variants. zoid's event surface stays unchanged.

## 3. Current state (what exists)

- `crates/zoid-model/src/lib.rs` — `PROVIDERS` registry (4 entries: `ollama-local`, `ollama-cloud`, `opencode-go`, `anthropic-api`); `MODEL_CAPS` table; `canonical_id`, `entry`, `models_for`, `default_base_url`, `selectable`, `model_info`. `opencode-go` display is `"opencode · go"`.
- `crates/zoid-provider/src/lib.rs` — `Provider` trait, `Message` (with `tool_call_id`), `ProviderEvent` (`TextDelta`/`ThinkingDelta`/`ThinkingSignature`/`ToolCall`/`Usage`/`Truncated`/`Done`/`Error`), `CompletionRequest` (now carries `reassert: Option<String>` `[RV]`, added post-spec by commit `b95ce65`), `ToolCall`, `Usage`, `ToolSpec`, `ThinkingMode`/`EffortLevel`, `default_provider()`/`default_model()`, `parse_data_id_models`, `http_client()`, `stream_idle_timeout()`.
- `crates/zoid-provider/src/openai_compat.rs` — `OpenAICompatProvider` (OpenAI Chat Completions `/v1/chat/completions`, SSE, tool-calling with fragment accumulation, `with_base_url`/`with_idle_timeout`). Now drives thinking via `model_info(model).thinking_wire`.
- `crates/zoid-provider/src/anthropic/` `[RV]` — `AnthropicProvider` (Messages API `/v1/messages`, SSE via `eventsource_stream`, text-only P1b, `with_base_url`/`with_idle_timeout`/`with_betas`). Refactored from a single file to a directory module (`mod.rs` + `request.rs`/`parse.rs`/`types.rs`/`cache.rs`) post-spec; the import `crate::anthropic::AnthropicProvider` still resolves.
- `crates/zoid-provider/src/opencode_go.rs` — `OpenCodeGoProvider`: dedicated provider holding a static `GO_MODELS: &[(model, WireShape)]` (two variants) and delegating `stream()`/`list_models()` to `OpenAICompatProvider` or `AnthropicProvider`.
- `crates/zoid/src/main.rs` — `key_env_for` (family → env var), `select_provider` (family branch → provider impl), `provider_for_id` (quick-switch live fetch), settings secret-field lists.
- The Go design doc explicitly named the seams so `opencode-zen` slots in cleanly; this slice does exactly that.

## 4. Design

### 4.1 Architecture & module layout

**New crate modules in `zoid-provider`:**

1. **`openai_responses.rs`** — self-contained OpenAI Responses client. Pure functions for request body + SSE parse, plus an `OpenAIResponsesProvider` struct implementing `Provider`. Speaks `POST {base}/v1/responses`, `stream: true`, full tool-calling via `response.function_call_arguments.delta/.done`, reasoning summaries via `response.reasoning_*`, usage on `response.completed`. Structured like `openai_compat.rs`: `request_body()`, `parse_event()`, `new().with_base_url().with_idle_timeout()`. No opencode-zen-specifics — a generic leaf reusable later by direct-OpenAI, OpenRouter, etc.

2. **`google_gemini.rs`** — self-contained Google Gemini client. Pure functions for request body (`contents`/`systemInstruction`/`tools`/`generationConfig`) + chunk parse (`candidates[].content.parts[]` → `text` / `functionCall` / `thought`), plus a `GoogleGeminiProvider` struct implementing `Provider`. Speaks `POST {base}/v1/models/<model>:streamGenerateContent?alt=sse`. Structured identically to the other leaves. Generic — reusable by direct-Gemini later.

3. **`opencode_zen.rs`** — the dedicated `OpenCodeZenProvider` implementing `Provider`. Holds `api_key`, `base_url` (default `https://opencode.ai/zen`), `reqwest::Client`, idle timeout, and the static per-model wire-shape map. Its `stream()` consults the map and delegates to one of four sub-clients:
   - `OpenAIChat` → existing `OpenAICompatProvider` (with the Zen base_url + key).
   - `AnthropicMessages` → existing `AnthropicProvider` (with the Zen base_url + key).
   - `OpenAIResponses` → new `OpenAIResponsesProvider` (with the Zen base_url + key).
   - `GoogleGemini` → new `GoogleGeminiProvider` (with the Zen base_url + key).
   - `list_models()` hits `{base}/v1/models` (OpenAI-shape `data: [{id}]`, reused via `parse_data_id_models`).
   - Unknown model → `OpenAIChat` with a `tracing::warn!` (mirrors Go's fallback).

### 4.2 Wire shapes and the routing table

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZenWireShape {
    OpenAIChat,
    AnthropicMessages,
    OpenAIResponses,
    GoogleGemini,
}

const ZEN_MODELS: &[(&str, ZenWireShape)] = &[
    // Claude (Anthropic Messages)
    ("claude-fable-5",     ZenWireShape::AnthropicMessages),
    ("claude-opus-4-8",    ZenWireShape::AnthropicMessages),
    ("claude-opus-4-7",    ZenWireShape::AnthropicMessages),
    ("claude-opus-4-6",    ZenWireShape::AnthropicMessages),
    ("claude-opus-4-5",    ZenWireShape::AnthropicMessages),
    ("claude-sonnet-5",    ZenWireShape::AnthropicMessages),
    ("claude-sonnet-4-6",  ZenWireShape::AnthropicMessages),
    ("claude-sonnet-4-5",  ZenWireShape::AnthropicMessages),
    ("claude-haiku-4-5",   ZenWireShape::AnthropicMessages),
    ("qwen3.7-max",        ZenWireShape::AnthropicMessages),
    ("qwen3.7-plus",       ZenWireShape::AnthropicMessages),
    ("qwen3.6-plus",       ZenWireShape::AnthropicMessages),
    ("qwen3.5-plus",       ZenWireShape::AnthropicMessages),

    // GPT (OpenAI Responses)
    ("gpt-5.5",            ZenWireShape::OpenAIResponses),
    ("gpt-5.5-pro",        ZenWireShape::OpenAIResponses),
    ("gpt-5.4",            ZenWireShape::OpenAIResponses),
    ("gpt-5.4-pro",        ZenWireShape::OpenAIResponses),
    ("gpt-5.4-mini",       ZenWireShape::OpenAIResponses),
    ("gpt-5.4-nano",       ZenWireShape::OpenAIResponses),
    ("gpt-5.3-codex",      ZenWireShape::OpenAIResponses),
    ("gpt-5.3-codex-spark", ZenWireShape::OpenAIResponses),
    ("gpt-5.2",            ZenWireShape::OpenAIResponses),
    ("gpt-5.2-codex",      ZenWireShape::OpenAIResponses),
    ("gpt-5.1",            ZenWireShape::OpenAIResponses),
    ("gpt-5.1-codex-max",  ZenWireShape::OpenAIResponses),
    ("gpt-5.1-codex",      ZenWireShape::OpenAIResponses),
    ("gpt-5.1-codex-mini", ZenWireShape::OpenAIResponses),
    ("gpt-5",              ZenWireShape::OpenAIResponses),
    ("gpt-5-codex",        ZenWireShape::OpenAIResponses),
    ("gpt-5-nano",         ZenWireShape::OpenAIResponses),

    // Chat Completions (OpenAI compat)
    ("deepseek-v4-pro",    ZenWireShape::OpenAIChat),
    ("deepseek-v4-flash",  ZenWireShape::OpenAIChat),
    ("glm-5.2",            ZenWireShape::OpenAIChat),
    ("glm-5.1",            ZenWireShape::OpenAIChat),
    ("glm-5",              ZenWireShape::OpenAIChat),
    ("grok-4.5",           ZenWireShape::OpenAIChat),
    ("grok-build-0.1",     ZenWireShape::OpenAIChat),
    ("kimi-k2.5",          ZenWireShape::OpenAIChat),
    ("kimi-k2.6",          ZenWireShape::OpenAIChat),
    ("kimi-k2.7-code",     ZenWireShape::OpenAIChat),
    ("minimax-m3",         ZenWireShape::OpenAIChat),
    ("minimax-m2.7",       ZenWireShape::OpenAIChat),
    ("minimax-m2.5",       ZenWireShape::OpenAIChat),
    ("big-pickle",         ZenWireShape::OpenAIChat),
    ("hy3-free",           ZenWireShape::OpenAIChat),
    ("mimo-v2.5-free",     ZenWireShape::OpenAIChat),
    ("north-mini-code-free", ZenWireShape::OpenAIChat),
    ("nemotron-3-ultra-free", ZenWireShape::OpenAIChat),
    ("deepseek-v4-flash-free", ZenWireShape::OpenAIChat),

    // Google Gemini
    ("gemini-3.5-flash",   ZenWireShape::GoogleGemini),
    ("gemini-3.1-pro",     ZenWireShape::GoogleGemini),
    ("gemini-3-flash",     ZenWireShape::GoogleGemini),
];
```

`wire_shape_for(model)` mirrors `OpenCodeGoProvider::wire_shape_for`: table lookup, unknown → `OpenAIChat` + `tracing::warn!`. The first entry in `ZEN_MODELS` and the registry's `models[0]` must agree (the default model).

The wire-shape map lives only in `opencode_zen.rs`. `MODEL_CAPS` carries per-model capabilities (context window, max output, tools, prompt cache, thinking support) — caps and wire shape stay separate concerns, matching Go.

### 4.3 OpenAI Responses client (`openai_responses.rs`)

Source: OpenAI's published OpenAPI spec (`openai/openai-openapi`), `CreateResponse` / `ResponseStreamEvent` / `FunctionTool` / `Reasoning` schemas.

**Request body** (`request_body(req) -> Value`):
- `model`.
- `input`: zoid's `messages` → Responses input shape. A `user`/`assistant` text message → `{role, content:[{type:"input_text"|"output_text", text}]}`. A `Tool` message → a top-level `function_call_output` input item `{type:"function_call_output", call_id, output}`. `system` → `instructions` (a top-level field, not a message).
- `tools`: zoid's `ToolSpec[]` → `[{type:"function", name, description, parameters, strict:false}]`.
- `reasoning`: derived from `ThinkingMode` **via the model-driven `thinking_wire` pattern** `[RV]` — when `model_info(model).thinking_wire == OpenAI`, map `ThinkingMode` to `reasoning.effort`: `Off` → omit the field; `Auto` → `{effort:"medium"}`; `Effort(level)` → `{effort: "low"|"medium"|"high"|"xhigh"}` (zoid's `EffortLevel::Max` → `"xhigh"`). This matches `openai_compat.rs`'s existing `thinking_params()` pattern rather than a leaf-local heuristic.
- `max_output_tokens`, `stream:true`, `tool_choice:"auto"`.

**SSE parse** (`parse_event(line) -> Vec<ProviderEvent>`): SSE via `eventsource_stream` (same plumbing as `anthropic.rs`). Discriminate on the `type` field:
- `response.output_text.delta` → `TextDelta(delta)`.
- `response.function_call_arguments.delta` → accumulate `delta` into a per-`item_id` buffer (fragment assembly, like `openai_compat.rs`'s tool-call accumulator).
- `response.function_call_arguments.done` → emit one `ToolCall{call_id, name, arguments(JSON string→Value)}`; flush that item's buffer. The `call_id` is the function tool call's `call_id` (carried on the output item, surfaced via `response.output_item.added`/`.done` or the `.done` event itself — confirm the exact field during implementation; the `.done` event carries `item_id`, `name`, `arguments`).
- `response.reasoning_summary_text.delta` / `response.reasoning_text.delta` → `ThinkingDelta(delta)`.
- `response.completed` → `Usage{input_tokens, output_tokens, cached: input_tokens_details.cached_tokens, thinking_tokens: output_tokens_details.reasoning_tokens}` then `Done`.
- `response.incomplete` → `Truncated` then `Done`.
- `response.failed` → `Error(msg)`.
- All other event types (audio, web_search, file_search, mcp, code_interpreter, image_gen, refusal, content_part.added/done, output_item.added/done, created, in_progress, queued) → ignored (logged at trace).

**Usage emit semantics:** single additive `Usage` on `response.completed` (disjoint/once — no double-count risk under the agent loop's summation). `response.completed`'s `response.usage` carries `input_tokens`, `input_tokens_details.cached_tokens`, `output_tokens`, `output_tokens_details.reasoning_tokens`, `total_tokens`.

### 4.4 Google Gemini client (`google_gemini.rs`)

Source: Google's Generative Language API discovery doc (`generativelanguage.googleapis.com/$discovery/rest?version=v1beta`), `GenerateContentRequest` / `GenerateContentResponse` / `Candidate` / `Content` / `Part` / `UsageMetadata` / `FunctionDeclaration` schemas.

**Request** (`request_body(req, model) -> (path_suffix, body)`): the model is in the **path**, not the body, so the provider builds the URL per call. `path_suffix = format!("v1/models/{model}:streamGenerateContent")`, query `?alt=sse` for SSE framing.
- `contents`: zoid's messages → `[{role:"user"|"model", parts:[…]}]`.
  - A `user`/`assistant` text message → `{role, parts:[{text}]}` (`assistant` → `role:"model"`).
  - A `Tool` message → `{role:"user", parts:[{functionResponse:{name, response:{content: <text>}}}]}` (Gemini wraps tool results as user-role `functionResponse` parts).
  - An `assistant` message with `tool_calls` → `{role:"model", parts:[{functionCall:{id,name,args}}]}`.
- `system` → `systemInstruction:{parts:[{text}]}` (top-level, not in `contents`).
- `tools` → `[{functionDeclarations:[{name, description, parameters}]}]` (parameters is a JSON Schema object).
- `thinking` → `generationConfig.thinkingConfig:{includeThoughts:true, thinkingBudget:…}` (derive from `ThinkingMode`; `Off` → omit). **Leaf-local rendering `[RV]`:** `MODEL_CAPS` sets `thinking: Toggle, thinking_wire: None` for Gemini models. The leaf consults `req.thinking != Off` directly (not `thinking_wire`), mirroring how `ollama.rs` renders its own thinking param without a dedicated `ThinkingWireShape::Ollama` variant. Gemini's `thought:true` parts map to `ThinkingDelta`.
- `max_output_tokens` → `generationConfig.maxOutputTokens`.

**SSE parse** (`parse_chunk(obj) -> Vec<ProviderEvent>`): `?alt=sse` returns one `GenerateContentResponse` JSON object per SSE `data:` line. For each chunk:
- `candidates[].content.parts[]`: a part `{text}` → `TextDelta`; `{functionCall:{id,name,args}}` → `ToolCall{id, name, args}`; `{thought:true, text}` → `ThinkingDelta(text)` (when thoughts enabled).
- `candidates[].finishReason == "MAX_TOKENS"` → `Truncated`.
- `usageMetadata` (present on the final chunk) → `Usage{input_tokens: promptTokenCount, output_tokens: candidatesTokenCount, cached: cachedContentTokenCount, thinking_tokens: thoughtsTokenCount}` then `Done`. Additive-once on the final frame (matches Ollama's single-snapshot emit; safe under the agent loop's summation).
- `promptFeedback.blockReason` (if present) → `Error(reason)`.

**Endpoint note:** `?alt=sse` switches Gemini's default JSON-array-of-chunks response to SSE `data:` framing, so the client reuses the same `eventsource-stream` plumbing as the other leaves. Confirmed shape from Google's discovery doc.

### 4.5 Registry, bin wiring, and Secrets label prettification

**Registry (`zoid-model/src/lib.rs`):** one new entry appended to `PROVIDERS` (after `anthropic-api`):

```rust
ProviderEntry {
    id: "opencode-zen",
    display: "opencode · zen",
    family: "opencode-zen",
    transport: Transport::Http { default_base_url: "https://opencode.ai/zen" },
    models: &[ /* placeholder — first = default model */ ],
    status: Status::Available,
}
```

The existing `opencode-go` entry is unchanged (id, family, display, base_url all stay as-is). `canonical_id("opencode-zen")` → passthrough (no legacy alias). `selectable()` now returns 5 entries; the existing `selectable_has_four_providers` test updates to five and adds `opencode-zen`.

Placeholder `MODEL_CAPS` entries: one per Zen model id with conservative fields (real caps filled during implementation plan alongside the model list).

**Bin wiring (`main.rs`):** three functions gain an `"opencode-zen"` family arm — mechanical copy-adapt of the `"opencode-go"` arm:
- `key_env_for`: add `Some("opencode-zen") => Some("OPENCODE_GO_API_KEY")` (shared key).
- `select_provider`: new `match family` arm `"opencode-zen"` → `OpenCodeZenProvider::new(k).with_base_url(base_url)`, label `"opencode-zen"`, ready `true`; no-key fallback → `default_provider()`, label `"opencode-zen"`, `false`.
- `provider_for_id`: new arm `"opencode-zen"` → `OpenCodeZenProvider::new(k).with_base_url(base_url)` (for quick-switch live model fetch).

**Secrets section prettification (`zoid-tui/src/config_view.rs` + `main.rs`):** the Secrets section rows render friendly labels instead of env-var names, via a new `FieldRow::secret_key: Option<&'static str>`:
- `FieldRow` gains `secret_key: Option<&'static str>` (defaults `None`; set only on Secret rows).
- `build_sections`' Secrets section: `label: "opencode"`, `secret_key: Some("OPENCODE_GO_API_KEY")`; `label: "ollama"`, `secret_key: Some("OLLAMA_API_KEY")`; `label: "anthropic"`, `secret_key: Some("ANTHROPIC_API_KEY")`.
- `current_config_field` (main.rs ~3040) returns enough for the commit flow to resolve the key — either by carrying `secret_key` in its tuple, or by having the commit arm re-fetch the row from `app.shell.config_sections`. The commit arm (`Action::ConfigFieldCommit` at ~3714, `s.set(label, &buffer)`) uses `secret_key.unwrap_or(label)` instead of `label`.
- `refresh_config_sections` (`key_status` at ~3140) keeps passing the real env-var names to `status()`; only the rendered label changes.
- `field_target` (~2907) and the edit-prompt label (~3931/3946) use `secret_key` for keying and `label` for the prompt string shown to the user.

**Settings UI:** no new secret field. The existing `OPENCODE_GO_API_KEY` credential already serves both Go and Zen (shared key). The provider picker shows a distinct `opencode · zen` row, so *selection* is isolated even though the *key* isn't. The spec records the shared key as the deliberate, brainstorm-approved relaxation from full billing isolation.

## 5. Data flow

1. User selects `opencode · zen` in the provider picker (or `config.provider = "opencode-zen"`).
2. `select_provider` resolves family `opencode-zen`, reads `OPENCODE_GO_API_KEY` (env wins, else secret store), constructs `OpenCodeZenProvider::new(key).with_base_url(effective_base_url)`.
3. Per turn, the agent loop builds a `CompletionRequest{model, …}` and calls `provider.stream(req, sink)`.
4. `OpenCodeZenProvider::stream` looks up `wire_shape_for(&req.model)` and delegates to the matching sub-client, constructed with the Zen `base_url` + `api_key` + `idle_timeout`.
5. The sub-client POSTs to its wire-shape endpoint, parses the SSE stream, and emits `ProviderEvent`s into `sink` (`TextDelta`/`ThinkingDelta`/`ToolCall`/`Usage`/`Truncated`/`Error`/`Done`). The agent loop consumes `sink` exactly as it does for Go/Ollama/Anthropic — no agent-loop changes.
6. `list_models()` (picker live-fetch) hits `{base}/v1/models` and parses with the shared `parse_data_id_models`.

## 6. Model catalog — filled (52 models, 4 wire shapes)

The concrete Zen model ids, their wire shapes, and per-model caps were confirmed via API research (see `docs/superpowers/spikes/2026-07-10-opencode-zen-api-research.md`). The `/v1/models` endpoint returns 52 models; model→wire-shape mapping is determined by the upstream provider:

- **Anthropic Messages (13):** claude-fable-5, claude-opus-4-5..4-8, claude-sonnet-4-5..4-6/5, claude-haiku-4-5, qwen3.5-plus..3.7-max
- **OpenAI Responses (17):** gpt-5..5.5, gpt-*-codex, gpt-5.4-mini/nano
- **OpenAI Chat Completions (19):** deepseek-v4-*, glm-5.*, grok-4.5, grok-build-0.1, kimi-k2.*, minimax-m*, big-pickle, hy3-free, mimo-v2.5-free, north-mini-code-free, nemotron-3-ultra-free, deepseek-v4-flash-free
- **Google Gemini (3):** gemini-3-flash, gemini-3.1-pro, gemini-3.5-flash

Default model: `claude-sonnet-4-5` (Anthropic Messages, matching Go's default family).
Deprecated models excluded: claude-opus-4-1, claude-sonnet-4.

`ZEN_MODELS[0]` = `claude-sonnet-4-5` = the registry entry's `models[0]`.
Each Zen model id gets an explicit `MODEL_CAPS` entry (family-level conservative caps).
The four-way routing table covers all 52 models.

Constraints on the placeholders so the fill-in is mechanical: (filled — see §4.2 `ZEN_MODELS` for the concrete 52-model table; see `docs/superpowers/spikes/2026-07-10-opencode-zen-api-research.md` for the research)
- `ZEN_MODELS[0]` must equal the registry entry's `models[0]` (the default model).
- Each Zen model id gets exactly one `MODEL_CAPS` entry (case-insensitive lookup is already supported by `model_info`).
- The four-way routing table covers every model in the registry's `models` list — no model without a wire-shape entry.
- Conservative caps for unconfirmed models: `context_window` from public docs or `ZOID_CONTEXT_CEILING` override; `tools`/`prompt_cache`/`thinking` per the model's real capabilities. Unknown → `DEFAULT_MODEL_INFO` (32k, tools:true, prompt_cache:false, no thinking) is already the fallback, but each known Zen model should get an explicit entry to avoid the 32k floor.

## 7. Error handling

- Transport errors (connect, TLS, non-2xx) surface as a final `ProviderEvent::Error(string)` then `Done`, matching the existing providers.
- `response.failed` (Responses) and `promptFeedback.blockReason` (Gemini) → `Error`.
- Mid-stream idle stall (no bytes for `idle_timeout`) → stream abandoned with `Error`, matching the existing `stream_idle_timeout` contract.
- `is_context_length_error` heuristic already covers "context length"/"context window"/"too long"/"maximum context" strings from all four shapes; no new heuristic needed.

## 8. Testing

Offline, `TcpListener`-stubbed, matching the existing stance (no live-endpoint CI):
- **`zoid-model`**: registry entry exists + selectable (assert 5 providers, including `opencode-zen`); `canonical_id("opencode-zen")` passthrough; `opencode-go` display unchanged (`"opencode · go"`); table-driven caps assertion for each Zen model id (mirrors `opencode_go_model_caps_match_reconciled_table`).
- **`config_view.rs` / Secrets section**: `build_sections` renders `opencode`/`ollama`/`anthropic` labels with `secret_key` set to the real env-var names; a secret-edit commit stores under `secret_key` not `label` (unit test: build sections, assert the Secrets row labels are the friendly names and `secret_key` is the env-var name; assert `field_target`/the commit resolves the key from `secret_key`).
- **`opencode_zen.rs`**: `wire_shape_for_known_models_matches_table`; `wire_shape_for_unknown_defaults_to_openai_chat`; `with_base_url` propagation; `TcpListener` stubs asserting each wire shape routes to the right path (`/v1/chat/completions`, `/v1/messages`, `/v1/responses`, `…/streamGenerateContent`) — mirrors Go's `openai_compat_model_routes_to_chat_completions` / `anthropic_model_routes_to_messages`.
- **`openai_responses.rs`**: pure `request_body()` unit test (message/instruction/tool/reasoning mapping); `parse_event()` unit tests with fixture SSE lines for each surfaced event type (`output_text.delta`, `function_call_arguments.delta/.done`, `reasoning_summary_text.delta`, `completed` w/ usage, `incomplete`→Truncated, `failed`→Error); a `TcpListener` streaming round-trip feeding a scripted event stream and asserting the emitted `ProviderEvent` sequence.
- **`google_gemini.rs`**: pure `request_body()` unit test (contents/systemInstruction/tools/thinkingConfig mapping, path suffix includes the model); `parse_chunk()` unit tests with fixture chunks (text part, functionCall part, thought part, finishReason=MAX_TOKENS, usageMetadata final frame); a `TcpListener` streaming round-trip.
- **`main.rs`**: `key_env_for("opencode-zen")` returns `OPENCODE_GO_API_KEY`; `entry_requires_key("opencode-zen")` is true.

## 9. Open questions (to resolve before/during implementation plan)

1. **~~Zen model catalog~~** — RESOLVED. 52 models across four wire shapes: 17 OpenAI Responses (GPT), 13 Anthropic Messages (Claude + Qwen), 19 OpenAI Chat Completions (deepseek, glm, grok, kimi, minimax, misc), 3 Google Gemini. Disabled models (claude-opus-4-1, claude-sonnet-4) excluded. See `docs/superpowers/spikes/2026-07-10-opencode-zen-api-research.md`.
2. **~~Default Zen base URL~~** — RESOLVED: `https://opencode.ai/zen` (confirmed via curl).
3. **Responses `call_id` source** — the `.done` event carries `item_id`/`name`/`arguments`; confirm `call_id` presence on a real capture (confirmed the endpoint works with `input` string + `max_output_tokens`; need a tool-bearing capture for the `function_call_arguments` event shape).
4. **Gemini tool-call `id`** — `FunctionCall.id` is optional in the schema; confirm whether Zen's Gemini models populate it (falls back to empty string, matching Ollama's call-id-less shape, if absent).
5. **~~ZEN_MODELS[0]~~** — RESOLVED: `claude-sonnet-4-5` (same family as Go's default, user-perceived continuity).
6. **~~Gemini thinking wire-shape~~ `[RV]`~~** — RESOLVED (2026-07-11 brainstorm): leaf-local. `MODEL_CAPS` sets `thinking: Toggle, thinking_wire: None`; `google_gemini.rs` renders `thinkingConfig.includeThoughts` from `req.thinking != Off`. No new `ThinkingWireShape::Gemini` variant (mirrors Ollama's leaf-local approach).