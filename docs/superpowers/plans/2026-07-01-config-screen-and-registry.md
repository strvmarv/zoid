# Config Screen, Config System, Secrets & Model Registry — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give zoid an in-app full-screen configuration screen backed by a layered TOML config system, an encrypted-DB secret store, and a basic model registry — replacing today's scattered env-var-only config.

**Architecture:** A basic model registry (`zoid-provider::model`) becomes the single source for known providers/models and per-model context windows. A pure `Config` value (`zoid-core::config`) is merged from ordered layers (defaults → user-global TOML → project TOML → local TOML → `ZOID_*` env), each field tracking provenance. Secrets live encrypted in `zoid.db` behind a `SecretStore` seam (env wins at read). A two-pane full-screen overlay (`zoid-tui`) renders the resolved config with provenance tags and writes edits back to the active layer, applying live where possible; `economy.*` config feeds the live `ContextPolicy`.

**Tech Stack:** Rust 2021; `toml` + `serde` (config files); `chacha20poly1305` + `rand` (secret AEAD); existing `rusqlite`; `ratatui`; `insta` snapshots; `proptest`.

## Global Constraints

- **Secrets never in committed or file config** — read only from env or the encrypted DB; never written to any `*.toml`. (Amends core §7.1 to sanction encrypted `zoid.db`.)
- **Tokens, not dollars** — no pricing config or display.
- **§16 design tokens** — no literal glyphs/hex in rendered UI outside `crates/zoid-tui/src/tokens.rs` (comments/detection-sentinels exempt).
- **Single static binary** — new deps must be pure-Rust (no OpenSSL). `chacha20poly1305`, `rand`, `toml` all qualify.
- **User-global config is the base; repo config is an optional override** created only on demand.
- **Commit message trailer** (every commit): end with
  `Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY`
  and **never** add a Co-Authored-By/co-author trailer.
- **Provider read precedence for secrets & model/motion:** env var wins over stored/file value.
- **Crypto:** XChaCha20-Poly1305; app key is 32 random bytes in `~/.local/share/zoid/secret.key` at `0600`, **not** in the DB.
- **model field:** `provider` cycles the registry's fixed known set; `model` is free-text with a registry-backed cycle over that provider's known models (free-text fallback always allowed).

---

## File Structure

- `crates/zoid-provider/src/model.rs` **(new)** — `ModelInfo`, known providers/models, `model_info()`; `context_ceiling()` folds in here.
- `crates/zoid-provider/src/lib.rs` **(modify)** — `pub mod model;`, delete `model_ceiling`, re-point `context_ceiling`.
- `crates/zoid-core/src/config.rs` **(new)** — `Config`, `EconomyConfig`, `PartialConfig`, `Provenance`, TOML parse, pure merge, single-layer serialize.
- `crates/zoid-core/src/secret.rs` **(new)** — `SecretStore` trait, `EncryptedDb`, key-file mgmt, AEAD.
- `crates/zoid-core/src/store.rs` **(modify)** — add `secrets` table to `open()`.
- `crates/zoid-core/src/lib.rs` **(modify)** — `pub mod config; pub mod secret;`.
- `crates/zoid/src/main.rs` **(modify)** — config path resolution + layer load; apply config; `SecretStore` injection; economy policy from config; open/route the config screen.
- `crates/zoid-tui/src/config_view.rs` **(new)** — pure view-model (sections/fields/provenance) for the screen.
- `crates/zoid-tui/src/state.rs` **(modify)** — `Overlay::Config`, config-edit state.
- `crates/zoid-tui/src/command.rs` **(modify)** — `Command::OpenConfig`, parse `:config`.
- `crates/zoid-tui/src/palette.rs` **(modify)** — "Open settings" row in the settings group.
- `crates/zoid-tui/src/render.rs` **(modify)** — render the two-pane config overlay.
- `crates/zoid-tui/src/route.rs` **(modify)** — key routing for the config overlay.
- `crates/zoid-tui/src/tokens.rs` **(modify only if needed)** — reuse existing glyph/color tokens.

---

# Phase 0 — Basic Model Registry

### Task 1: Model registry in zoid-provider

**Files:**
- Create: `crates/zoid-provider/src/model.rs`
- Modify: `crates/zoid-provider/src/lib.rs` (add `pub mod model;`; delete `model_ceiling`; re-point `context_ceiling`)

**Interfaces:**
- Produces:
  - `pub struct ModelInfo { pub context_window: u64, pub max_output: u64, pub tools: bool }`
  - `pub const KNOWN_PROVIDERS: &[&str]` (= `["ollama", "anthropic"]`)
  - `pub fn models_for(provider: &str) -> &'static [&'static str]`
  - `pub fn model_info(model: &str) -> ModelInfo`
  - `pub fn context_ceiling(model: &str) -> u64` (unchanged signature; now delegates to `model_info`)

- [ ] **Step 1: Write the failing tests** — create `crates/zoid-provider/src/model.rs`:

```rust
//! Basic, caps-only model registry (spec 2026-07-01-model-registry.md): one
//! source of truth for known providers/models and per-model capabilities.
//! No cost/pricing (economy is tokens-only). Wire-derived caps (Ollama
//! /api/show) are a future refinement.

/// Stable, model-agnostic capabilities of a model. No cost fields by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub context_window: u64,
    pub max_output: u64, // 0 = "use provider default"
    pub tools: bool,
}

/// The providers the config screen can cycle. First entry is the default.
pub const KNOWN_PROVIDERS: &[&str] = &["ollama", "anthropic"];

/// Known model ids for a provider (first = that provider's default). Ollama can
/// run arbitrary tags, so this is a convenience list, not a closed set — the
/// config screen offers free-text entry alongside it.
pub fn models_for(provider: &str) -> &'static [&'static str] {
    match provider {
        "ollama" => &["glm-5.2:cloud"],
        "anthropic" => &["claude-sonnet-4-6", "claude-opus-4-8"],
        _ => &[],
    }
}

/// Capabilities for `model`, matched by family (case-insensitive), else DEFAULT.
pub fn model_info(model: &str) -> ModelInfo {
    let m = model.to_ascii_lowercase();
    // Claude is a known 200k family; everything else (incl. GLM, whose exact
    // window is a registry TODO) takes the 256k conservative default.
    let context_window = if m.contains("claude") { 200_000 } else { 256_000 };
    ModelInfo { context_window, max_output: 0, tools: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_info_caps_by_family_else_default() {
        assert_eq!(model_info("claude-sonnet-4-6").context_window, 200_000);
        assert_eq!(model_info("CLAUDE-opus").context_window, 200_000);
        assert_eq!(model_info("glm-5.2:cloud").context_window, 256_000);
        assert_eq!(model_info("llama3.1:70b").context_window, 256_000);
        assert!(model_info("anything").tools);
    }

    #[test]
    fn known_providers_and_models() {
        assert_eq!(KNOWN_PROVIDERS, &["ollama", "anthropic"]);
        assert_eq!(models_for("ollama"), &["glm-5.2:cloud"]);
        assert!(models_for("anthropic").contains(&"claude-sonnet-4-6"));
        assert!(models_for("unknown").is_empty());
    }
}
```

