# OpenCode Go provider — flat-rate subscription, per-model wire routing

Date: 2026-07-05
Status: Design (approved in brainstorm; awaiting spec review → writing-plans)
Extends: `2026-07-01-model-registry.md` (registry shape), `2026-07-03-settings-redesign-design.md` (provider picker, key gate)

## 1. Problem

OpenCode Go is a flat-rate LLM subscription ($5 first month, then $10/mo) at `https://opencode.ai/go` offering reliable access to 13 open coding models. A user with an OpenCode Go API key should be able to select `opencode-go` in zoid's provider picker, pick any of the 13 models, and have zoid stream completions + tool-calls against the Go endpoint — with no changes to existing provider behavior.

The wrinkle: Go routes its 13 models across **two wire shapes**, and the wire shape is a property of the **(offering × model)** pair, not the model alone:

| Wire shape | Go models | Endpoint |
|---|---|---|
| OpenAI Chat Completions | GLM-5.2, GLM-5.1, Kimi K2.7 Code, Kimi K2.6, DeepSeek V4 Pro, DeepSeek V4 Flash, MiMo-V2.5, MiMo-V2.5-Pro | `POST {base}/v1/chat/completions` |
| Anthropic Messages | MiniMax M3/M2.7/M2.5, Qwen3.7 Max/Plus | `POST {base}/v1/messages` |

zoid has an Anthropic Messages client (reusable with a `base_url` override) but **no OpenAI Chat Completions client** — its `ollama.rs` speaks Ollama's native `/api/chat`, not OpenAI's `/v1/chat/completions` shape. So this slice adds a generic OpenAI-compat client alongside the dedicated `OpenCodeGoProvider` that owns per-model routing.

This spec also corrects a pre-existing bug: `MODEL_CAPS` lists `deepseek-v4-pro` with a 128,000-token context window, but DeepSeek's own docs confirm it is 1,000,000 (384K max output). The fix lands in this slice because the spec surfaces it.

## 2. Goals / Non-goals

### In scope (this spec → one plan)
- A dedicated `opencode-go` provider: one registry entry, one `OPENCODE_GO_API_KEY`, one picker row. Wire routing per model is invisible to the user.
- A new generic `OpenAICompatProvider` client speaking OpenAI Chat Completions (`/v1/chat/completions`, SSE, tool-calling with fragment accumulation).
- An `OpenCodeGoProvider` that delegates `stream()`/`list_models()`/`fetch_model_info()` to one of two sub-clients (`OpenAICompatProvider` or the existing `AnthropicProvider`) based on a static per-model wire-shape map.
- One new `ProviderEntry` in `zoid-model` (`opencode-go`, family `opencode-go`, transport `Http`, default base_url `https://opencode.ai/zen/go`).
- 12 new `MODEL_CAPS` entries + 1 in-place correction (`deepseek-v4-pro`: 128k → 1M, max_output 384K, prompt_cache true).
- Bin wiring: `key_env_for`, `select_provider`, `provider_for_id` gain `opencode-go` arms; settings UI gains `OPENCODE_GO_API_KEY`.
- A `Message.tool_call_id: Option<String>` field (additive; OpenAI-compat tool-result `tool_call_id`; Ollama/Anthropic ignore it).
- Tests for all new logic; offline `TcpListener` stubs for streaming/routing (no live-endpoint CI).

### Out of scope (separate future specs)
- **OpenCode Zen.** Zen adds ~50 more models and two more wire shapes zoid has no client for (OpenAI Responses `/v1/responses` for GPT-5.x, Google `/v1/models/<model>` for Gemini). The seams here are named so `opencode-zen` slots in cleanly later, but Zen itself is a separate slice.
- **Anthropic tool-calling (P1b.1).** The existing `AnthropicProvider` is text-only — it doesn't send a `tools` array and can't parse `tool_use` frames. This forces the 5 Anthropic-shape Go models to `tools: false` on day one (a zoid-implementation limitation, not a model limitation). Fixing it also fixes Claude's tool support and is a separate, larger piece.
- **Go usage-limit / budget tracking.** Go's $12/5h + $30/week + $60/month limits surface as HTTP 429, handled by the existing error surfacing. Economy is tokens-only; dollar-budget tracking is a separate feature.
- **Live-endpoint / network integration tests.** All tests run offline against stubbed `TcpListener` servers (matches the existing `ollama.rs`/`anthropic.rs` stance).

