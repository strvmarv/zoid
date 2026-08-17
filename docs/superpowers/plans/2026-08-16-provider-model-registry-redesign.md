# Provider/Model Registry Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace zoid's hand-synced Rust-const provider/model registry with a runtime-loaded TOML registry (single source of truth), a `zoid refresh-models` tool, Gemini as a first-class provider, and local-model unification.

**Architecture:** A new dependency-free `Registry` data struct lives in `zoid-model` (pure types, no I/O). A new `zoid-registry` crate parses two TOML files (`models.toml` shipped + `models.user.toml` user), merges them, and hosts the refresh tool's fetch/reconcile logic. `zoid`/`zoid-core` load the merged `Registry` once at startup and thread it (`Arc<Registry>`) into provider selection and model lookup. Caps and wire-shape become per-`(provider, model)` rows, fixing the `claude-sonnet-4-6` shadowing bug.

**Tech Stack:** Rust 2021, `toml` 0.8 (already a workspace dep), `serde` (workspace), `reqwest` (workspace), `anyhow` (workspace). No new external dependencies beyond what the workspace already declares.

## Global Constraints

- `zoid-model` MUST remain dependency-free (empty `[dependencies]`). It cannot use `serde` or `toml`; all TOML parsing lives in `zoid-registry`.
- Model ids are compared **case-insensitively**; provider ids are compared **exactly** after `canonical_id` alias resolution.
- `canonical_id` preserves legacy aliases: `"ollama"` → `"ollama-cloud"`, `"anthropic"` → `"anthropic-api"`; all other ids pass through.
- `ZOID_CONTEXT_CEILING` env override still wins over the registry (unchanged precedence).
- The shipped `models.toml` is a semantic transcription of today's consts, EXCEPT it resolves the `claude-sonnet-4-6` duplicate into two `(provider, model)` rows: `anthropic-api` → 1M, `opencode-zen` → 200K.
- `wire` rows only ever exist for `ollama-cloud`/`ollama-local` and `gemini-api` (the only wire-derived-caps providers).
- `ollama-local` is the only keyless provider (`key_url` and `key_env` both `None`); every other provider has both `Some`.
- Test command: `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` (fallback: `cargo test --workspace --features zoid/local-embed --no-fail-fast`).
- Commit after every task with a descriptive message.

---

## File Structure

**New files:**
- `crates/zoid-registry/Cargo.toml` — new crate manifest (depends on `zoid-model`, `toml`, `serde`, `anyhow`, `reqwest`).
- `crates/zoid-registry/src/lib.rs` — crate root: re-exports, `load`, `merge`.
- `crates/zoid-registry/src/raw.rs` — serde-deserializable mirror types (`RawProvider`, `RawModel`, `RawTransport`) + `From` conversions to `zoid-model` types.
- `crates/zoid-registry/src/parse.rs` — TOML string → `Registry` (shipped and user variants).
- `crates/zoid-registry/src/merge.rs` — merge user registry over shipped registry.
- `crates/zoid-registry/src/refresh.rs` — fetch + reconcile logic (the refresh tool's library).
- `crates/zoid-registry/src/fetch.rs` — per-provider live-list fetchers (Ollama, Anthropic, OpenAI-compat, Gemini).
- `crates/zoid-model/models.toml` — the shipped registry (transcription of today's consts).

**Modified files:**
- `Cargo.toml` — add `zoid-registry` to workspace members.
- `crates/zoid-model/src/lib.rs` — owned-type migration, add `WireShape`/`Source`/`ModelEntry`/`Registry`, delete consts.
- `crates/zoid-model/src/local_seed.rs` — DELETE (folded into TOML).
- `crates/zoid-provider/src/lib.rs` — `context_ceiling`/`has_prompt_cache`/`default_model`/`default_provider` take a `&Registry`; re-export `Registry`.
- `crates/zoid-provider/src/opencode_go.rs` — delete `GO_MODELS`; route via `Registry::wire_shape`.
- `crates/zoid-provider/src/opencode_zen.rs` — delete `ZEN_MODELS`; route via `Registry::wire_shape`.
- `crates/zoid/src/main.rs` — thread `Arc<Registry>`; rewrite `select_provider`/`provider_for_id`/`key_env_for`; add `refresh-models` subcommand; stale-selection recovery.
- `crates/zoid-core/src/store.rs` — delete `seed_local_models` + `local_models` table.
- `crates/zoid-core/src/session.rs` — delete `seed_local_models` handle method.
- `crates/zoid-core/src/skill.rs` — repurpose `refreshing-provider-models` skill body.
- `crates/zoid-tui/src/config_view.rs` — `provider_options`/`model_options` take `&Registry`.

---

## Phase 1 — Types + registry crate

### Task 1: Owned-type migration in `zoid-model`

**Files:**
- Modify: `crates/zoid-model/src/lib.rs` (types only; keep consts for now)

**Interfaces:**
- Produces: owned `ModelInfo`, `ProviderEntry`, `Transport`, `Status`, `ThinkingSupport`, `ThinkingWireShape`, plus new `WireShape`, `Source`, `ModelEntry`, `Registry` (all `Clone`, not `Copy`).

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` in `crates/zoid-model/src/lib.rs`:

```rust
#[test]
fn registry_types_are_owned_and_cloneable() {
    // ModelInfo is Clone (not Copy) and holds no &'static str.
    let info = ModelInfo {
        context_window: 200_000,
        max_output: 0,
        tools: true,
        prompt_cache: true,
        thinking: ThinkingSupport::None,
        thinking_wire: ThinkingWireShape::None,
    };
    let _clone = info.clone();

    // ProviderEntry owns Strings.
    let entry = ProviderEntry {
        id: "opencode-zen".to_string(),
        display: "opencode · zen".to_string(),
        family: "opencode-zen".to_string(),
        transport: Transport::Http {
            default_base_url: "https://opencode.ai/zen".to_string(),
        },
        status: Status::Available,
        key_url: Some("https://opencode.ai".to_string()),
        key_env: Some("OPENCODE_GO_API_KEY".to_string()),
        models: vec![],
    };
    assert_eq!(entry.id, "opencode-zen");
    assert_eq!(entry.transport, Transport::Http {
        default_base_url: "https://opencode.ai/zen".to_string()
    });
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-model registry_types_are_owned_and_cloneable`
Expected: FAIL — `ModelInfo`/`ProviderEntry`/`Transport` currently use `&'static str` and `Copy`; the `String`/`Vec` fields and `Clone`-only derive don't compile.

- [ ] **Step 3: Rewrite the types**

Replace the type definitions at the top of `crates/zoid-model/src/lib.rs` (lines 14–89) with owned versions. Keep the existing consts (`PROVIDERS`, `ZEN_MODEL_IDS`, `MODEL_CAPS`, `DEFAULT_MODEL_INFO`) and lookup fns (`canonical_id`, `entry`, `models_for`, `default_base_url`, `selectable`, `model_info`) untouched for now — they still compile against the old `&'static str` shape only if you ALSO keep the old field types. To avoid a broken tree, do this migration in two sub-steps:

**3a.** Add the NEW owned types under new names, leaving the old ones intact:

```rust
/// Wire protocol a (provider, model) pair routes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireShape {
    OpenAIChat,
    AnthropicMessages,
    OpenAIResponses,
    GoogleGemini,
    Ollama,
}

/// Provenance of a model row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Static,
    Wire,
    User,
}

/// One (provider, model) row: caps + wire shape + provenance + optional
/// local-provisioning fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub display: Option<String>,
    pub wire_shape: WireShape,
    pub source: Source,
    pub default: bool,
    pub hidden: bool,
    pub info: ModelInfo,
    pub runtime: Option<String>,
    pub download_source: Option<String>,
    pub quant: Option<String>,
    pub modelfile: Option<String>,
    pub num_ctx: Option<u32>,
    pub vram_curve: Option<String>,
}

/// Owned provider entry (replaces the `&'static str` version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEntry {
    pub id: String,
    pub display: String,
    pub family: String,
    pub transport: Transport,
    pub status: Status,
    pub key_url: Option<String>,
    pub key_env: Option<String>,
    pub models: Vec<ModelEntry>,
}

/// Owned transport (replaces the `&'static str` version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    Http { default_base_url: String },
    Cli { default_command: String },
    Sdk,
}

/// The merged registry: providers + their models, with lookup methods.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub providers: Vec<ProviderEntry>,
}
```

Note: `ModelInfo`, `ThinkingSupport`, `ThinkingWireShape`, `Status` are unchanged (they already hold no `&'static str`; `ModelInfo` is `Copy` and stays `Copy` — it has no string fields).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-model registry_types_are_owned_and_cloneable`
Expected: PASS (the new owned types compile and are cloneable).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(zoid-model): add owned registry types (WireShape, Source, ModelEntry, Registry)"
```

---

### Task 2: `Registry` lookup methods

**Files:**
- Modify: `crates/zoid-model/src/lib.rs`

**Interfaces:**
- Consumes: `Registry`, `ProviderEntry`, `ModelEntry`, `WireShape`, `Source` from Task 1.
- Produces: `Registry::entry`, `Registry::models_for`, `Registry::default_base_url`, `Registry::model_info`, `Registry::selectable`, `Registry::default_model`, `Registry::wire_shape`, `Registry::canonical_id`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn registry_lookup_methods() {
    let reg = Registry {
        providers: vec![ProviderEntry {
            id: "anthropic-api".to_string(),
            display: "anthropic · api key".to_string(),
            family: "anthropic".to_string(),
            transport: Transport::Http { default_base_url: "https://api.anthropic.com".to_string() },
            status: Status::Available,
            key_url: Some("https://console.anthropic.com".to_string()),
            key_env: Some("ANTHROPIC_API_KEY".to_string()),
            models: vec![
                ModelEntry {
                    id: "claude-sonnet-4-6".to_string(),
                    display: None,
                    wire_shape: WireShape::AnthropicMessages,
                    source: Source::Static,
                    default: true,
                    hidden: false,
                    info: ModelInfo { context_window: 1_000_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::Budget, thinking_wire: ThinkingWireShape::Anthropic },
                    runtime: None, download_source: None, quant: None, modelfile: None, num_ctx: None, vram_curve: None,
                },
            ],
        }],
    };

    assert!(reg.entry("anthropic-api").is_some());
    assert!(reg.entry("ANTHROPIC-API").is_none()); // provider ids are exact
    assert_eq!(reg.models_for("anthropic-api").len(), 1);
    assert_eq!(reg.default_base_url("anthropic-api"), Some("https://api.anthropic.com"));
    assert_eq!(reg.default_model("anthropic-api"), Some("claude-sonnet-4-6"));
    assert_eq!(reg.wire_shape("anthropic-api", "claude-sonnet-4-6"), Some(WireShape::AnthropicMessages));
    // model id lookup is case-insensitive
    assert_eq!(reg.model_info("anthropic-api", "CLAUDE-SONNET-4-6").context_window, 1_000_000);
    // unknown model → conservative default
    assert_eq!(reg.model_info("anthropic-api", "nope").context_window, 32_000);
    assert_eq!(reg.selectable().count(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-model registry_lookup_methods`
