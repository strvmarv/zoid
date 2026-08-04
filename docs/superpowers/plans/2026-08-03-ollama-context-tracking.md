# Ollama Context Tracking Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the Ollama provider to report the full prompt size as `input_tokens` on cache-hit turns, so the TUI status bar, calibration ratio, economy ledger, and churn timeline all show accurate context sizes.

**Architecture:** A single-function change in `ollama.rs`: on cache-hit turns (where `prompt_eval_count` is only the uncached tail), reconstruct the full prompt from the previous sub-turn's known size (`input_tokens = prev`, `cached = prev - curr`). No new fields, no consumer-side changes — every consumer inherits the fix because `input_tokens` now always represents (approximately) the full prompt.

**Tech Stack:** Rust, `zoid-provider` crate, `cargo nextest` / `cargo test`

## Global Constraints

- The fix touches only `crates/zoid-provider/src/ollama.rs` (implementation + tests) and `crates/zoid/src/agent.rs` (comment-only).
- No new fields on `TokenStat` or `provider::Usage`. No consumer-side changes.
- No schema migration, no DB format change.
- Tests run via `cargo nextest run -p zoid-provider` (preferred) or `cargo test -p zoid-provider`.
- The spec is at `docs/superpowers/specs/2026-08-03-ollama-context-tracking-design.md`.

---

### Task 1: Update the two existing Ollama cache tests for the new behavior

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:1179-1226` (two test functions)

**Interfaces:**
- Consumes: `parse_seq` (test helper at `ollama.rs:615`), `parse_line` (`ollama.rs:123`), `ProviderEvent::Usage`, `Usage` struct
- Produces: passing tests that assert the new n3 behavior (input=prev on cache-hit, cached=prev-curr)

- [ ] **Step 1: Update `implicit_cache_approx_second_subturn_credits_overlap` test**

This test has two sub-turns: `prompt_eval_count` 12000 then 13000. Under n3, the second sub-turn has `curr=13000 >= prev=12000`, so it's a cache miss: `input=13000`, `cached=0`. The old assertion of `cached: 12000` (via `min(curr, prev)`) no longer applies.

Replace the test at `ollama.rs:1179`:

```rust
    #[test]
    fn implicit_cache_approx_second_subturn_credits_overlap() {
        // Two sub-turns: 12k then 13k tokens. The second has curr=13000 >= prev=12000,
        // so it's a cache miss (full eval): input=13000 (the full prompt), cached=0.
        // The old min(curr, prev) synthetic cached no longer applies.
        let out = parse_seq(&[
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":12000,"eval_count":40}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":13000,"eval_count":10}"#,
        ]);
        // First sub-turn: cached 0 (prev 0).
        assert!(matches!(
            out[0][0],
            ProviderEvent::Usage(Usage {
                cached: 0,
                thinking_tokens: 0,
                input_tokens: 12000,
                output_tokens: 40
            })
        ));
        // Second sub-turn: cache miss (curr >= prev), input=13000, cached=0.
        assert!(matches!(
            out[1][0],
            ProviderEvent::Usage(Usage {
                cached: 0,
                thinking_tokens: 0,
                input_tokens: 13000,
                output_tokens: 10
            })
        ));
    }
```

- [ ] **Step 2: Update `implicit_cache_approx_shrinking_prompt_credits_smaller_overlap` test**

This test has two sub-turns: `prompt_eval_count` 50000 then 30000. Under n3, the second has `curr=30000 < prev=50000`, so it's a cache hit: `input=50000` (prev), `cached=20000` (prev - curr). The old assertion of `input=30000, cached=30000` changes.

Replace the test at `ollama.rs:1209`:

```rust
    #[test]
    fn implicit_cache_approx_shrinking_prompt_credits_smaller_overlap() {
        // A turn whose prompt is SMALLER than the previous (e.g. after eviction)
        // triggers the cache-hit reconstruction: input=prev (50000), cached=prev-curr
        // (20000). This is a false positive after eviction (the real prompt is
        // 30000), but it's bounded and self-corrects on the next cache-miss turn.
        let out = parse_seq(&[
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":50000,"eval_count":40}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":30000,"eval_count":10}"#,
        ]);
        assert!(matches!(
            out[1][0],
            ProviderEvent::Usage(Usage {
                cached: 20000,
                thinking_tokens: 0,
                input_tokens: 50000,
                output_tokens: 10
            })
        ));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