## 3. Current state (what exists)

- `crates/zoid-model/src/lib.rs` — `PROVIDERS` registry (5 entries: `ollama-local`, `ollama-cloud`, `anthropic-api`, `[planned] anthropic-cli`, `[planned] anthropic-sdk`); `MODEL_CAPS` table; `canonical_id`, `entry`, `models_for`, `default_base_url`, `selectable`, `model_info`.
- `crates/zoid-provider/src/lib.rs` — `Provider` trait, `Message` (no `tool_call_id`), `ProviderEvent`, `CompletionRequest`, `ToolCall`, `Usage`, `default_provider()`/`default_model()` (env chain: `OLLAMA_API_KEY` → ollama, `ANTHROPIC_API_KEY` → anthropic, else `FakeProvider`).
- `crates/zoid-provider/src/ollama.rs` — `OllamaProvider` (native `/api/chat`, NDJSON, tool-calling wired, `with_base_url`/`with_idle_timeout`).
- `crates/zoid-provider/src/anthropic.rs` — `AnthropicProvider` (Messages API `/v1/messages`, SSE via `eventsource_stream`, text-only P1b, `with_base_url`/`with_idle_timeout`).
- `crates/zoid/src/main.rs` — `key_env_for` (family → env var), `select_provider` (family branch → provider impl), `provider_for_id` (quick-switch live fetch), settings status/secret-field lists around lines 2014/3394.
- Settings-redesign spec already supports arbitrary provider→key mapping via `key_env_for`; the picker's "selecting a key-requiring provider prompts for the API key" flow works unchanged for new providers.

## 4. Design

### 4.1 Architecture & module layout

**New crate modules in `zoid-provider`:**

1. **`openai_compat.rs`** — self-contained OpenAI Chat Completions client. Pure functions for request body + SSE parse, plus an `OpenAICompatProvider` struct implementing `Provider`. Speaks `POST {base}/v1/chat/completions`, `stream_options: {include_usage: true}`, full tool-calling. Structured like `ollama.rs`: `request_body()`, `parse_chunk()`, `OpenAICompatProvider::new().with_base_url().with_idle_timeout()`. No opencode-go-specifics — a generic leaf reusable later by Zen's `/v1/chat/completions` models, OpenRouter, etc.

2. **`opencode_go.rs`** — the dedicated `OpenCodeGoProvider` implementing `Provider`. Holds `api_key`, `base_url` (default `https://opencode.ai/zen/go`), `reqwest::Client`, idle timeout, and the static per-model wire-shape map. Its `stream()` consults the map and delegates to one of two private sub-clients:
   - `OpenAICompatProvider` (with the opencode-go base_url + key) for the 8 OpenAI-compat models.
   - `AnthropicProvider` (with the opencode-go base_url + key) for the 5 Anthropic-shape models — reuses the existing `anthropic.rs` request-body + SSE-parse code as-is via `AnthropicProvider::new(key).with_base_url(go_base_url)`. No extraction/refactor needed for v1.
   - `list_models()` hits `{base}/v1/models` (OpenAI-shape `data: [{id}]`).
   - `fetch_model_info()` returns `Ok(None)`; static `MODEL_CAPS` is the fallback.

3. **`opencode_go.rs` owns the per-model wire-shape map** — a `const &[(&str, WireShape)]` listing all 13 model ids with their Go-side shape. One source of truth; Zen later gets its own map in `opencode_zen.rs`. The map is consulted only inside `OpenCodeGoProvider`.

**`zoid-model` registry changes:**
- One new `ProviderEntry`: `id: "opencode-go"`, `display: "opencode · go"`, `family: "opencode-go"`, `transport: Http { default_base_url: "https://opencode.ai/zen/go" }`, `models: &[...]` (13 ids, first = `glm-5.2`), `status: Available`. Inserted after `ollama-cloud` and before `anthropic-api` so it appears in the picker between the two ollama entries and anthropic.
- 12 new `MODEL_CAPS` entries; the existing `deepseek-v4-pro` entry corrected in place.
- No new `Transport` variant, no new `Status`, no change to `canonical_id` (no legacy alias).

**Bin (`main.rs`) changes:**
- `key_env_for` gains `Some("opencode-go") => Some("OPENCODE_GO_API_KEY")` before the existing anthropic/ollama arms.
- `select_provider` gains a `"opencode-go"` family arm.
- `provider_for_id` gains the same arm for quick-switch live-fetch.
- Settings status/secret-field lists gain `OPENCODE_GO_API_KEY`.