- [ ] **Step 2: Run tests — verify they fail (module not wired)**

Run: `cargo test -p zoid-provider model::`
Expected: FAIL to compile — `model` module not declared in `lib.rs`.

- [ ] **Step 3: Wire the module + re-point `context_ceiling`, delete `model_ceiling`**

In `crates/zoid-provider/src/lib.rs`: add near the other `pub mod` lines:

```rust
pub mod model;
```

Delete the existing private `fn model_ceiling(...)` entirely and change `context_ceiling` to delegate:

```rust
/// The context-window ceiling (tokens) for `model` — the economy ⑤ denominator.
/// `ZOID_CONTEXT_CEILING` (a positive integer) overrides the registry.
pub fn context_ceiling(model: &str) -> u64 {
    if let Ok(v) = std::env::var("ZOID_CONTEXT_CEILING") {
        if let Ok(n) = v.trim().parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }
    model::model_info(model).context_window
}
```

In the existing `selection_tests` module, delete the `model_ceiling_maps_known_caps_else_conservative_default` test (its logic now lives in `model::tests`). Leave `default_model_constants_are_wired`.

- [ ] **Step 4: Run tests — verify pass + clippy clean**

Run: `cargo test -p zoid-provider && cargo clippy -p zoid-provider`
Expected: PASS; no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/model.rs crates/zoid-provider/src/lib.rs
git commit -m "feat(provider): basic model registry (caps + known providers/models)

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

# Phase 1 — Config Core (types, merge, provenance, load)

### Task 2: Config types + defaults

**Files:**
- Create: `crates/zoid-core/src/config.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod config;`)
- Modify: `crates/zoid-core/Cargo.toml` (add `toml`); `Cargo.toml` workspace deps (add `toml = "0.8"`)

**Interfaces:**
- Produces:
  - `pub struct Config { pub provider: String, pub base_url: Option<String>, pub model: String, pub economy: EconomyConfig, pub reduced_motion: bool }`
  - `pub struct EconomyConfig { pub context_ceiling: Option<u64>, pub auto_evict_cold: bool, pub compact_threshold_pct: u8, pub token_ceiling: Option<u64> }`
  - `impl Default for Config` / `EconomyConfig`

- [ ] **Step 1: Add the `toml` dependency**

In root `Cargo.toml` under `[workspace.dependencies]` add:

```toml
toml = "0.8"
```

In `crates/zoid-core/Cargo.toml` under `[dependencies]` add:

```toml
toml = { workspace = true }
```

- [ ] **Step 2: Write the failing test** — create `crates/zoid-core/src/config.rs`:

```rust
//! Layered application configuration (core §7.1). Pure types + merge here;
//! file/env IO lives in the binary. Secrets are NOT part of Config (see
//! `secret.rs`) — never serialize an API key to a config file.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub provider: String,
    pub base_url: Option<String>,
    pub model: String,
    pub economy: EconomyConfig,
    pub reduced_motion: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EconomyConfig {
    /// None → defer to the model registry's context_ceiling().
    pub context_ceiling: Option<u64>,
    pub auto_evict_cold: bool,
    /// 0 disables compaction; else percent of the ceiling (1–100).
    pub compact_threshold_pct: u8,
    pub token_ceiling: Option<u64>,
}

impl Default for EconomyConfig {
    fn default() -> Self {
        Self { context_ceiling: None, auto_evict_cold: true, compact_threshold_pct: 0, token_ceiling: None }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "ollama".to_string(),
            base_url: None,
            model: String::new(), // empty → binary falls back to provider default_model()
            economy: EconomyConfig::default(),
            reduced_motion: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.provider, "ollama");
        assert!(c.economy.auto_evict_cold);
        assert_eq!(c.economy.compact_threshold_pct, 0);
        assert!(c.economy.context_ceiling.is_none());
    }
}
```

- [ ] **Step 3: Wire the module**

In `crates/zoid-core/src/lib.rs` add:

```rust
pub mod config;
```

- [ ] **Step 4: Run test — verify pass**

Run: `cargo test -p zoid-core config::tests::defaults_are_sane`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/zoid-core/Cargo.toml crates/zoid-core/src/config.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): config types + defaults

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 3: Partial layers, TOML parse, merge + provenance

**Files:**
- Modify: `crates/zoid-core/src/config.rs`

**Interfaces:**
- Consumes: `Config`, `EconomyConfig` (Task 2)
- Produces:
  - `pub enum Source { Default, UserGlobal, Project, Local, Env }`
  - `pub struct Provenance { pub provider: Source, pub base_url: Source, pub model: Source, pub context_ceiling: Source, pub auto_evict_cold: Source, pub compact_threshold_pct: Source, pub token_ceiling: Source, pub reduced_motion: Source }`
  - `pub struct PartialConfig { ... all Option<...> ... }` with `Deserialize`
  - `pub fn parse_toml(s: &str) -> anyhow::Result<PartialConfig>`
  - `pub fn merge(layers: &[(Source, PartialConfig)]) -> (Config, Provenance)`

- [ ] **Step 1: Write the failing tests** — append to `config.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source { Default, UserGlobal, Project, Local, Env }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    pub provider: Source,
    pub base_url: Source,
    pub model: Source,
    pub context_ceiling: Source,
    pub auto_evict_cold: Source,
    pub compact_threshold_pct: Source,
    pub token_ceiling: Source,
    pub reduced_motion: Source,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialEconomy {
    pub context_ceiling: Option<u64>,
    pub auto_evict_cold: Option<bool>,
    pub compact_threshold_pct: Option<u8>,
    pub token_ceiling: Option<u64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialConfig {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub reduced_motion: Option<bool>,
    pub economy: PartialEconomy,
}

/// Parse one TOML layer. Unknown keys are rejected so typos surface early.
pub fn parse_toml(s: &str) -> anyhow::Result<PartialConfig> {
    Ok(toml::from_str(s)?)
}

/// Merge layers in order; later layers override earlier. Records the winning
/// source per field. `layers` MUST start with `(Source::Default, _)` conceptually;
/// callers pass real layers and merge seeds from `Config::default()`.
pub fn merge(layers: &[(Source, PartialConfig)]) -> (Config, Provenance) {
    let mut cfg = Config::default();
    let mut prov = Provenance {
        provider: Source::Default, base_url: Source::Default, model: Source::Default,
        context_ceiling: Source::Default, auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default, token_ceiling: Source::Default,
        reduced_motion: Source::Default,
    };
    for (src, p) in layers {
        if let Some(v) = &p.provider { cfg.provider = v.clone(); prov.provider = *src; }
        if let Some(v) = &p.base_url { cfg.base_url = Some(v.clone()); prov.base_url = *src; }
        if let Some(v) = &p.model { cfg.model = v.clone(); prov.model = *src; }
        if let Some(v) = p.reduced_motion { cfg.reduced_motion = v; prov.reduced_motion = *src; }
        if let Some(v) = p.economy.context_ceiling { cfg.economy.context_ceiling = Some(v); prov.context_ceiling = *src; }
        if let Some(v) = p.economy.auto_evict_cold { cfg.economy.auto_evict_cold = v; prov.auto_evict_cold = *src; }
        if let Some(v) = p.economy.compact_threshold_pct { cfg.economy.compact_threshold_pct = v; prov.compact_threshold_pct = *src; }
        if let Some(v) = p.economy.token_ceiling { cfg.economy.token_ceiling = Some(v); prov.token_ceiling = *src; }
    }
    (cfg, prov)
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn later_layers_override_and_record_source() {
        let user = parse_toml("model = \"a\"\nreduced_motion = true\n[economy]\nauto_evict_cold = false").unwrap();
        let proj = parse_toml("model = \"b\"").unwrap();
        let (cfg, prov) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(cfg.model, "b");
        assert_eq!(prov.model, Source::Project);        // project overrode user
        assert!(cfg.reduced_motion);
        assert_eq!(prov.reduced_motion, Source::UserGlobal);
        assert!(!cfg.economy.auto_evict_cold);
        assert_eq!(prov.auto_evict_cold, Source::UserGlobal);
        assert_eq!(prov.provider, Source::Default);      // untouched
    }

    #[test]
    fn empty_layer_changes_nothing() {
        let (cfg, prov) = merge(&[(Source::UserGlobal, PartialConfig::default())]);
        assert_eq!(cfg, Config::default());
        assert_eq!(prov.model, Source::Default);
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(parse_toml("bogus = 1").is_err());
    }
}
```