The tests now assert the new n3 behavior, but the implementation hasn't changed yet.

Run: `cargo nextest run -p zoid-provider implicit_cache_approx 2>&1 | tail -20`
Expected: FAIL — `implicit_cache_approx_second_subturn_credits_overlap` asserts `cached: 0` but gets `12000`; `implicit_cache_approx_shrinking_prompt_credits_smaller_overlap` asserts `input_tokens: 50000, cached: 20000` but gets `input_tokens: 30000, cached: 30000`.

- [ ] **Step 4: Commit the test changes**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "test: update ollama cache tests for n3 reconstruction behavior

Two existing tests now assert the provider-side reconstruction:
- second_subturn_credits_overlap: curr>=prev → cache miss, cached=0
- shrinking_prompt: curr<prev → cache hit, input=prev, cached=prev-curr

Tests fail until the implementation is changed in the next task."
```

---

### Task 2: Implement the provider-side reconstruction in `ollama.rs`

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:184-213` (the `done` frame token-accounting block)

**Interfaces:**
- Consumes: `last_prompt_eval` (AtomicU64 field on `OllamaProvider`), `Usage` struct
- Produces: `ProviderEvent::Usage` with `input_tokens` ≈ full prompt on all turns, `cached` = warm prefix on cache-hit turns

- [ ] **Step 1: Replace the token-accounting block in the `done` frame handler**

The current code at `ollama.rs:184-213` (the block starting with the `// Token accounting:` comment through the `out.push(ProviderEvent::Usage(...))` call) needs to be replaced. Find this exact block:

```rust
    // Token accounting: the native /api/chat final frame carries
    // `prompt_eval_count` (input tokens) and `eval_count` (output tokens) as a
    // single cumulative snapshot. Emit Usage ONLY on that final (`done`) frame:
    // `ProviderEvent::Usage` is an additive delta the agent sums, so emitting it
    // only once here keeps the economy ledger from double-counting. Ordered
    // before Done: the agent accumulates Usage during the stream and records it
    // when the turn ends.
    let is_done = v.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
    if is_done {
        let input = v.get("prompt_eval_count").and_then(|n| n.as_u64());
        let output = v.get("eval_count").and_then(|n| n.as_u64());
        if input.is_some() || output.is_some() {
            let curr = input.unwrap_or(0);
            // Approximate the prompt-cache hit: Ollama's `keep_alive` holds the
            // model's KV cache warm for 30m, so the overlap between this prompt
            // and the previous sub-turn's prompt is served from the warm cache.
            // The native `/api/chat` `done` frame reports the whole prompt as
            // `prompt_eval_count` with no cache-read breakdown, so we derive it:
            // `cached = min(curr, prev)`. The first sub-turn (prev=0) yields
            // cached=0 — correct, nothing was warm yet. Store curr for next time.
            use std::sync::atomic::Ordering;
            let prev = last_prompt_eval.swap(curr, Ordering::Relaxed);
            let cached_approx = curr.min(prev);
            out.push(ProviderEvent::Usage(Usage {
                input_tokens: curr,
                output_tokens: output.unwrap_or(0),
                cached: cached_approx,
                thinking_tokens: 0,
            }));
        }
```

Replace it with:

```rust
    // Token accounting: the native /api/chat final frame carries
    // `prompt_eval_count` (input tokens) and `eval_count` (output tokens) as a
    // single cumulative snapshot. Emit Usage ONLY on that final (`done`) frame:
    // `ProviderEvent::Usage` is an additive delta the agent sums, so emitting it
    // only once here keeps the economy ledger from double-counting. Ordered
    // before Done: the agent accumulates Usage during the stream and records it
    // when the turn ends.
    let is_done = v.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
    if is_done {
        let input = v.get("prompt_eval_count").and_then(|n| n.as_u64());
        let output = v.get("eval_count").and_then(|n| n.as_u64());
        if input.is_some() || output.is_some() {
            let curr = input.unwrap_or(0);
            // prompt_eval_count reports only the tokens *evaluated* (the uncached
            // tail), not the full prompt — the warm KV-cache prefix is not counted.
            // On a cache-hit turn (curr < prev), reconstruct the full prompt from
            // the previous sub-turn's known size. On a cache-miss turn (curr >=
            // prev), curr is the full prompt.
            use std::sync::atomic::Ordering;
            let prev = last_prompt_eval.swap(curr, Ordering::Relaxed);
            let (input_tokens, cached) = if prev > 0 && curr < prev {
                // Cache hit: prev was the full prompt, curr is the uncached tail.
                // The real prompt is ~prev (it grew by the new turn's tokens, but
                // prev is far closer than curr). cached = the warm prefix.
                (prev, prev - curr)
            } else {
                // Cache miss or first turn: curr is the full prompt.
                (curr, 0)
            };
            out.push(ProviderEvent::Usage(Usage {
                input_tokens,
                output_tokens: output.unwrap_or(0),
                cached,
                thinking_tokens: 0,
            }));
        }
```

- [ ] **Step 2: Run the two updated tests to verify they pass**

Run: `cargo nextest run -p zoid-provider implicit_cache_approx 2>&1 | tail -20`
Expected: PASS — both `implicit_cache_approx_second_subturn_credits_overlap` and `implicit_cache_approx_shrinking_prompt_credits_smaller_overlap` pass.

- [ ] **Step 3: Run the unchanged first-subturn test to verify it still passes**

Run: `cargo nextest run -p zoid-provider implicit_cache_approx_first_subturn 2>&1 | tail -10`
Expected: PASS — first sub-turn has prev=0, so the `else` branch fires: `input=12000, cached=0` (unchanged behavior).

- [ ] **Step 4: Run the full zoid-provider test suite to check for regressions**

Run: `cargo nextest run -p zoid-provider 2>&1 | tail -20`
Expected: PASS — all tests pass. No other test depends on the `min(curr, prev)` synthetic cached behavior.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "fix: reconstruct full prompt size on ollama cache-hit turns

prompt_eval_count only reports evaluated (uncached) tokens, not the full
prompt. On cache-hit turns (curr < prev), emit input_tokens=prev (the last
known full prompt) and cached=prev-curr (the warm prefix). On cache-miss
turns, curr is the full prompt — unchanged.

Fixes the TUI status bar showing ~5k instead of ~200k on cache-hit turns,
the calibration ratio under-learning, and the economy ledger undercount."
```

---

### Task 3: Add two new tests (deep cache hit + 3-turn self-correction)

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs` (add two test functions after the existing `implicit_cache_approx_*` tests, before the closing `}` of the `#[cfg(test)] mod tests` block at line 1227)

**Interfaces:**
- Consumes: `parse_seq` (test helper at `ollama.rs:615`), `ProviderEvent::Usage`, `Usage` struct
- Produces: two new passing tests documenting the deep cache-hit and post-eviction self-correction behavior

- [ ] **Step 1: Add the deep cache-hit test**

Add this test after the `implicit_cache_approx_shrinking_prompt_credits_smaller_overlap` test (after line 1226, before the closing `}` of the test module):

```rust
    #[test]
    fn implicit_cache_approx_deep_cache_hit_reconstructs_full_prompt() {
        // A deep cache hit: the prompt is ~200k but only 5k was evaluated (the
        // new tail). input_tokens must reconstruct to prev (200000), not the
        // raw prompt_eval_count (5000). cached = 195000 (the warm prefix).
        let out = parse_seq(&[
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":200000,"eval_count":40}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":5000,"eval_count":10}"#,
        ]);
        // First sub-turn: cache miss (prev=0), input=200000, cached=0.
        assert!(matches!(
            out[0][0],
            ProviderEvent::Usage(Usage {
                input_tokens: 200000,
                cached: 0,
                thinking_tokens: 0,
                output_tokens: 40
            })
        ));
        // Second sub-turn: deep cache hit (curr=5000 < prev=200000).
        // input=200000 (prev), cached=195000 (prev - curr).
        assert!(matches!(
            out[1][0],
            ProviderEvent::Usage(Usage {
                input_tokens: 200000,
                cached: 195000,
                thinking_tokens: 0,
                output_tokens: 10
            })
        ));
    }
```

