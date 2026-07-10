# Prompt Caching Spike — OpenCode Go (GLM-5.2)

**Date:** 2026-07-09
**Endpoint:** `https://opencode.ai/zen/go/v1/chat/completions`
**Model:** `glm-5.2` (wire shape: OpenAI Chat Completions)

## Summary

OpenCode Go's GLM-5.2 endpoint **does support automatic prefix caching** — no
client-side `cache_control` breakpoints are needed. The server detects cached
prefixes and reports `cached_tokens` in `usage.prompt_tokens_details.cached_tokens`.

However, the cache has a **very short effective window (~1-5 seconds)**, and
there is a **cache-write propagation delay (~1 second)**. This means:

- Requests fired in rapid succession (<1s apart) hit the cache.
- Requests ≥1-2s apart miss the cache — the previous write hasn't propagated
  or has already expired.

**This is the root cause of the 11.8M-token usage spike** the user observed
when switching from Ollama to OpenCode Go.

## Evidence

### 1. Server supports automatic caching (no client hints needed)

Two identical non-streaming requests, back-to-back:

| Call | prompt_tokens | cached_tokens |
|------|---------------|---------------|
| First call | 99 | 0 |
| Second call | 99 | 64 |

The second call reports 64/99 tokens cached. No `cache_control` field needed.

### 2. Streaming returns `cached_tokens` in the final usage chunk

The SSE stream's final chunk (with empty `choices: []`) includes:
```json
{"usage": {"prompt_tokens": 178, "completion_tokens": 5,
 "prompt_tokens_details": {"cached_tokens": 128, "cache_write_tokens": null}}}
```

Our `OpenAICompatProvider` already parses this correctly (line 250-270 of
`openai_compat.rs`). The `cached` field is captured in the `ProviderEvent::Usage`
event.

### 3. Cache TTL is very short (~2-5 seconds)

Identical requests sent at various intervals:

| Delay after write | Cached? |
|-------------------|---------|
| 0.5s | hit (320/366 tokens) |
| 1s | miss, then hit on immediate follow-up |
| 2s | miss, then hit on immediate follow-up |
| 3s | miss, then hit on immediate follow-up |
| 5s+ | miss |

Pattern: the cache write takes ~1s to propagate. Two requests <1s apart: the
second hits the first's cache. Requests ≥2s apart: the cache has expired or
hasn't propagated yet.

### 4. Tools array changes invalidate the entire cache

| Scenario | prompt | cached |
|----------|--------|--------|
| 2 tools, growing conversation (turn 2) | 248 | 192 (hit) |
| **3 tools** (added one) | 314 | 64 (only system prefix survived) |

Changing the tools array between turns destroys the cache. In our agent loop,
the tool registry is **stable between turns** (same `zoid_tools::registry()`
call), so this isn't the primary issue. But it confirms tools are part of the
cached prefix.

### 5. JSON key ordering is stable

`serde_json` is used **without** the `preserve_order` feature, so `Value::Object`
uses `BTreeMap` → keys are serialized in sorted (deterministic) order. Tool
specs serialize identically across turns.

### 6. Warmer pattern does NOT meaningfully help

| Step | prompt | cached |
|------|--------|--------|
| Turn 1 (initial request) | 187 | 128 (from earlier system prefix) |
| *5s delay (simulating tool execution)* | | |
| Turn 2 warmer (max_tokens=1, same prefix) | 196 | 128 (only system prefix cached) |
| Turn 2 real request (immediately after) | 205 | 128 (only system prefix cached) |

The warmer writes a cache entry with the system prefix (128 tokens), but the
conversation history (messages beyond the system prompt) is NOT cached because
the warmer's request diverges at its own user message. The real request shares
the system prefix (128 tokens) but nothing more.

### 7. Non-standard cache parameters are silently accepted but ineffective

The server accepts `cache_control` and `prompt_cache_key` in the request body
without error, but they don't appear to have any effect on caching behavior.
The caching is purely automatic and server-side.

## Root Cause

**Server-side cache architecture:** The OpenCode Go endpoint for GLM-5.2 has
an automatic prefix cache with a ~1-5 second effective window. In the zoid
agent loop, each sub-turn follows this pattern:

1. Send request to API → response streamed back (cache written)
2. Execute tool calls (takes 2-10+ seconds)
3. Build next request with tool results → send to API

By step 3, the cache from step 1 has **expired**. The entire conversation
context (system prompt + tools + all prior messages) is re-evaluated from
scratch. In an agentic task with many tool-call cycles, this compounds — each
sub-turn pays full token cost instead of paying only for the new delta.

With Ollama, the `keep_alive` parameter holds the KV cache warm for 30 minutes,
so tool execution delays (seconds) don't affect caching. OpenCode Go doesn't
have an equivalent mechanism.

## What We Can't Fix

- The server's cache TTL (server-side, not configurable via the API).
- The cache-write propagation delay (~1s, server-side).

## Options Considered

### Option A: No-op (document and move on)
The caching works correctly when requests are rapid. The agent loop already
sends requests as fast as possible — the delay is inherent to tool execution.
Document the behavior; accept the token cost.

### Option B: Concurrent warmer request
Before each real API request, fire a tiny "warmer" request (max_tokens=1) with
the same system+tools prefix to write the cache, then immediately send the real
request. **Finding: This only caches the system prefix (~128 tokens), not the
conversation history.** The warmer diverges at its own user message. Not
worthwhile.

### Option C: Pre-write cache with the actual request
Send the real request body with `max_tokens: 1` as a warmer, then immediately
re-send with the real `max_tokens`. This would cache the *entire* prefix
(system + tools + conversation) because the warmer's request is identical up to
the max_tokens parameter. But this doubles the token cost (two full prompt
evaluations) and the warmer's response is wasted. **Not worthwhile** — it
increases total tokens, not decreases them.

### Option D: Investigate Ollama as a backend
If GLM-5.2 can be served via Ollama (or an Ollama-compatible endpoint), the
`keep_alive` mechanism would hold the KV cache warm for 30 minutes, eliminating
the cache-expiry problem. But this requires a local Ollama setup with GLM-5.2,
which may not be available.

### Option E: Add a `cache_warmup` capability to the provider
Add an optional method to the `Provider` trait that sends a preflight request
to warm the cache before the real request. Only useful if we can cache the
*full* prefix (Option C), which isn't cost-effective.

## Recommendation

**Option A (no-op)** — the issue is fundamentally server-side. The cache works
correctly; it just expires too fast for the agent loop's tool-execution cadence.
No client-side change can fix a ~2-5s server cache TTL.

The Ollama path (with `keep_alive`) already handles this correctly. Users who
need efficient caching should use Ollama for local models, or accept the token
cost for remote providers with short cache TTLs.

## Files Changed

None. This was a spike only — no code changes.