- [ ] **Step 2: Run tests — verify fail then pass**

Run: `cargo test -p zoid-core config::`
Expected: compiles and PASSES (all code above is complete).

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(core): config partial layers, toml parse, merge + provenance

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 4: Single-layer TOML serialize (write-back)

**Files:**
- Modify: `crates/zoid-core/src/config.rs`

**Interfaces:**
- Produces: `pub fn set_in_toml(existing: &str, dotted_key: &str, value: TomlValue) -> anyhow::Result<String>` where `pub enum TomlValue { Str(String), Int(i64), Bool(bool), Unset }`
  - Edits/inserts `dotted_key` (e.g. `"model"`, `"economy.context_ceiling"`) in the parsed TOML document, preserving other keys; `Unset` removes the key. Returns serialized TOML text.

- [ ] **Step 1: Write the failing tests** — append to `config.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TomlValue { Str(String), Int(i64), Bool(bool), Unset }

/// Set (or, for `Unset`, remove) a dotted key in a TOML document string,
/// preserving all other content. Only the top-level table and a single nested
/// table (e.g. `economy.*`) are supported — matching Config's shape.
pub fn set_in_toml(existing: &str, dotted_key: &str, value: TomlValue) -> anyhow::Result<String> {
    let mut doc: toml::Table = if existing.trim().is_empty() {
        toml::Table::new()
    } else {
        existing.parse()?
    };
    let to_val = |v: &TomlValue| -> Option<toml::Value> {
        match v {
            TomlValue::Str(s) => Some(toml::Value::String(s.clone())),
            TomlValue::Int(i) => Some(toml::Value::Integer(*i)),
            TomlValue::Bool(b) => Some(toml::Value::Boolean(*b)),
            TomlValue::Unset => None,
        }
    };
    match dotted_key.split_once('.') {
        None => {
            match to_val(&value) {
                Some(v) => { doc.insert(dotted_key.to_string(), v); }
                None => { doc.remove(dotted_key); }
            }
        }
        Some((table, key)) => {
            let entry = doc.entry(table.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(t) = entry {
                match to_val(&value) {
                    Some(v) => { t.insert(key.to_string(), v); }
                    None => { t.remove(key); }
                }
            }
        }
    }
    Ok(toml::to_string_pretty(&doc)?)
}

#[cfg(test)]
mod write_tests {
    use super::*;

    #[test]
    fn sets_top_level_and_nested_preserving_others() {
        let src = "model = \"old\"\n[economy]\nauto_evict_cold = true\n";
        let out = set_in_toml(src, "model", TomlValue::Str("new".into())).unwrap();
        let out = set_in_toml(&out, "economy.context_ceiling", TomlValue::Int(512000)).unwrap();
        let p = parse_toml(&out).unwrap();
        assert_eq!(p.model.as_deref(), Some("new"));
        assert_eq!(p.economy.context_ceiling, Some(512000));
        assert_eq!(p.economy.auto_evict_cold, Some(true)); // preserved
    }

    #[test]
    fn unset_removes_key() {
        let out = set_in_toml("model = \"x\"\n", "model", TomlValue::Unset).unwrap();
        assert!(parse_toml(&out).unwrap().model.is_none());
    }

    #[test]
    fn writes_into_empty_document() {
        let out = set_in_toml("", "reduced_motion", TomlValue::Bool(true)).unwrap();
        assert_eq!(parse_toml(&out).unwrap().reduced_motion, Some(true));
    }
}
```

- [ ] **Step 2: Run tests — verify pass**

Run: `cargo test -p zoid-core config::write_tests`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(core): single-layer TOML write-back preserving other keys

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

# Phase 2 — Encrypted Secret Store

### Task 5: `secrets` table in the store

**Files:**
- Modify: `crates/zoid-core/src/store.rs:19-34` (the `open()` `execute_batch`)

**Interfaces:**
- Produces: a `secrets(name TEXT PRIMARY KEY, ciphertext BLOB NOT NULL, nonce BLOB NOT NULL, created_ts INTEGER NOT NULL)` table, created idempotently.

- [ ] **Step 1: Write the failing test** — append to `store.rs` tests module:

```rust
#[test]
fn open_creates_secrets_table() {
    let s = EventStore::open(":memory:").unwrap();
    // If the table exists this query succeeds (0 rows); otherwise it errors.
    let n: i64 = s
        .conn
        .query_row("SELECT count(*) FROM secrets", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}
```

(If `conn` is private, add `#[cfg(test)] pub(crate) fn conn(&self) -> &Connection { &self.conn }` and use it — check existing tests for the established accessor pattern first and reuse it.)

- [ ] **Step 2: Run test — verify it fails**

Run: `cargo test -p zoid-core open_creates_secrets_table`
Expected: FAIL — `no such table: secrets`.

- [ ] **Step 3: Add the table** — extend the `execute_batch` string in `open()`:

```rust
            CREATE TABLE IF NOT EXISTS secrets (
                name        TEXT PRIMARY KEY,
                ciphertext  BLOB NOT NULL,
                nonce       BLOB NOT NULL,
                created_ts  INTEGER NOT NULL
            );
```

(Append inside the same batch, before the closing `",`.)

- [ ] **Step 4: Run test — verify pass**

Run: `cargo test -p zoid-core open_creates_secrets_table`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(core): add encrypted secrets table to the store schema

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 6: SecretStore seam + EncryptedDb + AEAD + key file

**Files:**
- Create: `crates/zoid-core/src/secret.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod secret;`)
- Modify: `crates/zoid-core/Cargo.toml` + workspace `Cargo.toml` (add `chacha20poly1305`, `rand`)

