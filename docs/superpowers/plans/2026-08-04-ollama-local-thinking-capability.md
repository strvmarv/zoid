# Ollama Local Thinking Capability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make zoid honor local Ollama models' `thinking` capability reported by `/api/show`, instead of hardcoding `thinking: ThinkingSupport::None`, and default thinking on for `ollama-local` when the user hasn't set `[thinking].enabled`.

**Architecture:** Two independent changes. **Track A** (Tasks 1–2) adds a `parse_ollama_thinking` helper that reads the `capabilities` array from the `/api/show` body already fetched, and wires it into `fetch_model_info`. **Track B** (Tasks 3–4) widens `resolve_thinking`'s signature with `(thinking_enabled_src: Source, provider: &str)` and adds a provider-aware default clause: when the user set no `[thinking].enabled` key (`Source::Default`) and the provider is `ollama-local`, treat `enabled` as `true`. The capability gate then runs unchanged. Track A and B are decoupled: A fixes the capability data; B fixes the default. Either lands independently.

**Tech Stack:** Rust (`serde_json`, `async_trait`), Ollama 0.21.1 `/api/show` HTTP API.

---

## Handoff Context

**Status: planned, not started.** No code written.

**Companion spec:** `docs/superpowers/specs/2026-08-04-ollama-local-thinking-capability-design.md` (commit `eeea8c7`). Read it for the root-cause analysis, the turn-1 stall evidence, and the design reasoning. This plan is the execution detail; the spec is the reasoning.

### What already exists — no new plumbing is needed

- `crates/zoid-provider/src/ollama.rs:507` — `fetch_model_info` already calls `POST /api/show` and reads the context window via `parse_ollama_context_window`. This is where Track A adds one parallel parse.
- `crates/zoid-provider/src/ollama.rs:270` — `parse_ollama_context_window(body: &str) -> Option<u64>` is the lenient-parse pattern the new `parse_ollama_thinking` mirrors: `serde_json::from_str(body).ok()?`, unknown/!json → `None`, never panics.
- `crates/zoid/src/main.rs:6818` — `resolve_thinking` is pure, and both call sites (turn at 7437, subagent at 7339) already pass through it. This is where Track B widens the signature.
- `crates/zoid-core/src/config.rs:472` — `Source` enum (`Default`, `UserGlobal`, `Project`, `Local`, `Env`), `Copy + PartialEq`.
- `crates/zoid-core/src/config.rs:494` — `Provenance.thinking_enabled: Source` is the provenance field the new gate reads.
- `crates/zoid-model/src/lib.rs:649` — `canonical_id(raw: &str) -> &str` canonicalizes provider ids (`"ollama"` → `"ollama-cloud"`); used by the new gate to match `ollama-local` regardless of spelling.

### Two design decisions that are not obvious from the tasks

**1. The default lives in `resolve_thinking`, not in config.** The "obvious" fix — flip the config default at a "config finalized" site — fails: the `context_target` clamp is duplicated across boot (main.rs:2491) and reload (main.rs:4254), and `load_config` (main.rs:215) has no provider context. Putting the default in `resolve_thinking` collapses the duplication (both call sites already route through it), makes the subagent path correct by construction, and removes the "remember to mutate `app.config` before every read" footgun. See spec §"Why the obvious fix doesn't work".

**2. The gate is `Source::Default` on the `enabled` key, not "section absent."** Provenance tracks the `enabled` key (config.rs:771), not the `[thinking]` section. A user who writes `[thinking]\neffort = "high"` with no `enabled` key has `thinking_enabled == Source::Default` — they left `enabled` to defaults, so the provider-aware default applies, and their `effort` flows through. That's correct: "I want high-effort thinking; leave the master switch to the default." See spec §"Why `Source::Default` is the right gate".

### Measured constants this plan relies on

- The Ollama `/api/show` response for `qwythos:latest` reports `capabilities: ["completion","tools","thinking","vision"]` (verified against the live daemon, 2026-08-04).
- The turn-1 stall evidence: session `01KZ7DA32AVGKZGECQFWK287A4`, 39 ModelDelta fragments (~40 tokens), `Usage {"input":3692,"output":40,"thinking":0}`, zero `ThinkingDelta` events.

---

## Global Constraints

