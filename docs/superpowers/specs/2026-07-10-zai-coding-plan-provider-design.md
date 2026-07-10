# ZAI Coding Plan Provider + GLM 5.2

## Problem & approach

Add a new LLM provider `zai-coding-plan` that reaches Zhipu AI's GLM models via
the Coding Plan endpoint. Start with `glm-5.2` only.

ZAI's Coding Plan API is OpenAI Chat Completions–compatible — SSE streaming,
tools, thinking, and the usage shape all match what `openai_compat.rs` already
parses. One wrinkle: ZAI's endpoint paths are `{base}/chat/completions` and
`{base}/models` — the `/v1/` segment that `OpenAICompatProvider` hardcodes is
absent (ZAI already includes `/v4` in its base URL, and `/v1` 404s).

**Approach: thin ZAI provider module (C).** A `zai.rs` provider module delegates
`stream`/`list_models` to `OpenAICompatProvider`, with the one change being that
the OpenAI-compat leaf gains a configurable path prefix instead of a hardcoded
`/v1`. The ZAI module sets the prefix to empty. No new HTTP/parsing logic.

## Live API findings (probed against `https://api.z.ai/api/coding/paas/v4`)

- **Base URL:** `https://api.z.ai/api/coding/paas/v4` (Coding Plan endpoint; the
  `/api/openai/v1` path 404s for Coding Plan keys).
- **Chat completions:** `POST {base}/chat/completions` (no `/v1/` prefix).
- **Models list:** `GET {base}/models` (no `/v1/` prefix), standard
  `{data:[{id}]}` shape — `parse_data_id_models` handles parsing.
- **Model id:** `glm-5.2`.
- **Context window:** 1,048,576 (1M) — confirmed via ZAI docs + OpenRouter +
  NVIDIA NIM.
- **Max output:** 131,072 — confirmed via OpenRouter + NVIDIA NIM.
- **Tools:** Yes — standard OpenAI `tool_calls` shape with `id`, `index`,
  `function.{name,arguments}`.
- **Thinking:** DeepSeek wire shape — `thinking:{type:"enabled"|"disabled"}` +
  `reasoning_effort:"high"|"max"`. `reasoning_content` arrives in
  `delta.reasoning_content` (parsed by existing `openai_compat` →
  `ThinkingDelta`). Thinking is on by default (consumed tokens when no params
  were sent).
- **Prompt cache:** Reportable — usage includes
  `prompt_tokens_details.cached_tokens` (0 on cold requests).
- **Usage:** `prompt_tokens`, `completion_tokens`,
  `prompt_tokens_details.cached_tokens`, `completion_tokens_details.reasoning_tokens`
  — the existing `parse_chunk` reads the first three. (`reasoning_tokens` is not
  surfaced as `thinking_tokens` — existing behavior across all OpenAI-compat
  providers; out of scope here.)
- **finish_reason:** `stop`, `length`, `tool_calls` — all standard; existing
  `parse_chunk` handles them.
- **`stream_options:{include_usage:true}`** is sent by the builder but isn't
  required by ZAI — usage arrives on the final chunk regardless.

## `zoid-model` registry changes

### New `ProviderEntry`

Added to `PROVIDERS` after `opencode-go`:

```rust
ProviderEntry {
    id: "zai-coding-plan",
    display: "zai · coding plan",
    family: "zai",
    transport: Transport::Http {
        default_base_url: "https://api.z.ai/api/coding/paas/v4",
    },
    models: &["glm-5.2"],   // first = default
    status: Status::Available,
}
```

The `family: "zai"` seam keeps the door open for a future `zai-api` (direct
per-token) entry without touching key wiring — mirrors the `ollama-local` /
`ollama-cloud` two-entries-one-family pattern.

### `MODEL_CAPS` — update existing `glm-5.2` entry

The `glm-5.2` model id is shared across the opencode-go and zai providers
(single source of truth, consistent with how `deepseek-v4-pro` is shared).
Corrections from live probing + ZAI docs:

| field            | current | new              | source                          |
|------------------|---------|------------------|---------------------------------|
| `context_window` | 1,000,000 | 1,000,000      | confirmed (ZAI 1M)              |
| `max_output`     | 0       | 131,072          | confirmed (OpenRouter / NIM)    |
| `tools`          | true    | true             | confirmed (live tool-call)      |
| `prompt_cache`   | true    | true             | confirmed (`cached_tokens` field)|
| `thinking`       | None    | ToggleWithEffort | confirmed (`thinking:{type}` + `reasoning_effort`)|
| `thinking_wire`  | None    | DeepSeek         | confirmed (matches deepseek builder)|

## `zoid-provider` changes

### `openai_compat.rs` — parameterize the path prefix

Add a `path_prefix: String` field to `OpenAICompatProvider` (default `"/v1"`).
A new builder `with_path_prefix(prefix)` lets callers override it (empty string
for ZAI). The two `format!` sites change:

- `format!("{}/v1/chat/completions", self.base_url)` →
  `format!("{}{}/chat/completions", self.base_url, self.path_prefix)`
- `format!("{}/v1/models", self.base_url)` →
  `format!("{}{}/models", self.base_url, self.path_prefix)`

Default behavior (opencode-go, existing tests that use `OpenAICompatProvider`)
is unchanged — the default `/v1` prefix preserves the old URLs.

### New `zai.rs` module

Mirrors `opencode_go.rs` structure:

```rust
pub struct ZaiProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,   // reserved (same as opencode-go)
    idle_timeout: Duration,
}

impl ZaiProvider {
    pub fn new(api_key: String) -> Self
    // base_url from registry default_base_url("zai-coding-plan")
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self
}

impl Provider for ZaiProvider {
    async fn stream(&self, req, sink) -> Result<()> {
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_path_prefix("")          // ← the one difference
            .with_idle_timeout(self.idle_timeout)
            .stream(req, sink)
            .await
    }
    async fn list_models(&self) -> Result<Vec<String>> {
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_path_prefix("")
            .with_idle_timeout(self.idle_timeout)
            .list_models()
            .await
    }
}
```

No wire-shape map needed — GLM 5.2 is OpenAI-compat only.

### `lib.rs`

Add `pub mod zai;`.

## `zoid` (main.rs) wiring

Four touch points, following the existing `family`-based dispatch:

1. **`key_env_for`** — add `Some("zai") => Some("ZAI_API_KEY")` to the match on
   `entry(id).map(|e| e.family)` (alongside `opencode-go` and `anthropic`).
2. **`select_provider`** — add a `"zai" =>` arm in the `match family` block that
   builds `ZaiProvider::new(k).with_base_url(base_url)`, falling back to
   `default_provider()` on no key (mirroring the opencode-go arm).
3. **`provider_for_id`** — add the same `"zai"` arm for the live model-fetch
   path.
4. **`key_status`** — add `("ZAI_API_KEY", status("ZAI_API_KEY"))` so the config
   UI shows it as a secret row.

## Testing

- **`zai.rs` tests:** a recording-server test (reusing the pattern from
  `opencode_go.rs`) verifying the request line hits `/chat/completions` (no
  `/v1/`), and a `with_base_url` URL-propagation test.
- **`openai_compat.rs` tests:** a regression test confirming the default path
  prefix still emits `/v1/chat/completions` (guards the backward-compat
  default).
- **`zoid-model` tests:**
  - Update `glm_models_have_no_thinking` → `glm_5.2_has_thinking_with_effort`
    (asserts `ToggleWithEffort` / `DeepSeek`).
  - Add `max_output` (131,072) assertions for `glm-5.2`.
  - Update `selectable_has_four_providers` → five providers; add `zai-coding-plan`
    to the list.
  - Add a `zai-coding-plan` registry entry test (id, family, base URL, models,
    status).
- **`main.rs` tests:** add `key_env_for("zai-coding-plan")` →
  `Some("ZAI_API_KEY")` assertion.

## Out of scope (YAGNI)

- The `zai-api` (direct per-token) entry — reserved by the `family: "zai"` seam
  but not built.
- Other GLM models (`glm-5.1`, `glm-4.7`, the flash models) — registry lists
  `glm-5.2` only; users can free-text others, and they'll fall back to
  `DEFAULT_MODEL_INFO`.
- `reasoning_tokens` surfacing as `thinking_tokens` — the existing `parse_chunk`
  reports `thinking_tokens: 0` (it doesn't read
  `completion_tokens_details.reasoning_tokens`). That's existing behavior across
  all OpenAI-compat providers; out of scope here.