**Interfaces:**
- Produces:
  - `pub enum SecretStatus { Set { from_env: bool }, NotSet }`
  - `pub trait SecretStore { fn get(&self, name: &str) -> Option<String>; fn set(&self, name: &str, val: &str) -> anyhow::Result<()>; fn clear(&self, name: &str) -> anyhow::Result<()>; fn status(&self, name: &str) -> SecretStatus; }`
  - `pub struct EncryptedDb { /* db path + key */ }` with `pub fn open(db_path: &str, key_path: &std::path::Path) -> anyhow::Result<Self>`
  - EncryptedDb resolves `get`/`status` env-first (reads the env var whose name == `name`).

- [ ] **Step 1: Add dependencies**

Root `Cargo.toml` `[workspace.dependencies]`:

```toml
chacha20poly1305 = "0.10"
rand = "0.8"
```

`crates/zoid-core/Cargo.toml` `[dependencies]`:

```toml
chacha20poly1305 = { workspace = true }
rand = { workspace = true }
```

- [ ] **Step 2: Write the failing tests** — create `crates/zoid-core/src/secret.rs`:

```rust
//! Encrypted secret store (spec 2026-07-01-config-screen-design.md §3).
//! Threat model: HYGIENE at rest — defeats casual exposure (cat/grep/git/
//! screen-share/backup), NOT a same-uid local attacker. The app key lives in a
//! separate 0600 file, never in the DB, so a copied DB can't be decrypted.

use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce};
use rusqlite::{params, Connection};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStatus {
    Set { from_env: bool },
    NotSet,
}

pub trait SecretStore {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, val: &str) -> Result<()>;
    fn clear(&self, name: &str) -> Result<()>;
    fn status(&self, name: &str) -> SecretStatus;
}

/// Encrypted-DB backend. Env var (same name) wins on read.
pub struct EncryptedDb {
    conn: Connection,
    cipher: XChaCha20Poly1305,
}

impl EncryptedDb {
    /// Open the store at `db_path`, loading (or creating, 0600) the 32-byte app
    /// key at `key_path`. `db_path` may be an existing zoid.db (the `secrets`
    /// table is created by `EventStore::open`, but we also ensure it here so the
    /// store is usable standalone in tests).
    pub fn open(db_path: &str, key_path: &Path) -> Result<Self> {
        let key = load_or_create_key(key_path)?;
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS secrets (
                name TEXT PRIMARY KEY, ciphertext BLOB NOT NULL,
                nonce BLOB NOT NULL, created_ts INTEGER NOT NULL);",
        )?;
        let cipher = XChaCha20Poly1305::new((&key).into());
        Ok(Self { conn, cipher })
    }

    fn stored(&self, name: &str) -> Option<String> {
        let row: Option<(Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT ciphertext, nonce FROM secrets WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let (ct, nonce) = row?;
        let pt = self.cipher.decrypt(XNonce::from_slice(&nonce), ct.as_ref()).ok()?;
        String::from_utf8(pt).ok()
    }
}

impl SecretStore for EncryptedDb {
    fn get(&self, name: &str) -> Option<String> {
        // env wins
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
        self.stored(name)
    }

    fn set(&self, name: &str, val: &str) -> Result<()> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, val.as_bytes())
            .map_err(|e| anyhow::anyhow!("encrypt failed: {e}"))?;
        self.conn.execute(
            "INSERT INTO secrets (name, ciphertext, nonce, created_ts) VALUES (?1,?2,?3,?4)
             ON CONFLICT(name) DO UPDATE SET ciphertext=?2, nonce=?3, created_ts=?4",
            params![name, ct, nonce.as_slice(), 0i64],
        )?;
        Ok(())
    }

    fn clear(&self, name: &str) -> Result<()> {
        self.conn.execute("DELETE FROM secrets WHERE name = ?1", params![name])?;
        Ok(())
    }

    fn status(&self, name: &str) -> SecretStatus {
        if std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false) {
            return SecretStatus::Set { from_env: true };
        }
        if self.stored(name).is_some() {
            SecretStatus::Set { from_env: false }
        } else {
            SecretStatus::NotSet
        }
    }
}

/// Load the 32-byte app key, or generate + persist it at 0600 on first use.
fn load_or_create_key(path: &Path) -> Result<[u8; 32]> {
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            return Ok(k);
        }
    }
    use rand::RngCore;
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(path, k).with_context(|| format!("writing key file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (tempfile::TempDir, String, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db").to_str().unwrap().to_string();
        let key = dir.path().join("secret.key");
        (dir, db, key)
    }

    #[test]
    fn round_trip_encrypts_and_decrypts() {
        let (_d, db, key) = tmp();
        let s = EncryptedDb::open(&db, &key).unwrap();
        s.set("MY_KEY", "sk-abc123").unwrap();
        assert_eq!(s.get("MY_KEY").as_deref(), Some("sk-abc123"));
        assert!(matches!(s.status("MY_KEY"), SecretStatus::Set { from_env: false }));
        s.clear("MY_KEY").unwrap();
        assert_eq!(s.get("MY_KEY"), None);
        assert!(matches!(s.status("MY_KEY"), SecretStatus::NotSet));
    }

    #[test]
    fn key_file_is_0600_and_ciphertext_is_not_plaintext() {
        let (_d, db, key) = tmp();
        let s = EncryptedDb::open(&db, &key).unwrap();
        s.set("K", "secret-value").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&key).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let raw: Vec<u8> = s
            .conn
            .query_row("SELECT ciphertext FROM secrets WHERE name='K'", [], |r| r.get(0))
            .unwrap();
        assert!(!raw.windows(6).any(|w| w == b"secret"));
    }
}
```

Note on env-precedence tests: env vars are process-global and unsafe to mutate in parallel tests — the `from_env` path is covered by manual smoke + the `status`/`get` code reading `std::env::var`. Do NOT add a test that sets a process env var.

- [ ] **Step 3: Wire module**

`crates/zoid-core/src/lib.rs`: add `pub mod secret;`.

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p zoid-core secret:: && cargo clippy -p zoid-core`
Expected: PASS; no warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/zoid-core/Cargo.toml crates/zoid-core/src/secret.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): encrypted-DB SecretStore (XChaCha20-Poly1305, 0600 key file)

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

# Phase 3 — Wire config + secrets + economy into the binary

### Task 7: Config + secret paths and startup load

**Files:**
- Modify: `crates/zoid/src/main.rs` (add `resolve_config_dir`, `resolve_secret_key_path`, load config layers, apply to `shell`/`model`/`reduced_motion`)

**Interfaces:**
- Consumes: `zoid_core::config::{Config, Provenance, Source, PartialConfig, parse_toml, merge}`, `zoid_core::secret::EncryptedDb`, `zoid_provider::context_ceiling`.
- Produces: an in-scope `config: Config`, `prov: Provenance`, and a `secrets: EncryptedDb`, used later by the screen and provider.

- [ ] **Step 1: Write failing tests** — add to `main.rs` tests:

```rust
#[test]
fn resolve_config_dir_prefers_xdg_then_home() {
    let x = resolve_config_dir(|k| match k {
        "XDG_CONFIG_HOME" => Some("/x/cfg".into()),
        _ => None,
    });
    assert_eq!(x, std::path::PathBuf::from("/x/cfg/zoid"));
    let h = resolve_config_dir(|k| match k {
        "HOME" => Some("/home/u".into()),
        _ => None,
    });
    assert_eq!(h, std::path::PathBuf::from("/home/u/.config/zoid"));
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p zoid resolve_config_dir_prefers_xdg_then_home`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement path resolvers + loader** — add to `main.rs`:

```rust
fn resolve_config_dir(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".config"));
    base.join("zoid")
}

