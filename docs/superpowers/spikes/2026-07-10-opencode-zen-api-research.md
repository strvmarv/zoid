# OpenCode Zen Provider — API Research

**Date:** 2026-07-10
**Endpoint base:** `https://opencode.ai/zen`
**Auth:** Single API key (`sk-...`), but header varies by wire shape:
  - OpenAI Chat Completions / OpenAI Responses: `Authorization: Bearer <key>`
  - Anthropic Messages: `x-api-key: <key>` + `anthropic-version: 2023-06-01`
  - Google Gemini: `x-goog-api-key: <key>`

## Four Wire Shapes

### 1. OpenAI Chat Completions — `POST /v1/chat/completions`
- Auth: `Authorization: Bearer <key>`
- Standard OpenAI chat completions body (`model`, `messages`, `max_tokens`, `tools`, `stream`)
- SSE streaming with `stream_options: { include_usage: true }`
- Usage: `usage.prompt_tokens_details.cached_tokens`
- **Already implemented** in `OpenAICompatProvider`

### 2. Anthropic Messages — `POST /v1/messages`
- Auth: `x-api-key: <key>` + `anthropic-version: 2023-06-01`
- Standard Anthropic messages body (`model`, `messages`, `max_tokens`, `system`, `tools`, `stream`)
- SSE streaming with `event:` / `data:` SSE events
- Cache breakpoints via `cache_control`
- **Already implemented** in `AnthropicProvider`

### 3. OpenAI Responses — `POST /v1/responses`
- Auth: `Authorization: Bearer <key>`
- Body: `{ model, input, instructions, max_output_tokens, stream, tools }`
  - `input` can be a string or array of `{role, content}` items
  - `instructions` is the system prompt equivalent
  - Content items use `{type: "input_text", text: "..."}` shape
- SSE streaming events: `response.created`, `response.in_progress`,
  `response.output_item.added`, `response.content_part.added`, ...,
  `response.completed`
- Usage: `usage.input_tokens`, `usage.output_tokens`,
  `usage.input_tokens_details.cached_tokens`
- **NOT yet implemented** — needs new `OpenAIResponsesProvider`

### 4. Google Gemini — `POST /v1/models/{model}:generateContent`
- Auth: `x-goog-api-key: <key>` (NOT Bearer!)
- Body: `{ contents: [{ role, parts: [{ text }] }], generationConfig: { maxOutputTokens } }`
- Streaming: `POST /v1/models/{model}:streamGenerateContent?alt=sse`
- **NOT yet implemented** — needs new `GoogleGeminiProvider`

## Model Catalog (52 models, 4 wire shapes)

### OpenAI Responses (17 models)
| Model ID | Endpoint |
|---|---|
| gpt-5.5 | /v1/responses |
| gpt-5.5-pro | /v1/responses |
| gpt-5.4 | /v1/responses |
| gpt-5.4-pro | /v1/responses |
| gpt-5.4-mini | /v1/responses |
| gpt-5.4-nano | /v1/responses |
| gpt-5.3-codex | /v1/responses |
| gpt-5.3-codex-spark | /v1/responses |
| gpt-5.2 | /v1/responses |
| gpt-5.2-codex | /v1/responses |
| gpt-5.1 | /v1/responses |
| gpt-5.1-codex | /v1/responses |
| gpt-5.1-codex-max | /v1/responses |
| gpt-5.1-codex-mini | /v1/responses |
| gpt-5 | /v1/responses |
| gpt-5-codex | /v1/responses |
| gpt-5-nano | /v1/responses |

### Anthropic Messages (12 models)
| Model ID | Endpoint |
|---|---|
| claude-fable-5 | /v1/messages |
| claude-opus-4-8 | /v1/messages |
| claude-opus-4-7 | /v1/messages |
| claude-opus-4-6 | /v1/messages |
| claude-opus-4-5 | /v1/messages |
| claude-sonnet-5 | /v1/messages |
| claude-sonnet-4-6 | /v1/messages |
| claude-sonnet-4-5 | /v1/messages |
| claude-haiku-4-5 | /v1/messages |
| qwen3.7-max | /v1/messages |
| qwen3.7-plus | /v1/messages |
| qwen3.6-plus | /v1/messages |
| qwen3.5-plus | /v1/messages |

Note: Pricing table lists `qwen3.7-max` but API returns `qwen3.7-plus` only.
The docs list 4 Qwen models; the API has 4 qwen IDs. The doc endpoint table
lists qwen3.7-max through qwen3.5-plus all on /v1/messages.