**No changes to:** `zoid-core`, `zoid-model`'s `Transport`/`Status`/`canonical_id`, the existing `anthropic-api` or `ollama-*` registry entries, the existing `AnthropicProvider` or `OllamaProvider` impls, the config schema (the existing `provider`/`base_url`/`model` fields carry opencode-go unchanged), the TUI (the provider picker already iterates `selectable()`), the skill/mode systems, or the event/economy/context machinery.

**Why this layout:** the new `opencode-go` family is one self-contained vertical (registry entry → key → provider impl → two sub-clients). Nothing about the existing two families changes. The `OpenAICompatProvider` is a generic leaf — when Zen lands, `opencode_zen.rs` delegates to it + `AnthropicProvider` + (future) `OpenAIResponsesProvider` + `GoogleProvider`, with its own per-model map. The seams are named for that future without building it now.

### 4.2 `OpenAICompatProvider` wire details

**Endpoint:** `POST {base}/v1/chat/completions` with `Authorization: Bearer {key}`, `Content-Type: application/json`.

**Request body** (`request_body(req: &CompletionRequest) -> Value`):
```jsonc
{
  "model": req.model,
  "stream": true,
  "stream_options": { "include_usage": true },
  "max_tokens": req.max_tokens,
  "messages": [
    { "role": "system", "content": sys },   // if present, leading
    // per-message mapping (below)
  ],
  // tools, if any (below)
}
```

**Message mapping** (mirrors `ollama.rs` with the OpenAI-specific `arguments`-as-JSON-string and `tool_call_id`):
- `MsgRole::User` → `{"role":"user","content": m.content}`
- `MsgRole::Assistant` → `{"role":"assistant","content": m.content}`; if `m.tool_calls` non-empty, add `"tool_calls": [{"id","type":"function","function":{"name","arguments": <args-as-JSON-string>}}]`. OpenAI sends `arguments` as a JSON-encoded **string**, not an object — serialize `tc.args` with `serde_json::to_string`.
- `MsgRole::Tool` → `{"role":"tool","tool_call_id": <m.tool_call_id or "">,"content": m.content}`. OpenAI identifies tool results by `tool_call_id`, not Ollama's `tool_name`. Requires the agent loop to populate `tool_call_id` on the resulting `Message::tool` (see 4.2.1).
- If `req.tools` non-empty: `"tools": [{"type":"function","function":{"name","description","parameters": t.parameters}}]`.

