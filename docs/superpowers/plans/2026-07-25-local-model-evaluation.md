# Local Model Evaluation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Determine whether a locally-hosted model can drive zoid's agent loop on an RTX 3060, and fix the `options.num_ctx` omission that would otherwise invalidate every measurement.

**Architecture:** Two independent tracks. **Track B** (Tasks 1–5) is a TDD fix inside `crates/zoid-provider/src/ollama.rs` plus one wiring line in `crates/zoid/src/main.rs`, run in a git worktree. **Track A** (Tasks 6–8) is a benchmark harness living entirely in the session scratchpad that consumes zoid as a library and makes no repository changes. The tracks are deliberately decoupled: Track A's harness injects `num_ctx` into the request JSON itself (it must vary it per-run anyway to bisect metric 4), so it never depends on Track B having landed.

**Tech Stack:** Rust (`serde_json`, `tokio`, `reqwest`, `async_trait`), Python 3 for the harness runner, Ollama 0.21.1 HTTP API.

---

## Handoff Context

**Status: planned, not started.** No code written, no models pulled, no worktree
created. The spec and this plan were produced in a session that stopped at the
execution handoff. Start at Task 1 (Track B) or Task 6 (Track A) — they are
independent.

**Companion spec:** `docs/superpowers/specs/2026-07-25-local-model-evaluation-design.md`
(commit `220ab58`). Read it for the candidate table, decision rule, and risks. This
plan is the execution detail; the spec is the reasoning.

### What was already measured — do not re-derive

| Fact | Evidence |
|---|---|
| Hardware: RTX 3060 **12 GB VRAM** (~11.2 GB usable), i5-14500 6c/6t, 23 GB RAM (~17 GB free), 210 GB free `/home` | `nvidia-smi`, `lscpu`, `free -h`, `df -h` |
| Ollama 0.21.1 at `/usr/local/bin/ollama`; `devstral` (14 GB) and `qwen2.5-coder:14b` (9 GB) already pulled | `ollama list` |
| `qwen2.5-coder:14b` reports `qwen2.context_length: 32768`, **`parameters: None`**, `capabilities: ['completion','tools','insert']`, `Q4_K_M` | `POST /api/show` against the live daemon |
| zoid's fixed per-turn overhead: **13 tools / 6,112 bytes ≈ 1,700 tokens** of schemas (heaviest: `edit` 778 B, `submit_feedback` 731 B, `web_fetch` 648 B) plus a ~130-token `SYSTEM_PROMPT` | temporary test over `zoid_tools::registry()` |
| `zoid-tools` has `zoid-provider` and `serde_json` as direct `[dependencies]`, so the Task 6 generator compiles | `crates/zoid-tools/Cargo.toml` |

### The defect this plan fixes

`ollama::request_body` (`crates/zoid-provider/src/ollama.rs:58-68`) emits only
`model`, `stream`, `messages`, `keep_alive`, `think`, `tools` — never
`options.num_ctx`. Correct for Ollama Cloud, which sizes context server-side.
Wrong for a local daemon, which applies its own default and then **silently
truncates** rather than erroring, so:

1. `is_context_length_error` (`crates/zoid-provider/src/lib.rs:343`) never fires.
2. The first thing evicted is the system prompt and tool schemas — the model
   loses its instructions and tools while still emitting fluent prose.
3. `fetch_model_info` (`ollama.rs:450`) reports the model's *trained* context as
   the ceiling, which flows into `context_ceiling` (`lib.rs:318`) and becomes the
   economy ⑤ denominator. zoid displays a window the daemon never granted.

### What already exists — no new provider is needed

- `crates/zoid-model/src/lib.rs:88` — `ollama-local` is a first-class registry
  entry: `Status::Available`, `default_base_url: "http://localhost:11434"`,
  `models: &[]` (local tags are free-text).
- `crates/zoid/src/main.rs:1046` — already branches on
  `canonical_id(&config.provider) == "ollama-local"` and constructs with an empty
  API key. **This is the only site that knows the local/cloud distinction**, and
  it is where Task 5 adds one builder call.
- `ollama.rs` already implements `list_models` (`/api/tags`) and
  `fetch_model_info` (`/api/show`).

### Two design decisions that are not obvious from the tasks

**1. `request_body` takes `num_ctx` as a parameter rather than sniffing
`base_url`.** The variant decision belongs at `main.rs:1046`, which already
branches on it — not buried inside a serializer where it would be untestable
without a live daemon. A useful side effect: passing `None` at the nine existing
test call sites turns tests that already existed and already passed into the
cloud byte-identity regression suite.

**2. The Track A harness owns `options.num_ctx`, not the generator.** The golden
body is dumped with no `options` key and `bench.py` injects the value per
request. This is what fully decouples the tracks — the harness has to vary
`num_ctx` anyway to bisect metric 4, so Track A never waits on Track B.

### This plan is falsifiable — Task 7 Step 2 comes first

The claim that Ollama truncates silently rather than erroring came from
documented behavior, not measurement. **Task 7 Step 2 tests it against a model
already on disk, before any of the ~52 GB of candidates is pulled.** If Ollama
0.21.1 does not truncate silently — or now errors — stop and report. Track B
remains defensible (an explicit window beats an undocumented default) but its
urgency drops and the spec's framing needs correcting.