fn resolve_secret_key_path(env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".local/share"));
    base.join("zoid").join("secret.key")
}

/// Load config from files + env, in precedence order. Missing files = empty
/// layers; a malformed file is skipped with a stderr note (non-fatal).
fn load_config() -> (zoid_core::config::Config, zoid_core::config::Provenance) {
    use zoid_core::config::{merge, parse_toml, PartialConfig, Source};
    let env = |k: &str| std::env::var(k).ok();
    let cfg_dir = resolve_config_dir(env);
    let read = |p: PathBuf| -> Option<PartialConfig> {
        let text = std::fs::read_to_string(&p).ok()?;
        match parse_toml(&text) {
            Ok(pc) => Some(pc),
            Err(e) => { eprintln!("zoid: ignoring {}: {e}", p.display()); None }
        }
    };
    let mut layers: Vec<(Source, PartialConfig)> = Vec::new();
    if let Some(p) = read(cfg_dir.join("config.toml")) { layers.push((Source::UserGlobal, p)); }
    if let Some(p) = read(PathBuf::from("./.zoid/config.toml")) { layers.push((Source::Project, p)); }
    if let Some(p) = read(PathBuf::from("./.zoid/config.local.toml")) { layers.push((Source::Local, p)); }
    // env layer
    let mut envp = PartialConfig::default();
    if let Ok(m) = std::env::var("ZOID_MODEL") { if !m.is_empty() { envp.model = Some(m); } }
    if let Ok(v) = std::env::var("ZOID_CONTEXT_CEILING") {
        if let Ok(n) = v.trim().parse::<u64>() { if n > 0 { envp.economy.context_ceiling = Some(n); } }
    }
    if std::env::var("ZOID_REDUCED_MOTION").map(|v| !v.is_empty()).unwrap_or(false) {
        envp.reduced_motion = Some(true);
    }
    layers.push((Source::Env, envp));
    merge(&layers)
}
```

- [ ] **Step 4: Apply config at startup** — in the startup sequence (near `main.rs:306-312`), replace the ad-hoc `model`/`reduced_motion` reads:

```rust
    let (config, prov) = load_config();
    // model: config wins over provider default; empty → provider default_model()
    let model = if config.model.is_empty() { default_model().to_string() } else { config.model.clone() };
    // ... existing shell setup ...
    shell.reduced_motion = config.reduced_motion;
    shell.ctx_ceiling = config.economy.context_ceiling
        .unwrap_or_else(|| zoid_provider::context_ceiling(&model));
```

Keep `config` and `prov` in scope (the config screen reads them). Open the secret store once:

```rust
    let secret_key = resolve_secret_key_path(|k| std::env::var(k).ok());
    let secrets = zoid_core::secret::EncryptedDb::open(&db_path.to_string_lossy(), &secret_key)
        .map(std::sync::Arc::new)
        .ok(); // None → secrets unavailable this run (non-fatal)