### OpenAI Chat Completions (19 models)
| Model ID | Endpoint |
|---|---|
| deepseek-v4-pro | /v1/chat/completions |
| deepseek-v4-flash | /v1/chat/completions |
| deepseek-v4-flash-free | /v1/chat/completions |
| minimax-m3 | /v1/chat/completions |
| minimax-m2.7 | /v1/chat/completions |
| minimax-m2.5 | /v1/chat/completions |
| glm-5.2 | /v1/chat/completions |
| glm-5.1 | /v1/chat/completions |
| glm-5 | /v1/chat/completions |
| kimi-k2.5 | /v1/chat/completions |
| kimi-k2.6 | /v1/chat/completions |
| kimi-k2.7-code | /v1/chat/completions |
| grok-4.5 | /v1/chat/completions |
| grok-build-0.1 | /v1/chat/completions |
| big-pickle | /v1/chat/completions |
| mimo-v2.5-free | /v1/chat/completions |
| north-mini-code-free | /v1/chat/completions |
| nemotron-3-ultra-free | /v1/chat/completions |
| hy3-free | /v1/chat/completions |

Note: `hy3-free` appears in the /v1/models API response but not in the docs
endpoint table. It's likely OpenAI-compat (chat/completions) based on naming
convention matching the other `*-free` models.

### Google Gemini (3 models)
| Model ID | Endpoint |
|---|---|
| gemini-3.5-flash | /v1/models/{model}:generateContent |
| gemini-3.1-pro | /v1/models/{model}:generateContent |
| gemini-3-flash | /v1/models/{model}:generateContent |

### Deprecated models (from API but not docs pricing table)
These appear in /v1/models but are deprecated per the docs:
- claude-opus-4-1 (dep Aug 5, 2026)
- claude-sonnet-4 (dep Jun 15, 2026)

## list_models() approach

The `/v1/models` endpoint returns all 52 models in standard OpenAI format:
```json
{"object":"list","data":[{"id":"glm-5.2","object":"model","created":...,"owned_by":"opencode"},...]}
```

Two options:
1. **Static table** (like opencode-go): hardcode the 52-model wire-shape map.
   Pros: no startup latency, works offline. Cons: drifts when Zen adds models.
2. **Dynamic fetch + heuristic**: call `/v1/models` for the list, then route
   each model to a wire shape by calling the endpoint and checking the response.
   Pros: always current. Cons: startup latency, can't know wire shape without
   trying endpoints.

**Recommendation:** Static table, same as opencode-go. The wire shape is
determined by the model's upstream provider (OpenAI/Anthropic/Google/other),
not by a runtime probe. The static table is the only reliable way to know
which endpoint to POST to without trial-and-error.

## Key differences from opencode-go

1. **Base URL:** `https://opencode.ai/zen` (vs `https://opencode.ai/zen/go`)
2. **Four wire shapes** (vs two for go): adds OpenAI Responses + Google Gemini
3. **Two new sub-providers needed:** `OpenAIResponsesProvider`, `GoogleGeminiProvider`
4. **Gemini auth is different:** `x-goog-api-key` header, not `Authorization: Bearer`
5. **Gemini URL is per-model:** `/v1/models/{model}:generateContent` (not a shared endpoint)
6. **52 models** (vs ~13 for go)
7. **`/v1/models` works with Bearer auth** (same key works for all shapes)

## Confirmed working (2026-07-10)

- `/v1/chat/completions` with `glm-5.2` — 200 ✓
- `/v1/messages` with `claude-sonnet-4-5` — 200 ✓
- `/v1/responses` with `gpt-5.4-nano` — 200 ✓ (stream + non-stream)
- `/v1/models/gemini-3-flash:generateContent` with `x-goog-api-key` — 200 ✓

## Tool-call captures (2026-07-11) — resolves spec open questions #3 and #4

### OpenAI Responses: `call_id` vs `item_id` (spec §9 q3)

**RESOLVED: `call_id` ≠ `item_id`. They are distinct values.** The
`function_call_output` input item on the next turn MUST use `call_id`, not
`item_id`.

Non-streaming response `output[0]`:
```json
{
  "id": "fc_064905c2d2b6eb62006a530939c8b081949c6a609833d343e3",   // item_id
  "type": "function_call",
  "name": "get_weather",
  "call_id": "call_yQdsSPBGsR5JFINRZS9p9b1l",                       // call_id (DIFFERENT)
  "arguments": "{\"location\":\"San Francisco\"}"
}
```

Streaming event sequence (tool-bearing call):
- `response.output_item.added` → carries the **full item including `call_id`**:
  `{"item":{"id":"fc_...","type":"function_call","call_id":"call_...","arguments":""}}`
- `response.function_call_arguments.delta` → carries **`item_id` only** (no `call_id`):
  `{"item_id":"fc_...","delta":"{\""}`
- `response.function_call_arguments.done` → carries **`item_id` only** (no `call_id`):
  `{"item_id":"fc_...","arguments":"{\"location\":\"San Francisco\"}"}`