### Repo state warning

Another session was committing to this repo on 2026-07-25: commit `4c7dabc`
(`docs: move exit-worktree CWD bug writeup to docs/bugs/`) landed on `main`
between the spec and plan commits. **Check `main` before branching a worktree for
Track B.**

---

## Global Constraints

- **Ollama Cloud request bodies must remain byte-identical to today's.** `num_ctx` is meaningful only for a local daemon. Every change is gated on `Option<u32>` being `None` for cloud.
- **Env-var parsing follows the existing idiom** in `crates/zoid-provider/src/lib.rs:44` (`stream_idle_timeout`): a positive integer wins, anything else falls back to a named default constant.
- **Do not mutate env vars inside tests.** The repo states this explicitly at `crates/zoid-provider/src/lib.rs:373-375` — env is process-global and unsafe under parallel test execution. Test the pure parser instead.
- **Track A makes zero repository changes.** Generators are temporary files that are deleted in the same task that creates them.
- **Commit messages: no `Co-Authored-By` or any co-author trailer.**
- Measured constants this plan relies on: zoid's fixed per-turn overhead is **13 tools / 6,112 bytes ≈ 1,700 tokens** of schemas plus a **~130-token** `SYSTEM_PROMPT`.

---

## File Structure

**Track B — modified:**
- `crates/zoid-provider/src/ollama.rs` — the whole fix. Gains `DEFAULT_LOCAL_NUM_CTX`, `parse_num_ctx`, `configured_num_ctx`; `request_body` gains a `num_ctx` parameter; `OllamaProvider` gains a `num_ctx` field, a `with_num_ctx` builder, and a clamp in `fetch_model_info`.
- `crates/zoid/src/main.rs:1046-1054` — the `ollama-local` construction branch gains one builder call. This is the only place that knows the local/cloud distinction.

**Track A — created in scratchpad** (`$SCRATCH` = `/tmp/claude-1000/-home-gomanjoe-source-zoid/bde42ca9-c9a7-4e07-a2cd-55795be6b453/scratchpad`):
- `$SCRATCH/golden_body.json` — one zoid-generated request body, the benchmark payload.
- `$SCRATCH/bench.py` — the runner: replays the golden body per model, varies `num_ctx`, records metrics.
- `$SCRATCH/results.json` / `$SCRATCH/REPORT.md` — output.

**Track A — temporary, deleted within its own task:**
- `crates/zoid-tools/tests/_tmp_golden_body.rs` — generator. Lives in `zoid-tools` because that crate already depends on `zoid-provider` (see its `use zoid_provider::{ToolCall, ToolSpec};`), giving one test access to both `zoid_tools::registry()` and `zoid_provider::ollama::request_body`.

---

## Track B — the `num_ctx` fix

Run these in a git worktree, not on `main`.

### Task 1: Context-window configuration

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs` (add after `DEFAULT_OLLAMA_MODEL` at line 14)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub const DEFAULT_LOCAL_NUM_CTX: u32`, `pub fn parse_num_ctx(raw: Option<&str>) -> u32`, `pub fn configured_num_ctx() -> u32`. Task 5 calls `configured_num_ctx()`.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` in `crates/zoid-provider/src/ollama.rs`:

```rust
#[test]
fn parse_num_ctx_accepts_positive_integers() {
    assert_eq!(parse_num_ctx(Some("32768")), 32768);
    assert_eq!(parse_num_ctx(Some("  8192  ")), 8192);
    assert_eq!(parse_num_ctx(Some("1")), 1);
}

#[test]
fn parse_num_ctx_falls_back_on_invalid_input() {
    assert_eq!(parse_num_ctx(None), DEFAULT_LOCAL_NUM_CTX);
    assert_eq!(parse_num_ctx(Some("")), DEFAULT_LOCAL_NUM_CTX);
    assert_eq!(parse_num_ctx(Some("0")), DEFAULT_LOCAL_NUM_CTX);
    assert_eq!(parse_num_ctx(Some("-4096")), DEFAULT_LOCAL_NUM_CTX);
    assert_eq!(parse_num_ctx(Some("lots")), DEFAULT_LOCAL_NUM_CTX);
    assert_eq!(parse_num_ctx(Some("32768.5")), DEFAULT_LOCAL_NUM_CTX);
    // Beyond u32 — must not panic or wrap.
    assert_eq!(parse_num_ctx(Some("99999999999")), DEFAULT_LOCAL_NUM_CTX);
}