- **Cloud request bodies must remain unaffected.** `parse_ollama_thinking` runs only inside `OllamaProvider::fetch_model_info`, which is constructed for both `ollama-local` and `ollama-cloud`. The cloud path is safe because cloud models report their real capabilities via `/api/show` too — if a cloud model reports `thinking`, sending `think: true` is correct.
- **`resolve_thinking` stays pure.** No IO, no global state, no env access. The two new args (`thinking_enabled_src: Source`, `provider: &str`) are passed in by callers. This keeps it unit-testable without an `App`.
- **Do not mutate env vars inside tests.** The repo states this at `crates/zoid-provider/src/lib.rs:373-375` — env is process-global and unsafe under parallel test execution. Test the pure parser instead.
- **`ThinkingSupport::Toggle` is the correct variant for Ollama.** The enum doc says `/// On/off only (Ollama)` (zoid-model/lib.rs:32). The native `/api/chat` `think` field is a bare bool. Do not use `ToggleWithEffort` (that's for OpenAI-compat models with an effort knob).
- **Commit messages: no `Co-Authored-By` or any co-author trailer.**

---

## File Structure

**Track A — modified:**
- `crates/zoid-provider/src/ollama.rs` — gains `parse_ollama_thinking` (after `parse_ollama_context_window` at line ~296), and `fetch_model_info` (line 507-533) reads it alongside the existing context-window parse. Tests in the same file's `mod tests`.

**Track B — modified:**
- `crates/zoid/src/main.rs` — `resolve_thinking` (line 6818) widens signature + gains the provider-aware default clause. Two call sites updated: turn (7437), subagent (7339). Five existing tests (7493-7549) updated to the new signature.

**No new files. No config changes. No registry edits.**

---

## Track A — read the daemon's thinking capability

### Task 1: `parse_ollama_thinking` helper

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs` (add after `parse_ollama_context_window` at line ~296)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `crate::model::ThinkingSupport` (from zoid-model, re-exported).
- Produces: `pub fn parse_ollama_thinking(body: &str) -> ThinkingSupport`. Task 2 calls it inside `fetch_model_info`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/zoid-provider/src/ollama.rs`, near the existing `parse_context_window_*` tests (around line 1051):

```rust
#[test]
fn parse_thinking_toggle_when_capabilities_include_thinking() {
    let body = r#"{"capabilities":["completion","tools","thinking","vision"]}"#;
    assert_eq!(
        parse_ollama_thinking(body),
        crate::model::ThinkingSupport::Toggle
    );
}

#[test]
fn parse_thinking_none_when_capabilities_omit_thinking() {
    let body = r#"{"capabilities":["completion","tools"]}"#;
    assert_eq!(
        parse_ollama_thinking(body),
        crate::model::ThinkingSupport::None
    );
}

#[test]
fn parse_thinking_none_when_capabilities_absent() {
    assert_eq!(parse_ollama_thinking(r#"{"model_info":{}}"#), crate::model::ThinkingSupport::None);
}

#[test]
fn parse_thinking_none_when_capabilities_null() {
    assert_eq!(parse_ollama_thinking(r#"{"capabilities":null}"#), crate::model::ThinkingSupport::None);
}

#[test]
fn parse_thinking_none_when_malformed_json() {
    assert_eq!(parse_ollama_thinking("not json"), crate::model::ThinkingSupport::None);
    assert_eq!(parse_ollama_thinking(""), crate::model::ThinkingSupport::None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-provider --lib ollama::tests::parse_thinking -- --nocapture`
Expected: FAIL to compile — `cannot find function 'parse_ollama_thinking' in this scope`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/zoid-provider/src/ollama.rs` directly after `parse_ollama_context_window` (which ends at line ~296):

```rust
/// Parse the Ollama `/api/show` `capabilities` array for thinking support.
/// Returns `ThinkingSupport::Toggle` when the array contains `"thinking"`,
/// `None` otherwise (including absent, non-array, null, or malformed). Lenient:
/// mirrors `parse_ollama_context_window` — unknown/!json → `None`, never panics.
/// The caller (`fetch_model_info`) falls back to "no thinking" on any parse
/// failure.
pub fn parse_ollama_thinking(body: &str) -> crate::model::ThinkingSupport {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return crate::model::ThinkingSupport::None,
    };
    let caps = match v.get("capabilities").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return crate::model::ThinkingSupport::None,
    };
    if caps.iter().any(|c| c.as_str() == Some("thinking")) {
        crate::model::ThinkingSupport::Toggle
    } else {
        crate::model::ThinkingSupport::None
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-provider --lib ollama::tests::parse_thinking -- --nocapture`
Expected: PASS, all five new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): parse_ollama_thinking reads /api/show capabilities

Pure lenient parser mirroring parse_ollama_context_window: returns
ThinkingSupport::Toggle when the capabilities array includes 'thinking',
None otherwise. Never panics on malformed/absent input."
```

---

### Task 2: Wire `parse_ollama_thinking` into `fetch_model_info`

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:507-533` (`fetch_model_info`)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `parse_ollama_thinking` from Task 1.
- Produces: `OllamaProvider::fetch_model_info` returns a `ModelInfo` whose `thinking` field reflects the daemon's reported capability. The bin's `spawn_model_info_fetch` → `app.fetched_model_info` → `resolve_thinking` path consumes this unchanged (the plumbing already prefers the fetched value over the static table).

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/zoid-provider/src/ollama.rs`. This tests the *parse* feeding the *struct*, not a live daemon — construct a `ModelInfo` the way `fetch_model_info` does, using the new parse:

```rust
#[test]
fn fetch_model_info_thinking_reflects_capabilities() {
    // A /api/show body with the thinking capability yields Toggle.
    let body = r#"{"capabilities":["completion","tools","thinking"],"model_info":{"qwen35.context_length":1048576.0}}"#;
    let window = parse_ollama_context_window(body);
    let thinking = parse_ollama_thinking(body);
    assert!(window.is_some(), "context window must parse");
    assert_eq!(thinking, crate::model::ThinkingSupport::Toggle);
}

#[test]
fn fetch_model_info_thinking_none_without_capability() {
    let body = r#"{"capabilities":["completion","tools"],"model_info":{"qwen35.context_length":32768.0}}"#;
    let thinking = parse_ollama_thinking(body);
    assert_eq!(thinking, crate::model::ThinkingSupport::None);
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p zoid-provider --lib ollama::tests::fetch_model_info_thinking -- --nocapture`
Expected: PASS — the helpers already exist from Task 1; these tests assert they compose correctly into the shape `fetch_model_info` builds.

- [ ] **Step 3: Write the implementation**

In `crates/zoid-provider/src/ollama.rs`, change `fetch_model_info` (currently lines 517-532). Replace the `let window = ...` block and the `Ok(window.map(...))` block with:

```rust
        let body = resp.text().await?;
        let window = parse_ollama_context_window(&body);
        let thinking = parse_ollama_thinking(&body);
        Ok(window.map(|w| crate::model::ModelInfo {
            // A local daemon silently truncates past its actual context window.
            // If we requested `num_ctx`, clamp the reported window to that value
            // so the preflight gate and the economy view reflect the real limit
            // — not the model's theoretical max that the daemon won't honor.
            context_window: self.num_ctx
                .filter(|&n| (n as u64) < w)
                .map(|n| n as u64)
                .unwrap_or(w),
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking,
            thinking_wire: crate::model::ThinkingWireShape::None,
        }))
```

The only change vs. the existing code is: add `let thinking = parse_ollama_thinking(&body);` after the `window` parse, and change `thinking: crate::model::ThinkingSupport::None` to `thinking,`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-provider --lib ollama:: -- --nocapture`
Expected: PASS — all new tests plus every pre-existing `ollama::tests` test.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs
git commit -m "feat(provider): fetch_model_info reads thinking capability from /api/show

Was hardcoded to ThinkingSupport::None, discarding the daemon's
capabilities array. Now parse_ollama_thinking feeds the ModelInfo that
flows into app.fetched_model_info → resolve_thinking."
```

---

## Track B — provider-aware thinking default

### Task 3: Widen `resolve_thinking` with the provider-aware default

**Files:**
- Modify: `crates/zoid/src/main.rs:6818-6846` (`resolve_thinking`)
- Test: same file, `mod tests` (lines 7493-7549)

**Interfaces:**
- Consumes: `zoid_core::config::Source` (config.rs:472), `zoid_provider::model::canonical_id` (zoid-model/lib.rs:649).
- Produces: `fn resolve_thinking(config_thinking: &ThinkingConfig, thinking_enabled_src: Source, provider: &str, model_support: ThinkingSupport) -> ThinkingMode`. Task 4 updates the two call sites to pass the new args.

**Why the signature widens:** the provider-aware default needs `Source::Default` (provenance) and the provider id. Passing them as args keeps the function pure and unit-testable. Both call sites already have `app` in scope, so `app.prov.thinking_enabled` and `&app.config.provider` are one field and one borrow away.

- [ ] **Step 1: Write the failing tests**

Replace the existing five `resolve_thinking_*` tests (main.rs:7493-7549) with versions using the widened signature, plus new tests for the provider-aware default. Add these to the same `mod tests`:

```rust
    #[test]
    fn resolve_thinking_forces_off_when_unsupported() {
        let cfg = zoid_core::config::ThinkingConfig { enabled: true, effort: Some("high".into()) };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::Default, "anthropic-api", zoid_provider::model::ThinkingSupport::None);
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_off_when_config_disabled() {
        let cfg = zoid_core::config::ThinkingConfig { enabled: false, effort: None };
        // Explicit enabled=false → Source::UserGlobal → user override wins.
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::UserGlobal, "ollama-local", zoid_provider::model::ThinkingSupport::Toggle);
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_auto_when_enabled_no_effort() {
        let cfg = zoid_core::config::ThinkingConfig { enabled: true, effort: None };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::UserGlobal, "anthropic-api", zoid_provider::model::ThinkingSupport::Budget);
        assert_eq!(mode, zoid_provider::ThinkingMode::Auto);
    }

    #[test]
    fn resolve_thinking_effort_when_enabled_with_effort() {
        let cfg = zoid_core::config::ThinkingConfig { enabled: true, effort: Some("max".into()) };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::UserGlobal, "anthropic-api", zoid_provider::model::ThinkingSupport::Adaptive);
        assert_eq!(mode, zoid_provider::ThinkingMode::Effort(zoid_provider::EffortLevel::Max));
    }

    #[test]
    fn resolve_thinking_provider_default_flips_on_for_ollama_local() {
        // Source::Default (user set no [thinking].enabled) + ollama-local + capable → Auto.
        let cfg = zoid_core::config::ThinkingConfig { enabled: false, effort: None };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::Default, "ollama-local", zoid_provider::model::ThinkingSupport::Toggle);
        assert_eq!(mode, zoid_provider::ThinkingMode::Auto);
    }

    #[test]
    fn resolve_thinking_provider_default_off_for_cloud() {
        // Same Default provenance, but a cloud provider → stays Off (false default).
        let cfg = zoid_core::config::ThinkingConfig { enabled: false, effort: None };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::Default, "ollama-cloud", zoid_provider::model::ThinkingSupport::Toggle);
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_provider_default_off_when_capability_none() {
        // Provider default flips on, but the model doesn't support thinking → Off.
        let cfg = zoid_core::config::ThinkingConfig { enabled: false, effort: None };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::Default, "ollama-local", zoid_provider::model::ThinkingSupport::None);
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_env_override_wins_over_provider_default() {
        // ZOID_THINKING=off → Source::Env → user override wins, even for ollama-local.
        let cfg = zoid_core::config::ThinkingConfig { enabled: false, effort: None };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::Env, "ollama-local", zoid_provider::model::ThinkingSupport::Toggle);
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }

    #[test]
    fn resolve_thinking_effort_only_section_flows_through() {
        // User wrote [thinking] effort="high" with no enabled key. thinking_enabled
        // is Source::Default (the enabled key was absent), so the provider default
        // flips enabled to true, and effort flows through. The result is Effort(High).
        let cfg = zoid_core::config::ThinkingConfig { enabled: false, effort: Some("high".into()) };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::Default, "ollama-local", zoid_provider::model::ThinkingSupport::Toggle);
        assert_eq!(mode, zoid_provider::ThinkingMode::Effort(zoid_provider::EffortLevel::High));
    }

    #[test]
    fn resolve_thinking_canonical_id_matches_legacy_ollama_spelling() {
        // "ollama" canonicalizes to "ollama-cloud", so the local default does NOT
        // apply to the legacy spelling. Only "ollama-local" matches.
        let cfg = zoid_core::config::ThinkingConfig { enabled: false, effort: None };
        let mode = resolve_thinking(&cfg, zoid_core::config::Source::Default, "ollama", zoid_provider::model::ThinkingSupport::Toggle);
        assert_eq!(mode, zoid_provider::ThinkingMode::Off);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid --lib resolve_thinking -- --nocapture`
Expected: FAIL to compile — `this function takes 2 arguments but 4 were supplied`.

- [ ] **Step 3: Write the implementation**

Replace `resolve_thinking` at `crates/zoid/src/main.rs:6818-6846` with:

```rust
/// Resolve the final `ThinkingMode` from config + provenance + provider + model
/// capability. Pure — takes explicit args so it's unit-testable. No IO, no
/// global state.
///
/// **Provider-aware default:** when `thinking_enabled_src == Source::Default`
/// (the user set no `[thinking].enabled` key in any config layer) and the
/// provider is `ollama-local`, `enabled` is treated as `true`. This makes
/// thinking available by default for local models that support it; the
/// capability gate below still returns `Off` if the model doesn't support
/// thinking. An explicit `enabled = false` (any `Source != Default`) always
/// wins — that's the user override. `ZOID_THINKING` sets `Source::Env`, which
/// is `!= Default`, so env wins too.
fn resolve_thinking(
    config_thinking: &zoid_core::config::ThinkingConfig,
    thinking_enabled_src: zoid_core::config::Source,
    provider: &str,
    model_support: zoid_provider::model::ThinkingSupport,
) -> zoid_provider::ThinkingMode {
    use zoid_provider::ThinkingMode;
    // Effective enabled flag: the user's value, or true for ollama-local when
    // the user set no [thinking].enabled key (provenance Default). An explicit
    // enabled = false (provenance != Default) always wins.
    let enabled = if thinking_enabled_src == zoid_core::config::Source::Default
        && zoid_provider::model::canonical_id(provider) == "ollama-local"
    {
        true
    } else {
        config_thinking.enabled
    };
    match model_support {
        zoid_provider::model::ThinkingSupport::None => ThinkingMode::Off,
        _ if !enabled => ThinkingMode::Off,
        _ => {
            match &config_thinking.effort {
                None => ThinkingMode::Auto,
                Some(e) => {
                    use zoid_provider::EffortLevel;
                    let level = match e.as_str() {
                        "low" => EffortLevel::Low,
                        "medium" => EffortLevel::Medium,
                        "high" => EffortLevel::High,
                        "max" => EffortLevel::Max,
                        _ => EffortLevel::High,
                    };
                    ThinkingMode::Effort(level)
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid --lib resolve_thinking -- --nocapture`
Expected: PASS — all ten `resolve_thinking_*` tests.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(thinking): provider-aware default for ollama-local in resolve_thinking

resolve_thinking widens to (config, thinking_enabled_src, provider,
model_support). When Source::Default + ollama-local, enabled defaults to
true; the capability gate still returns Off for non-thinking models. An
explicit enabled=false or ZOID_THINKING=off (Source != Default) wins."
```

---

### Task 4: Update the two `resolve_thinking` call sites

**Files:**
- Modify: `crates/zoid/src/main.rs:7339` (subagent spawn), `:7437` (turn spawn)
- Test: `cargo build` (no new tests — the call sites are wiring, behavior is covered by Task 3's unit tests)

**Interfaces:**
- Consumes: the widened `resolve_thinking` from Task 3.
- Produces: no new API — wiring only.

**Why both sites have the args:** both are inside closures/blocks with `app` in scope. `app.prov.thinking_enabled` is the provenance field; `&app.config.provider` is the provider id.

- [ ] **Step 1: Update the subagent call site**

At `crates/zoid/src/main.rs:7339`, change:

```rust
            resolve_thinking(&app.config.thinking, model_support)
```

to:

```rust
            resolve_thinking(
                &app.config.thinking,
                app.prov.thinking_enabled,
                &app.config.provider,
                model_support,
            )
```

- [ ] **Step 2: Update the turn call site**

At `crates/zoid/src/main.rs:7437`, change:

```rust
    turn_config.thinking = resolve_thinking(&app.config.thinking, model_support);
```

to:

```rust
    turn_config.thinking = resolve_thinking(
        &app.config.thinking,
        app.prov.thinking_enabled,
        &app.config.provider,
        model_support,
    );
```

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build -p zoid`
Expected: compiles cleanly. (If there are other `resolve_thinking` call sites in non-test code the grep missed, the compiler will name them — update each the same way.)

- [ ] **Step 4: Run the full test suite to verify no regressions**

Run: `cargo test -p zoid --lib -- --nocapture`
Expected: PASS — all tests, including the `resolve_thinking_*` suite from Task 3 and the pre-existing `spawn_turn`/`model_switch` tests.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(thinking): wire provenance + provider into resolve_thinking call sites

Both the turn (main.rs:7437) and subagent (main.rs:7339) call sites now
pass app.prov.thinking_enabled and &app.config.provider, enabling the
ollama-local thinking default."
```

---

## Post-implementation smoke test (not a task — manual, after both tracks land)

After both tracks are merged and zoid is rebuilt:

1. `cargo build --release` and run zoid against the local daemon with `provider = "ollama-local"`, `model = "qwythos:latest"`, no `[thinking]` section in config.
2. Send "give me a summary of this repository and it's purpose" from a repo.
3. Expect: qwythos streams `ThinkingDelta` events (visible in the thinking view if enabled), then calls `ls`/`read` — **no preamble-only stall, no "continue" needed**.
4. Contrast with the pre-fix session `01KZ7DA32AVGKZGECQFWK287A4` (40-token preamble, `thinking: 0`, stall). The done-frame usage should now show `thinking > 0`.
5. Set `[thinking] enabled = false` in config, reload, repeat. Expect: no thinking, behavior reverts to the pre-fix announce-then-stop. This confirms the user override.