- [ ] **Step 2: Add the 3-turn self-correction test**

Add this test after the deep cache-hit test:

```rust
    #[test]
    fn implicit_cache_approx_eviction_self_corrects_on_next_cache_miss() {
        // 3-turn sequence verifying the eviction false-positive self-corrects:
        //   Turn 1: full prompt 50000 (cache miss, prev=0 → input=50000, cached=0).
        //   Turn 2: eviction shrinks prompt to 30000 (curr < prev → false positive:
        //     input=50000, cached=20000 — overcounts, the real prompt is 30000).
        //   Turn 3: cache miss at new smaller size, reports 35000 (curr >= prev
        //     → input=35000, cached=0 — self-corrects, no longer overcounting).
        let out = parse_seq(&[
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":50000,"eval_count":40}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":30000,"eval_count":10}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":35000,"eval_count":20}"#,
        ]);
        // Turn 1: cache miss (prev=0), input=50000, cached=0.
        assert!(matches!(
            out[0][0],
            ProviderEvent::Usage(Usage {
                input_tokens: 50000,
                cached: 0,
                thinking_tokens: 0,
                output_tokens: 40
            })
        ));
        // Turn 2: false positive (curr=30000 < prev=50000).
        // input=50000 (prev), cached=20000 (prev - curr). Overcounts.
        assert!(matches!(
            out[1][0],
            ProviderEvent::Usage(Usage {
                input_tokens: 50000,
                cached: 20000,
                thinking_tokens: 0,
                output_tokens: 10
            })
        ));
        // Turn 3: self-correction (curr=35000 >= prev=30000, the last swap).
        // input=35000 (curr = full prompt), cached=0. No longer overcounting.
        assert!(matches!(
            out[2][0],
            ProviderEvent::Usage(Usage {
                input_tokens: 35000,
                cached: 0,
                thinking_tokens: 0,
                output_tokens: 20
            })
        ));
    }
```

- [ ] **Step 3: Run the new tests to verify they pass**

Run: `cargo nextest run -p zoid-provider implicit_cache_approx_deep_cache_hit implicit_cache_approx_eviction_self_corrects 2>&1 | tail -20`
Expected: PASS — both new tests pass.

- [ ] **Step 4: Run the full zoid-provider test suite**

Run: `cargo nextest run -p zoid-provider 2>&1 | tail -20`
Expected: PASS — all tests pass, no regressions.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "test: add deep cache-hit and eviction self-correction tests

- deep_cache_hit: 200k prompt with 5k evaluated tail → input=200k
- eviction_self_corrects: 3-turn sequence verifying the post-eviction
  overcount is corrected on the next cache-miss turn, not perpetuated"
```

---

### Task 4: Correct the three wrong code comments

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:300-306` (the `last_prompt_eval` field doc)
- Modify: `crates/zoid-provider/src/ollama.rs:606-608` (the `parse_first` helper doc)
- Modify: `crates/zoid/src/agent.rs:736-741` (the calibration ratio comment)

**Interfaces:**
- Consumes: none (comment-only changes)
- Produces: accurate documentation of `prompt_eval_count` semantics

- [ ] **Step 1: Correct the `last_prompt_eval` field doc in `ollama.rs`**

Find the field doc at `ollama.rs:300-306`:

```rust
    /// The previous sub-turn's `prompt_eval_count` (full prompt size). Ollama's
    /// `keep_alive` holds the model's KV cache warm for 30m, so the bulk of each
    /// new prompt is a re-evaluation of a warm prefix — but the native `/api/chat`
    /// `done` frame reports it all as `prompt_eval_count` with no cache-read
    /// breakdown. We approximate: the overlap with the previous prompt is
    /// "cached" (warm in KV). Cross-stream state so `parse_line`'s `done` frame
    /// can read it. Ollama implicit-cache approximation.
```

Replace with:

```rust
    /// The previous sub-turn's `prompt_eval_count` (uncached tail on cache-hit
    /// turns, full prompt on cache-miss turns). Ollama's `keep_alive` holds the
    /// model's KV cache warm for 30m, so on a cache-hit turn only the new
    /// (uncached) tail is evaluated — `prompt_eval_count` is that tail, not the
    /// full prompt. On a cache-miss turn it's the full prompt. We use it to
    /// reconstruct the full prompt size on cache-hit turns (see the `done` frame
    /// handler). Cross-stream state so `parse_line`'s `done` frame can read it.
```

- [ ] **Step 2: Correct the `parse_first` helper doc in `ollama.rs`**

Find the helper doc at `ollama.rs:606-608`:

```rust
    /// Call `parse_line` with a fresh (zero) `last_prompt_eval`. Used by tests
    /// that don't exercise the implicit-cache approximation (the first sub-turn:
    /// prev=0, so cached=0, matching the old behavior).
```

Replace with:

```rust
    /// Call `parse_line` with a fresh (zero) `last_prompt_eval`. Used by tests
    /// that don't exercise the cache-hit reconstruction (the first sub-turn:
    /// prev=0, so the else branch fires: input=curr, cached=0).
```

- [ ] **Step 3: Correct the `parse_seq` helper doc in `ollama.rs`**

Find the helper doc at `ollama.rs:613-614`:

```rust
    /// Call `parse_line` with a shared `last_prompt_eval` across multiple
    /// sub-turns, so the implicit-cache approximation sees a growing prefix.
```

Replace with:

```rust
    /// Call `parse_line` with a shared `last_prompt_eval` across multiple
    /// sub-turns, so the cache-hit reconstruction sees the previous prompt size.
```

- [ ] **Step 4: Correct the calibration comment in `agent.rs`**

Find the comment at `agent.rs:736-741`:

```rust
    // Calibration ratio: real_input_tokens / context_window.total_tokens from
    // the last non-cached sub-turn. The chars/4 estimate undercounts 5-7x for
    // code/tool output, so when the provider reports 0 (Ollama cached prompt)
    // we scale the current estimate by this ratio to approximate the real
    // context size. Updated on every sub-turn where the provider reports a
    // non-zero input. Mutable, lives for the turn (across sub-turns).
```

Replace with:

```rust
    // Calibration ratio: real_input_tokens / context_window.total_tokens from
    // the last non-cached sub-turn. The chars/4 estimate undercounts 5-7x for
    // code/tool output, so on cache-hit turns (where the Ollama provider
    // reconstructs input from the previous sub-turn's size) we scale the
    // current estimate by this ratio to approximate the real context size.
    // Updated on every sub-turn where the provider reports a non-zero input.
    // Mutable, lives for the turn (across sub-turns).
```

- [ ] **Step 5: Run the full test suite to verify comment changes don't break anything**

Run: `cargo nextest run -p zoid-provider 2>&1 | tail -10 && cargo nextest run -p zoid --no-fail-fast 2>&1 | tail -20`
Expected: PASS — comment-only changes, no behavior change.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs crates/zoid/src/agent.rs
git commit -m "docs: correct three wrong comments about prompt_eval_count semantics

The codebase had three conflicting models of prompt_eval_count:
1. ollama.rs last_prompt_eval field doc: 'full prompt size' (wrong)
2. agent.rs calibration comment: 'reports 0 on cached prompts' (wrong)
3. ollama.rs parse_first helper doc: references 'old behavior'

All three now correctly document that prompt_eval_count is the uncached
tail on cache-hit turns and the full prompt on cache-miss turns, per
empirical evidence from the session DB."
```

---

### Task 5: Full workspace test run

**Files:**
- None (verification only)

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo nextest run --workspace --no-fail-fast 2>&1 | tail -30`
Expected: PASS — all workspace tests pass, no regressions in any crate.

- [ ] **Step 2: If any TUI snapshot tests fail due to ctx_used changes, update them**

The TUI snapshot tests render the status bar with `ctx_used`. If any snapshots capture a value derived from the old undercounted `input`, they may need updating. However, snapshot tests use synthetic `TokenStat` values (not real provider data), so they should be unaffected.

Run: `cargo insta test --accept -p zoid-tui 2>&1 | tail -10`
Check: `git diff --stat` — if any snapshots changed, verify the diff is only the `ctx_used` value and not an unrelated regression.

If no snapshots changed: done. If they did: commit the accepted snapshots.