#[test]
fn default_local_num_ctx_clears_zoid_fixed_overhead() {
    // zoid sends ~1,850 tokens of fixed overhead (13 tool schemas ≈ 1,700
    // tokens + a ~130-token system prompt) before the user types anything.
    // The default must leave that a small fraction of the window.
    assert!(DEFAULT_LOCAL_NUM_CTX >= 32768);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-provider --lib ollama::tests::parse_num_ctx -- --nocapture`
Expected: FAIL to compile — `cannot find function 'parse_num_ctx' in this scope`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/zoid-provider/src/ollama.rs` directly after the `DEFAULT_OLLAMA_MODEL` const (line 14):

```rust
/// Default context window requested from a **local** Ollama daemon when
/// `ZOID_NUM_CTX` is unset. A local daemon applies its own (small) default and
/// then silently truncates an over-long prompt rather than erroring, so the
/// client must ask. zoid's fixed per-turn overhead is ~1,850 tokens (13 tool
/// schemas ≈ 1,700 tokens plus the system prompt), which 32K leaves ample room
/// around. Never sent to Ollama Cloud, which sizes context server-side.
pub const DEFAULT_LOCAL_NUM_CTX: u32 = 32768;

/// Parse a `ZOID_NUM_CTX` value. Mirrors the contract of
/// `crate::stream_idle_timeout` (lib.rs:44): a positive integer wins, and
/// anything else — absent, empty, zero, negative, non-numeric, or beyond u32 —
/// falls back to `DEFAULT_LOCAL_NUM_CTX`.
pub fn parse_num_ctx(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_LOCAL_NUM_CTX)
}

/// The configured local context window: `ZOID_NUM_CTX` or the default. Read at
/// provider-construction time by the bin's `ollama-local` branch.
pub fn configured_num_ctx() -> u32 {
    parse_num_ctx(std::env::var("ZOID_NUM_CTX").ok().as_deref())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-provider --lib ollama::tests -- --nocapture`
Expected: PASS, including all pre-existing `ollama::tests`.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): ZOID_NUM_CTX parsing for local Ollama

A local daemon applies its own context default and silently truncates
rather than erroring, so the client must request a window explicitly.
Pure parser (no env mutation in tests, per lib.rs:373-375)."
```

---

### Task 2: Emit `options.num_ctx` in the request body

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:19` (signature), `:58-79` (body), `:322` (call site), and the nine test call sites at `:535, :569, :603, :632, :648, :663, :679, :690`
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `DEFAULT_LOCAL_NUM_CTX` from Task 1.
- Produces: `pub fn request_body(req: &CompletionRequest, num_ctx: Option<u32>) -> Value`. Task 3 calls it with `self.num_ctx`.

**Why a parameter and not inference:** `request_body` is consumed only inside `ollama.rs`; sibling providers define their own same-named function with no shared trait, so widening the signature is contained. Passing `None` at every existing test call site turns those tests into the cloud-parity regression suite this task requires.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/zoid-provider/src/ollama.rs`:

```rust
/// A minimal request used by the num_ctx body tests.
fn ctx_req() -> CompletionRequest {
    CompletionRequest {
        model: "ornith:9b".into(),
        system: Some("be terse".into()),
        messages: vec![Message::user("hi")],
        max_tokens: 1024,
        tools: vec![],
        thinking: crate::ThinkingMode::Off,
        reassert: None,
    }
}

#[test]
fn local_body_carries_options_num_ctx() {
    let body = request_body(&ctx_req(), Some(32768));
    assert_eq!(body["options"]["num_ctx"], json!(32768));
}

#[test]
fn cloud_body_omits_options_entirely() {
    // Byte-identical-to-today guarantee: Ollama Cloud sizes context
    // server-side, and an unexpected `options` key must never appear.
    let body = request_body(&ctx_req(), None);
    assert!(body.get("options").is_none());
}