Expected: FAIL — `Registry` has no methods.

- [ ] **Step 3: Implement the methods**

Add an `impl Registry` block:

```rust
impl Registry {
    /// Resolve a stored/legacy provider id to its canonical id.
    pub fn canonical_id(raw: &str) -> &str {
        match raw {
            "ollama" => "ollama-cloud",
            "anthropic" => "anthropic-api",
            other => other,
        }
    }

    /// The registry entry for a provider id (resolving legacy aliases).
    pub fn entry(&self, id: &str) -> Option<&ProviderEntry> {
        let id = Self::canonical_id(id);
        self.providers.iter().find(|e| e.id == id)
    }

    /// The model entries for a provider (empty for unknown ids).
    pub fn models_for(&self, provider: &str) -> &[ModelEntry] {
        self.entry(provider).map(|e| e.models.as_slice()).unwrap_or(&[])
    }

    /// The default base URL for an HTTP-transport provider, else `None`.
    pub fn default_base_url(&self, provider: &str) -> Option<&str> {
        match self.entry(provider).map(|e| &e.transport) {
            Some(Transport::Http { default_base_url }) => Some(default_base_url.as_str()),
            _ => None,
        }
    }

    /// Iterator over selectable (Available) entries.
    pub fn selectable(&self) -> impl Iterator<Item = &ProviderEntry> {
        self.providers.iter().filter(|e| e.status == Status::Available)
    }

    /// The default model id for a provider: the `default = true` row, else the
    /// first row. `None` when the provider has no models.
    pub fn default_model(&self, provider: &str) -> Option<&str> {
        let models = self.models_for(provider);
        models
            .iter()
            .find(|m| m.default)
            .or_else(|| models.first())
            .map(|m| m.id.as_str())
    }

    /// The wire shape for a (provider, model) pair. `None` when unknown.
    pub fn wire_shape(&self, provider: &str, model: &str) -> Option<WireShape> {
        let m = model.to_ascii_lowercase();
        self.models_for(provider)
            .iter()
            .find(|e| e.id.to_ascii_lowercase() == m)
            .map(|e| e.wire_shape)
    }

    /// Capabilities for a (provider, model) pair, looked up case-insensitively.
    /// Unknown models get a conservative default (32k, no prompt cache).
    pub fn model_info(&self, provider: &str, model: &str) -> ModelInfo {
        let m = model.to_ascii_lowercase();
        self.models_for(provider)
            .iter()
            .find(|e| e.id.to_ascii_lowercase() == m)
            .map(|e| e.info)
            .unwrap_or(DEFAULT_MODEL_INFO)
    }
}
```

Note: `DEFAULT_MODEL_INFO` (the existing const) is reused as the conservative fallback. It stays in place for now.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-model registry_lookup_methods`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "feat(zoid-model): add Registry lookup methods"
```

---

### Task 3: Create the `zoid-registry` crate skeleton

**Files:**
- Create: `crates/zoid-registry/Cargo.toml`
- Create: `crates/zoid-registry/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces: `zoid_registry` crate with `load`, `parse_shipped`, `parse_user`, `merge` (stubs returning `anyhow::Result<Registry>` for now).

- [ ] **Step 1: Add the crate to the workspace**

Edit `Cargo.toml`, add `"crates/zoid-registry"` to the `members` array (alphabetical, after `crates/zoid-provider`):

```toml
members = ["crates/zoid-core", "crates/zoid-model", "crates/zoid-plugin", "crates/zoid-provider", "crates/zoid-registry", "crates/zoid-tui", "crates/zoid-tools", "crates/zoid-syntax", "crates/zoid", "crates/zoid-testkit", "crates/zoid-companion", "crates/zoid-mcp", "crates/zoid-embed", "crates/zoid-web", "crates/zoid-plugin-import"]
```

- [ ] **Step 2: Write the crate manifest**

`crates/zoid-registry/Cargo.toml`:

```toml
[package]
name = "zoid-registry"
version.workspace = true
edition.workspace = true
repository.workspace = true
license.workspace = true

[dependencies]
zoid-model = { path = "../zoid-model" }
toml = { workspace = true }
serde = { workspace = true }
anyhow = { workspace = true }
reqwest = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 3: Write the crate root**

`crates/zoid-registry/src/lib.rs`:

```rust
//! Loads and merges the provider/model registry from TOML, and hosts the
//! refresh tool's fetch + reconcile logic. `zoid-model` stays dependency-free;
//! this crate owns all TOML/serde parsing and network I/O.

pub mod fetch;
pub mod merge;
pub mod parse;
pub mod raw;
pub mod refresh;

use anyhow::Result;
use std::path::Path;
use zoid_model::Registry;

/// Load the merged registry from the shipped and user TOML files.
/// A missing user file is treated as empty; a malformed user file falls back
/// to the shipped file alone (reported via the returned warning string).
pub fn load(shipped: &Path, user: &Path) -> Result<(Registry, Option<String>)> {
    let shipped_text = std::fs::read_to_string(shipped)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", shipped.display()))?;
    let shipped_reg = parse::parse_shipped(&shipped_text)?;

    let user_text = match std::fs::read_to_string(user) {
        Ok(t) => t,
        Err(_) => return Ok((shipped_reg, None)), // missing user file → shipped alone
    };
    match parse::parse_user(&user_text) {
        Ok(user_reg) => Ok((merge::merge(shipped_reg, user_reg), None)),
        Err(e) => Ok((
            shipped_reg,
            Some(format!(
                "ignoring malformed user registry {}: {e} (hidden/user rows dropped)",
                user.display()
            )),
        )),
    }
}
```

- [ ] **Step 4: Create empty module stubs so the crate compiles**

Create `crates/zoid-registry/src/raw.rs`, `parse.rs`, `merge.rs`, `fetch.rs`, `refresh.rs` with minimal content:

`raw.rs`:
```rust
//! serde-deserializable mirror types (filled in Task 4).
```

`parse.rs`:
```rust
//! TOML → Registry (filled in Task 4).
use anyhow::Result;
use zoid_model::Registry;

pub fn parse_shipped(_text: &str) -> Result<Registry> {
    Ok(Registry::default())
}

pub fn parse_user(_text: &str) -> Result<Registry> {
    Ok(Registry::default())
}
```

`merge.rs`:
```rust
//! Merge user registry over shipped (filled in Task 5).
use zoid_model::Registry;

pub fn merge(shipped: Registry, _user: Registry) -> Registry {
    shipped
}
```

`fetch.rs`:
```rust
//! Per-provider live-list fetchers (filled in Task 13).
```

`refresh.rs`:
```rust
//! Fetch + reconcile (filled in Task 14).
```

- [ ] **Step 5: Build to verify the crate compiles**