**Streaming response** — OpenAI Chat Completions streams as SSE (`data: {json}\n\n`, terminated by `data: [DONE]`). Parse via `eventsource_stream::Eventsource` (same as `anthropic.rs`). Per-chunk mapping (`parse_chunk(data: &str) -> Vec<ProviderEvent>`):
- `choices[0].delta.content` (non-empty string) → `TextDelta`.
- `choices[0].delta.tool_calls[]` → fragments accumulated per-`index` in a caller-held `HashMap<u32, ToolCallAccumulator>` (id + name lock on first sighting, arguments string concatenates across chunks). Flush all accumulated tool calls as `ToolCall` events at `[DONE]` (or stream end). OpenAI tool-call args arrive piecewise (unlike Ollama's whole-object); the accumulator is the new bit.
- `choices[0].finish_reason`:
  - `"stop"` → no event (`[DONE]` drives `Done`).
  - `"length"` → `Truncated` (before `Done`).
  - `"tool_calls"` → no event here (accumulated `ToolCall`s flush before `Done`).
- `usage` (present only on the final chunk when `include_usage: true`, with `choices: []`) → `Usage { input_tokens: usage.prompt_tokens, output_tokens: usage.completion_tokens, cached: usage.prompt_tokens_details.cached_tokens or 0 }`. Emitted once, before `Done`. (OpenAI's `cached_tokens` surfaces cache reads; if absent, `cached: 0`.)
- `data: [DONE]` → flush accumulated tool calls, then `Done`.
- Error objects (`{"error":{...}}`) → `Error`. Non-2xx HTTP handled by the same error-body-timeout pattern as the other two providers.

**`list_models()`** — `GET {base}/v1/models`, bearer auth. Parses the `data:[{id}]` shape (same shape as `parse_anthropic_models` at `anthropic.rs:137`; extract to a shared `parse_data_id_models` helper in `lib.rs`, or call directly).

**`fetch_model_info()`** — `Ok(None)`. The OpenAI `/v1/models` endpoint lists ids but not context windows; static `MODEL_CAPS` is the fallback.

**Idle timeout / connect timeout / error-body timeout** — reuse `crate::http_client()`, `crate::stream_idle_timeout()`, and the three-tier timeout pattern (initial send, between SSE events, error-body read) from `anthropic.rs`. `with_idle_timeout` builder for tests.

**Tracing** — `tracing::info!(kind="provider", provider="openai-compat", model=%req.model, ttft_ms, total_ms, "provider stream complete")` on success.

#### 4.2.1 `Message.tool_call_id` plumbing (existing agent loop)

The agent loop currently builds `Message::tool(name, content)` for tool results, with `tool_name` set and `tool_calls[].id` carried only on the assistant message. For OpenAI-compat, the tool result must reference the originating `tool_call_id`.

- Extend `Message` with `tool_call_id: Option<String>`. Default `None` on all existing constructors (`Message::user`, `::assistant`, `::tool`); no behavior change for existing providers (Ollama's writer ignores it and emits `tool_name`; Anthropic ignores it).
- Add `Message::tool_with_call_id(name, call_id, content)` — the agent loop's tool-result construction site (`crates/zoid/src/agent.rs`, `map_msg`/`build_request` around lines 143-189) populates `tool_call_id` from the originating `ChatMsg::ToolResult.id` **unconditionally** — the request-body writers are provider-agnostic and can't tell whether the active provider is OpenAI-compat, so populating it always is the simplest correct implementation. The existing `ollama.rs`/`anthropic.rs` request-body writers ignore the field (Ollama emits `tool_name`; Anthropic is text-only), so there's no behavior change for existing providers.
- Pure test asserts the field threads through; existing tool-dispatch tests cover it transitively.

### 4.3 `OpenCodeGoProvider` delegation & wire-shape map

**The map** (in `opencode_go.rs`):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireShape { OpenAICompat, Anthropic }

const GO_MODELS: &[(&str, WireShape)] = &[
    // OpenAI-compat: POST {base}/v1/chat/completions
    ("glm-5.2",           WireShape::OpenAICompat),
    ("glm-5.1",           WireShape::OpenAICompat),
    ("kimi-k2.7-code",    WireShape::OpenAICompat),
    ("kimi-k2.6",         WireShape::OpenAICompat),
    ("deepseek-v4-pro",   WireShape::OpenAICompat),
    ("deepseek-v4-flash", WireShape::OpenAICompat),
    ("mimo-v2.5",         WireShape::OpenAICompat),
    ("mimo-v2.5-pro",     WireShape::OpenAICompat),
    // Anthropic-shape: POST {base}/v1/messages
    ("minimax-m3",        WireShape::Anthropic),
    ("minimax-m2.7",      WireShape::Anthropic),
    ("minimax-m2.5",      WireShape::Anthropic),
    ("qwen3.7-max",       WireShape::Anthropic),
    ("qwen3.7-plus",      WireShape::Anthropic),
];
```

Lookup is exact-match on the model id passed to `stream()`. A model id not in the map defaults to **`OpenAICompat`** — the safer default since (a) 8/13 Go models are OpenAI-compat and (b) the OpenAI-compat endpoint is the one Go's `/v1/models` discovery enumerates. An unknown-model warning is logged via `tracing::warn!` so the mismatch is observable without failing the turn.

**The provider struct:**
```rust
pub struct OpenCodeGoProvider {
    api_key: String,
    base_url: String,            // "https://opencode.ai/zen/go"
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenCodeGoProvider {
    pub fn new(api_key: String) -> Self { /* default base_url from model::default_base_url("opencode-go") */ }
    pub fn with_base_url(...) -> Self { /* trim trailing slash, ignore empty */ }
    pub fn with_idle_timeout(...) -> Self { /* for tests */ }
    fn wire_shape_for(&self, model: &str) -> WireShape { /* map lookup or OpenAICompat default */ }
}
```

**Delegation in `stream()`:**
```rust
async fn stream(&self, req: &CompletionRequest, sink: ...) -> Result<()> {
    match self.wire_shape_for(&req.model) {
        WireShape::OpenAICompat => {
            OpenAICompatProvider::new(self.api_key.clone())
                .with_base_url(&self.base_url)
                .with_idle_timeout(self.idle_timeout)
                .stream(req, sink).await
        }
        WireShape::Anthropic => {
            AnthropicProvider::new(self.api_key.clone())
                .with_base_url(&self.base_url)
                .with_idle_timeout(self.idle_timeout)
                .stream(req, sink).await
        }
    }
}
```

A fresh sub-client is constructed per `stream()` call. This is cheap (clones two `String`s + reuses a `reqwest::Client` pool internally if we share one) and keeps each delegation stateless.

**Shared HTTP client (small optimization):** `OpenAICompatProvider::new` and `AnthropicProvider::new` each call `crate::http_client()`, building a fresh `reqwest::Client`. To avoid doubling that per Go turn, `OpenCodeGoProvider` constructs **one** `reqwest::Client` up front and passes it into sub-clients via a new `with_client(client)` builder on each — a non-breaking addition. If deferred for simplicity, the per-turn cost is negligible; flag for a follow-up.

**`list_models()`** — `GET {base}/v1/models`, bearer auth. Reuses the `data:[{id}]` parser. Returns Go's live model list; the registry's static 13 is the fallback if the fetch fails (via the existing `AgentUpdate::ModelsFetched` → picker path).

**`fetch_model_info()`** — `Ok(None)`. Static `MODEL_CAPS` is the fallback for all 13 Go models.

**Tracing** — sub-clients each emit their own `provider="openai-compat"` / `provider="anthropic"` line; `OpenCodeGoProvider` does not add a second wrapper line (the sub-client's trace names the wire shape, `model=%req.model` disambiguates).

**What this design deliberately does NOT do:**
- No caching of sub-clients on the struct (stateless delegation).
- No shared `OpenCodeGoProvider`-level state mutated across turns.
- No special handling for the free `big-pickle` model (not in Go's 13; if it appears in `/v1/models` live, the OpenAICompat default handles it).
- No Go-specific retry/rate-limit handling.

### 4.4 Registry, `MODEL_CAPS`, and bin wiring

**Registry entry** (added to `PROVIDERS` in `crates/zoid-model/src/lib.rs`, after `ollama-cloud`, before `anthropic-api`):
```rust
ProviderEntry {
    id: "opencode-go",
    display: "opencode · go",
    family: "opencode-go",
    transport: Transport::Http {
        default_base_url: "https://opencode.ai/zen/go",
    },
    models: &[
        "glm-5.2", "glm-5.1",
        "kimi-k2.7-code", "kimi-k2.6",
        "deepseek-v4-pro", "deepseek-v4-flash",
        "mimo-v2.5", "mimo-v2.5-pro",
        "minimax-m3", "minimax-m2.7", "minimax-m2.5",
        "qwen3.7-max", "qwen3.7-plus",
    ],
    status: Status::Available,
},
```
First model (`glm-5.2`) is the default. No `canonical_id` alias (no legacy id to remap).

**`MODEL_CAPS` — final reconciled table** (12 new + 1 in-place correction):

| Model id | ctx window | max_output | tools | prompt_cache | Source / note |
|---|---|---|---|---|---|
| `glm-5.2` | 1,000,000 | 0 | true | true | reconciled w/ existing `glm-5.2:cloud` |
| `glm-5.1` | 1,000,000 | 0 | true | true | inferred from GLM-5.x sibling |
| `kimi-k2.7-code` | 262,144 | 0 | true | true | Moonshot docs (confirmed) |
| `kimi-k2.6` | 262,144 | 0 | true | true | Moonshot docs (confirmed) |
| `deepseek-v4-pro` | **1,000,000** | **384,000** | true | **true** | **CORRECTED** existing entry (was 128k/0/false) via api-docs.deepseek.com |
| `deepseek-v4-flash` | 1,000,000 | 384,000 | true | true | DeepSeek docs (confirmed) |
| `mimo-v2.5` | 128,000 | 0 | true | true | unconfirmed — approx; override via `ZOID_CONTEXT_CEILING` |
| `mimo-v2.5-pro` | 128,000 | 0 | true | true | unconfirmed — approx; override via `ZOID_CONTEXT_CEILING` |
| `minimax-m3` | 200,000 | 0 | false | true | unconfirmed — approx; tools false (Anthropic P1b) |
| `minimax-m2.7` | 200,000 | 0 | false | true | unconfirmed — approx; tools false (Anthropic P1b) |
| `minimax-m2.5` | 200,000 | 0 | false | true | unconfirmed — approx; tools false (Anthropic P1b) |
| `qwen3.7-max` | 256,000 | 0 | false | true | unconfirmed — approx; tools false (Anthropic P1b) |
| `qwen3.7-plus` | 256,000 | 0 | false | true | unconfirmed — approx; tools false (Anthropic P1b) |

**`tools` flag rationale:** `tools` means "zoid's provider implementation can actually wire tool-calling for this model" — not the model's raw capability (the existing "capability lie" comments at `zoid-model/src/lib.rs:113-118`).
- `true` for the 8 OpenAI-compat models: the new `openai_compat.rs` implements tool-calling end-to-end (sends `tools`, parses `tool_calls` with the fragment accumulator, plumbs `tool_call_id`). DeepSeek, Kimi, GLM, MiMo all natively support tool-calling.
- `false` for the 5 Anthropic-shape models: forced by the existing text-only `AnthropicProvider` (P1b). The underlying MiniMax/Qwen models **do** support tool-calling natively, but zoid's Anthropic client can't speak it yet. Parallel to the existing `claude-sonnet-4-6`/`claude-opus-4-8` stance. Flips to `true` when the deferred P1b.1 Anthropic `tool_use`/`tool_result` mapping lands.

**`prompt_cache` flag rationale:** `true` for all 13 Go models — Go advertises cached-read pricing for every one of them (GLM $0.26, DeepSeek $0.0028, Kimi $0.19, MiniMax $0.06, Qwen $0.04–0.50, MiMo $0.0028). Both clients parse the standard cache-read fields: OpenAI-compat reads `usage.prompt_tokens_details.cached_tokens`; Anthropic-shape reads `cache_read_input_tokens` (existing `anthropic.rs:99-102`) and the request body sends `cache_control: {type: "ephemeral"}` breakpoints.
- **Caveat (post-merge verification):** this is an optimistic stance. If empirical testing against the live Go endpoint shows the field is absent or always-zero, flip the affected models to `false` (the UI would otherwise show a misleading 0 sparkline instead of honest "n/a"). The spec lists this as a manual verification task, not a CI test.
- The corrected `deepseek-v4-pro` entry also flips `prompt_cache` to `true` (was `false`, set when it was Ollama-cloud-only). DeepSeek's own docs describe a Context Caching feature with cached-read pricing at $0.0028 vs $0.14 uncached — a real cache exists.

Unconfirmed entries carry `// unconfirmed — approx from public claims; override via ZOID_CONTEXT_CEILING` comments in the source. The corrected `deepseek-v4-pro` entry carries `// corrected via api-docs.deepseek.com (was 128_000/0/false)`.

**Existing test update:** two existing tests assert the current wrong `deepseek-v4-pro` values and must be updated in this slice so `cargo test --workspace` stays green:
- `zoid-model/src/lib.rs:224-225` (`model_info_exact_lookup`): `context_window == 128_000`, `!prompt_cache` → update to `1_000_000` / `prompt_cache == true` (and assert `max_output == 384_000`).
- `zoid-model/src/lib.rs:240` (`model_info_case_insensitive`): `model_info("DEEPSEEK-V4-PRO").context_window == 128_000` → update to `1_000_000`.

The §6 regression test is the same assertion, kept explicit in the test module.

**Bin (`main.rs`) changes — three functions:**

1. **`key_env_for`** — new arm before the existing family branch:
   ```rust
   match zoid_provider::model::entry(id).map(|e| e.family) {
       Some("opencode-go") => Some("OPENCODE_GO_API_KEY"),
       Some("anthropic") => Some("ANTHROPIC_API_KEY"),
       _ => Some("OLLAMA_API_KEY"),
   }
   ```
   `entry_requires_key("opencode-go")` returns `true` (not `ollama-local`); no change to that function.

2. **`select_provider`** — new family arm:
   ```rust
   "opencode-go" => match key_for("OPENCODE_GO_API_KEY") {
       Some(k) => (
           Arc::new(zoid_provider::opencode_go::OpenCodeGoProvider::new(k)
               .with_base_url(base_url)),
           "opencode-go",
           true,
       ),
       None => (default_provider(), "opencode-go", false),
   },
   ```

3. **`provider_for_id`** — same arm for quick-switch live fetch.

**Settings UI** — the `key_status` array at `main.rs:2013-2016` (iterared generically by `zoid_tui::config_view::build_sections`) gains `OPENCODE_GO_API_KEY` alongside `OLLAMA_API_KEY` / `ANTHROPIC_API_KEY`. The settings-redesign spec's "selecting a key-requiring provider prompts for the API key" flow works unchanged for `opencode-go`.

**`default_provider()` and `default_model()` in `zoid-provider/src/lib.rs`** — **unchanged.** The env-var precedence (`OLLAMA_API_KEY` → ollama, `ANTHROPIC_API_KEY` → anthropic, else `FakeProvider`) stays as-is; `OPENCODE_GO_API_KEY` is not added to that chain. opencode-go is reachable only via explicit config (`provider = "opencode-go"`) or the picker — not a default-provider candidate. This matches how `anthropic-api` already works.

**Config file** — a user adds:
```toml
provider = "opencode-go"
model = "glm-5.2"
```
and sets `OPENCODE_GO_API_KEY` in env or via the settings secret store. No new config fields, no schema change.

## 5. Known day-one functional gap

The 5 Anthropic-shape Go models (MiniMax M3/M2.7/M2.5, Qwen3.7 Max/Plus) **cannot call tools from zoid on day one**. This is a zoid-implementation limitation (the existing `AnthropicProvider` is text-only P1b), not a model limitation — the underlying models support tool-calling natively, and Go serves them via Anthropic `/v1/messages` which has a tool-use schema zoid's client doesn't yet speak.

The `tools: false` flag in `MODEL_CAPS` honestly advertises this (no "capability lie"). The gap closes when the deferred P1b.1 Anthropic `tool_use`/`tool_result` wire mapping lands — that work also fixes Claude's tool support and is a separate, larger spec.

Users who need tool-calling against Go on day one should pick one of the 8 OpenAI-compat models (GLM, Kimi, DeepSeek, MiMo).

## 6. Testing & verification

**Conventions** (mirror existing): inline `#[cfg(test)] mod tests` per file; `#[test]` for pure logic; `#[tokio::test]` for async/streaming; throwaway `tokio::net::TcpListener` for timeout/error-path tests (pattern at `ollama.rs:773-789`); no external test crates beyond the workspace's `proptest`/`insta`/`tempfile`. No live-endpoint / network integration tests.

### `openai_compat.rs` (new)
- **`request_body` (pure, ~8 tests):** body shape; `system` prepended; no `tools` key when empty; assistant `tool_calls` emits `arguments` as JSON-encoded **string**; `MsgRole::Tool` emits `tool_call_id`; tools array shape.
- **`parse_chunk` (pure, ~10 tests):** `delta.content` → `TextDelta`; empty/absent → none; `finish_reason:"length"` → `Truncated`; `finish_reason:"stop"`/`"tool_calls"` → none; `usage` → `Usage` with `cached_tokens` populating `cached` (and absent → 0); `data: [DONE]` → flush + `Done`; `{"error":{...}}` → `Error`; malformed/empty → none.
- **Tool-call fragment accumulator (pure, ~4 tests):** single-chunk; two-chunk (id+name then args fragment); two distinct tool calls interleaved; arguments JSON-string re-parsed to object for `ToolCall.args`.
- **`OpenAICompatProvider` streaming (tokio, ~3 tests):** happy path (SSE chunks + `[DONE]` → ordered events); stalled stream → idle-timeout `Error` (mirrors `ollama.rs:804`); non-2xx with stalled body → HTTP status in `Error` (mirrors `ollama.rs:850`).
- **`list_models` (tokio, ~1 test):** `data:[{id}]` → parsed list.

### `opencode_go.rs` (new)
- **Wire-shape map (pure, ~3 tests):** `wire_shape_for` returns the documented shape for each of the 13 registry ids (table-driven); unknown model → `OpenAICompat` default + `tracing::warn!`.
- **Delegation routing (tokio, ~2 tests):** `TcpListener` server records the request path; for an OpenAI-compat model the request hits `/v1/chat/completions` with an OpenAI-shaped body; for an Anthropic-shape model it hits `/v1/messages` with an Anthropic-shaped body. `with_base_url` propagates to the sub-client (recorded URL uses the override, not the default).

### `zoid-model/src/lib.rs`
- `entry("opencode-go")` returns the new entry; `family == "opencode-go"`; `default_base_url("opencode-go") == Some("https://opencode.ai/zen/go")`; `models_for("opencode-go")` returns the 13 ids with `glm-5.2` first; `selectable()` includes `opencode-go` and still excludes `anthropic-cli`/`anthropic-sdk` (regression).
- `model_info` for each of the 13 ids returns the reconciled caps (table-driven test asserting context_window, tools, prompt_cache per §4.4).
- **Regression test for the `deepseek-v4-pro` correction:** `model_info("deepseek-v4-pro").context_window == 1_000_000`, `max_output == 384_000`, `prompt_cache == true`. Locks the fix so a future accidental revert surfaces.
- `canonical_id("opencode-go") == "opencode-go"` (no alias).

### `crates/zoid/src/main.rs`
- `key_env_for("opencode-go") == Some("OPENCODE_GO_API_KEY")`; `key_env_for("ollama-local") == None` (regression); `key_env_for("anthropic-api") == Some("ANTHROPIC_API_KEY")` (regression); `entry_requires_key("opencode-go") == true`.
- `select_provider` / `provider_for_id` for `opencode-go`: with `OPENCODE_GO_API_KEY` set, returns an `OpenCodeGoProvider`; without the key, falls back to `default_provider()` with `provider_label == "opencode-go"` and `ready == false`. (Env-var manipulation is process-global; tests use the existing isolation pattern at `main.rs:4156-4165`.)

### `Message` struct change (`zoid-provider/src/lib.rs`)
- Existing `Message::user`/`assistant`/`tool` constructors default `tool_call_id` to `None` (existing tests pass unchanged).
- `Message::tool_with_call_id(name, call_id, content)` sets the field; pure test asserts it threads through; the agent loop's tool-dispatch path populates it from the originating `ToolCall.id`.

### Not tested in CI (documented)
- **Live Go endpoint:** all tests run offline against stubbed `TcpListener` servers (matches the existing `ollama.rs`/`anthropic.rs` stance).
- **`prompt_cache: true` empirical validation:** the optimistic-true stance is verified by running zoid against a live Go endpoint and observing the cache sparkline. Listed as a post-merge verification task for the user; if it shows a misleading 0, flip `prompt_cache` to `false` for the affected models per the §4.4 caveat.
- **The 5 Anthropic-shape models' tool-calling:** no test asserts tool-calling works for MiniMax/Qwen on day one (it doesn't — P1b limitation). Tests assert `model_info("minimax-m3").tools == false` to lock the honest stance.
- **Rate-limit / $12-per-5h budget handling:** out of scope; Go's HTTP 429 flows through the existing error surfacing.

### Definition of Done
1. `cargo test --workspace` green (all new + existing tests pass).
2. `cargo fmt --check` + `cargo clippy --workspace -- -D warnings` clean (workspace conventions).
3. The 13 Go model ids appear in the provider picker when `opencode-go` is selected.
4. **Manual smoke (documented, not CI):** set `OPENCODE_GO_API_KEY`, `provider = "opencode-go"`, `model = "glm-5.2"`, run a turn; assert streaming text + tool-calling works against the live endpoint. Repeat for one Anthropic-shape model (e.g. `qwen3.7-max`) — assert streaming text works, tools are not advertised (honest `tools: false`).
5. **Manual smoke:** confirm the cache sparkline renders (non-"n/a") for a Go model with cached-read pricing; if it shows a misleading 0, flip `prompt_cache` to `false` for the affected models per the §4.4 caveat.

## 7. Open questions / follow-ups

- **P1b.1 — Anthropic tool-calling:** lifts the 5 Anthropic-shape Go models (and Claude) from `tools: false` to `true`. Separate spec.
- **OpenCode Zen:** separate `opencode-zen` registry entry + `opencode_zen.rs` provider with its own per-model wire-shape map covering 4 wire shapes (OpenAI Responses, OpenAI Chat Completions, Anthropic Messages, Google). Adds `OpenAIResponsesProvider` + `GoogleProvider` clients. The seams in this spec are named for it.
- **Shared `reqwest::Client` plumbing:** the `with_client(client)` builder on `OpenAICompatProvider`/`AnthropicProvider` is optional in v1; if deferred, the per-turn `http_client()` cost is negligible.