- `response.output_item.done` → carries the **full item including `call_id`** (same as `.added`).

**Implementation impact:** the accumulator must learn `call_id` from
`response.output_item.added` (keyed by `item_id`) and emit it when flushing
on `response.function_call_arguments.done`. Using `item_id` as the tool-call
id (the plan's original assumption) would break the next-turn
`function_call_output` round-trip — the provider can't match the result back.

### Google Gemini: `functionCall.id` (spec §9 q4)

**RESOLVED: `functionCall.id` is ABSENT.** Zen's Gemini `functionCall` parts
carry only `name` and `args`:
```json
{"functionCall":{"name":"get_weather","args":{"location":"San Francisco"}}}
```
No `id` field at all. The fallback (empty-string id, matching Ollama's
call-id-less shape) is correct.

### Google Gemini: thought parts are NOT `thought:true, text:"..."` (NEW)

**The spec's §4.4 `thought:true, text` part shape does NOT match Zen.** Zen's
Gemini surfaces thinking via an opaque `thoughtSignature` (base64 blob) on
**empty-text parts**, not readable thought text:

Streaming (with `thinkingConfig.includeThoughts: true`):
- First chunk: `{"text":"Hello."}` + `"usageMetadata":{}`
- Final chunk: `{"text":"","thoughtSignature":"AY89a1+cmbdq5mYb..."}` +
  `"finishReason":"STOP"` + full `usageMetadata`

There is **no** `thought:true` boolean and **no** readable thought text —
thoughts are encoded in the opaque signature, not surfaced as deltas.

**Implementation impact:** the plan's `parse_thought_part_yields_thinking_delta`
test fixture (`{"thought":true,"text":"pondering"}`) does not match Zen's
actual shape. For v1, thoughts are silently dropped (no readable delta). The
`parse_chunk` thought handling should check for `thought:true` AND
`thoughtSignature` (ignore both), not assume readable text. The
`ThinkingDelta` emit path is dead code for Zen's Gemini — keep the parse
logic (it's correct per Google's schema for direct-Gemini use) but don't
expect Zen to exercise it.

### Google Gemini: `{"type":"ping"}` SSE events (NEW)

Zen's Gemini stream injects non-standard `{"type":"ping","cost":"..."}` and
`{"type":"ping","x-opencode-type":"inference-cost",...}` events between
content chunks. These carry no `candidates` array and no standard
`usageMetadata` shape. `parse_chunk` naturally ignores them (no `candidates` →
empty vec; `usageMetadata` absent → no Usage emit), but the implementation
must not crash on the `type` field being present.

### Google Gemini: `usageMetadata` field shape confirmed

Final-chunk `usageMetadata`:
```json
{
  "candidatesTokensDetails": [{"modality":"TEXT","tokenCount":2}],
  "promptTokensDetails": [{"modality":"TEXT","tokenCount":6}],
  "candidatesTokenCount": 2,
  "promptTokenCount": 6,
  "thoughtsTokenCount": 71,
  "totalTokenCount": 79
}
```
- `cachedContentTokenCount` is **absent** (not zero — the field is missing).
  `unwrap_or(0)` is correct; Gemini cached reads always report 0.
- `thoughtsTokenCount` maps to zoid's `thinking_tokens`.

## Gotchas discovered during research

### Gemini `usageMetadata` on every chunk

Zen's Gemini `streamGenerateContent?alt=sse` emits `usageMetadata` on **every**
chunk, not just the final one. Intermediate chunks carry `"usageMetadata":{}`
(empty object). Only the final chunk has real values:
```json
"usageMetadata": {
    "candidatesTokensDetails": [{"modality":"TEXT","tokenCount":1}],
    "promptTokensDetails": [{"modality":"TEXT","tokenCount":5}],
    "candidatesTokenCount": 7,
    "promptTokenCount": 10,
    "thoughtsTokenCount": 189,
    "totalTokenCount": 206
}
```

**Parse logic must gate on `promptTokenCount` being present**, not on
`usageMetadata` existing — an empty `{}` is `Some`, not `None`. Failing to
gate emits premature `Usage{0,0,0,0}` + `Done` on the first intermediate
chunk, killing the stream after the first text delta.

Also: `cachedContentTokenCount` is **absent** from Zen's Gemini response
(not zero — the field is missing). `unwrap_or(0)` is safe; cached reads
always report 0 via Gemini.

### Zen Responses rejects small `max_output_tokens` with streaming

The `/v1/responses` endpoint returns HTTP 400 when `max_output_tokens < ~50`
and `stream: true`. Values ≥ 50 work. Non-streaming requests accept small
values. The zoid agent loop always sends `max_tokens >= 4096`, so this is
not a production issue. Unit tests use `max_tokens: 8` but hit `TcpListener`
stubs, not the real API.