```

- [ ] **Step 5: Run — verify pass + full build**

Run: `cargo test -p zoid resolve_config_dir_prefers_xdg_then_home && cargo build`
Expected: PASS; builds.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): load layered config + open secret store at startup

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 8: Economy policy from config + provider credentials from secrets

**Files:**
- Modify: `crates/zoid/src/main.rs` (the `ContextPolicy::default()` site ~`422`, and provider construction)
- Modify: `crates/zoid-provider/src/lib.rs` (`default_provider` gains a key-lookup closure) — **or** construct the provider in the bin from `config.provider` + `secrets`.

**Interfaces:**
- Consumes: `config.economy`, `secrets: Option<Arc<EncryptedDb>>`, `zoid_provider::{ollama, anthropic}`.
- Produces: a `ContextPolicy` built from config; provider built from `config.provider` + resolved key.

- [ ] **Step 1: Write failing test (policy mapping)** — add to `main.rs` tests a pure helper + test:

```rust
#[test]
fn policy_from_config_maps_pct_to_absolute() {
    let econ = zoid_core::config::EconomyConfig {
        context_ceiling: Some(200_000), auto_evict_cold: false,
        compact_threshold_pct: 80, token_ceiling: Some(50_000),
    };
    let p = policy_from_config(&econ, 200_000);
    assert!(!p.auto_evict_cold);
    assert_eq!(p.token_ceiling, Some(50_000));
    assert_eq!(p.compact_threshold, Some(160_000)); // 80% of 200k
    // 0% disables compaction
    let econ0 = zoid_core::config::EconomyConfig { compact_threshold_pct: 0, ..econ };
    assert_eq!(policy_from_config(&econ0, 200_000).compact_threshold, None);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p zoid policy_from_config_maps_pct_to_absolute`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement `policy_from_config` + use it** — add to `main.rs`:

```rust
fn policy_from_config(
    econ: &zoid_core::config::EconomyConfig,
    ceiling: u64,
) -> zoid_core::assembler::ContextPolicy {
    let compact_threshold = if econ.compact_threshold_pct == 0 {
        None
    } else {
        Some(ceiling.saturating_mul(econ.compact_threshold_pct as u64) / 100)
    };
    zoid_core::assembler::ContextPolicy {
        token_ceiling: econ.token_ceiling,
        auto_evict_cold: econ.auto_evict_cold,
        compact_threshold,
    }
}
```

Replace the hardcoded `let policy = zoid_core::assembler::ContextPolicy::default();` (main.rs ~422) with:

```rust
            let policy = policy_from_config(&config.economy, app.shell.ctx_ceiling);
```

(`config` is in scope from Task 7; `app.shell.ctx_ceiling` holds the resolved ceiling.)

- [ ] **Step 4: Provider credentials from the secret store** — where `default_provider()` is called, construct explicitly from `config.provider` + `secrets`:

```rust
    let key_for = |name: &str| -> Option<String> {
        secrets.as_ref().and_then(|s| {
            use zoid_core::secret::SecretStore;
            s.get(name)
        })
    };
    let provider: std::sync::Arc<dyn zoid_provider::Provider> = match config.provider.as_str() {
        "anthropic" => match key_for("ANTHROPIC_API_KEY") {
            Some(k) => std::sync::Arc::new(zoid_provider::anthropic::AnthropicProvider::new(k)),
            None => zoid_provider::default_provider(), // offline echo fallback
        },
        _ => match key_for("OLLAMA_API_KEY") {
            Some(k) => std::sync::Arc::new(zoid_provider::ollama::OllamaProvider::new(k)),
            None => zoid_provider::default_provider(),
        },
    };
```

(Keep `default_provider()` as the offline fallback. `base_url` override is out of scope for this task — the provider structs don't yet expose a setter; note it as a follow-up.)

- [ ] **Step 5: Run tests + build + clippy**

Run: `cargo test -p zoid && cargo clippy -p zoid`
Expected: PASS; no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): economy policy from config; provider key from secret store

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

# Phase 4 — The Configuration Screen

### Task 9: Open path — Overlay, Command, palette entry

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (`Overlay::Config`)
- Modify: `crates/zoid-tui/src/command.rs` (`Command::OpenConfig`, parse `config`)
- Modify: `crates/zoid-tui/src/palette.rs` (settings-group row)

**Interfaces:**
- Produces: `Overlay::Config` variant; `Command::OpenConfig`; palette row with `command: Some(Command::OpenConfig)`.

- [ ] **Step 1: Write failing tests**

In `command.rs` tests:

```rust
#[test]
fn parses_config_command() {
    assert_eq!(parse_command(":config"), Command::OpenConfig);
}
```

In `palette.rs` tests:

```rust
#[test]
fn settings_group_has_open_settings() {
    let items = all_items(Mode::Chat);
    assert!(selectable_matches(&items, "settings")
        .iter()
        .any(|&i| items[i].command == Some(Command::OpenConfig)));
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p zoid-tui parses_config_command settings_group_has_open_settings`
Expected: FAIL (variant/row missing).

- [ ] **Step 3: Implement**

`state.rs` — add `Config` to `enum Overlay`:

```rust
pub enum Overlay {
    None,
    Palette,
    CommandLine,
    Objects,
    Verbs,
    Sessions,
    Config,
}
```

`command.rs` — add variant and parse arm:

```rust
    // in enum Command:
    OpenConfig,
```
```rust
        "config" => Command::OpenConfig,
```

`palette.rs` — add a row to the settings group (before "Quit zoid"):

```rust
        PaletteItem {
            group: "settings".to_string(),
            icon: glyph::SETTINGS,
            label: "Open settings",
            hint: "provider · model · economy · secrets",
            keybind: ":config",
            command: Some(Command::OpenConfig),
        },
```

- [ ] **Step 4: Run tests — verify pass** (snapshots for the palette will change)

Run: `cargo insta test --accept -p zoid-tui && cargo test -p zoid-tui`
Expected: PASS; the palette snapshot(s) update to include the new row.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/command.rs crates/zoid-tui/src/palette.rs crates/zoid-tui/tests/snapshots
git commit -m "feat(tui): config-screen open path (Overlay::Config, :config, palette row)

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 10: Config view-model

**Files:**
- Create: `crates/zoid-tui/src/config_view.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (`pub mod config_view;` + re-exports as needed)

**Interfaces:**
- Consumes: `zoid_core::config::{Config, Provenance, Source}`, `zoid_core::secret::SecretStatus`.
- Produces:
  - `pub enum FieldKind { Text, Uint, Bool, Cycle(&'static [&'static str]), Secret }`
  - `pub struct FieldRow { pub label: &'static str, pub value: String, pub kind: FieldKind, pub source: Source, pub env_shadowed: bool }`
  - `pub struct Section { pub title: String, pub rows: Vec<FieldRow> }`
  - `pub fn build_sections(cfg: &Config, prov: &Provenance, key_status: &[(&'static str, SecretStatus)]) -> Vec<Section>`

- [ ] **Step 1: Write the failing test** — create `config_view.rs`:

```rust
//! Pure view-model for the configuration screen: turns a resolved Config +
//! Provenance + secret statuses into rendered sections. No IO, no rendering.

use zoid_core::config::{Config, Provenance, Source};
use zoid_core::secret::SecretStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind { Text, Uint, Bool, Cycle(&'static [&'static str]), Secret }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRow {
    pub label: &'static str,
    pub value: String,
    pub kind: FieldKind,
    pub source: Source,
    pub env_shadowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section { pub title: String, pub rows: Vec<FieldRow> }

pub fn build_sections(
    cfg: &Config,
    prov: &Provenance,
    key_status: &[(&'static str, SecretStatus)],
) -> Vec<Section> {
    let onoff = |b: bool| if b { "on".to_string() } else { "off".to_string() };
    let opt = |o: &Option<u64>| o.map(|n| n.to_string()).unwrap_or_else(|| "(none)".into());

    let provider_model = Section {
        title: "Provider & Model".into(),
        rows: vec![
            FieldRow { label: "provider", value: cfg.provider.clone(),
                kind: FieldKind::Cycle(zoid_provider::model::KNOWN_PROVIDERS),
                source: prov.provider, env_shadowed: false },
            // model is free-text (Ollama runs arbitrary tags) with a
            // registry-backed cycle layered on in routing (Task 12): a cycle key
            // steps through models_for(cfg.provider); typing overrides freely.
            FieldRow { label: "model", value: cfg.model.clone(),
                kind: FieldKind::Text, source: prov.model,
                env_shadowed: prov.model == Source::Env },
            FieldRow { label: "base_url", value: cfg.base_url.clone().unwrap_or_default(),
                kind: FieldKind::Text, source: prov.base_url, env_shadowed: false },
        ],
    };
    let economy = Section {
        title: "Economy ⑤".into(),
        rows: vec![
            FieldRow { label: "context ceiling", value: opt(&cfg.economy.context_ceiling),
                kind: FieldKind::Uint, source: prov.context_ceiling,
                env_shadowed: prov.context_ceiling == Source::Env },
            FieldRow { label: "auto-evict cold", value: onoff(cfg.economy.auto_evict_cold),
                kind: FieldKind::Bool, source: prov.auto_evict_cold, env_shadowed: false },
            FieldRow { label: "compact at %", value: cfg.economy.compact_threshold_pct.to_string(),
                kind: FieldKind::Uint, source: prov.compact_threshold_pct, env_shadowed: false },
            FieldRow { label: "token ceiling", value: opt(&cfg.economy.token_ceiling),
                kind: FieldKind::Uint, source: prov.token_ceiling, env_shadowed: false },
        ],
    };
    let interface = Section {
        title: "Interface".into(),
        rows: vec![
            FieldRow { label: "reduced motion", value: onoff(cfg.reduced_motion),
                kind: FieldKind::Bool, source: prov.reduced_motion,
                env_shadowed: prov.reduced_motion == Source::Env },
        ],
    };
    let secrets = Section {
        title: "Secrets".into(),
        rows: key_status.iter().map(|(name, st)| {
            let (value, shadowed) = match st {
                SecretStatus::Set { from_env: true } => ("set".to_string(), true),
                SecretStatus::Set { from_env: false } => ("set".to_string(), false),
                SecretStatus::NotSet => ("not set".to_string(), false),
            };
            FieldRow { label: name, value, kind: FieldKind::Secret,
                source: if shadowed { Source::Env } else { Source::Default }, env_shadowed: shadowed }
        }).collect(),
    };
    vec![provider_model, economy, interface, secrets]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_four_sections_with_env_shadow() {
        let cfg = Config::default();
        // Inline provenance: all Default except `model` shadowed by env.
        let prov = Provenance {
            provider: Source::Default, base_url: Source::Default, model: Source::Env,
            context_ceiling: Source::Default, auto_evict_cold: Source::Default,
            compact_threshold_pct: Source::Default, token_ceiling: Source::Default,
            reduced_motion: Source::Default,
        };
        let ks = [("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
                  ("ANTHROPIC_API_KEY", SecretStatus::NotSet)];
        let secsecs = build_sections(&cfg, &prov, &ks);
        assert_eq!(secsecs.len(), 4);
        let model_row = &secsecs[0].rows[1];
        assert_eq!(model_row.label, "model");
        assert!(model_row.env_shadowed);
        let sec = secsecs.iter().find(|s| s.title == "Secrets").unwrap();
        assert!(sec.rows[0].env_shadowed);            // OLLAMA set from env
        assert_eq!(sec.rows[1].value, "not set");
    }
}
```

(The test constructs `Provenance` inline — do not add production-only test helpers to `zoid-core`.)

- [ ] **Step 2: Wire module + zoid-provider dep**

`crates/zoid-tui/src/lib.rs`: `pub mod config_view;`
Confirm `zoid-tui/Cargo.toml` depends on `zoid-provider` and `zoid-core` (add `zoid-provider = { path = "../zoid-provider" }` if missing).

- [ ] **Step 3: Run tests — verify pass**

Run: `cargo test -p zoid-tui config_view::`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/config_view.rs crates/zoid-tui/src/lib.rs crates/zoid-tui/Cargo.toml
git commit -m "feat(tui): config screen view-model (sections/fields/provenance)

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 11: Render the two-pane config overlay

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (add `render_config`, dispatch when `Overlay::Config`)
- Modify: `crates/zoid-tui/src/state.rs` (add `pub config_section: usize`, `pub config_field: usize`, `pub config_edit: Option<String>` to `ShellState`; init in `new()`)
- Test: `crates/zoid-tui/tests/shell_snapshot.rs` (new snapshot)

**Interfaces:**
- Consumes: `config_view::{Section, FieldRow, FieldKind}`, `Source`, tokens.
- Produces: `fn render_config(frame, state, sections: &[Section], area)`; a snapshot-covered full-screen render.

- [ ] **Step 1: Add state fields** — in `ShellState` add (with the others) and init to `0`/`None` in `new()`:

```rust
    pub config_section: usize,
    pub config_field: usize,
    pub config_edit: Option<String>, // Some(buffer) while editing the current field
```

- [ ] **Step 2: Write the failing snapshot test** — in `shell_snapshot.rs`:

```rust
#[test]
fn config_overlay_frame() {
    use zoid_tui::config_view::build_sections;
    use zoid_core::config::{Config, Provenance, Source};
    use zoid_core::secret::SecretStatus;
    let mut s = ShellState::new();
    s.overlay = Overlay::Config;
    let cfg = Config::default();
    let prov = Provenance { // all Default except model shadowed
        provider: Source::Default, base_url: Source::Default, model: Source::Env,
        context_ceiling: Source::Default, auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default, token_ceiling: Source::Default,
        reduced_motion: Source::Default,
    };
    let ks = [("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
              ("ANTHROPIC_API_KEY", SecretStatus::NotSet)];
    let sections = build_sections(&cfg, &prov, &ks);
    insta::assert_snapshot!(draw_config(&s, &sections, 100, 24));
}
```

Add a `draw_config` test helper mirroring the existing `draw` helper in that file (build a `TestBackend`, call `render_config`). Follow the existing `draw` helper's exact construction.

- [ ] **Step 3: Run — verify fail**

Run: `cargo test -p zoid-tui config_overlay_frame`
Expected: FAIL — `render_config`/`draw_config` not defined.

- [ ] **Step 4: Implement `render_config`** in `render.rs`. Two-pane: left nav (section titles; active = `CHAT_ACCENT`, others `DIM`), right = active section's rows with right-aligned provenance tag and `[env] ⚠` when shadowed. All glyphs/colors via `tokens`:

```rust
pub fn render_config(frame: &mut Frame, state: &ShellState, sections: &[crate::config_view::Section], area: Rect) {
    use crate::tokens::{color, glyph};
    use ratatui::widgets::{Block, Borders, Paragraph};
    use ratatui::text::{Line, Span};
    use ratatui::style::Style;
    let block = Block::default().borders(Borders::ALL).title(" zoid · settings ")
        .border_style(Style::new().fg(color::CHAT_ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Left nav (width 18), right detail.
    let nav_w = 18u16.min(inner.width.saturating_sub(1));
    let cols = ratatui::layout::Layout::horizontal([
        ratatui::layout::Constraint::Length(nav_w),
        ratatui::layout::Constraint::Min(1),
    ]).split(inner);

    let nav: Vec<Line> = sections.iter().enumerate().map(|(i, s)| {
        let active = i == state.config_section;
        let marker = if active { glyph::COLLAPSED } else { ' ' };
        Line::from(Span::styled(
            format!("{marker} {}", s.title),
            Style::new().fg(if active { color::CHAT_ACCENT } else { color::DIM }),
        ))
    }).collect();
    frame.render_widget(Paragraph::new(nav), cols[0]);

    let sec = &sections[state.config_section.min(sections.len().saturating_sub(1))];
    let rows: Vec<Line> = sec.rows.iter().enumerate().map(|(i, r)| {
        let cur = i == state.config_field;
        let val = if cur { if let Some(buf) = &state.config_edit { format!("{buf}{}", glyph::CARET) } else { r.value.clone() } } else { r.value.clone() };
        let (tag_txt, tag_col) = match r.source {
            zoid_core::config::Source::Default => ("[default]", color::DIM),
            zoid_core::config::Source::UserGlobal => ("[user]", color::CHAT_ACCENT),
            zoid_core::config::Source::Project => ("[repo]", color::BRANCH),
            zoid_core::config::Source::Local => ("[local]", color::BRANCH),
            zoid_core::config::Source::Env => ("[env]", color::WARN),
        };
        let mut spans = vec![
            Span::styled(format!(" {:<16} ", r.label), Style::new().fg(if cur { color::CHAT_ACCENT } else { color::TXT })),
            Span::styled(format!("{:<28}", val), Style::new().fg(color::TXT)),
            Span::styled(format!("{tag_txt} "), Style::new().fg(tag_col)),
        ];
        if r.env_shadowed { spans.push(Span::styled(format!("{}", glyph::WARNING), Style::new().fg(color::WARN))); }
        Line::from(spans)
    }).collect();
    frame.render_widget(Paragraph::new(rows), cols[1]);
}
```

Ensure no literal hex/glyph escapes §16 — every glyph is from `glyph::*`, every color from `color::*`.

- [ ] **Step 5: Dispatch from the main draw** — where overlays are dispatched (search `Overlay::Palette =>` in `render.rs`), add an `Overlay::Config` arm that builds sections and calls `render_config`. Because `render.rs` needs the resolved `Config`/`Provenance`/secret statuses, thread them via `ShellState` (add `pub config_sections: Vec<config_view::Section>` computed in the bin each frame) OR pass through the existing render entry signature. Prefer: the bin computes `sections` once per frame and stores on `ShellState`; `render_config` reads `state.config_sections`. Update Task 10 wiring accordingly.

- [ ] **Step 6: Run + accept snapshot**

Run: `cargo insta test --accept -p zoid-tui && cargo test -p zoid-tui`
Expected: new `config_overlay_frame` snapshot stored; all pass. Inspect the `.snap` to confirm the two panes, provenance tags, and `[env] ⚠` render.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/src/state.rs crates/zoid-tui/tests
git commit -m "feat(tui): render two-pane config overlay with provenance tags

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 12: Routing, editing, save-back & live-apply

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (handle keys while `Overlay::Config`)
- Modify: `crates/zoid/src/main.rs` (apply `Command`/edits: write config via `set_in_toml`, store/clear secrets, reload config, live-apply)

**Interfaces:**
- Consumes: `Overlay::Config`, `ShellState.config_*`, `config_view` field kinds, `zoid_core::config::set_in_toml`, `SecretStore`.
- Produces: an `Action`/`Command` path the bin executes for: move field/section, begin/commit/cancel edit, toggle bool, cycle, save→user, save→repo (`r`), store/clear secret, `esc` close.

- [ ] **Step 1: Write failing routing tests** — in `route.rs` tests, assert key→action mapping for the config overlay (mirror the existing overlay routing tests). Example:

```rust
#[test]
fn config_overlay_nav_and_escape() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Config;
    // ↓ moves field, → moves section, esc closes
    assert!(matches!(route_key(&s, key(KeyCode::Down)), Action::ConfigMoveField(1)));
    assert!(matches!(route_key(&s, key(KeyCode::Right)), Action::ConfigMoveSection(1)));
    assert!(matches!(route_key(&s, key(KeyCode::Esc)), Action::CloseOverlay));
}
```

Use the file's existing `route_key`/`Action`/`key(..)` names — inspect the current object/verb overlay routing tests and follow them exactly (names may differ; match what's there).

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p zoid-tui config_overlay_nav_and_escape`
Expected: FAIL — new `Action` variants / routing missing.

- [ ] **Step 3: Implement routing** — add `Action` variants (`ConfigMoveField(i32)`, `ConfigMoveSection(i32)`, `ConfigBeginEdit`, `ConfigEditChar(char)`, `ConfigEditBackspace`, `ConfigCommitEdit`, `ConfigCancelEdit`, `ConfigToggle`, `ConfigCycle(i32)`, `ConfigSaveToRepo`, `ConfigClearSecret`) and route keys while `state.overlay == Overlay::Config`:
  - `Up/Down` → `ConfigMoveField(-1/+1)`; `Left/Right` → `ConfigMoveSection(-1/+1)`
  - `Enter` → if not editing and field is `Bool` → `ConfigToggle`; if `Secret` → `ConfigBeginEdit`; else `ConfigBeginEdit`; if editing → `ConfigCommitEdit`
  - `Space` on `Bool`/`Cycle` → `ConfigToggle`/`ConfigCycle(1)`; `Space`/`Tab` on the **model** field cycles `zoid_provider::model::models_for(cfg.provider)` (free-text typing via `Enter` still overrides — the cycle is a convenience, not a closed list)
  - `char`/`Backspace` while editing → `ConfigEditChar`/`ConfigEditBackspace`
  - `r` (not editing) → `ConfigSaveToRepo`; `x` on `Secret` → `ConfigClearSecret`
  - `Esc` → if editing → `ConfigCancelEdit` else `CloseOverlay`

- [ ] **Step 4: Execute actions in the bin** — in `main.rs`'s action handler:
  - Movement/edit-buffer actions mutate `app.shell.config_*` (pure state).
  - `ConfigToggle`/`ConfigCycle`/`ConfigCommitEdit` → compute the new value for the current field, then:
    - write it to the **active target** file (user-global by default; repo for `ConfigSaveToRepo`) via `set_in_toml` (read file → set → write; create dirs);
    - `reload` config: `let (c, p) = load_config(); app.config = c; app.prov = p;` and recompute `app.shell.config_sections`;
    - **live-apply**: `reduced_motion` → `app.shell.reduced_motion`; economy → next-frame `policy_from_config`; model/provider → next turn; `ctx_ceiling` recompute.
  - `ConfigCommitEdit` on a `Secret` field → `secrets.set(name, buffer)`; `ConfigClearSecret` → `secrets.clear(name)`; never write secrets to TOML.
  - `CloseOverlay` → `app.shell.overlay = Overlay::None`.

- [ ] **Step 5: Run tests + build + clippy**

Run: `cargo test -p zoid-tui && cargo test -p zoid && cargo clippy --workspace`
Expected: PASS; no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/route.rs crates/zoid/src/main.rs
git commit -m "feat(config): routing, inline edit, save-to-user/repo, secret store/clear

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task 13: `.gitignore` the local secret/config + docs

**Files:**
- Modify: `.gitignore` (ensure `./.zoid/config.local.toml` is ignored)
- Modify: `docs/superpowers/specs/2026-06-30-zoid-core-architecture.md` (one-line note that §7.1 config + encrypted-DB secrets are now implemented; the model registry is basic/caps-only)

- [ ] **Step 1: Ensure gitignore**

Add to `.gitignore` (if not already covered):

```
.zoid/config.local.toml
```

- [ ] **Step 2: Doc note** — in the §7.1 area, append one line:

```markdown
> **Status (2026-07-01):** §7.1 TOML config + precedence and an encrypted-DB
> secret store are implemented (see `2026-07-01-config-screen-design.md`). API
> keys may now also live encrypted in `zoid.db` (never in files).
```

- [ ] **Step 3: Commit**

```bash
git add .gitignore docs/superpowers/specs/2026-06-30-zoid-core-architecture.md
git commit -m "chore: gitignore local config; note config/secrets implemented

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

## Final Verification

- [ ] `cargo test --workspace` — all suites green.
- [ ] `cargo clippy --workspace` — zero warnings.
- [ ] `cargo fmt --check` — clean (run `cargo fmt` if not).
- [ ] `cargo insta test -p zoid-tui` — no pending snapshot diffs.
- [ ] Manual smoke: launch zoid, `:config`, cycle provider, edit context ceiling, toggle reduced motion, store a dummy key, `esc`; confirm `~/.config/zoid/config.toml` written and the key absent from it.

## Notes for the executor

- **`base_url` override** is surfaced in the screen but not yet applied to provider construction (the provider structs hard-code their base URL). Wiring it is a small follow-up — do NOT silently skip it; render it read-informational and note the gap, or extend `OllamaProvider::new`/`AnthropicProvider::new` with a base-url setter as a bonus task if time permits.
- **Threading `config`/`prov`/`secrets` into the render loop:** the bin owns them; it recomputes `app.shell.config_sections` whenever config changes (open + after each edit), so `render.rs` stays a pure reader of `ShellState`.
- **Env-mutation in tests is forbidden** (process-global, unsafe under parallel `cargo test`). Cover env precedence by construction/manual smoke, per the existing provider tests' convention.