Run: `cargo build -p zoid-registry`
Expected: PASS (compiles with stubs).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/zoid-registry
git commit -m "feat(zoid-registry): add crate skeleton with load/parse/merge stubs"
```

---

### Task 4: TOML parsing (`raw.rs` + `parse.rs`)

**Files:**
- Modify: `crates/zoid-registry/src/raw.rs`
- Modify: `crates/zoid-registry/src/parse.rs`

**Interfaces:**
- Consumes: `zoid_model::{ModelInfo, ModelEntry, ProviderEntry, Registry, Source, Status, ThinkingSupport, ThinkingWireShape, Transport, WireShape}`.
- Produces: `parse::parse_shipped(&str) -> Result<Registry>`, `parse::parse_user(&str) -> Result<Registry>`.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-registry/src/parse.rs` test module (append to the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = r#"
[[provider]]
id = "anthropic-api"
display = "anthropic · api key"
family = "anthropic"
transport = { kind = "http", default_base_url = "https://api.anthropic.com" }
status = "available"
key_url = "https://console.anthropic.com"
key_env = "ANTHROPIC_API_KEY"

  [[provider.model]]
  id = "claude-sonnet-4-6"
  wire_shape = "anthropic-messages"
  source = "static"
  default = true
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "budget"
  thinking_wire = "anthropic"
"#;

    #[test]
    fn parse_shipped_reads_provider_and_model() {
        let reg = parse_shipped(SHIPPED).unwrap();
        assert_eq!(reg.providers.len(), 1);
        let p = &reg.providers[0];
        assert_eq!(p.id, "anthropic-api");
        assert_eq!(p.key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(p.models.len(), 1);
        let m = &p.models[0];
        assert_eq!(m.id, "claude-sonnet-4-6");
        assert_eq!(m.wire_shape, zoid_model::WireShape::AnthropicMessages);
        assert_eq!(m.source, zoid_model::Source::Static);
        assert!(m.default);
        assert_eq!(m.info.context_window, 1_000_000);
        assert_eq!(m.info.thinking, zoid_model::ThinkingSupport::Budget);
        assert_eq!(m.info.thinking_wire, zoid_model::ThinkingWireShape::Anthropic);
    }

    #[test]
    fn parse_rejects_unknown_enum_string() {
        let bad = SHIPPED.replace("thinking = \"budget\"", "thinking = \"bogus\"");
        assert!(parse_shipped(&bad).is_err());
    }

    #[test]
    fn parse_rejects_duplicate_model_id() {
        let dup = format!("{SHIPPED}\n  [[provider.model]]\n  id = \"claude-sonnet-4-6\"\n  wire_shape = \"anthropic-messages\"\n  source = \"static\"\n");
        assert!(parse_shipped(&dup).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-registry parse_shipped_reads_provider_and_model`
Expected: FAIL — `parse_shipped` returns `Registry::default()`.

- [ ] **Step 3: Implement `raw.rs`**

```rust
//! serde-deserializable mirror types for the TOML registry, plus `From`
//! conversions into the dependency-free `zoid_model` types.

use serde::Deserialize;
use zoid_model::{ModelEntry, ModelInfo, ProviderEntry, Registry, Source, Status, ThinkingSupport, ThinkingWireShape, Transport, WireShape};

#[derive(Debug, Deserialize)]
pub struct RawRegistry {
    #[serde(default)]
    pub provider: Vec<RawProvider>,
}

#[derive(Debug, Deserialize)]
pub struct RawProvider {
    pub id: String,
    pub display: String,
    pub family: String,
    pub transport: RawTransport,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub key_url: Option<String>,
    #[serde(default)]
    pub key_env: Option<String>,
    #[serde(default)]
    pub model: Vec<RawModel>,
}

fn default_status() -> String {
    "available".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RawTransport {
    Http { default_base_url: String },
    Cli { default_command: String },
    Sdk,
}

#[derive(Debug, Deserialize)]
pub struct RawModel {
    pub id: String,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default = "default_wire_shape")]
    pub wire_shape: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default = "default_ctx")]
    pub context_window: u64,
    #[serde(default)]
    pub max_output: u64,
    #[serde(default = "default_true")]
    pub tools: bool,
    #[serde(default)]
    pub prompt_cache: bool,
    #[serde(default = "default_thinking")]
    pub thinking: String,
    #[serde(default = "default_thinking_wire")]
    pub thinking_wire: String,
    // local-only provisioning fields
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub download_source: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub modelfile: Option<String>,
    #[serde(default)]
    pub num_ctx: Option<u32>,
    #[serde(default)]
    pub vram_curve: Option<String>,
}

fn default_wire_shape() -> String { "openai-chat".to_string() }
fn default_source() -> String { "static".to_string() }
fn default_ctx() -> u64 { 32_000 }
fn default_true() -> bool { true }
fn default_thinking() -> String { "none".to_string() }
fn default_thinking_wire() -> String { "none".to_string() }

fn parse_wire_shape(s: &str) -> anyhow::Result<WireShape> {
    Ok(match s {
        "openai-chat" => WireShape::OpenAIChat,
        "anthropic-messages" => WireShape::AnthropicMessages,
        "openai-responses" => WireShape::OpenAIResponses,
        "google-gemini" => WireShape::GoogleGemini,
        "ollama" => WireShape::Ollama,
        other => anyhow::bail!("unknown wire_shape: {other}"),
    })
}

fn parse_source(s: &str) -> anyhow::Result<Source> {
    Ok(match s {
        "static" => Source::Static,
        "wire" => Source::Wire,
        "user" => Source::User,
        other => anyhow::bail!("unknown source: {other}"),
    })
}

fn parse_thinking(s: &str) -> anyhow::Result<ThinkingSupport> {
    Ok(match s {
        "none" => ThinkingSupport::None,
        "toggle" => ThinkingSupport::Toggle,
        "toggle-with-effort" => ThinkingSupport::ToggleWithEffort,
        "budget" => ThinkingSupport::Budget,
        "adaptive" => ThinkingSupport::Adaptive,
        other => anyhow::bail!("unknown thinking: {other}"),
    })
}

fn parse_thinking_wire(s: &str) -> anyhow::Result<ThinkingWireShape> {
    Ok(match s {
        "none" => ThinkingWireShape::None,
        "anthropic" => ThinkingWireShape::Anthropic,
        "deepseek" => ThinkingWireShape::DeepSeek,
        "openai" => ThinkingWireShape::OpenAI,
        "ollama" => ThinkingWireShape::Ollama,
        other => anyhow::bail!("unknown thinking_wire: {other}"),
    })
}

fn parse_status(s: &str) -> anyhow::Result<Status> {
    Ok(match s {
        "available" => Status::Available,
        "planned" => Status::Planned,
        other => anyhow::bail!("unknown status: {other}"),
    })
}

impl TryFrom<RawRegistry> for Registry {
    type Error = anyhow::Error;
    fn try_from(raw: RawRegistry) -> anyhow::Result<Registry> {
        let mut providers = Vec::with_capacity(raw.provider.len());
        for rp in raw.provider {
            let transport = match rp.transport {
                RawTransport::Http { default_base_url } => Transport::Http { default_base_url },
                RawTransport::Cli { default_command } => Transport::Cli { default_command },
                RawTransport::Sdk => Transport::Sdk,
            };
            let mut models = Vec::with_capacity(rp.model.len());
            let mut seen = std::collections::HashSet::new();
            for rm in rp.model {
                let key = rm.id.to_ascii_lowercase();
                if !seen.insert(key) {
                    anyhow::bail!("duplicate model id in provider {}: {}", rp.id, rm.id);
                }
                models.push(ModelEntry {
                    id: rm.id,
                    display: rm.display,
                    wire_shape: parse_wire_shape(&rm.wire_shape)?,
                    source: parse_source(&rm.source)?,
                    default: rm.default,
                    hidden: rm.hidden,
                    info: ModelInfo {
                        context_window: rm.context_window,
                        max_output: rm.max_output,
                        tools: rm.tools,
                        prompt_cache: rm.prompt_cache,
                        thinking: parse_thinking(&rm.thinking)?,
                        thinking_wire: parse_thinking_wire(&rm.thinking_wire)?,
                    },
                    runtime: rm.runtime,
                    download_source: rm.download_source,
                    quant: rm.quant,
                    modelfile: rm.modelfile,
                    num_ctx: rm.num_ctx,
                    vram_curve: rm.vram_curve,
                });
            }
            providers.push(ProviderEntry {
                id: rp.id,
                display: rp.display,
                family: rp.family,
                transport,
                status: parse_status(&rp.status)?,
                key_url: rp.key_url.filter(|s| !s.is_empty()),
                key_env: rp.key_env.filter(|s| !s.is_empty()),
                models,
            });
        }
        Ok(Registry { providers })
    }
}
```

- [ ] **Step 4: Implement `parse.rs`**

```rust
//! TOML → Registry.

use anyhow::Result;
use zoid_model::Registry;

use crate::raw::RawRegistry;

/// Parse the shipped registry. `source` defaults to `static`; `wire`/`user`
/// sources are rejected here (they belong in the user file).
pub fn parse_shipped(text: &str) -> Result<Registry> {
    let raw: RawRegistry = toml::from_str(text)?;
    let reg = Registry::try_from(raw)?;
    for p in &reg.providers {
        for m in &p.models {
            anyhow::ensure!(
                m.source == zoid_model::Source::Static,
                "shipped registry must only contain source = \"static\" (found {} in {})",
                m.id,
                p.id
            );
        }
    }
    Ok(reg)
}

/// Parse the user registry. `source` must be `wire` or `user` (never `static`).
pub fn parse_user(text: &str) -> Result<Registry> {
    let raw: RawRegistry = toml::from_str(text)?;
    let reg = Registry::try_from(raw)?;
    for p in &reg.providers {
        for m in &p.models {
            anyhow::ensure!(
                m.source != zoid_model::Source::Static,
                "user registry must not contain source = \"static\" (found {} in {})",
                m.id,
                p.id
            );
        }
    }
    Ok(reg)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-registry`
Expected: PASS (all three parse tests).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-registry/src/raw.rs crates/zoid-registry/src/parse.rs
git commit -m "feat(zoid-registry): TOML parsing with enum validation and duplicate detection"
```

---

### Task 5: Merge logic (`merge.rs`)

**Files:**
- Modify: `crates/zoid-registry/src/merge.rs`

**Interfaces:**
- Consumes: `zoid_model::{Registry, ProviderEntry, ModelEntry, Source}`.
- Produces: `merge::merge(shipped: Registry, user: Registry) -> Registry`.

- [ ] **Step 1: Write the failing test**

Replace `merge.rs` with:

```rust
//! Merge user registry over shipped registry.

use zoid_model::{ModelEntry, ModelInfo, ProviderEntry, Registry, Source, Status, ThinkingSupport, ThinkingWireShape, Transport, WireShape};

/// Merge `user` over `shipped`. User rows override shipped rows by
/// `(provider.id, model.id)` (case-insensitive on model id). A user row may
/// add a new provider or model. `hidden = true` hides a shipped model. A user
/// `default = true` demotes the shipped default.
pub fn merge(shipped: Registry, user: Registry) -> Registry {
    let mut providers: Vec<ProviderEntry> = shipped.providers;

    for up in user.providers {
        match providers.iter_mut().find(|p| p.id == up.id) {
            Some(existing) => {
                // Merge models: user rows override by (case-insensitive) id.
                for um in up.models {
                    let key = um.id.to_ascii_lowercase();
                    match existing.models.iter_mut().find(|m| m.id.to_ascii_lowercase() == key) {
                        Some(em) => {
                            if um.default {
                                // demote any other default in this provider
                                for m in existing.models.iter_mut() {
                                    m.default = false;
                                }
                            }
                            *em = um;
                        }
                        None => {
                            if um.default {
                                for m in existing.models.iter_mut() {
                                    m.default = false;
                                }
                            }
                            existing.models.push(um);
                        }
                    }
                }
            }
            None => providers.push(up),
        }
    }

    Registry { providers }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, default: bool, hidden: bool) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            display: None,
            wire_shape: WireShape::OpenAIChat,
            source: Source::Static,
            default,
            hidden,
            info: ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
            runtime: None, download_source: None, quant: None, modelfile: None, num_ctx: None, vram_curve: None,
        }
    }

    fn provider(id: &str, models: Vec<ModelEntry>) -> ProviderEntry {
        ProviderEntry {
            id: id.to_string(), display: id.to_string(), family: id.to_string(),
            transport: Transport::Http { default_base_url: "https://x".to_string() },
            status: Status::Available, key_url: Some("https://x".to_string()), key_env: Some("K".to_string()),
            models,
        }
    }

    #[test]
    fn user_row_overrides_shipped_by_id() {
        let shipped = Registry { providers: vec![provider("p", vec![model("a", true, false)])] };
        let mut m = model("a", false, false);
        m.info.context_window = 999_999;
        let user = Registry { providers: vec![provider("p", vec![m])] };
        let merged = merge(shipped, user);
        assert_eq!(merged.providers[0].models[0].info.context_window, 999_999);
    }

    #[test]
    fn user_default_demotes_shipped_default() {
        let shipped = Registry { providers: vec![provider("p", vec![model("a", true, false), model("b", false, false)])] };
        let user = Registry { providers: vec![provider("p", vec![model("b", true, false)])] };
        let merged = merge(shipped, user);
        let defaults: Vec<&str> = merged.providers[0].models.iter().filter(|m| m.default).map(|m| m.id.as_str()).collect();
        assert_eq!(defaults, vec!["b"]);
    }

    #[test]
    fn user_can_add_new_provider() {
        let shipped = Registry { providers: vec![provider("p", vec![]) ] };
        let user = Registry { providers: vec![provider("q", vec![model("x", false, false)])] };
        let merged = merge(shipped, user);
        assert_eq!(merged.providers.len(), 2);
        assert!(merged.entry("q").is_some());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-registry merge`
Expected: FAIL — the stub `merge` returns `shipped` unchanged.

- [ ] **Step 3: The implementation is already in the file above** (the `merge` fn is written in Step 1). Verify it compiles and passes.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-registry`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-registry/src/merge.rs
git commit -m "feat(zoid-registry): merge user registry over shipped with default demotion"
```

---

## Phase 2 — Switch consumers

### Task 6: Ship `models.toml` (transcription of today's consts)

**Files:**
- Create: `crates/zoid-model/models.toml`

**Interfaces:**
- Produces: the shipped registry file, a faithful transcription of `PROVIDERS` + `ZEN_MODEL_IDS` + `MODEL_CAPS`, with the `claude-sonnet-4-6` duplicate resolved into two `(provider, model)` rows.

- [ ] **Step 1: Write the shipped TOML**

Create `crates/zoid-model/models.toml` with all six providers and their models. Transcribe every model id from `PROVIDERS[].models` and `ZEN_MODEL_IDS`, and every `MODEL_CAPS` entry, into `(provider, model)` rows. Mark `default = true` on `glm-5.2:cloud` (ollama-cloud) and `claude-sonnet-4-6` (anthropic-api). Resolve the `claude-sonnet-4-6` duplicate: `anthropic-api` → 1M, `opencode-zen` → 200K.

The full file (abbreviated here for the plan; the implementer transcribes ALL entries from `crates/zoid-model/src/lib.rs`):

```toml
# Shipped provider/model registry. Replaced wholesale on upgrade; never edited
# by the tool or user in normal operation. User overrides live in models.user.toml.

[[provider]]
id = "ollama-local"
display = "ollama · local"
family = "ollama"
transport = { kind = "http", default_base_url = "http://localhost:11434" }
status = "available"

  [[provider.model]]
  id = "qwythos"
  display = "Qwythos 9B (Claude Mythos 5, 1M)"
  wire_shape = "ollama"
  source = "static"
  context_window = 1048576
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "toggle"
  thinking_wire = "ollama"
  runtime = "ollama"
  download_source = "hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M"
  quant = "Q4_K_M"
  modelfile = """FROM hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M
TEMPLATE \"\"\"{{ if .System }}<|im_start|>system
{{ .System }}<|im_end|>{{ end }}<|im_start|>user
{{ .Prompt }}<|im_end|>
<|im_start|>assistant\"\"\"
PARAMETER stop <|im_end|>
PARAMETER stop <|im_start|>"""
  num_ctx = 98304
  vram_curve = """[{"num_ctx":32768,"vram_mb":7000},{"num_ctx":65536,"vram_mb":8500},{"num_ctx":98304,"vram_mb":10000},{"num_ctx":131072,"vram_mb":12000}]"""

[[provider]]
id = "ollama-cloud"
display = "ollama · cloud"
family = "ollama"
transport = { kind = "http", default_base_url = "https://ollama.com" }
status = "available"
key_url = "https://ollama.com"
key_env = "OLLAMA_API_KEY"

  [[provider.model]]
  id = "glm-5.2:cloud"
  wire_shape = "ollama"
  source = "static"
  default = true
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "none"
  thinking_wire = "none"

[[provider]]
id = "opencode-go"
display = "opencode · go"
family = "opencode-go"
transport = { kind = "http", default_base_url = "https://opencode.ai/zen/go" }
status = "available"
key_url = "https://opencode.ai"
key_env = "OPENCODE_GO_API_KEY"

  # 13 models: glm-5.2 (default), glm-5.1, kimi-k2.7-code, kimi-k2.6,
  # deepseek-v4-pro, deepseek-v4-flash, mimo-v2.5, mimo-v2.5-pro (OpenAIChat),
  # minimax-m3, minimax-m2.7, minimax-m2.5, qwen3.7-max, qwen3.7-plus (Anthropic).
  [[provider.model]]
  id = "glm-5.2"
  wire_shape = "openai-chat"
  source = "static"
  default = true
  context_window = 1000000
  max_output = 131072
  tools = true
  prompt_cache = true
  thinking = "toggle-with-effort"
  thinking_wire = "deepseek"
  # ... (transcribe the remaining 12 Go models from MODEL_CAPS)

[[provider]]
id = "anthropic-api"
display = "anthropic · api key"
family = "anthropic"
transport = { kind = "http", default_base_url = "https://api.anthropic.com" }
status = "available"
key_url = "https://console.anthropic.com/settings/keys"
key_env = "ANTHROPIC_API_KEY"

  [[provider.model]]
  id = "claude-sonnet-4-6"
  wire_shape = "anthropic-messages"
  source = "static"
  default = true
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "budget"
  thinking_wire = "anthropic"

  [[provider.model]]
  id = "claude-opus-4-8"
  wire_shape = "anthropic-messages"
  source = "static"
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "adaptive"
  thinking_wire = "anthropic"

[[provider]]
id = "zai-coding-plan"
display = "zai · coding plan"
family = "zai"
transport = { kind = "http", default_base_url = "https://api.z.ai/api/coding/paas/v4" }
status = "available"
key_url = "https://z.ai"
key_env = "ZAI_API_KEY"

  [[provider.model]]
  id = "glm-5.2"
  wire_shape = "openai-chat"
  source = "static"
  default = true
  context_window = 1000000
  max_output = 131072
  tools = true
  prompt_cache = true
  thinking = "toggle-with-effort"
  thinking_wire = "deepseek"
  # ... glm-5-turbo, glm-4.7

[[provider]]
id = "opencode-zen"
display = "opencode · zen"
family = "opencode-zen"
transport = { kind = "http", default_base_url = "https://opencode.ai/zen" }
status = "available"
key_url = "https://opencode.ai"
key_env = "OPENCODE_GO_API_KEY"

  # 52 models across four wire shapes. First entry is the default.
  [[provider.model]]
  id = "claude-sonnet-4-5"
  wire_shape = "anthropic-messages"
  source = "static"
  default = true
  context_window = 200000
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "none"
  thinking_wire = "none"
  # ... (transcribe the remaining 51 Zen models from ZEN_MODEL_IDS + MODEL_CAPS)
```

**Transcription rules (apply to every model):**
- `wire_shape` comes from `GO_MODELS`/`ZEN_MODELS` (OpenAICompat→`openai-chat`, Anthropic→`anthropic-messages`, OpenAIResponses→`openai-responses`, GoogleGemini→`google-gemini`).
- `context_window`/`max_output`/`tools`/`prompt_cache`/`thinking`/`thinking_wire` come from `MODEL_CAPS`.
- `thinking`/`thinking_wire` enum names map: `None`→`none`, `Toggle`→`toggle`, `ToggleWithEffort`→`toggle-with-effort`, `Budget`→`budget`, `Adaptive`→`adaptive`; `Anthropic`→`anthropic`, `DeepSeek`→`deepseek`, `OpenAI`→`openai`, `Ollama`→`ollama`.
- `claude-sonnet-4-6` appears TWICE: `anthropic-api` (1M, Budget/Anthropic) and `opencode-zen` (200K, None/None). This is the intended bug fix.
- `glm-5.2` appears in `opencode-go`, `zai-coding-plan`, and `opencode-zen` (three separate rows, each with its own caps).

- [ ] **Step 2: Verify the transcription parses**

Run: `cargo test -p zoid-registry` (add a temporary test that `include_str!`s the file and parses it — or just run a one-off check below).

Add this test to `crates/zoid-registry/src/parse.rs` tests:

```rust
#[test]
fn shipped_models_toml_parses() {
    let text = include_str!("../../zoid-model/models.toml");
    let reg = parse_shipped(text).unwrap();
    assert_eq!(reg.selectable().count(), 6);
    assert!(reg.entry("opencode-zen").unwrap().models.len() >= 52);
    assert!(reg.entry("opencode-go").unwrap().models.len() == 13);
}
```

Run: `cargo test -p zoid-registry shipped_models_toml_parses`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-model/models.toml crates/zoid-registry/src/parse.rs
git commit -m "feat: ship models.toml transcription of the const registry"
```

---

### Task 7: Thread `Arc<Registry>` through `zoid-provider` helpers

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs`

**Interfaces:**
- Consumes: `zoid_model::Registry`.
- Produces: `context_ceiling(reg: &Registry, provider: &str, model: &str) -> u64`, `has_prompt_cache(reg: &Registry, provider: &str, model: &str) -> bool`, `default_model(reg: &Registry) -> String`, `default_provider(reg: &Registry) -> Arc<dyn Provider>`.

- [ ] **Step 1: Write the failing test**

Append to `crates/zoid-provider/src/lib.rs` tests:

```rust
#[test]
fn context_ceiling_uses_registry_and_env_override() {
    let reg = zoid_model::Registry::default();
    // empty registry → conservative default 32k
    assert_eq!(context_ceiling(&reg, "p", "m"), 32_000);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider context_ceiling_uses_registry_and_env_override`
Expected: FAIL — `context_ceiling` currently takes only `model`.

- [ ] **Step 3: Rewrite the helpers**

Replace `context_ceiling`, `has_prompt_cache`, `default_model`, `default_provider` in `crates/zoid-provider/src/lib.rs` (lines 305–327) with:

```rust
/// The context-window ceiling (tokens) for a (provider, model) pair — the
/// economy ⑤ denominator. `ZOID_CONTEXT_CEILING` (a positive integer)
/// overrides the registry.
pub fn context_ceiling(reg: &model::Registry, provider: &str, model: &str) -> u64 {
    if let Ok(v) = std::env::var("ZOID_CONTEXT_CEILING") {
        if let Ok(n) = v.trim().parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }
    reg.model_info(provider, model).context_window
}

/// Whether the (provider, model) reports a token-level prompt cache.
pub fn has_prompt_cache(reg: &model::Registry, provider: &str, model: &str) -> bool {
    reg.model_info(provider, model).prompt_cache
}

/// The default model id for the env-selected provider.
pub fn default_model(reg: &model::Registry) -> String {
    let provider = if std::env::var("OLLAMA_API_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false)
    {
        "ollama-cloud"
    } else {
        "anthropic-api"
    };
    reg.default_model(provider)
        .map(str::to_string)
        .unwrap_or_default()
}

/// Select the provider from the environment (unchanged env-driven logic), but
/// the default *model* comes from the registry's `default = true` flag.
pub fn default_provider(reg: &model::Registry) -> Arc<dyn Provider> {
    if let Ok(key) = std::env::var("OLLAMA_API_KEY") {
        if !key.is_empty() {
            return Arc::new(ollama::OllamaProvider::new(key));
        }
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Arc::new(anthropic::AnthropicProvider::new(key));
        }
    }
    Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("(no OLLAMA_API_KEY / ANTHROPIC_API_KEY — offline echo) ".into()),
        ProviderEvent::TextDelta("hello from zoid's fake provider.".into()),
        ProviderEvent::Done,
    ]))
}
```

Note: `default_provider`'s `reg` parameter is currently unused (env-driven provider selection is unchanged); it is accepted for signature symmetry and future use. Add `#[allow(unused_variables)]` if the compiler warns.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-provider context_ceiling_uses_registry_and_env_override`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/lib.rs
git commit -m "refactor(zoid-provider): thread Registry through context_ceiling/has_prompt_cache/default_model"
```

---

### Task 8: Route composite providers via `Registry::wire_shape`

**Files:**
- Modify: `crates/zoid-provider/src/opencode_go.rs`
- Modify: `crates/zoid-provider/src/opencode_zen.rs`

**Interfaces:**
- Consumes: `zoid_model::{Registry, WireShape}`.
- Produces: `OpenCodeGoProvider::new(api_key, reg: Arc<Registry>)`, `OpenCodeZenProvider::new(api_key, reg: Arc<Registry>)` — both hold `Arc<Registry>` and route `stream()` via `reg.wire_shape(provider, model)`.

- [ ] **Step 1: Write the failing test**

In `opencode_go.rs` tests, replace the `wire_shape_for_known_models_matches_table` test with a registry-driven one:

```rust
#[test]
fn routes_via_registry_wire_shape() {
    use zoid_model::{ModelEntry, ModelInfo, ProviderEntry, Registry, Source, Status, ThinkingSupport, ThinkingWireShape, Transport, WireShape};
    let reg = Registry { providers: vec![ProviderEntry {
        id: "opencode-go".to_string(), display: "go".into(), family: "opencode-go".into(),
        transport: Transport::Http { default_base_url: "https://x".into() },
        status: Status::Available, key_url: Some("https://x".into()), key_env: Some("K".into()),
        models: vec![ModelEntry {
            id: "minimax-m3".to_string(), display: None, wire_shape: WireShape::AnthropicMessages,
            source: Source::Static, default: false, hidden: false,
            info: ModelInfo { context_window: 200_000, max_output: 0, tools: false, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
            runtime: None, download_source: None, quant: None, modelfile: None, num_ctx: None, vram_curve: None,
        }],
    }] };
    let p = OpenCodeGoProvider::new("k".into(), std::sync::Arc::new(reg));
    assert_eq!(p.wire_shape_for("minimax-m3"), WireShape::AnthropicMessages);
    assert_eq!(p.wire_shape_for("unknown"), WireShape::OpenAIChat);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider routes_via_registry_wire_shape`
Expected: FAIL — `OpenCodeGoProvider::new` takes one arg and `wire_shape_for` reads the deleted `GO_MODELS`.

- [ ] **Step 3: Rewrite `opencode_go.rs`**

Replace the `GO_MODELS` const and the `wire_shape_for` method. The struct gains an `Arc<Registry>` field:

```rust
use std::sync::Arc;
use zoid_model::{Registry, WireShape};

pub struct OpenCodeGoProvider {
    api_key: String,
    base_url: String,
    reg: Arc<Registry>,
    #[allow(dead_code)]
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenCodeGoProvider {
    pub fn new(api_key: String, reg: Arc<Registry>) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("opencode-go")
                .unwrap_or("https://opencode.ai/zen/go")
                .to_string(),
            reg,
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
        }
    }

    // ... with_base_url / with_idle_timeout unchanged ...

    fn wire_shape_for(&self, model: &str) -> WireShape {
        self.reg
            .wire_shape("opencode-go", model)
            .unwrap_or_else(|| {
                tracing::warn!(model = %model, "opencode-go: model not in registry; defaulting to OpenAIChat");
                WireShape::OpenAIChat
            })
    }
}
```

Update the `stream()` match to use `WireShape` (the `zoid_model` enum) instead of the local `WireShape` enum — delete the local `enum WireShape { OpenAICompat, Anthropic }` and map:

```rust
match self.wire_shape_for(&req.model) {
    WireShape::OpenAIChat => { /* OpenAICompatProvider ... */ }
    WireShape::AnthropicMessages => { /* AnthropicProvider ... */ }
    other => {
        tracing::warn!(shape = ?other, "opencode-go: unexpected wire shape; defaulting to OpenAIChat");
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_idle_timeout(self.idle_timeout)
            .stream(req, sink).await
    }
}
```

- [ ] **Step 4: Apply the same change to `opencode_zen.rs`**

Replace `ZEN_MODELS` and the local `ZenWireShape` enum with `zoid_model::WireShape`. The struct gains `reg: Arc<Registry>`; `new(api_key, reg)`. `wire_shape_for` becomes:

```rust
fn wire_shape_for(&self, model: &str) -> WireShape {
    self.reg
        .wire_shape("opencode-zen", model)
        .unwrap_or_else(|| {
            tracing::warn!(model = %model, "opencode-zen: model not in registry; defaulting to OpenAIChat");
            WireShape::OpenAIChat
        })
}
```

Map the four `WireShape` variants to the four sub-clients (`OpenAIChat`→OpenAICompat, `AnthropicMessages`→Anthropic, `OpenAIResponses`→OpenAIResponses, `GoogleGemini`→GoogleGemini).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS (routing tests now feed from the in-memory registry).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-provider/src/opencode_go.rs crates/zoid-provider/src/opencode_zen.rs
git commit -m "refactor(zoid-provider): route composite providers via Registry::wire_shape"
```

---

### Task 9: Rewrite `select_provider`/`provider_for_id`/`key_env_for` in `main.rs`

**Files:**
- Modify: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `zoid_model::Registry`, `zoid_provider::{context_ceiling, has_prompt_cache, default_model}` (new signatures from Task 7), `OpenCodeGoProvider::new(key, reg)`, `OpenCodeZenProvider::new(key, reg)` (new signatures from Task 8).
- Produces: `select_provider(config, secrets, reg: &Registry) -> (Arc<dyn Provider>, String, bool)`, `provider_for_id(id, secrets, reg: &Registry) -> Option<Arc<dyn Provider>>`, `key_env_for(id, reg: &Registry) -> Option<String>`.

- [ ] **Step 1: Add `reg: &Registry` to the three functions and replace the `family`/`key_env_for` matches**

`key_env_for` (lines 1093–1103) becomes:

```rust
fn key_env_for(id: &str, reg: &zoid_model::Registry) -> Option<String> {
    reg.entry(id).and_then(|e| e.key_env.clone())
}
```

`entry_requires_key` (lines 1086–1090) becomes:

```rust
fn entry_requires_key(id: &str, reg: &zoid_model::Registry) -> bool {
    reg.entry(id).map(|e| e.key_url.is_some()).unwrap_or(true)
}
```

`select_provider` (lines 1111–1195): add `reg: &zoid_model::Registry` parameter. Replace the `family`-based `match` with a `wire_shape`-based dispatch. The key resolution closure stays (env → secret store). The `ollama-local` special case stays. For the remaining providers, dispatch on `reg.entry(&config.provider).map(|e| e.wire_shape)` is NOT correct (wire_shape is per-model, not per-provider) — instead dispatch on the provider's `family` is ALSO not needed. The correct dispatch: each provider id maps to a concrete provider constructor. Since the registry no longer carries a "which Rust type" field, keep a small explicit match on the canonical provider id (this is the one place a per-provider constructor mapping is unavoidable):

```rust
fn select_provider(
    config: &zoid_core::config::Config,
    secrets: &Option<std::sync::Arc<zoid_core::secret::EncryptedDb>>,
    reg: &zoid_model::Registry,
) -> (Arc<dyn Provider>, String, bool) {
    let key_for = |name: &str| -> Option<String> {
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() { return Some(v); }
        }
        secrets.as_ref().and_then(|s| {
            use zoid_core::secret::SecretStore;
            s.get(name)
        })
    };
    let canon = zoid_model::Registry::canonical_id(&config.provider);
    if canon == "ollama-local" {
        let base_url = effective_base_url(config);
        return (
            Arc::new(
                zoid_provider::ollama::OllamaProvider::new(String::new())
                    .with_base_url(base_url)
                    .with_num_ctx(zoid_provider::ollama::configured_num_ctx(config.economy.num_ctx)),
            ),
            "ollama".to_string(),
            true,
        );
    }
    let base_url = effective_base_url(config);
    let key_env = reg.entry(canon).and_then(|e| e.key_env.clone());
    let key = key_env.as_deref().and_then(key_for);
    let (provider, name): (Arc<dyn Provider>, &str) = match canon {
        "opencode-go" => (Arc::new(zoid_provider::opencode_go::OpenCodeGoProvider::new(key.clone().unwrap_or_default(), Arc::new(reg.clone())).with_base_url(base_url)), "opencode-go"),
        "opencode-zen" => (Arc::new(zoid_provider::opencode_zen::OpenCodeZenProvider::new(key.clone().unwrap_or_default(), Arc::new(reg.clone())).with_base_url(base_url)), "opencode-zen"),
        "anthropic-api" => (Arc::new(zoid_provider::anthropic::AnthropicProvider::new(key.clone().unwrap_or_default()).with_base_url(base_url)), "anthropic"),
        "zai-coding-plan" => (Arc::new(zoid_provider::zai::ZaiProvider::new(key.clone().unwrap_or_default()).with_base_url(base_url)), "zai"),
        "gemini-api" => (Arc::new(zoid_provider::google_gemini::GoogleGeminiProvider::new(key.clone().unwrap_or_default()).with_base_url(base_url)), "gemini"),
        _ => (Arc::new(zoid_provider::ollama::OllamaProvider::new(key.clone().unwrap_or_default()).with_base_url(base_url)), "ollama"),
    };
    (provider, name.to_string(), key.is_some())
}
```

Note: `reg.clone()` requires `Registry: Clone` (it is, from Task 1). The `gemini-api` arm is added now (Phase 4 wires the registry entry; the constructor already exists).

- [ ] **Step 2: Apply the same to `provider_for_id`** (lines 1242–1292): add `reg: &Registry`, replace the `family` match with the same canonical-id match, using `reg.clone()` for the composite providers.

- [ ] **Step 3: Update all call sites**

`select_provider` is called at lines 2536, 4360, 4951, 4997. `provider_for_id` at 1299 (via `spawn_switch_model_fetch`). `key_env_for` at 11386–11414 (tests) and in `spawn_switch_model_fetch`/onboarding. Update each to pass `&app.registry` (or the local `reg`). Add `registry: Arc<Registry>` to the `App` struct and populate it at boot (see Task 10).

- [ ] **Step 4: Build to surface all remaining call sites**

Run: `cargo build -p zoid 2>&1 | head -50`
Expected: compiler errors listing every call site that needs the new `reg` argument. Fix each mechanically.

- [ ] **Step 5: Run tests**

Run: `cargo test -p zoid`
Expected: PASS (after updating the `key_env_for`/`entry_requires_key` tests to pass a `&Registry`).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "refactor(zoid): thread Registry through provider selection"
```

---

### Task 10: Load the registry at boot and add stale-selection recovery

**Files:**
- Modify: `crates/zoid/src/main.rs`
- Modify: `crates/zoid-tui/src/state.rs` (add a banner field if needed)

**Interfaces:**
- Consumes: `zoid_registry::load`, `zoid_model::Registry`.
- Produces: `App.registry: Arc<Registry>` populated at boot; stale-selection check that opens `Overlay::ProviderSwitch` with a banner, or runs the offline `FakeProvider` on dismiss.

- [ ] **Step 1: Add `registry` to the `App` struct and load it at boot**

In `main.rs`, add `pub registry: std::sync::Arc<zoid_model::Registry>` to the `App` struct. At boot (near line 2525, after `load_config()`), load the registry:

```rust
let cfg_dir = resolve_config_dir(|k: &str| std::env::var(k).ok());
let shipped_path = cfg_dir.join("models.toml");
let user_path = cfg_dir.join("models.user.toml");
// Fall back to the embedded shipped file if the on-disk one is absent.
let (registry, reg_warning) = match zoid_registry::load(&shipped_path, &user_path) {
    Ok(r) => r,
    Err(e) => {
        tracing::warn!(error = %e, "failed to load registry; using embedded default");
        let shipped = include_str!("../../zoid-model/models.toml");
        (zoid_registry::parse::parse_shipped(shipped).unwrap_or_default(), Some(e.to_string()))
    }
};
let registry = std::sync::Arc::new(registry);
```

Note: the shipped `models.toml` is embedded via `include_str!` as a fallback so the binary always runs even if the on-disk file is missing/corrupt. The on-disk shipped file is the primary source (so upgrades can replace it); the embedded copy is the bootstrap default.

- [ ] **Step 2: Add the stale-selection check**

After `select_provider` (line 2536), before building the shell, validate the selection:

```rust
let provider_stale = registry.entry(&config.provider).is_none();
let model_stale = !provider_stale
    && !config.model.is_empty()
    && registry
        .models_for(&config.provider)
        .iter()
        .all(|m| m.id != config.model && !m.hidden);
```

If `provider_stale || model_stale`, set a flag that (a) seeds `switch_providers`/`switch_models` from the registry, (b) opens `Overlay::ProviderSwitch`, and (c) sets a banner. On dismiss without a valid selection, run the offline `FakeProvider` and keep a persistent banner.

- [ ] **Step 3: Write the failing test**

Add a test that a removed model triggers the quick-switch path (assert `app.shell.overlay == Overlay::ProviderSwitch` and a banner is set), and that a valid selection does not.

- [ ] **Step 4: Run test to verify it fails, then implement**

Run: `cargo test -p zoid <test_name>`
Expected: FAIL, then PASS after wiring.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid-tui/src/state.rs
git commit -m "feat(zoid): load registry at boot and recover from stale selection"
```

---

### Task 11: Delete the consts and port the lock tests

**Files:**
- Modify: `crates/zoid-model/src/lib.rs` (delete `PROVIDERS`, `ZEN_MODEL_IDS`, `MODEL_CAPS`, and the old `&'static str` `ProviderEntry`/`Transport`/`entry`/`models_for`/`default_base_url`/`selectable`/`model_info` free fns)
- Modify: `crates/zoid-tui/src/config_view.rs` (`provider_options`/`model_options` take `&Registry`)

**Interfaces:**
- Consumes: `Registry` methods from Task 2.
- Produces: `zoid-model` exposes only the owned `Registry` + types; `config_view::provider_options(reg, current_id)`, `config_view::model_options(reg, provider_id, current_model)`.

- [ ] **Step 1: Delete the old consts and free functions**

In `crates/zoid-model/src/lib.rs`, delete `PROVIDERS`, `ZEN_MODEL_IDS`, `MODEL_CAPS`, and the free functions `entry`, `models_for`, `default_base_url`, `selectable`, `model_info` (the `Registry` methods from Task 2 replace them). Keep `canonical_id` as a free fn (it's used by `config_view` and `main.rs` without a `Registry`), OR move it to `Registry::canonical_id` and update callers. Keep `DEFAULT_MODEL_INFO` (used by `Registry::model_info`).

- [ ] **Step 2: Update `config_view.rs`**

`provider_options` and `model_options` take `&Registry`:

```rust
pub fn provider_options(reg: &model::Registry, current_id: &str) -> Vec<PickOption> {
    let cur = model::Registry::canonical_id(current_id);
    reg.providers.iter().map(|e| { /* same body, but e.id/e.display are String */ }).collect()
}

pub fn model_options(reg: &model::Registry, provider_id: &str, current_model: &str) -> Vec<PickOption> {
    let mut models: Vec<&model::ModelEntry> = reg.models_for(provider_id).iter().filter(|m| !m.hidden).collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.iter().map(|m| PickOption {
        id: m.id.clone(),
        label: m.display.clone().unwrap_or_else(|| m.id.clone()),
        detail: String::new(),
        selectable: true,
        is_current: m.id == current_model,
    }).collect()
}
```

- [ ] **Step 3: Port the lock tests**

Replace the deleted const-lock tests in `zoid-model/src/lib.rs` with tests that load the shipped TOML and assert the same invariants. Move them to `zoid-registry` (which can `include_str!` the TOML) or keep a `#[cfg(test)]` in `zoid-model` that uses `include_str!` + a minimal inline parse. Since `zoid-model` is dependency-free, put these tests in `zoid-registry/src/parse.rs`:

```rust
#[test]
fn shipped_registry_invariants() {
    let reg = parse_shipped(include_str!("../../zoid-model/models.toml")).unwrap();
    // six selectable providers (gemini-api lands in Phase 4 → seven)
    let ids: Vec<&str> = reg.selectable().map(|e| e.id.as_str()).collect();
    assert_eq!(ids.len(), 6);
    for id in ["ollama-local", "ollama-cloud", "opencode-go", "opencode-zen", "anthropic-api", "zai-coding-plan"] {
        assert!(ids.contains(&id));
    }
    // key_url invariant: ollama-local None, all others Some
    for e in reg.selectable() {
        if e.id == "ollama-local" { assert!(e.key_url.is_none()); }
        else { assert!(e.key_url.is_some(), "{} must have key_url", e.id); }
    }
    // opencode-go has 13 models
    assert_eq!(reg.entry("opencode-go").unwrap().models.len(), 13);
    // every opencode-zen model has explicit caps >= 128k
    for m in &reg.entry("opencode-zen").unwrap().models {
        assert!(m.info.context_window >= 128_000, "{} needs explicit caps", m.id);
    }
    // claude-sonnet-4-6 is split: anthropic-api 1M, opencode-zen 200K
    assert_eq!(reg.model_info("anthropic-api", "claude-sonnet-4-6").context_window, 1_000_000);
    assert_eq!(reg.model_info("opencode-zen", "claude-sonnet-4-6").context_window, 200_000);
}
```

- [ ] **Step 4: Build the workspace to surface remaining consumers**

Run: `cargo build --workspace 2>&1 | head -80`
Expected: compiler errors listing every remaining consumer of the deleted consts/free fns. Fix each (mostly `config_view` and `main.rs` call sites, plus `zoid-tui` tests).

- [ ] **Step 5: Run the full test suite**

Run: `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: delete const registry, port lock tests to shipped TOML"
```

---

## Phase 3 — Refresh tool

### Task 12: Per-provider fetchers (`fetch.rs`)

**Files:**
- Modify: `crates/zoid-registry/src/fetch.rs`

**Interfaces:**
- Consumes: `reqwest`, `zoid_model::Registry`.
- Produces: `fetch::list_models(provider_id: &str, base_url: &str, key: &str) -> Result<Vec<String>>`, `fetch::caps(provider_id: &str, base_url: &str, key: &str, model: &str) -> Result<Option<ModelInfo>>`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ollama_tags_shape() {
        let body = r#"{"models":[{"name":"glm-5.2:cloud"},{"name":"llama3"}]}"#;
        assert_eq!(parse_ollama_tags(body), vec!["glm-5.2:cloud", "llama3"]);
    }

    #[test]
    fn parse_data_id_shape() {
        let body = r#"{"data":[{"id":"gpt-5.4"},{"id":"gpt-5"}]}"#;
        assert_eq!(parse_data_id(body), vec!["gpt-5.4", "gpt-5"]);
    }

    #[test]
    fn parse_gemini_models_shape() {
        let body = r#"{"models":[{"name":"models/gemini-3-flash"}]}"#;
        assert_eq!(parse_gemini_models(body), vec!["gemini-3-flash"]);
    }

    #[test]
    fn parse_gemini_caps_shape() {
        let body = r#"{"models":[{"name":"models/gemini-3-flash","inputTokenLimit":1000000,"outputTokenLimit":8192}]}"#;
        let caps = parse_gemini_caps(body, "gemini-3-flash").unwrap();
        assert_eq!(caps.context_window, 1_000_000);
        assert_eq!(caps.max_output, 8192);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-registry fetch`
Expected: FAIL — module is empty.

- [ ] **Step 3: Implement the fetchers and parsers**

```rust
//! Per-provider live-list fetchers and response parsers.

use anyhow::Result;
use zoid_model::ModelInfo;

/// Parse Ollama `/api/tags` → `.models[].name`.
pub fn parse_ollama_tags(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|arr| arr.iter().filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Parse OpenAI-compat/Anthropic `/v1/models` → `.data[].id`.
pub fn parse_data_id(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
        .map(|arr| arr.iter().filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Parse Gemini `/v1/models` → `.models[].name` (strip the `models/` prefix).
pub fn parse_gemini_models(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|arr| arr.iter().filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(|s| s.trim_start_matches("models/").to_string())).collect())
        .unwrap_or_default()
}

/// Parse Gemini `/v1beta/models` caps for one model → `ModelInfo`.
pub fn parse_gemini_caps(body: &str, model: &str) -> Option<ModelInfo> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let arr = v.get("models")?.as_array()?;
    let m = arr.iter().find(|m| m.get("name").and_then(|n| n.as_str()).map(|s| s.trim_start_matches("models/")) == Some(model))?;
    Some(ModelInfo {
        context_window: m.get("inputTokenLimit").and_then(|n| n.as_u64()).unwrap_or(0),
        max_output: m.get("outputTokenLimit").and_then(|n| n.as_u64()).unwrap_or(0),
        tools: true,
        prompt_cache: false,
        thinking: zoid_model::ThinkingSupport::Toggle,
        thinking_wire: zoid_model::ThinkingWireShape::None,
    })
}

/// Fetch the live model id list for a provider.
pub async fn list_models(provider_id: &str, base_url: &str, key: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let (url, auth_header, auth_value) = match provider_id {
        "ollama-cloud" | "ollama-local" => (format!("{base_url}/api/tags"), "authorization", format!("Bearer {key}")),
        "anthropic-api" => (format!("{base_url}/v1/models"), "x-api-key", key.to_string()),
        "gemini-api" => (format!("{base_url}/v1/models"), "x-goog-api-key", key.to_string()),
        "zai-coding-plan" => (format!("{base_url}/models"), "authorization", format!("Bearer {key}")),
        _ => (format!("{base_url}/v1/models"), "authorization", format!("Bearer {key}")),
    };
    let mut req = client.get(&url).header(auth_header, &auth_value);
    if provider_id == "anthropic-api" {
        req = req.header("anthropic-version", "2023-06-01");
    }
    let body = req.send().await?.error_for_status()?.text().await?;
    Ok(match provider_id {
        "ollama-cloud" | "ollama-local" => parse_ollama_tags(&body),
        "gemini-api" => parse_gemini_models(&body),
        _ => parse_data_id(&body),
    })
}

/// Fetch wire-derived caps for a model (Ollama `/api/show`, Gemini `/v1beta/models`).
pub async fn caps(provider_id: &str, base_url: &str, key: &str, model: &str) -> Result<Option<ModelInfo>> {
    let client = reqwest::Client::new();
    match provider_id {
        "ollama-cloud" | "ollama-local" => {
            let body = client.post(format!("{base_url}/api/show"))
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .json(&serde_json::json!({ "model": model }))
                .send().await?.error_for_status()?.text().await?;
            let v: serde_json::Value = serde_json::from_str(&body)?;
            let window = v.get("model_info").and_then(|m| m.get("context_length")).and_then(|n| n.as_u64());
            Ok(window.map(|w| ModelInfo { context_window: w, max_output: 0, tools: true, prompt_cache: true, thinking: zoid_model::ThinkingSupport::None, thinking_wire: zoid_model::ThinkingWireShape::None }))
        }
        "gemini-api" => {
            let body = client.get(format!("{base_url}/v1beta/models"))
                .header("x-goog-api-key", key)
                .send().await?.error_for_status()?.text().await?;
            Ok(parse_gemini_caps(&body, model))
        }
        _ => Ok(None),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-registry fetch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-registry/src/fetch.rs
git commit -m "feat(zoid-registry): per-provider fetchers and parsers"
```

---

### Task 13: Reconcile logic (`refresh.rs`)

**Files:**
- Modify: `crates/zoid-registry/src/refresh.rs`

**Interfaces:**
- Consumes: `fetch::{list_models, caps}`, `zoid_model::{Registry, ModelEntry, Source, ModelInfo}`.
- Produces: `refresh::reconcile(reg: &Registry, keys: &HashMap<String, String>) -> Result<ReconcileReport>` where `ReconcileReport` carries the new/updated/removed/reported rows and the serialized user TOML.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_adds_wire_rows_only_for_ollama_and_gemini() {
        // A registry with ollama-cloud (wire-capable) and anthropic-api (static-only).
        // reconcile() should add wire rows for ollama-cloud but only REPORT (not add)
        // for anthropic-api.
        // (Full test uses a mock fetcher; see Step 3 for the seam.)
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-registry refresh`
Expected: FAIL — module empty.

- [ ] **Step 3: Implement reconcile with a fetcher seam**

To keep `reconcile` testable without network, define a fetcher trait and a real (reqwest) impl:

```rust
//! Fetch + reconcile: regenerate wire rows from live endpoints.

use anyhow::Result;
use std::collections::HashMap;
use zoid_model::{ModelEntry, ModelInfo, Registry, Source, WireShape};

/// A report of what reconcile did (and what it left for a human).
#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub added: Vec<(String, String)>,      // (provider, model)
    pub updated: Vec<(String, String)>,
    pub removed: Vec<(String, String)>,
    pub reported: Vec<String>,             // human-actionable notes
    pub skipped: Vec<String>,              // providers skipped (no key / error)
}

/// Seam for fetching live lists + caps (mockable in tests).
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    async fn list(&self, provider: &str, base_url: &str, key: &str) -> Result<Vec<String>>;
    async fn caps(&self, provider: &str, base_url: &str, key: &str, model: &str) -> Result<Option<ModelInfo>>;
}

pub struct ReqwestFetcher;
#[async_trait::async_trait]
impl Fetcher for ReqwestFetcher {
    async fn list(&self, p: &str, b: &str, k: &str) -> Result<Vec<String>> { crate::fetch::list_models(p, b, k).await }
    async fn caps(&self, p: &str, b: &str, k: &str, m: &str) -> Result<Option<ModelInfo>> { crate::fetch::caps(p, b, k, m).await }
}

/// Reconcile the registry against live endpoints. `keys` maps provider id → key.
/// Only Ollama and Gemini produce `wire` rows; other providers are reported only.
pub async fn reconcile(
    reg: &Registry,
    keys: &HashMap<String, String>,
    fetcher: &dyn Fetcher,
) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    let wire_capable = |id: &str| id == "ollama-cloud" || id == "ollama-local" || id == "gemini-api";

    for p in &reg.providers {
        let Some(key) = keys.get(&p.id) else {
            report.skipped.push(format!("{}: no key", p.id));
            continue;
        };
        let base_url = match &p.transport {
            zoid_model::Transport::Http { default_base_url } => default_base_url.clone(),
            _ => { report.skipped.push(format!("{}: non-HTTP", p.id)); continue; }
        };
        let live = match fetcher.list(&p.id, &base_url, key).await {
            Ok(l) => l,
            Err(e) => { report.skipped.push(format!("{}: fetch error: {e}", p.id)); continue; }
        };
        let live_lower: Vec<String> = live.iter().map(|s| s.to_ascii_lowercase()).collect();

        if wire_capable(&p.id) {
            // add new wire rows
            for id in &live {
                let exists = p.models.iter().any(|m| m.id.to_ascii_lowercase() == id.to_ascii_lowercase());
                if !exists {
                    report.added.push((p.id.clone(), id.clone()));
                }
            }
            // remove wire rows absent from live
            for m in &p.models {
                if m.source == Source::Wire && !live_lower.contains(&m.id.to_ascii_lowercase()) {
                    report.removed.push((p.id.clone(), m.id.clone()));
                }
            }
        } else {
            // report-only: new live models and absent static/user models
            for id in &live {
                let exists = p.models.iter().any(|m| m.id.to_ascii_lowercase() == id.to_ascii_lowercase());
                if !exists {
                    report.reported.push(format!("{}: new model {} (needs manual caps)", p.id, id));
                }
            }
            for m in &p.models {
                if !live_lower.contains(&m.id.to_ascii_lowercase()) {
                    report.reported.push(format!("{}: model {} absent from live (static/user, not removed)", p.id, m.id));
                }
            }
        }
    }
    Ok(report)
}
```

- [ ] **Step 4: Write a mock-fetcher test and run it**

```rust
struct MockFetcher { lists: HashMap<String, Vec<String>> }
#[async_trait::async_trait]
impl Fetcher for MockFetcher {
    async fn list(&self, p: &str, _b: &str, _k: &str) -> Result<Vec<String>> { Ok(self.lists.get(p).cloned().unwrap_or_default()) }
    async fn caps(&self, _p: &str, _b: &str, _k: &str, _m: &str) -> Result<Option<ModelInfo>> { Ok(None) }
}

#[tokio::test]
async fn reconcile_adds_wire_for_ollama_reports_for_anthropic() {
    let reg = /* build a registry with ollama-cloud (1 static model) and anthropic-api (1 static model) */;
    let mut lists = HashMap::new();
    lists.insert("ollama-cloud".to_string(), vec!["glm-5.2:cloud".to_string(), "new-cloud-model".to_string()]);
    lists.insert("anthropic-api".to_string(), vec!["claude-sonnet-4-6".to_string(), "new-anthropic-model".to_string()]);
    let keys = HashMap::from([("ollama-cloud".to_string(), "k".to_string()), ("anthropic-api".to_string(), "k".to_string())]);
    let report = reconcile(&reg, &keys, &MockFetcher { lists }).await.unwrap();
    assert!(report.added.contains(&("ollama-cloud".to_string(), "new-cloud-model".to_string())));
    assert!(report.reported.iter().any(|s| s.contains("new-anthropic-model")));
    assert!(!report.added.iter().any(|(p, _)| p == "anthropic-api"));
}
```

Run: `cargo test -p zoid-registry refresh`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-registry/src/refresh.rs
git commit -m "feat(zoid-registry): reconcile logic with fetcher seam"
```

---

### Task 14: `zoid refresh-models` subcommand + skill repurpose

**Files:**
- Modify: `crates/zoid/src/main.rs` (add subcommand)
- Modify: `crates/zoid-core/src/skill.rs` (repurpose skill body)

**Interfaces:**
- Consumes: `zoid_registry::refresh::{reconcile, ReqwestFetcher}`, `zoid_registry::load`.
- Produces: `zoid refresh-models` CLI that fetches, reconciles, writes `models.user.toml`, and prints the report.

- [ ] **Step 1: Add the subcommand**

In `main.rs`, before the TUI starts, check `std::env::args()` for `refresh-models`. If present, run the tool and exit:

```rust
if std::env::args().any(|a| a == "refresh-models") {
    return run_refresh_models();
}

async fn run_refresh_models() -> anyhow::Result<()> {
    let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
    let shipped = cfg_dir.join("models.toml");
    let user = cfg_dir.join("models.user.toml");
    let (reg, _warn) = zoid_registry::load(&shipped, &user)?;

    // Resolve keys via env → secret store (same precedence as select_provider).
    let secret_key = resolve_secret_key_path(|k| std::env::var(k).ok());
    let secrets = zoid_core::secret::EncryptedDb::open(&secret_key.to_string_lossy(), &secret_key).ok();
    let mut keys = std::collections::HashMap::new();
    for p in &reg.providers {
        if let Some(env) = &p.key_env {
            if let Ok(v) = std::env::var(env) {
                if !v.is_empty() { keys.insert(p.id.clone(), v); continue; }
            }
            if let Some(s) = &secrets {
                use zoid_core::secret::SecretStore;
                if let Some(v) = s.get(env) { keys.insert(p.id.clone(), v); }
            }
        }
    }

    let report = zoid_registry::refresh::reconcile(&reg, &keys, &zoid_registry::refresh::ReqwestFetcher).await?;
    // Write wire rows to models.user.toml (preserving user rows) — see Step 3.
    println!("{report:#?}");
    Ok(())
}
```

- [ ] **Step 2: Write the user-file writer**

Add `zoid_registry::refresh::write_user_file(user_path, &reg, &report)` that serializes the `wire` rows (added/updated) and preserves existing `user` rows. This is the inverse of `parse_user` — serialize `ModelEntry` with `source = "wire"`/`"user"` back to TOML. Add a `serialize` module or a `ToToml` impl in `raw.rs`.

- [ ] **Step 3: Repurpose the skill**

Replace `REFRESHING_PROVIDER_MODELS_BODY` in `crates/zoid-core/src/skill.rs` with a slim pointer:

```rust
const REFRESHING_PROVIDER_MODELS_BODY: &str = concat!(
    "# Refreshing Provider Models\n\n",
    "The provider/model registry is a TOML file, not Rust code. To refresh it,\n",
    "run the built-in tool for the user:\n\n",
    "```bash\n",
    "zoid refresh-models\n",
    "```\n\n",
    "This fetches live model lists from each provider that has a key, adds/updates\n",
    "`wire` rows (Ollama + Gemini only), removes retired `wire` rows, and reports\n",
    "(never deletes) `static`/`user` rows that are absent live. It writes results\n",
    "to `models.user.toml`.\n\n",
    "After running it, report the diff to the user: which models were added,\n",
    "updated, removed, and which need manual attention (new models on providers\n",
    "without wire-derived caps).\n\n",
    "Do NOT hand-edit the registry TOML to add models — run the tool instead.\n",
);
```

Update the skill's `description` to match ("run `zoid refresh-models` to refresh the provider/model registry").

- [ ] **Step 4: Run tests**

Run: `cargo test -p zoid-core` and `cargo test -p zoid`
Expected: PASS (update the skill-body test at `skill.rs:464` if it asserts the old body).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid-core/src/skill.rs crates/zoid-registry/src/refresh.rs crates/zoid-registry/src/raw.rs
git commit -m "feat: zoid refresh-models subcommand + repurpose skill"
```

---

## Phase 4 — Gemini + local-model unification

### Task 15: Add `gemini-api` provider entry

**Files:**
- Modify: `crates/zoid-model/models.toml`

**Interfaces:**
- Produces: a 7th selectable provider `gemini-api` with the three Gemini models as `static` rows.

- [ ] **Step 1: Add the provider to `models.toml`**

Append:

```toml
[[provider]]
id = "gemini-api"
display = "gemini · api key"
family = "gemini"
transport = { kind = "http", default_base_url = "https://generativelanguage.googleapis.com" }
status = "available"
key_url = "https://aistudio.google.com/app/apikey"
key_env = "GEMINI_API_KEY"

  [[provider.model]]
  id = "gemini-3.5-flash"
  wire_shape = "google-gemini"
  source = "static"
  default = true
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = false
  thinking = "toggle"
  thinking_wire = "none"

  [[provider.model]]
  id = "gemini-3.1-pro"
  wire_shape = "google-gemini"
  source = "static"
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = false
  thinking = "toggle"
  thinking_wire = "none"

  [[provider.model]]
  id = "gemini-3-flash"
  wire_shape = "google-gemini"
  source = "static"
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = false
  thinking = "toggle"
  thinking_wire = "none"
```

- [ ] **Step 2: Update the invariant test count**

In `crates/zoid-registry/src/parse.rs`, change `assert_eq!(ids.len(), 6)` to `assert_eq!(ids.len(), 7)` and add `"gemini-api"` to the membership list.

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid-registry`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-model/models.toml crates/zoid-registry/src/parse.rs
git commit -m "feat: add gemini-api as a first-class provider"
```

---

### Task 16: Drop SQLite local-model seeding

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (delete `seed_local_models` + `local_models` table + test helpers)
- Modify: `crates/zoid-core/src/session.rs` (delete `seed_local_models` handle method + wiring)
- Modify: `crates/zoid/src/main.rs` (delete the boot-time `seed_local_models` call)
- Delete: `crates/zoid-model/src/local_seed.rs`

**Interfaces:**
- Consumes: nothing new (provisioning now reads the registry's `ollama-local` rows).
- Produces: removal of the seed-only SQLite table and its callers.

- [ ] **Step 1: Delete `local_seed.rs`**

Run: `rm crates/zoid-model/src/local_seed.rs` and remove `pub mod local_seed;` from `crates/zoid-model/src/lib.rs`.

- [ ] **Step 2: Delete `seed_local_models` from `store.rs`**

Remove the `seed_local_models` method (lines 179–295) and the test-only helpers `local_model_count`/`local_model_source` (lines 297–320), plus their tests (lines 1743–1840).

- [ ] **Step 3: Delete the session wiring**

In `crates/zoid-core/src/session.rs`, remove the `seed_local_models` handle method (lines 373–380) and the `seed_local_models_via_handle_creates_and_seeds` test (lines 822–840).

- [ ] **Step 4: Delete the boot call**

In `crates/zoid/src/main.rs`, remove the `session.seed_local_models().await` call (lines 2363–2368).

- [ ] **Step 5: Build and test**

Run: `cargo build --workspace 2>&1 | head -40` then `cargo test -p zoid-core -p zoid`
Expected: PASS (no remaining references to `local_seed`/`seed_local_models`).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: drop SQLite local-model seeding; read provisioning from registry"
```

---

### Task 17: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast`
Expected: PASS.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -20`
Expected: no new warnings.

- [ ] **Step 3: Manual smoke**

Run: `cargo run -p zoid -- refresh-models` (with no keys set)
Expected: prints a report with every provider skipped ("no key"), exits cleanly.

- [ ] **Step 4: Commit any final fixes**

```bash
git add -A && git commit -m "chore: final verification fixes" || echo "nothing to commit"
```

---

## Self-Review Notes

**Spec coverage:** §1 (data model/file format) → Tasks 1–6; §2 (crate layout/owned migration) → Tasks 1–3, 7–11; §3 (refresh tool) → Tasks 12–14; §4 (Gemini) → Task 15; §5 (local-model) → Task 16; §6 (migration) → Tasks 6, 11; §7 (error handling) → Tasks 4, 5, 13; §8 (testing) → each task's test steps + Task 17.

**Known follow-ups (not in this plan, flagged for the implementer):** the `default_provider` `reg` parameter is currently unused (env-driven provider selection is unchanged); the `gemini-api` `select_provider` arm is added in Task 9 but only becomes reachable once Task 15 ships the registry entry. Both are intentional and covered by the phase ordering.