#[test]
fn num_ctx_does_not_disturb_other_body_fields() {
    let with = request_body(&ctx_req(), Some(8192));
    let without = request_body(&ctx_req(), None);
    for key in ["model", "stream", "messages", "keep_alive", "think"] {
        assert_eq!(with[key], without[key], "field `{key}` changed");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-provider --lib ollama::tests -- --nocapture`
Expected: FAIL to compile — `this function takes 1 argument but 2 arguments were supplied`.

- [ ] **Step 3: Write the implementation**

Change the signature at `crates/zoid-provider/src/ollama.rs:19` and update the doc comment:

```rust
/// Build the native Ollama `/api/chat` request body. System prompt is a leading
/// `{"role":"system"}` message. Only `model`/`messages`/`stream` are sent — the
/// native API does not take OpenAI's `max_tokens`/`stream_options`.
///
/// `num_ctx` is `Some` only for **local** daemons (`ollama-local`), which
/// otherwise apply their own default and silently truncate. It is `None` for
/// Ollama Cloud, which sizes context server-side — and when `None` the emitted
/// body is byte-identical to the pre-`num_ctx` body.
pub fn request_body(req: &CompletionRequest, num_ctx: Option<u32>) -> Value {
```

Then, immediately after the `let mut body = json!({ ... });` block ends (currently line 68, before the `if !req.tools.is_empty()` block), insert:

```rust
    if let Some(n) = num_ctx {
        body["options"] = json!({ "num_ctx": n });
    }
```

Update the live call site at line 322 from `.json(&request_body(req))` to:

```rust
                .json(&request_body(req, self.num_ctx))
```

This will not compile until Task 3 adds the field. To keep Task 2 independently green, use `.json(&request_body(req, None))` here and change it to `self.num_ctx` in Task 3.

Update all nine test call sites mechanically: `request_body(&req)` → `request_body(&req, None)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-provider --lib ollama:: -- --nocapture`
Expected: PASS — all new tests plus every pre-existing body test, which now assert the `None` (cloud) path.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): emit options.num_ctx for local Ollama bodies

request_body gains an explicit num_ctx parameter rather than inferring
locality from base_url — the variant decision belongs at the call site,
not buried in a serializer. Existing body tests now pass None and serve
as the cloud byte-identity regression suite."
```

---

### Task 3: Thread `num_ctx` through `OllamaProvider`

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:256-271` (struct), `:274-284` (`new`), `:298-303` (add builder after `with_idle_timeout`), `:322` (call site)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `request_body(req, num_ctx)` from Task 2.
- Produces: `OllamaProvider::with_num_ctx(self, num_ctx: u32) -> Self`, and a private `num_ctx: Option<u32>` field. Task 4 reads the field; Task 5 calls the builder.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn new_defaults_num_ctx_to_none_for_cloud() {
    assert_eq!(OllamaProvider::new("k".into()).num_ctx, None);
}

#[test]
fn with_num_ctx_sets_the_field() {
    let p = OllamaProvider::new(String::new())
        .with_base_url("http://localhost:11434")
        .with_num_ctx(16384);
    assert_eq!(p.num_ctx, Some(16384));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-provider --lib ollama::tests::with_num_ctx -- --nocapture`
Expected: FAIL to compile — `no field 'num_ctx' on type 'OllamaProvider'`.

- [ ] **Step 3: Write the implementation**

Add the field to the struct at `crates/zoid-provider/src/ollama.rs:256`, after `idle_timeout`:

```rust
    /// Explicit context window for `options.num_ctx`. `Some` only for
    /// `ollama-local`; `None` for Ollama Cloud, which sizes context
    /// server-side and whose request body must stay byte-identical.
    num_ctx: Option<u32>,
```

Add `num_ctx: None,` to the `Self { ... }` literal in `new` (after `idle_timeout`).

Add the builder after `with_idle_timeout`:

```rust
    /// Request an explicit context window (`options.num_ctx`). Set only for
    /// `ollama-local`: a local daemon otherwise applies its own default and
    /// silently truncates the prompt — evicting the system prompt and tool
    /// schemas — without ever returning an error.
    pub fn with_num_ctx(mut self, num_ctx: u32) -> Self {
        self.num_ctx = Some(num_ctx);
        self
    }
```

Change line 322 from `.json(&request_body(req, None))` to:

```rust
                .json(&request_body(req, self.num_ctx))
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-provider --lib ollama:: -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): OllamaProvider::with_num_ctx builder

Defaults to None so cloud construction is unchanged."
```

---

### Task 4: Stop reporting a context ceiling we did not request

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:450-472` (`fetch_model_info`)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: the `num_ctx` field from Task 3, `parse_ollama_context_window` (existing, line 227).
- Produces: no new public API — behavior change only.

**Why:** `/api/show` reports the model's *trained* context. Verified against the local daemon: `qwen2.5-coder:14b` returns `qwen2.context_length: 32768` with `parameters: None`. That value currently flows into `context_ceiling` (`lib.rs:318`) and becomes the economy ⑤ denominator, so zoid displays a window it was never granted. The usable ceiling is the smaller of what the weights support and what we asked for.

- [ ] **Step 1: Write the failing test**

Add to `mod tests`. This tests the clamp as a pure helper so it needs no live daemon:

```rust
#[test]
fn effective_context_window_clamps_to_requested_num_ctx() {
    // /api/show reports the model's trained context; we may have asked for less.
    assert_eq!(effective_context_window(32768, Some(8192)), 8192);
    // Asking for more than the weights support does not inflate the ceiling.
    assert_eq!(effective_context_window(32768, Some(131072)), 32768);
    // Cloud: no request of ours, so the reported value stands.
    assert_eq!(effective_context_window(32768, None), 32768);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-provider --lib ollama::tests::effective_context_window -- --nocapture`
Expected: FAIL to compile — `cannot find function 'effective_context_window' in this scope`.

- [ ] **Step 3: Write the implementation**

Add above `impl Provider for OllamaProvider` in `crates/zoid-provider/src/ollama.rs`:

```rust
/// The context window zoid may actually use: the smaller of what `/api/show`
/// reports the weights support (`reported`) and what we asked the daemon for
/// (`requested`). `None` means we made no request — Ollama Cloud — so the
/// reported value stands. Without this clamp the economy ⑤ gauge would use a
/// denominator the local daemon never granted.
pub fn effective_context_window(reported: u64, requested: Option<u32>) -> u64 {
    match requested {
        Some(n) => reported.min(n as u64),
        None => reported,
    }
}
```

Then change line 460 in `fetch_model_info` from:

```rust
        let window = parse_ollama_context_window(&body);
```

to:

```rust
        let window = parse_ollama_context_window(&body)
            .map(|w| effective_context_window(w, self.num_ctx));
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-provider --lib ollama:: -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "fix(provider): clamp local context ceiling to the requested num_ctx

/api/show reports the model's trained window (qwen2.5-coder:14b → 32768,
parameters: None), which fed context_ceiling and made the economy gauge
report a window the daemon never granted."
```

---

### Task 5: Wire the local branch in the bin

**Files:**
- Modify: `crates/zoid/src/main.rs:1046-1054`
- Test: `cargo build` plus a manual smoke against the local daemon

**Interfaces:**
- Consumes: `configured_num_ctx()` (Task 1) and `with_num_ctx` (Task 3).
- Produces: nothing downstream. This is the terminal wiring.

- [ ] **Step 1: Apply the change**

`main.rs:1046` is the only site that already knows the variant — it branches on `canonical_id(&config.provider) == "ollama-local"`. Replace the construction:

```rust
    // ollama-local: usable without a key (localhost, no auth). Construct directly.
    if zoid_provider::model::canonical_id(&config.provider) == "ollama-local" {
        let base_url = effective_base_url(config);
        return (
            Arc::new(
                zoid_provider::ollama::OllamaProvider::new(String::new())
                    .with_base_url(base_url)
                    .with_num_ctx(zoid_provider::ollama::configured_num_ctx()),
            ),
            "ollama",
            true, // no key required → treat as ready
        );
    }
```

Leave the `_ =>` arm at line 1099 (the `ollama-cloud` path) untouched — it must not call `with_num_ctx`.

- [ ] **Step 2: Verify it compiles and the whole suite is green**

Run: `cargo build && cargo test --workspace`
Expected: builds clean, all tests pass.

- [ ] **Step 3: Smoke-test against the live daemon**

```bash
ZOID_NUM_CTX=16384 ZOID_LOG=debug cargo run -- --provider ollama-local --model qwen2.5-coder:14b
```

Send one turn, then confirm the daemon allocated the window:

```bash
ollama ps
```

Expected: the `CONTEXT` column reads 16384, not the server default.

- [ ] **Step 4: Confirm cloud is unaffected**

Run: `cargo test -p zoid-provider --lib ollama::tests::cloud_body_omits_options_entirely -- --nocapture`
Expected: PASS. This is the guard that `ollama-cloud` bodies are byte-identical.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat: request an explicit context window for ollama-local

Local Ollama otherwise applies its own default and silently truncates,
evicting the system prompt and tool schemas without erroring. Cloud is
untouched: only the ollama-local branch calls with_num_ctx."
```

---

## Track A — the benchmark harness

Scratchpad only. `$SCRATCH` = `/tmp/claude-1000/-home-gomanjoe-source-zoid/bde42ca9-c9a7-4e07-a2cd-55795be6b453/scratchpad`

### Task 6: Generate the golden request body

**Files:**
- Create (temporary): `crates/zoid-tools/tests/_tmp_golden_body.rs`
- Create: `$SCRATCH/golden_body.json`
- Delete: `crates/zoid-tools/tests/_tmp_golden_body.rs` (in Step 4 of this task)

**Interfaces:**
- Consumes: `zoid_tools::registry()`, `zoid_provider::ollama::request_body`.
- Produces: `$SCRATCH/golden_body.json` — a `/api/chat` body with `messages` and `tools` populated and **no `options` key**. Tasks 7 and 8 read this file.

**Why a throwaway:** the spec requires Track A to make no repository changes. `zoid-tools` is the host because it already depends on `zoid-provider`, so a single test can reach both crates.

- [ ] **Step 1: Write the generator**

Create `crates/zoid-tools/tests/_tmp_golden_body.rs`:

```rust
//! TEMPORARY generator — deleted in the same task that creates it.
//! Emits the exact `/api/chat` body zoid would send, so the benchmark
//! measures the real payload rather than a hand-written approximation.

use zoid_provider::{CompletionRequest, Message, ThinkingMode, ToolSpec};

/// Copy of `zoid::agent::SYSTEM_PROMPT` (crates/zoid/src/agent.rs:36). The
/// `zoid` crate is a binary, so an integration test cannot import it. If that
/// constant changes, regenerate this file.
const SYSTEM_PROMPT: &str =
    "You are zoid, a terminal coding assistant. Be concise and precise. \
     You can call tools to read, write, edit, and search files and run shell \
     commands in the user's working directory. Use them when helpful. \
     Brief single-line narration alongside tool calls is good. But when a task \
     is done, do NOT reframe or re-explain the whole effort in long paragraphs: \
     close with a short recap — a few lines or a tight list of what changed and \
     any next step. Don't restate what the tool calls and diffs already showed.";

#[test]
fn emit_golden_body() {
    let out = std::env::var("GOLDEN_OUT").expect("set GOLDEN_OUT to the target path");

    let tools: Vec<ToolSpec> = zoid_tools::registry().iter().map(|t| t.spec()).collect();
    assert!(!tools.is_empty(), "registry must not be empty");

    let req = CompletionRequest {
        // Overwritten per-model by the harness.
        model: "PLACEHOLDER".into(),
        system: Some(SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user(
            "List the Rust source files in the current directory, then read the \
             smallest one and summarize what it does in two sentences.",
        )],
        max_tokens: 2048,
        tools,
        thinking: ThinkingMode::Off,
        reassert: None,
    };

    // `None` → no `options` key. The harness injects `num_ctx` itself, because
    // it must vary that value per-run to bisect the max usable window.
    // NOTE: if Track B has not landed yet, this call takes one argument —
    // drop the `, None`.
    let body = zoid_provider::ollama::request_body(&req, None);
    assert!(body.get("options").is_none(), "harness owns options");

    let json = serde_json::to_string_pretty(&body).unwrap();
    std::fs::write(&out, &json).unwrap();
    println!("wrote {} bytes to {out}", json.len());
}
```

- [ ] **Step 2: Run it to produce the golden body**

```bash
cd /home/gomanjoe/source/zoid
SCRATCH=/tmp/claude-1000/-home-gomanjoe-source-zoid/bde42ca9-c9a7-4e07-a2cd-55795be6b453/scratchpad
GOLDEN_OUT="$SCRATCH/golden_body.json" \
  cargo test -p zoid-tools --test _tmp_golden_body -- --nocapture
```

Expected: prints `wrote N bytes`.

- [ ] **Step 3: Verify the payload is the real one**

```bash
python3 -c "
import json,sys
b=json.load(open('$SCRATCH/golden_body.json'))
print('tools:', len(b['tools']))
print('tool bytes:', len(json.dumps(b['tools'])))
print('has options:', 'options' in b)
print('roles:', [m['role'] for m in b['messages']])
"
```

Expected: `tools: 13`, tool bytes near 6112, `has options: False`, roles `['system', 'user']`.

- [ ] **Step 4: Delete the generator and confirm a clean tree**

```bash
rm crates/zoid-tools/tests/_tmp_golden_body.rs
git status --short
```

Expected: no output. Track A has left no trace in the repo.

---

### Task 7: Build the runner and confirm the truncation premise (Step 0)

**Files:**
- Create: `$SCRATCH/bench.py`
- Reads: `$SCRATCH/golden_body.json`

**Interfaces:**
- Consumes: `golden_body.json` from Task 6.
- Produces: `bench.py` exposing `run(model, num_ctx, prompt=None) -> dict` with keys `ok`, `error`, `prompt_eval_count`, `eval_count`, `prompt_tps`, `gen_tps`, `tool_calls`, `content`, `done_reason`. Task 8 imports it.

**Why Step 0 first:** the entire justification for Track B is that Ollama truncates silently rather than erroring. That claim came from documented behavior, not measurement. Confirm it against the control model before spending pull bandwidth on five candidates.

- [ ] **Step 1: Write the runner**

Create `$SCRATCH/bench.py`:

```python
#!/usr/bin/env python3
"""Replay zoid's golden /api/chat body against local Ollama models."""
import json, time, urllib.request, pathlib, sys

SCRATCH = pathlib.Path(__file__).parent
GOLDEN = json.loads((SCRATCH / "golden_body.json").read_text())
HOST = "http://localhost:11434"


def run(model, num_ctx, prompt=None, timeout=600):
    """One /api/chat turn. num_ctx=None omits options entirely."""
    body = json.loads(json.dumps(GOLDEN))  # deep copy
    body["model"] = model
    if num_ctx is not None:
        body["options"] = {"num_ctx": num_ctx}
    if prompt is not None:
        body["messages"] = [body["messages"][0], {"role": "user", "content": prompt}]

    req = urllib.request.Request(
        f"{HOST}/api/chat",
        data=json.dumps(body).encode(),
        headers={"content-type": "application/json"},
    )
    out = {"model": model, "num_ctx": num_ctx, "ok": False, "error": None,
           "tool_calls": [], "content": "", "done_reason": None}
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            for raw in resp:
                if not raw.strip():
                    continue
                frame = json.loads(raw)
                if "error" in frame:
                    out["error"] = frame["error"]
                    return out
                msg = frame.get("message") or {}
                out["content"] += msg.get("content") or ""
                for tc in msg.get("tool_calls") or []:
                    out["tool_calls"].append(tc)
                if frame.get("done"):
                    out["done_reason"] = frame.get("done_reason")
                    for k in ("prompt_eval_count", "prompt_eval_duration",
                              "eval_count", "eval_duration"):
                        out[k] = frame.get(k)
                    out["ok"] = True
    except Exception as e:  # noqa: BLE001 — surface any transport failure verbatim
        out["error"] = f"{type(e).__name__}: {e}"
    out["wall_s"] = round(time.time() - t0, 2)

    # Durations are nanoseconds.
    for count, dur, name in (("prompt_eval_count", "prompt_eval_duration", "prompt_tps"),
                             ("eval_count", "eval_duration", "gen_tps")):
        c, d = out.get(count), out.get(dur)
        out[name] = round(c / (d / 1e9), 1) if c and d else None
    return out


if __name__ == "__main__":
    print(json.dumps(run(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else None), indent=2))
```

- [ ] **Step 2: Run Step 0 — confirm truncation against the control**

```bash
SCRATCH=/tmp/claude-1000/-home-gomanjoe-source-zoid/bde42ca9-c9a7-4e07-a2cd-55795be6b453/scratchpad
python3 -c "
import sys; sys.path.insert(0, '$SCRATCH')
from bench import run
unset = run('qwen2.5-coder:14b', None)
large = run('qwen2.5-coder:14b', 32768)
print('unset  prompt_eval_count:', unset.get('prompt_eval_count'), 'err:', unset.get('error'))
print('32768  prompt_eval_count:', large.get('prompt_eval_count'), 'err:', large.get('error'))
"
```

Expected outcome and its interpretation:
- **`unset` reports materially fewer prompt tokens than `32768`** → truncation confirmed, Track B justified by measurement. Record both numbers.
- **The two counts match** → the premise is wrong for Ollama 0.21.1; it may have raised its default above our payload. Record this, and **report it before continuing** — Track B is still defensible (an explicit window beats an undocumented default) but its urgency drops and the spec's framing needs correcting.
- **`unset` returns an error mentioning context length** → Ollama now errors rather than truncating. Also a premise correction; report it.

- [ ] **Step 3: Record Ollama's actual default**

```bash
ollama ps
```

Note the `CONTEXT` column for the loaded model. This is the number the spec's "~45% consumed at rest" claim depends on; correct the spec if it differs.

- [ ] **Step 4: Verify tool-call plumbing works at all**

```bash
python3 -c "
import sys; sys.path.insert(0, '$SCRATCH')
from bench import run
r = run('qwen2.5-coder:14b', 32768)
print('tool_calls:', json.dumps(r['tool_calls'], indent=2) if r['tool_calls'] else 'NONE')
print('gen_tps:', r['gen_tps'], 'prompt_tps:', r['prompt_tps'])
" 2>&1 | head -40
```

Expected: at least one tool call naming a real zoid tool (`ls`, `glob`, or `read`). The control is known tools-capable, so a failure here means the harness is wrong, not the model.

---

### Task 8: Benchmark the candidates and write the report

**Files:**
- Create: `$SCRATCH/results.json`, `$SCRATCH/REPORT.md`
- Reads: `$SCRATCH/bench.py`

**Interfaces:**
- Consumes: `run()` from Task 7.
- Produces: the report that drives the spec's decision rule.

- [ ] **Step 1: Pull the candidates**

Pull smallest-first, so a disk or bandwidth problem surfaces cheaply:

```bash
ollama pull ornith:9b                                    # 5.6 GB
ollama pull hf.co/prism-ml/Ternary-Bonsai-27B-gguf       # 7.17 GB
ollama pull north-mini-code-1.0                          # 19 GB
ollama pull laguna-xs-2.1                                # 20 GB
ollama list
df -h /home
```

Expected: all five present (`qwen2.5-coder:14b` already is). ~52 GB against 210 GB free.

If the `hf.co/` pull fails, fall back to a Modelfile:

```bash
printf 'FROM hf.co/prism-ml/Ternary-Bonsai-27B-gguf\n' > "$SCRATCH/Modelfile.bonsai"
ollama create bonsai-27b -f "$SCRATCH/Modelfile.bonsai"
```

- [ ] **Step 2: Run metrics 1–4 for every candidate**

```bash
SCRATCH=/tmp/claude-1000/-home-gomanjoe-source-zoid/bde42ca9-c9a7-4e07-a2cd-55795be6b453/scratchpad
python3 - <<'PY'
import sys, json, subprocess
SCRATCH = "/tmp/claude-1000/-home-gomanjoe-source-zoid/bde42ca9-c9a7-4e07-a2cd-55795be6b453/scratchpad"
sys.path.insert(0, SCRATCH)
from bench import run

MODELS = ["qwen2.5-coder:14b", "ornith:9b",
          "hf.co/prism-ml/Ternary-Bonsai-27B-gguf",
          "north-mini-code-1.0", "laguna-xs-2.1"]
LADDER = [8192, 16384, 32768, 65536]

results = {}
for m in MODELS:
    entry = {"rungs": []}
    for ctx in LADDER:
        r = run(m, ctx)
        ps = subprocess.run(["ollama", "ps"], capture_output=True, text=True).stdout
        r["ollama_ps"] = ps
        # Metric 3 gate: did it emit a usable tool call?
        r["tool_gate"] = bool(r["tool_calls"])
        entry["rungs"].append(r)
        print(f"{m:42} ctx={ctx:<6} ok={r['ok']} tools={r['tool_gate']} "
              f"gen_tps={r['gen_tps']} err={r['error']}")
        if r["error"]:
            break  # higher rungs will only fail harder
    results[m] = entry
    subprocess.run(["ollama", "stop", m], capture_output=True)

json.dump(results, open(f"{SCRATCH}/results.json", "w"), indent=2)
print("\nwrote results.json")
PY
```

Record for each rung: loaded or not, the VRAM/RAM split from `ollama ps`, `prompt_tps`, `gen_tps`, and whether a tool call appeared.

- [ ] **Step 3: Validate tool calls through zoid's own parser**

A tool call that Ollama emits but zoid cannot parse is a failure. Feed one captured response frame per candidate through `ollama::parse_line` rather than trusting the shape by eye:

```bash
cd /home/gomanjoe/source/zoid
python3 -c "
import json
r = json.load(open('$SCRATCH/results.json'))
for m, e in r.items():
    tc = next((rung['tool_calls'] for rung in e['rungs'] if rung['tool_calls']), None)
    print(m, '→', json.dumps(tc[0]) if tc else 'NO TOOL CALL')
"
```

For any candidate that produced a tool call, reconstruct the NDJSON frame and assert `parse_line` yields a `ProviderEvent::ToolCall`. A candidate whose frame does not parse **fails metric 3 regardless of its benchmark scores**.

- [ ] **Step 4: Run metric 5 — multi-turn survival**

For each candidate that passed metric 3, drive a 5-step loop: after each tool call, append a synthetic `{"role":"tool","tool_name":<name>,"content":<plausible result>}` message and re-send. Record whether the model keeps issuing well-formed tool calls through step 5 or degrades into prose.

```bash
python3 - <<'PY'
import sys, json
SCRATCH = "/tmp/claude-1000/-home-gomanjoe-source-zoid/bde42ca9-c9a7-4e07-a2cd-55795be6b453/scratchpad"
sys.path.insert(0, SCRATCH)
from bench import run, GOLDEN
import urllib.request, copy

FAKE = {"ls": "src/\nCargo.toml\nREADME.md",
        "glob": "src/lib.rs\nsrc/main.rs",
        "read": "pub fn main() { println!(\"hi\"); }"}

def multiturn(model, ctx, steps=5):
    body = copy.deepcopy(GOLDEN); body["model"] = model
    body["options"] = {"num_ctx": ctx}
    survived = 0
    for i in range(steps):
        req = urllib.request.Request(f"http://localhost:11434/api/chat",
            data=json.dumps(body).encode(),
            headers={"content-type": "application/json"})
        calls, content = [], ""
        with urllib.request.urlopen(req, timeout=600) as resp:
            for raw in resp:
                if not raw.strip(): continue
                f = json.loads(raw)
                if "error" in f: return survived, f["error"]
                msg = f.get("message") or {}
                content += msg.get("content") or ""
                calls += msg.get("tool_calls") or []
        if not calls:
            return survived, f"step {i+1}: no tool call, prose only"
        survived += 1
        name = calls[0]["function"]["name"]
        body["messages"].append({"role": "assistant", "content": content,
                                 "tool_calls": calls})
        body["messages"].append({"role": "tool", "tool_name": name,
                                 "content": FAKE.get(name, "ok")})
    return survived, None

for m in json.load(open(f"{SCRATCH}/results.json")):
    try:
        n, err = multiturn(m, 32768)
        print(f"{m:42} survived {n}/5 steps  {err or ''}")
    except Exception as e:
        print(f"{m:42} ERROR {e}")
PY
```

- [ ] **Step 5: Write the report**

Create `$SCRATCH/REPORT.md` with one row per candidate and these columns: loads, VRAM/RAM split, max usable `num_ctx`, prompt tok/s, generation tok/s, tool gate (pass/fail), multi-turn steps survived. Then state which branch of the spec's decision rule fired:

- a candidate passed metrics 1–5 → local coding is viable; name it and recommend a real zoid session against it;
- candidates passed 1, 2, 4 but failed 3 → the blocker is packaging; recommend revisiting the deferred llama.cpp + `openai_compat` transport;
- only the control passed → this hardware is below the bar; record the numbers for a future revisit.

Include the Step 0 truncation numbers and Ollama's measured default, and flag any place where a measurement contradicts the spec so the spec can be corrected.

- [ ] **Step 6: Persist the findings**

Store the outcome to total-recall (`memory_store`, `project: zoid`, `entryType: decision`, warm tier), including the decision-rule branch and the per-model numbers, so a later session does not re-pull 52 GB to re-learn them.

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| Step 0 — confirm truncation premise | Task 7, Steps 2–3 |
| Golden request body from zoid's own code | Task 6 |
| Metric 1 — loads, VRAM/RAM split | Task 8, Step 2 |
| Metric 2 — tok/s | Task 8, Step 2 (`prompt_tps`, `gen_tps`) |
| Metric 3 — tool-call correctness via `parse_line` | Task 8, Step 3 |
| Metric 4 — max usable `num_ctx` | Task 8, Step 2 (the `LADDER`) |
| Metric 5 — multi-turn survival | Task 8, Step 4 |
| Track B: emit `options.num_ctx` | Tasks 2, 3, 5 |
| Track B: `ZOID_NUM_CTX` from env | Task 1 |
| Track B: honest `fetch_model_info` | Task 4 |
| Track B: cloud byte-identity | Task 2 (`cloud_body_omits_options_entirely`), Task 5 Step 4 |
| Decision rule | Task 8, Step 5 |
| Excluded: Qwen-AgentWorld | No task — correctly excluded in the spec |

**Type consistency:** `num_ctx` is `Option<u32>` in the struct (Task 3), the `request_body` parameter (Task 2), and `effective_context_window`'s second argument (Task 4). `configured_num_ctx()` and `parse_num_ctx()` return bare `u32`; `with_num_ctx` takes a bare `u32` and wraps it. `effective_context_window` returns `u64` to match `ModelInfo::context_window`.

**Known ordering coupling:** Task 2 temporarily hardcodes `request_body(req, None)` at `ollama.rs:322` so it is independently green; Task 3 changes it to `self.num_ctx`. This is called out in both tasks. Tasks 6–8 are independent of Tasks 1–5 except for the one-argument-vs-two note in Task 6, Step 1.
