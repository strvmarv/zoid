# Settings Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the cramped stacked settings card with a full-screen three-column (Sections | Fields | contextual picker) config page, backed by a transport-aware provider registry that owns default endpoints and supports explicit provider flavors.

**Architecture:** The `zoid-provider` registry (`model.rs`) grows from a flat `&[&str]` into a structured `ProviderEntry` table carrying `transport` (Http/Cli/Sdk) and `status` (Available/Planned); the registry becomes the single source of truth for default endpoints and resolves legacy ids via an alias map. The TUI config screen (`render_config`) is rewritten from a single centered `Paragraph` into a full-frame horizontal `Layout` whose third column is a contextual picker that appears only for list-valued fields (provider/model). An `Alt+P` quick-switch overlay reuses the same picker/registry.

**Tech Stack:** Rust 2021 workspace, ratatui 0.29, crossterm 0.28, insta snapshots.

## Global Constraints

- Minimum supported window size: **160×40** (snapshot/test baseline). Below baseline: degrade gracefully, never blank.
- Provider IDs are hyphenated `family-variant` slugs; code reads `ProviderEntry` struct fields, never substring-parses the id.
- Legacy alias map (verbatim): `ollama` → `ollama-cloud`, `anthropic` → `anthropic-api`.
- `[planned]` provider entries are visible in the picker but NOT selectable (skipped by cursor movement).
- Registry is the single source of truth for default endpoints; provider constructors read from it.
- All UI glyphs/colors come from `tokens` (§16 token purity) — no untokenized literals in render code.
- Never add a `Co-Authored-By` / co-author trailer to commit messages (user global rule).
- Registry `models` lists are a **fallback only** — the model picker fetches the live list via `Provider::list_models()` (Ollama `/api/tags`, Anthropic `/v1/models`); no hardcoded model list is authoritative.
- Selecting a key-requiring provider with no key present prompts for the key (stored via the secret store) BEFORE fetching models. `ollama-local` requires no key.
- Model-list fetch failures are logged, never fatal — the picker keeps the fallback list.
- OUT of scope: implementing the `anthropic-cli` subprocess provider and `anthropic-sdk` (seam + `[planned]` entries only).

---

## Phase 1 — Provider registry (zoid-provider)

### Task 1: Structured provider registry with transport + status + aliases

**Files:**
- Modify: `crates/zoid-provider/src/model.rs` (replace `KNOWN_PROVIDERS` + `models_for`; keep `ModelInfo`/`model_info`)
- Modify: `crates/zoid-tui/src/config_view.rs:56` (drops `KNOWN_PROVIDERS` reference — handled in Task 4; note here so Task 1 compiles the crate in isolation by leaving a temporary shim, see Step 3)

**Interfaces:**
- Produces:
  - `pub enum Transport { Http { default_base_url: &'static str }, Cli { default_command: &'static str }, Sdk }`
  - `pub enum Status { Available, Planned }`
  - `pub struct ProviderEntry { pub id, pub display, pub family, pub transport, pub models, pub status: &'static str/... }` (see code)
  - `pub const PROVIDERS: &[ProviderEntry]`
  - `pub fn canonical_id(raw: &str) -> &str`
  - `pub fn entry(id: &str) -> Option<&'static ProviderEntry>`
  - `pub fn models_for(provider: &str) -> &'static [&'static str]`
  - `pub fn default_base_url(provider: &str) -> Option<&'static str>`
  - `pub fn selectable() -> impl Iterator<Item = &'static ProviderEntry>`

- [ ] **Step 1: Write the failing tests**

Replace the existing `known_providers_and_models` test in `crates/zoid-provider/src/model.rs` with these (keep `model_info_caps_by_family_else_default` unchanged):

```rust
    #[test]
    fn canonical_id_maps_legacy_aliases() {
        assert_eq!(canonical_id("ollama"), "ollama-cloud");
        assert_eq!(canonical_id("anthropic"), "anthropic-api");
        assert_eq!(canonical_id("ollama-local"), "ollama-local"); // pass-through
        assert_eq!(canonical_id("unknown"), "unknown");
    }

    #[test]
    fn entry_resolves_through_alias_and_transport() {
        let e = entry("ollama").unwrap(); // legacy → ollama-cloud
        assert_eq!(e.id, "ollama-cloud");
        assert_eq!(e.family, "ollama");
        assert_eq!(e.transport, Transport::Http { default_base_url: "https://ollama.com" });

        let local = entry("ollama-local").unwrap();
        assert_eq!(local.transport, Transport::Http { default_base_url: "http://localhost:11434" });

        let cli = entry("anthropic-cli").unwrap();
        assert_eq!(cli.transport, Transport::Cli { default_command: "claude" });
        assert_eq!(cli.status, Status::Planned);
    }

    #[test]
    fn models_for_by_id_and_alias() {
        assert_eq!(models_for("ollama"), &["glm-5.2:cloud"]); // alias → cloud
        assert_eq!(models_for("ollama-cloud"), &["glm-5.2:cloud"]);
        assert!(models_for("ollama-local").is_empty()); // local tags are free-text
        assert!(models_for("anthropic-api").contains(&"claude-sonnet-4-6"));
        assert!(models_for("nonexistent").is_empty());
    }

    #[test]
    fn default_base_url_only_for_http() {
        assert_eq!(default_base_url("anthropic-api"), Some("https://api.anthropic.com"));
        assert_eq!(default_base_url("anthropic-cli"), None); // Cli has no url
        assert_eq!(default_base_url("anthropic-sdk"), None);
    }

    #[test]
    fn selectable_excludes_planned() {
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert!(ids.contains(&"ollama-local"));
        assert!(ids.contains(&"ollama-cloud"));
        assert!(ids.contains(&"anthropic-api"));
        assert!(!ids.contains(&"anthropic-cli"));
        assert!(!ids.contains(&"anthropic-sdk"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider model::`
Expected: FAIL — `Transport`, `Status`, `canonical_id`, `entry`, `default_base_url`, `selectable` not found.

- [ ] **Step 3: Implement the registry**

In `crates/zoid-provider/src/model.rs`, replace the `KNOWN_PROVIDERS` const and the `models_for` fn (lines 14–26) with:

```rust
/// How a provider entry is reached. Http/Cli carry their default connection
/// value; Sdk has none (ambient auth). This is the growth seam for new
/// transports (spec 2026-07-03-settings-redesign).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Http { default_base_url: &'static str },
    Cli { default_command: &'static str },
    Sdk,
}

/// Whether an entry is implemented (selectable) or a visible-but-inert seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    Planned,
}

/// One provider flavor. `id` is a stable hyphenated `family-variant` key;
/// code reads these fields, never substring-parses `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEntry {
    pub id: &'static str,
    pub display: &'static str,
    pub family: &'static str,
    pub transport: Transport,
    pub models: &'static [&'static str],
    pub status: Status,
}

/// The provider registry. Order is the picker display order.
pub const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        id: "ollama-local",
        display: "ollama · local",
        family: "ollama",
        transport: Transport::Http { default_base_url: "http://localhost:11434" },
        models: &[], // local tags are arbitrary; free-text entry
        status: Status::Available,
    },
    ProviderEntry {
        id: "ollama-cloud",
        display: "ollama · cloud",
        family: "ollama",
        transport: Transport::Http { default_base_url: "https://ollama.com" },
        models: &["glm-5.2:cloud"],
        status: Status::Available,
    },
    ProviderEntry {
        id: "anthropic-api",
        display: "anthropic · api key",
        family: "anthropic",
        transport: Transport::Http { default_base_url: "https://api.anthropic.com" },
        models: &["claude-sonnet-4-6", "claude-opus-4-8"],
        status: Status::Available,
    },
    ProviderEntry {
        id: "anthropic-cli",
        display: "anthropic · Claude Code CLI",
        family: "anthropic",
        transport: Transport::Cli { default_command: "claude" },
        models: &["claude-sonnet-4-6", "claude-opus-4-8"],
        status: Status::Planned,
    },
    ProviderEntry {
        id: "anthropic-sdk",
        display: "anthropic · SDK",
        family: "anthropic",
        transport: Transport::Sdk,
        models: &["claude-sonnet-4-6", "claude-opus-4-8"],
        status: Status::Planned,
    },
];

/// Resolve a stored/legacy provider id to its canonical registry id.
/// Preserves today's behavior: bare `ollama` meant the cloud endpoint.
pub fn canonical_id(raw: &str) -> &str {
    match raw {
        "ollama" => "ollama-cloud",
        "anthropic" => "anthropic-api",
        other => other,
    }
}

/// The registry entry for a provider id (resolving legacy aliases).
pub fn entry(id: &str) -> Option<&'static ProviderEntry> {
    let id = canonical_id(id);
    PROVIDERS.iter().find(|e| e.id == id)
}

/// Known model ids for a provider (first = default). Empty for free-text-only
/// providers (local Ollama) or unknown ids. Resolves legacy aliases.
pub fn models_for(provider: &str) -> &'static [&'static str] {
    entry(provider).map(|e| e.models).unwrap_or(&[])
}

/// The registry default base URL for an HTTP-transport provider, else `None`.
pub fn default_base_url(provider: &str) -> Option<&'static str> {
    match entry(provider).map(|e| e.transport) {
        Some(Transport::Http { default_base_url }) => Some(default_base_url),
        _ => None,
    }
}

/// Iterator over selectable (Available) entries — `[planned]` excluded.
pub fn selectable() -> impl Iterator<Item = &'static ProviderEntry> {
    PROVIDERS.iter().filter(|e| e.status == Status::Available)
}
```

To keep `zoid-tui` compiling until Task 4 rewrites it, add a temporary back-compat const at the end of the non-test section (removed in Task 4):

```rust
/// TEMPORARY back-compat shim for config_view; removed in the settings redesign
/// Task 4 once the picker replaces the provider Cycle field.
pub const KNOWN_PROVIDERS: &[&str] = &["ollama-local", "ollama-cloud", "anthropic-api"];
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider model::`
Expected: PASS (5 new tests + `model_info_caps_by_family_else_default`).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/model.rs
git commit -m "feat(provider): structured registry with transport, status, and legacy aliases"
```

---

### Task 2: Provider constructors read default endpoint from the registry

**Files:**
- Modify: `crates/zoid-provider/src/ollama.rs:144-150` (`new`)
- Modify: `crates/zoid-provider/src/anthropic.rs:104-110` (`new`)

**Interfaces:**
- Consumes: `crate::model::default_base_url(&str) -> Option<&'static str>` (Task 1)
- Produces: unchanged public signatures; only the seeded default changes source.

- [ ] **Step 1: Confirm the existing default tests express the invariant**

The existing tests already assert the defaults (`ollama.rs:240 new_uses_default_base_url` expects `"https://ollama.com"`; `anthropic.rs:182` expects `"https://api.anthropic.com"`). These become the regression guard that the registry value matches. No new test needed; they must keep passing after the change.

- [ ] **Step 2: Point `OllamaProvider::new` at the registry**

In `crates/zoid-provider/src/ollama.rs`, change the `base_url` initializer inside `new` (line 147) from the literal to the registry value (bare ollama historically = cloud):

```rust
            base_url: crate::model::default_base_url("ollama-cloud")
                .unwrap_or("https://ollama.com")
                .to_string(),
```

- [ ] **Step 3: Point `AnthropicProvider::new` at the registry**

In `crates/zoid-provider/src/anthropic.rs`, change the `base_url` initializer inside `new` (line 107):

```rust
            base_url: crate::model::default_base_url("anthropic-api")
                .unwrap_or("https://api.anthropic.com")
                .to_string(),
```

- [ ] **Step 4: Run the provider tests**

Run: `cargo test -p zoid-provider`
Expected: PASS — `new_uses_default_base_url` (both) still green because the registry holds the same URLs; single source now.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/ollama.rs crates/zoid-provider/src/anthropic.rs
git commit -m "refactor(provider): constructors read default base_url from registry (single source)"
```

---

### Task 3: `select_provider` resolves canonical id + registry-seeded base_url

**Files:**
- Modify: `crates/zoid/src/main.rs:302-343` (`select_provider`)

**Interfaces:**
- Consumes: `zoid_provider::model::{canonical_id, entry, default_base_url}` (Task 1)
- Produces: `select_provider` unchanged signature `(config, secrets) -> (Arc<dyn Provider>, &'static str, bool)`; now family-driven with registry-seeded effective base_url.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid/src/main.rs` (in its `#[cfg(test)] mod tests`, or create one if none — search for an existing test module first). Test the pure resolution helper we will extract:

```rust
    #[test]
    fn effective_base_url_prefers_override_then_registry() {
        use zoid_core::config::Config;
        // No override → registry default for the canonical id.
        let mut c = Config::default(); // provider = "ollama" (legacy) → ollama-cloud
        c.base_url = None;
        assert_eq!(effective_base_url(&c), "https://ollama.com");

        // Explicit local id, no override → local endpoint.
        c.provider = "ollama-local".into();
        c.base_url = None;
        assert_eq!(effective_base_url(&c), "http://localhost:11434");

        // Override wins over registry.
        c.base_url = Some("http://127.0.0.1:1234".into());
        assert_eq!(effective_base_url(&c), "http://127.0.0.1:1234");

        // Blank override falls back to registry.
        c.base_url = Some("   ".into());
        assert_eq!(effective_base_url(&c), "http://localhost:11434");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p zoid effective_base_url_prefers_override_then_registry`
Expected: FAIL — `effective_base_url` not found.

- [ ] **Step 3: Extract the helper and rewrite `select_provider` to be family-driven**

In `crates/zoid/src/main.rs`, add above `select_provider`:

```rust
/// The base URL to hand a provider: an explicit non-blank config override wins,
/// else the registry default for the (canonicalized) provider id, else empty
/// (which `with_base_url` treats as "keep the built-in default").
fn effective_base_url(config: &zoid_core::config::Config) -> String {
    if let Some(u) = config.base_url.as_ref() {
        if !u.trim().is_empty() {
            return u.clone();
        }
    }
    zoid_provider::model::default_base_url(&config.provider)
        .map(str::to_string)
        .unwrap_or_default()
}
```

Then replace the body of `select_provider` (the `let base_url = ...; match config.provider.as_str() { ... }` block, lines 322–342) with a family-driven match:

```rust
    let base_url = effective_base_url(config);
    let family = zoid_provider::model::entry(&config.provider)
        .map(|e| e.family)
        .unwrap_or("ollama");
    match family {
        "anthropic" => match key_for("ANTHROPIC_API_KEY") {
            Some(k) => (
                Arc::new(
                    zoid_provider::anthropic::AnthropicProvider::new(k).with_base_url(base_url),
                ),
                "anthropic",
                true,
            ),
            None => (default_provider(), "anthropic", false),
        },
        _ => match key_for("OLLAMA_API_KEY") {
            Some(k) => (
                Arc::new(zoid_provider::ollama::OllamaProvider::new(k).with_base_url(base_url)),
                "ollama",
                true,
            ),
            None => (default_provider(), "ollama", false),
        },
    }
```

- [ ] **Step 4: Run tests + full provider/core suites**

Run: `cargo test -p zoid effective_base_url_prefers_override_then_registry && cargo test -p zoid-provider -p zoid-core`
Expected: PASS. Legacy `provider = "ollama"` still resolves to the ollama family + cloud endpoint.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(provider): select_provider resolves canonical id + registry-seeded base_url"
```

---

## Phase 2 — View-model (zoid-tui config_view)

### Task 4: Transport-aware Provider & Model section with picker fields

**Files:**
- Modify: `crates/zoid-tui/src/config_view.rs`
- Modify: `crates/zoid-provider/src/model.rs` (remove the temporary `KNOWN_PROVIDERS` shim from Task 1)

**Interfaces:**
- Consumes: `zoid_provider::model::{entry, selectable, models_for, Transport, ProviderEntry, Status}` (Task 1)
- Produces:
  - `FieldKind::Pick` variant (replaces `Cycle`) marking a field that opens the col-3 picker.
  - `pub struct PickOption { pub id: &'static str, pub label: String, pub detail: String, pub selectable: bool, pub is_current: bool }`
  - `pub fn provider_options(current_id: &str) -> Vec<PickOption>`
  - `pub fn model_options(provider_id: &str, current_model: &str) -> Vec<PickOption>`
  - `FieldRow` unchanged shape; provider/model rows carry `FieldKind::Pick`, the connection row label derives from the active provider's transport.

- [ ] **Step 1: Write the failing tests**

Replace the `builds_four_sections_with_env_shadow` provider/model assertions and add new ones in `crates/zoid-tui/src/config_view.rs` tests:

```rust
    #[test]
    fn provider_options_annotate_endpoints_and_mark_planned() {
        let opts = provider_options("ollama-cloud");
        let cloud = opts.iter().find(|o| o.id == "ollama-cloud").unwrap();
        assert!(cloud.is_current);
        assert!(cloud.selectable);
        assert!(cloud.detail.contains("https://ollama.com"));

        let cli = opts.iter().find(|o| o.id == "anthropic-cli").unwrap();
        assert!(!cli.selectable); // planned
        assert!(cli.detail.contains("claude")); // command shown as its endpoint
        assert!(cli.label.contains("planned") || cli.detail.contains("planned"));
    }

    #[test]
    fn model_options_list_registry_models() {
        let opts = model_options("anthropic-api", "claude-opus-4-8");
        assert!(opts.iter().any(|o| o.id == "claude-sonnet-4-6" && o.selectable));
        let cur = opts.iter().find(|o| o.id == "claude-opus-4-8").unwrap();
        assert!(cur.is_current);
    }

    #[test]
    fn provider_and_model_rows_are_pick_kind() {
        let cfg = Config::default();
        let prov = Provenance { /* all Default */
            provider: Source::Default, base_url: Source::Default, model: Source::Default,
            context_ceiling: Source::Default, auto_evict_cold: Source::Default,
            compact_threshold_pct: Source::Default, token_ceiling: Source::Default,
            reduced_motion: Source::Default,
        };
        let sections = build_sections(&cfg, &prov, &[]);
        let pm = &sections[0];
        assert_eq!(pm.rows[0].label, "provider");
        assert!(matches!(pm.rows[0].kind, FieldKind::Pick));
        assert_eq!(pm.rows[1].label, "model");
        assert!(matches!(pm.rows[1].kind, FieldKind::Pick));
        // Active provider is HTTP → connection row is base_url.
        assert_eq!(pm.rows[2].label, "base_url");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui config_view::`
Expected: FAIL — `FieldKind::Pick`, `PickOption`, `provider_options`, `model_options` not found.

- [ ] **Step 3: Implement Pick kind + option builders + transport-aware connection row**

In `crates/zoid-tui/src/config_view.rs`:

Change `FieldKind` (line 9) — replace `Cycle(&'static [&'static str])` with `Pick`:

```rust
pub enum FieldKind {
    Text,
    Uint,
    Bool,
    /// Opens the col-3 contextual picker (provider / model).
    Pick,
    Secret,
}
```

Add the option type + builders (top of file after imports):

```rust
use zoid_provider::model::{self, Status, Transport};

/// One row in the col-3 picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickOption {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub selectable: bool,
    pub is_current: bool,
}

/// The provider picker options (all registry entries; `[planned]` shown but
/// not selectable), each annotated with its transport endpoint/command.
pub fn provider_options(current_id: &str) -> Vec<PickOption> {
    let cur = model::canonical_id(current_id);
    model::PROVIDERS
        .iter()
        .map(|e| {
            let (kind, endpoint) = match e.transport {
                Transport::Http { default_base_url } => ("http", default_base_url.to_string()),
                Transport::Cli { default_command } => ("cli", default_command.to_string()),
                Transport::Sdk => ("sdk", "—".to_string()),
            };
            let planned = e.status == Status::Planned;
            let mut detail = format!("{kind}  {endpoint}");
            if planned {
                detail.push_str("  planned");
            }
            PickOption {
                id: e.id.to_string(),
                label: e.display.to_string(),
                detail,
                selectable: !planned,
                is_current: e.id == cur,
            }
        })
        .collect()
}

/// The model picker options for a provider (registry convenience list).
pub fn model_options(provider_id: &str, current_model: &str) -> Vec<PickOption> {
    model::models_for(provider_id)
        .iter()
        .map(|m| PickOption {
            id: (*m).to_string(),
            label: (*m).to_string(),
            detail: String::new(),
            selectable: true,
            is_current: *m == current_model,
        })
        .collect()
}
```

Update the `provider_model` section builder (lines 50–78): make `provider` and `model` `FieldKind::Pick`, and derive the connection row from the active provider's transport:

```rust
    let active = model::entry(&cfg.provider);
    let connection_row = match active.map(|e| e.transport) {
        Some(Transport::Cli { .. }) => FieldRow {
            label: "command",
            value: cfg.base_url.clone().unwrap_or_default(), // reuses base_url slot until CLI impl adds `command`
            kind: FieldKind::Text,
            source: prov.base_url,
            env_shadowed: prov.base_url == Source::Env,
        },
        // Http (and Sdk, which simply shows an empty base_url) → base_url row.
        _ => FieldRow {
            label: "base_url",
            value: cfg.base_url.clone().unwrap_or_default(),
            kind: FieldKind::Text,
            source: prov.base_url,
            env_shadowed: prov.base_url == Source::Env,
        },
    };
    let provider_model = Section {
        title: "Provider & Model".into(),
        rows: vec![
            FieldRow {
                label: "provider",
                value: cfg.provider.clone(),
                kind: FieldKind::Pick,
                source: prov.provider,
                env_shadowed: prov.provider == Source::Env,
            },
            FieldRow {
                label: "model",
                value: cfg.model.clone(),
                kind: FieldKind::Pick,
                source: prov.model,
                env_shadowed: prov.model == Source::Env,
            },
            connection_row,
        ],
    };
```

Finally, remove the temporary `KNOWN_PROVIDERS` shim from `crates/zoid-provider/src/model.rs` (added in Task 1 Step 3).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-tui config_view:: && cargo build -p zoid-tui`
Expected: PASS. (Compile errors in `route.rs`/`render.rs` referencing `FieldKind::Cycle` are addressed in Tasks 7 & 9; if the crate fails to build here due to those, proceed — Task 7 fixes routing, Task 9 fixes render. To keep this task self-contained, temporarily replace any `FieldKind::Cycle(_)` match arm in `route.rs`/`render.rs` with `FieldKind::Pick` as a stub; Tasks 7/9 supersede.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/config_view.rs crates/zoid-provider/src/model.rs
git commit -m "feat(config-view): Pick fields + provider/model picker options + transport-aware connection row"
```

---

## Phase 3 — State + routing (zoid-tui)

### Task 5: ShellState column focus + picker state

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (add fields to `ShellState` + `new()`)

**Interfaces:**
- Produces on `ShellState`:
  - `pub enum ConfigCol { Fields, Picker }` (Sections is reached via Tab, not a focus column)
  - `pub config_col: ConfigCol`
  - `pub config_picker: Vec<crate::config_view::PickOption>` (populated when a Pick field drills open; empty = closed)
  - `pub config_picker_sel: usize`
  - Helper `ShellState::config_picker_open(&self) -> bool` = `!self.config_picker.is_empty()`

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-tui/src/state.rs` tests:

```rust
    #[test]
    fn config_picker_defaults_closed() {
        let s = ShellState::new();
        assert!(matches!(s.config_col, ConfigCol::Fields));
        assert!(!s.config_picker_open());
        assert_eq!(s.config_picker_sel, 0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui config_picker_defaults_closed`
Expected: FAIL — `ConfigCol`, `config_col`, `config_picker_open` not found.

- [ ] **Step 3: Add the state**

In `crates/zoid-tui/src/state.rs`, add near the other config fields:

```rust
/// Which column has focus inside the config overlay. Sections are switched with
/// Tab (not a focusable column); focus moves between the field list and the
/// contextual picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigCol {
    Fields,
    Picker,
}
```

Add fields to `struct ShellState` (after `config_sections`):

```rust
    /// Focused column in the config overlay (fields vs the drilled-open picker).
    pub config_col: ConfigCol,
    /// The open col-3 picker options; empty when no picker is drilled open.
    pub config_picker: Vec<crate::config_view::PickOption>,
    /// Highlighted row within the open picker.
    pub config_picker_sel: usize,
```

Initialize in `new()` (after `config_sections: Vec::new(),`):

```rust
            config_col: ConfigCol::Fields,
            config_picker: Vec::new(),
            config_picker_sel: 0,
```

Add the helper in `impl ShellState`:

```rust
    /// True when the col-3 contextual picker is drilled open.
    pub fn config_picker_open(&self) -> bool {
        !self.config_picker.is_empty()
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-tui config_picker_defaults_closed`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(state): config overlay column focus + picker state"
```

---

### Task 6: Config Actions + routing for drill / select / back

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (`Action` enum, `route_config_key`, remove `config_value_change`)

**Interfaces:**
- Consumes: `ShellState::{config_col, config_picker, config_picker_sel, config_picker_open}` (Task 5), `FieldKind::Pick` (Task 4)
- Produces new `Action`s: `ConfigDrillOpen`, `ConfigPickerMove(i32)`, `ConfigPickerSelect`, `ConfigPickerBack`. Removes `ConfigCycle(i32)`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/zoid-tui/src/route.rs` tests (create module if absent):

```rust
    use crate::config_view::{FieldKind, FieldRow, Section, PickOption};
    use zoid_core::config::Source;

    fn state_on_provider() -> ShellState {
        let mut s = ShellState::new();
        s.overlay = Overlay::Config;
        s.config_sections = vec![Section {
            title: "Provider & Model".into(),
            rows: vec![FieldRow {
                label: "provider", value: "ollama-cloud".into(),
                kind: FieldKind::Pick, source: Source::Default, env_shadowed: false,
            }],
        }];
        s
    }

    #[test]
    fn enter_on_pick_field_drills_open() {
        let s = state_on_provider();
        let a = route_config_key(&s, key(KeyCode::Enter));
        assert!(matches!(a, Action::ConfigDrillOpen));
    }

    #[test]
    fn picker_open_routes_movement_and_select() {
        let mut s = state_on_provider();
        s.config_col = ConfigCol::Picker;
        s.config_picker = vec![PickOption {
            id: "ollama-local".into(), label: "ollama · local".into(),
            detail: String::new(), selectable: true, is_current: false,
        }];
        assert!(matches!(route_config_key(&s, key(KeyCode::Down)), Action::ConfigPickerMove(1)));
        assert!(matches!(route_config_key(&s, key(KeyCode::Enter)), Action::ConfigPickerSelect));
        assert!(matches!(route_config_key(&s, key(KeyCode::Esc)), Action::ConfigPickerBack));
        assert!(matches!(route_config_key(&s, key(KeyCode::Left)), Action::ConfigPickerBack));
    }
```

Add a `key` helper if none exists in the module:

```rust
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui enter_on_pick_field_drills_open picker_open_routes_movement_and_select`
Expected: FAIL — new actions not found.

- [ ] **Step 3: Add the actions and rewrite `route_config_key`**

In `crates/zoid-tui/src/route.rs`, in the `Action` enum, remove `ConfigCycle(i32)` (line 61) and add:

```rust
    ConfigDrillOpen,
    ConfigPickerMove(i32),
    ConfigPickerSelect,
    ConfigPickerBack,
```

Replace `route_config_key` (lines 219–271) and delete `config_value_change` (lines 273–284) with:

```rust
fn route_config_key(state: &ShellState, key: KeyEvent) -> Action {
    // 1. Picker column captures keys while a Pick field is drilled open.
    if state.config_col == ConfigCol::Picker && state.config_picker_open() {
        return match key.code {
            KeyCode::Up => Action::ConfigPickerMove(-1),
            KeyCode::Down => Action::ConfigPickerMove(1),
            KeyCode::Enter => Action::ConfigPickerSelect,
            KeyCode::Left | KeyCode::Esc => Action::ConfigPickerBack,
            _ => Action::Noop,
        };
    }

    // 2. Inline text edit buffer (Text/Uint/Secret fields).
    if state.config_edit.is_some() {
        return match key.code {
            KeyCode::Enter => Action::ConfigCommitEdit,
            KeyCode::Esc => Action::ConfigCancelEdit,
            KeyCode::Backspace => Action::ConfigEditBackspace,
            KeyCode::Char(c) => Action::ConfigEditChar(c),
            _ => Action::Noop,
        };
    }

    // 3. Field-list navigation.
    let kind = state
        .config_sections
        .get(state.config_section)
        .and_then(|s| s.rows.get(state.config_field))
        .map(|r| r.kind.clone());

    match key.code {
        KeyCode::Up => Action::ConfigMoveField(-1),
        KeyCode::Down => Action::ConfigMoveField(1),
        KeyCode::Tab => Action::ConfigMoveSection(1),
        KeyCode::BackTab => Action::ConfigMoveSection(-1),
        KeyCode::Esc => Action::CloseOverlay,
        // Right/Enter act on the focused field.
        KeyCode::Right | KeyCode::Enter => match kind {
            Some(FieldKind::Pick) => Action::ConfigDrillOpen,
            Some(FieldKind::Bool) => Action::ConfigToggle,
            Some(FieldKind::Text) | Some(FieldKind::Uint) => Action::ConfigBeginEdit,
            _ => Action::Noop,
        },
        KeyCode::Char('r') => Action::ConfigSaveToRepo,
        KeyCode::Char('x') => {
            if matches!(kind, Some(FieldKind::Secret)) {
                Action::ConfigClearSecret
            } else {
                Action::Noop
            }
        }
        _ => Action::Noop,
    }
}
```

Add the needed imports at the top of `route.rs` if not present: `use crate::state::ConfigCol;` and ensure `FieldKind` is imported.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-tui enter_on_pick_field_drills_open picker_open_routes_movement_and_select`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/route.rs
git commit -m "feat(route): config picker drill/move/select/back; drop blind Cycle"
```

---

### Task 7: Apply handlers — drill, select (seed + auto-advance), model select

**Files:**
- Modify: `crates/zoid/src/main.rs` (config action handlers; search for `Action::ConfigCycle` and the config action match arm)

**Interfaces:**
- Consumes: `Action::{ConfigDrillOpen, ConfigPickerMove, ConfigPickerSelect, ConfigPickerBack}` (Task 6); `config_view::{provider_options, model_options}` (Task 4); `ShellState` picker fields (Task 5)
- Produces: handler arms operating on `&mut App` that (a) open the correct picker on `app.shell`, (b) on provider select: write `provider` then seed `base_url` (both via `apply_config_write`), auto-advance focus to the model field and open the model picker, (c) on model select: write `model`. Persistence goes through the existing `apply_config_write(app, dotted_key, value, false)` (`main.rs:1021`) — the same helper `ConfigCycle`/`ConfigToggle` use; it writes one TOML key, reloads config, calls `refresh_config_sections(app)`, and re-runs `select_provider`. Never introduce a second persistence path.

**Ground truth (read before writing):** the config action arms live in `handle_action(app: &mut App, action)` (`main.rs:1068`). They mutate `app.shell` (the `ShellState`) and `app.config`, and persist via `apply_config_write(app, key, TomlValue, false)`. `current_config_field(app) -> Option<(&'static str label, FieldKind)>` gives the focused row. `apply_config_write` RELOADS config from disk, so seeding `base_url` must go through a `apply_config_write` call (a direct `app.config.base_url = ...` would be clobbered by the reload).

- [ ] **Step 1: Write the failing test (pure connection-write helper)**

`apply_config_write` needs a `TomlValue` for the seeded `base_url`: the registry default, or `Unset` (remove the key) for non-HTTP transports. Add this pure, testable helper's test to `crates/zoid/src/main.rs` tests:

```rust
    #[test]
    fn base_url_write_seeds_registry_default_or_unsets() {
        use zoid_core::config::TomlValue;
        assert_eq!(base_url_write_for("ollama-local"), TomlValue::Str("http://localhost:11434".into()));
        assert_eq!(base_url_write_for("ollama"), TomlValue::Str("https://ollama.com".into())); // alias → cloud
        assert_eq!(base_url_write_for("anthropic-api"), TomlValue::Str("https://api.anthropic.com".into()));
        assert_eq!(base_url_write_for("anthropic-cli"), TomlValue::Unset); // Cli → clear base_url
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid base_url_write_seeds_registry_default_or_unsets`
Expected: FAIL — `base_url_write_for` not found.

- [ ] **Step 3: Implement the helper**

Add near `apply_config_write` in `crates/zoid/src/main.rs`:

```rust
/// The TOML write for `base_url` when a provider is selected: the registry
/// default endpoint (HTTP transports), or `Unset` to clear it (Cli/Sdk have no
/// URL). The user can still override afterward (which flips provenance to [user]).
fn base_url_write_for(id: &str) -> zoid_core::config::TomlValue {
    match zoid_provider::model::default_base_url(id) {
        Some(u) => zoid_core::config::TomlValue::Str(u.to_string()),
        None => zoid_core::config::TomlValue::Unset,
    }
}
```

- [ ] **Step 4: Run to verify the helper passes, then wire the action arms**

Run: `cargo test -p zoid base_url_write_seeds_registry_default_or_unsets`
Expected: PASS.

Then add the four action arms in `handle_action` (`main.rs`), replacing the removed `Action::ConfigCycle(dir)` arm (lines 1338–~1380). All operate on `app`/`app.shell`:

```rust
        Action::ConfigDrillOpen => {
            use zoid_tui::state::ConfigCol;
            if let Some((label, _)) = current_config_field(app) {
                app.shell.config_picker = match label {
                    "provider" => zoid_tui::config_view::provider_options(&app.config.provider),
                    "model" => {
                        zoid_tui::config_view::model_options(&app.config.provider, &app.config.model)
                    }
                    _ => Vec::new(),
                };
                if !app.shell.config_picker.is_empty() {
                    // Cursor lands on the current value, else the first selectable row.
                    app.shell.config_picker_sel = app
                        .shell
                        .config_picker
                        .iter()
                        .position(|o| o.is_current)
                        .or_else(|| app.shell.config_picker.iter().position(|o| o.selectable))
                        .unwrap_or(0);
                    app.shell.config_col = ConfigCol::Picker;
                }
            }
        }
        Action::ConfigPickerMove(d) => {
            let picker = &app.shell.config_picker;
            if !picker.is_empty() {
                let n = picker.len() as i32;
                let mut i = app.shell.config_picker_sel as i32;
                for _ in 0..n {
                    i = (i + d).rem_euclid(n);
                    if picker[i as usize].selectable {
                        break;
                    }
                }
                app.shell.config_picker_sel = i as usize;
            }
        }
        Action::ConfigPickerBack => {
            use zoid_tui::state::ConfigCol;
            app.shell.config_picker.clear();
            app.shell.config_col = ConfigCol::Fields;
        }
        Action::ConfigPickerSelect => {
            use zoid_core::config::TomlValue;
            use zoid_tui::state::ConfigCol;
            let chosen = app
                .shell
                .config_picker
                .get(app.shell.config_picker_sel)
                .filter(|o| o.selectable)
                .map(|o| o.id.clone());
            let label = current_config_field(app).map(|(l, _)| l).unwrap_or("");
            if let Some(id) = chosen {
                if label == "provider" {
                    // Write provider, then seed base_url from the registry.
                    apply_config_write(app, "provider", TomlValue::Str(id.clone()), false);
                    apply_config_write(app, "base_url", base_url_write_for(&id), false);
                    // Auto-advance to the model field and open its picker.
                    app.shell.config_picker.clear();
                    if let Some(mi) = app
                        .shell
                        .config_sections
                        .get(app.shell.config_section)
                        .and_then(|s| s.rows.iter().position(|r| r.label == "model"))
                    {
                        app.shell.config_field = mi;
                    }
                    app.shell.config_picker =
                        zoid_tui::config_view::model_options(&app.config.provider, &app.config.model);
                    app.shell.config_picker_sel = 0;
                    app.shell.config_col = if app.shell.config_picker.is_empty() {
                        ConfigCol::Fields
                    } else {
                        ConfigCol::Picker
                    };
                } else if label == "model" {
                    apply_config_write(app, "model", TomlValue::Str(id), false);
                    app.shell.config_picker.clear();
                    app.shell.config_col = ConfigCol::Fields;
                }
            }
        }
```

> `apply_config_write` already calls `refresh_config_sections(app)`, so after the provider write the rebuilt `config_sections` still has the `model` row at a stable index; setting `config_field` to it and rebuilding `config_picker` from the reloaded `app.config` is correct.

- [ ] **Step 5: Full workspace build + test**

Run: `cargo build && cargo test -p zoid`
Expected: PASS + workspace builds.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(config): picker apply — seed base_url on provider select, auto-advance to model"
```

---

## Phase 4 — Render (zoid-tui)

### Task 8: Full-screen three-column `render_config`

**Files:**
- Modify: `crates/zoid-tui/src/render.rs:656-764` (`render_config`) and its call site (line ~118, `Overlay::Config` arm)

**Interfaces:**
- Consumes: `ShellState` config + picker fields (Tasks 5); `FieldKind::Pick`; `config_view::PickOption`
- Produces: `render_config(frame, state, sections, area)` now renders full-frame three columns; picker column present iff `state.config_picker_open()`.

- [ ] **Step 1: Add a snapshot test scaffold**

In the existing `shell_snapshot` test file (find with `rg -l "shell_snapshot" crates/zoid-tui`), add a settings snapshot that drives a 160×40 buffer with the provider picker open. Use the existing snapshot harness pattern in that file (a `TestBackend::new(160, 40)`, build a `ShellState` with `overlay = Overlay::Config`, populated `config_sections` and an open `config_picker`, render, `assert_snapshot!`). Name it `config_overlay_provider_picker`.

- [ ] **Step 2: Run to capture the (initially empty) snapshot failing**

Run: `cargo test -p zoid-tui config_overlay_provider_picker`
Expected: FAIL (new snapshot / compile error until render implemented).

- [ ] **Step 3: Rewrite `render_config`**

Replace `render_config` (lines 656–764) in `crates/zoid-tui/src/render.rs` with a full-frame three-column renderer. Uses existing `color`/`glyph` tokens, `Layout`, `Block`, `Paragraph`, and the `pad_to`/`truncate` helpers already imported in the module:

```rust
pub fn render_config(
    frame: &mut Frame,
    state: &ShellState,
    sections: &[crate::config_view::Section],
    area: Rect,
) {
    use crate::config_view::FieldKind;
    use crate::text::{pad_to, truncate};
    use ratatui::layout::{Constraint, Direction, Layout};

    frame.render_widget(Clear, area);
    if sections.is_empty() {
        return;
    }
    let active = state.config_section.min(sections.len() - 1);

    // Outer full-frame card.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" zoid · settings ")
        .border_style(Style::new().fg(color::CHAT_ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Footer line reserved at the bottom of the inner area.
    let footer = "Tab section · ↑/↓ move · →/Enter drill · ←/Esc back";
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let body = rows[0];
    let foot = rows[1];

    // Column split: sections rail | fields | (picker, only if open).
    let picker_open = state.config_picker_open();
    let constraints: Vec<Constraint> = if picker_open {
        vec![Constraint::Length(22), Constraint::Length(40), Constraint::Min(20)]
    } else {
        vec![Constraint::Length(22), Constraint::Min(30)]
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(body);

    // Column 1: sections rail.
    let mut nav: Vec<Line> = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        let on = i == active;
        let marker = if on { glyph::COLLAPSED } else { ' ' };
        nav.push(Line::from(Span::styled(
            format!(" {marker} {}", s.title),
            Style::new().fg(if on { color::CHAT_ACCENT } else { color::DIM }),
        )));
    }
    frame.render_widget(Paragraph::new(nav), cols[0]);

    // Column 2: fields of the active section.
    let field_w = cols[1].width as usize;
    let mut fields: Vec<Line> = Vec::new();
    for (i, r) in sections[active].rows.iter().enumerate() {
        let cur = i == state.config_field && state.config_col == crate::state::ConfigCol::Fields;
        let val = if i == state.config_field {
            if let Some(buf) = &state.config_edit {
                let shown = if matches!(r.kind, FieldKind::Secret) {
                    glyph::MASK.to_string().repeat(buf.chars().count())
                } else {
                    buf.clone()
                };
                format!("{shown}{}", glyph::CARET)
            } else {
                r.value.clone()
            }
        } else {
            r.value.clone()
        };
        let (tag_txt, tag_col) = match r.source {
            zoid_core::config::Source::Default => ("[default]", color::DIM),
            zoid_core::config::Source::UserGlobal => ("[user]", color::CHAT_ACCENT),
            zoid_core::config::Source::Project => ("[repo]", color::BRANCH),
            zoid_core::config::Source::Local => ("[local]", color::BRANCH),
            zoid_core::config::Source::Env => ("[env]", color::WARN),
        };
        let marker = if cur { glyph::COLLAPSED } else { ' ' };
        let left = format!(" {marker} {}", pad_to(r.label, 12));
        let fixed = left.width() + tag_txt.width();
        let mid = field_w.saturating_sub(fixed).max(1);
        let val_shown = pad_to(&truncate(&val, mid), mid);
        fields.push(Line::from(vec![
            Span::styled(left, Style::new().fg(if cur { color::CHAT_ACCENT } else { color::TXT })),
            Span::styled(val_shown, Style::new().fg(color::TXT)),
            Span::styled(tag_txt.to_string(), Style::new().fg(tag_col)),
        ]));
    }
    frame.render_widget(Paragraph::new(fields), cols[1]);

    // Column 3: contextual picker (only when open).
    if picker_open {
        let pw = cols[2].width as usize;
        let mut pick: Vec<Line> = Vec::new();
        for (i, o) in state.config_picker.iter().enumerate() {
            let sel = i == state.config_picker_sel && state.config_col == crate::state::ConfigCol::Picker;
            let dot = if o.is_current { glyph::COLLAPSED } else { ' ' };
            let base = format!(" {dot} {}  {}", o.label, o.detail);
            let text = pad_to(&truncate(&base, pw.saturating_sub(1).max(1)), pw.saturating_sub(1).max(1));
            let style = if !o.selectable {
                Style::new().fg(color::DIM)
            } else if sel {
                Style::new().fg(color::TXT).bg(color::SEL_BG)
            } else {
                Style::new().fg(color::TXT)
            };
            pick.push(Line::from(Span::styled(text, style)));
        }
        frame.render_widget(Paragraph::new(pick), cols[2]);
    }

    // Footer.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {footer}"),
            Style::new().fg(color::DIM),
        ))),
        foot,
    );
}
```

Ensure the `Overlay::Config` call site (render.rs ~line 118) passes `frame.area()` (full frame) rather than a centered sub-rect. Confirm by reading lines 110–130; if it already passes the full area, no change.

- [ ] **Step 4: Review snapshot + accept**

Run: `cargo test -p zoid-tui config_overlay_provider_picker`
Then: `cargo insta review` (accept the new snapshot after visually confirming three columns render at 160×40 with the picker column populated).
Expected: PASS after accept.

- [ ] **Step 5: Regenerate any existing config snapshot**

Run: `cargo test -p zoid-tui` — the prior `config_overlay_frame` snapshot (single-card) will now differ. Review and accept the full-screen version: `cargo insta review`.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/tests/
git commit -m "feat(render): full-screen three-column settings with contextual picker"
```

---

### Task 9: Graceful degradation below baseline (picker overlays fields)

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (`render_config` column-split logic)

**Interfaces:**
- Consumes: same as Task 8.
- Produces: when `body.width < 22 + 40 + 20` (three-column minimum) and the picker is open, the picker renders as a floating sub-card overlaying column 2 instead of a third column; sections rail may abbreviate but never vanishes.

- [ ] **Step 1: Write the failing snapshot at a narrow size**

Add a snapshot `config_overlay_narrow_degrades` driving a `TestBackend::new(120, 30)` (below the 160×40 baseline) with the provider picker open. Assert it renders (no panic, sections + fields + an overlaid picker card visible).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui config_overlay_narrow_degrades`
Expected: FAIL (new snapshot).

- [ ] **Step 3: Add the degradation branch**

In `render_config`, replace the `let cols = ...` three-column split with a width check:

```rust
    const RAIL_W: u16 = 22;
    const FIELDS_W: u16 = 40;
    const PICKER_MIN: u16 = 20;
    let three_col_fits = body.width >= RAIL_W + FIELDS_W + PICKER_MIN;

    let cols = if picker_open && three_col_fits {
        Layout::default().direction(Direction::Horizontal)
            .constraints([Constraint::Length(RAIL_W), Constraint::Length(FIELDS_W), Constraint::Min(PICKER_MIN)])
            .split(body)
    } else {
        Layout::default().direction(Direction::Horizontal)
            .constraints([Constraint::Length(RAIL_W.min(body.width / 3).max(8)), Constraint::Min(20)])
            .split(body)
    };
```

Then, after rendering columns 1 and 2, render the picker as an overlay card when open but three columns don't fit:

```rust
    if picker_open && !three_col_fits {
        // Floating sub-card over the fields column (transient picker; overlay is
        // acceptable and keeps every row legible on small terminals).
        let over = crate::layout::centered(cols[1], cols[1].width.saturating_sub(2), (state.config_picker.len() as u16 + 2).min(cols[1].height));
        frame.render_widget(Clear, over);
        let pblock = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color::CHAT_ACCENT));
        let pinner = pblock.inner(over);
        frame.render_widget(pblock, over);
        // (reuse the same per-option line building as Task 8, rendered into `pinner`)
        // ... build `pick` Vec<Line> identically to Task 8 col-3, then:
        frame.render_widget(Paragraph::new(pick), pinner);
    }
```

Guard the Task-8 `if picker_open { ... col[2] ... }` block with `&& three_col_fits` so the picker renders in exactly one place.

- [ ] **Step 4: Accept snapshot**

Run: `cargo test -p zoid-tui config_overlay_narrow_degrades && cargo insta review`
Expected: PASS after accepting; picker visible as an overlay card, nothing blank.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/tests/
git commit -m "feat(render): settings degrades gracefully below baseline (picker overlays fields)"
```

---

## Phase 5 — ALT+P quick-switch

### Task 10: `Overlay::ProviderSwitch` state + open action

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (Overlay enum + quick-switch state)
- Modify: `crates/zoid-tui/src/route.rs` (`Action::OpenProviderSwitch`, `Alt+P` global combo, overlay dispatch)

**Interfaces:**
- Produces:
  - `Overlay::ProviderSwitch` variant.
  - `ShellState` fields: `pub switch_provider_sel: usize`, `pub switch_model_sel: usize`, `pub switch_pane: SwitchPane` where `pub enum SwitchPane { Provider, Model }`.
  - `Action::OpenProviderSwitch`, `Action::SwitchPaneMove(i32)`, `Action::SwitchItemMove(i32)`, `Action::SwitchApply`, `Action::SwitchCancel`.

- [ ] **Step 1: Write the failing test**

Add to `route.rs` tests:

```rust
    #[test]
    fn alt_p_opens_provider_switch() {
        let s = ShellState::new(); // overlay None, focus Input
        let a = route_key(&s, KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
        assert!(matches!(a, Action::OpenProviderSwitch));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui alt_p_opens_provider_switch`
Expected: FAIL — `OpenProviderSwitch` / `Alt+P` not wired.

- [ ] **Step 3: Add state + overlay + open combo**

In `state.rs`: add `ProviderSwitch` to the `Overlay` enum; add `SwitchPane` enum and the three fields to `ShellState` + `new()` defaults (`SwitchPane::Provider`, `0`, `0`).

In `route.rs` `route_key`, add to the global combos (after the `ctrl(&key, 'p')` palette combo, using the existing `alt` helper at line 87):

```rust
    if alt(&key, 'p') {
        return Action::OpenProviderSwitch;
    }
```

Add the overlay dispatch arm in `route_key`'s overlay match (near line 99):

```rust
        Overlay::ProviderSwitch => return route_provider_switch_key(state, key),
```

And the new routing fn:

```rust
fn route_provider_switch_key(_state: &ShellState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Left | KeyCode::Right => Action::SwitchPaneMove(if key.code == KeyCode::Left { -1 } else { 1 }),
        KeyCode::Up => Action::SwitchItemMove(-1),
        KeyCode::Down => Action::SwitchItemMove(1),
        KeyCode::Enter => Action::SwitchApply,
        KeyCode::Esc => Action::SwitchCancel,
        _ => Action::Noop,
    }
}
```

Add the new `Action` variants to the enum.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-tui alt_p_opens_provider_switch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/route.rs
git commit -m "feat(quick-switch): Overlay::ProviderSwitch state + Alt+P open + key routing"
```

---

### Task 11: Render + apply the quick-switch card

**Files:**
- Create/Modify: `crates/zoid-tui/src/render.rs` (`render_provider_switch`)
- Modify: `crates/zoid/src/main.rs` (open/apply handlers)

**Interfaces:**
- Consumes: `config_view::{provider_options, model_options}` (Task 4); quick-switch state (Task 10); `base_url_write_for` + `apply_config_write` (Task 7 / `main.rs:1021`).
- Produces: `pub fn render_provider_switch(frame, state, provider_opts, model_opts, area)`; main.rs arms for `OpenProviderSwitch`/`SwitchPaneMove`/`SwitchItemMove`/`SwitchApply`/`SwitchCancel`.

- [ ] **Step 1: Snapshot test**

Add `provider_switch_card` snapshot at 160×40: `overlay = Overlay::ProviderSwitch`, render with `provider_options("ollama-cloud")` and `model_options("anthropic-api", "")`. Assert the two-pane floating card renders.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui provider_switch_card`
Expected: FAIL.

- [ ] **Step 3: Implement `render_provider_switch`**

Add to `render.rs` a centered floating card (reuse `layout::centered`) with two side-by-side panes (providers | models), current marked with `glyph::COLLAPSED`, active pane's selection using `color::SEL_BG`, planned entries dim/skipped, footer `←/→ pane · ↑/↓ move · Enter apply · Esc cancel`. Mirror the column/option line-building from Task 8's picker so styling stays consistent (one shared private helper `picker_lines(opts, sel, active) -> Vec<Line>` is encouraged — extract it and call from both `render_config` and `render_provider_switch`).

- [ ] **Step 4: Wire main.rs handlers (app-based, via `apply_config_write`)**

Add arms in `handle_action`, all operating on `app`:
- `OpenProviderSwitch` → `app.shell.overlay = Overlay::ProviderSwitch`; seed `app.shell.switch_provider_sel` to the index of the current provider within `selectable()` (else 0); `switch_pane = SwitchPane::Provider`; `switch_model_sel = 0`.
- `SwitchPaneMove(d)` → flip `app.shell.switch_pane` between `Provider`/`Model`.
- `SwitchItemMove(d)` → move the selected index in the active pane, skipping non-selectable rows (same `rem_euclid` skip loop as `ConfigPickerMove`). The provider pane iterates `provider_options`; the model pane iterates `model_options(provider_at_cursor, &app.config.model)`.
- `SwitchApply` → resolve the highlighted provider id and highlighted model id, then persist both through the existing helper (reload-safe):
  ```rust
  apply_config_write(app, "provider", TomlValue::Str(provider_id.clone()), false);
  apply_config_write(app, "base_url", base_url_write_for(&provider_id), false);
  apply_config_write(app, "model", TomlValue::Str(model_id), false);
  app.shell.overlay = Overlay::None;
  ```
- `SwitchCancel` → `app.shell.overlay = Overlay::None` (no writes).

The render call in the draw path passes freshly computed `provider_options(&app.config.provider)` and `model_options(provider_at_cursor_id, &app.config.model)` so the model pane tracks the highlighted provider (mirror the settings picker: `provider_at_cursor_id` is the id of the row under `switch_provider_sel`).

- [ ] **Step 5: Accept snapshot + full build/test**

Run: `cargo test -p zoid-tui provider_switch_card && cargo insta review && cargo build && cargo test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid/src/main.rs crates/zoid-tui/tests/
git commit -m "feat(quick-switch): render Alt+P two-pane card + apply provider/model"
```

---

## Phase 6 — Dynamic model discovery + API-key gate

> Layers live model fetching over the static-fallback picker built in Phases 1–5. After these tasks the registry `models` lists are only shown offline / before a fetch / on error.

### Task 12: `Provider::list_models()` seam + Ollama/Anthropic implementations

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs` (`Provider` trait — add defaulted method)
- Modify: `crates/zoid-provider/src/ollama.rs` (impl `/api/tags` + a pure parse fn)
- Modify: `crates/zoid-provider/src/anthropic.rs` (impl `/v1/models` + a pure parse fn)

**Interfaces:**
- Produces: `async fn list_models(&self) -> anyhow::Result<Vec<String>>` on `Provider` (default `Ok(Vec::new())`); pure parsers `parse_ollama_tags(&str) -> Vec<String>` and `parse_anthropic_models(&str) -> Vec<String>`.

- [ ] **Step 1: Write the failing parser tests**

Add to `crates/zoid-provider/src/ollama.rs` tests:

```rust
    #[test]
    fn parses_ollama_tags_names() {
        let body = r#"{"models":[{"name":"glm-5.2:cloud"},{"name":"llama3.1:70b"}]}"#;
        assert_eq!(parse_ollama_tags(body), vec!["glm-5.2:cloud", "llama3.1:70b"]);
    }
    #[test]
    fn ollama_tags_empty_or_bad_is_empty() {
        assert!(parse_ollama_tags("{}").is_empty());
        assert!(parse_ollama_tags("not json").is_empty());
    }
```

Add to `crates/zoid-provider/src/anthropic.rs` tests:

```rust
    #[test]
    fn parses_anthropic_model_ids() {
        let body = r#"{"data":[{"id":"claude-opus-4-8","type":"model"},{"id":"claude-sonnet-4-6"}]}"#;
        assert_eq!(parse_anthropic_models(body), vec!["claude-opus-4-8", "claude-sonnet-4-6"]);
    }
    #[test]
    fn anthropic_models_bad_is_empty() {
        assert!(parse_anthropic_models("nope").is_empty());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-provider parses_ollama_tags_names parses_anthropic_model_ids`
Expected: FAIL — parse fns not found.

- [ ] **Step 3: Implement the pure parsers + trait method + HTTP impls**

Add the trait method with a default in `crates/zoid-provider/src/lib.rs` inside `pub trait Provider` (after `stream`):

```rust
    /// Fetch the provider's available model ids. Default: none (offline / seam).
    async fn list_models(&self) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }
```

In `crates/zoid-provider/src/ollama.rs`:

```rust
/// Extract model names from an Ollama `/api/tags` response body. Lenient:
/// unknown/!json → empty (the caller falls back to the registry list).
pub fn parse_ollama_tags(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
```

Implement `list_models` for `OllamaProvider` (add to its `impl Provider` block; mirror the header/auth pattern of `stream`):

```rust
    async fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        Ok(parse_ollama_tags(&resp.text().await?))
    }
```

In `crates/zoid-provider/src/anthropic.rs`:

```rust
/// Extract model ids from an Anthropic `/v1/models` response body. Lenient.
pub fn parse_anthropic_models(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
```

Implement `list_models` for `AnthropicProvider` (mirror `stream`'s `x-api-key` + `anthropic-version` headers):

```rust
    async fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?;
        Ok(parse_anthropic_models(&resp.text().await?))
    }
```

> Implementer: confirm the exact field names of `self.client` / `self.api_key` in each struct (read the struct + `stream` impl) and match the auth headers `stream` already uses. If `anthropic-version` is defined as a const in the file, reuse it rather than re-literal.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-provider`
Expected: PASS (parsers green; existing tests unaffected — default trait method keeps `FakeProvider` compiling).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/lib.rs crates/zoid-provider/src/ollama.rs crates/zoid-provider/src/anthropic.rs
git commit -m "feat(provider): list_models() seam + Ollama /api/tags + Anthropic /v1/models"
```

---

### Task 13: `select_provider` builds `ollama-local` without a key

**Files:**
- Modify: `crates/zoid/src/main.rs` (`select_provider`, ~lines 302–343 as rewritten in Task 3)

**Interfaces:**
- Consumes: `zoid_provider::model::entry` (Task 1)
- Produces: for the `ollama-local` id (no key present), `select_provider` returns a real `OllamaProvider` (empty key, localhost base_url) with `has_key == true` (it needs no key to be usable), instead of the offline `FakeProvider`.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid/src/main.rs` tests:

```rust
    #[test]
    fn ollama_local_needs_no_key() {
        // ollama-local is usable with no OLLAMA_API_KEY (localhost, no auth).
        assert!(entry_requires_key("ollama-local") == false);
        assert!(entry_requires_key("ollama-cloud"));
        assert!(entry_requires_key("anthropic-api"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid ollama_local_needs_no_key`
Expected: FAIL — `entry_requires_key` not found.

- [ ] **Step 3: Add `entry_requires_key` and branch `select_provider` on it**

Add the helper in `crates/zoid/src/main.rs`:

```rust
/// Whether a provider id needs an API key to be usable. Local Ollama (localhost)
/// does not; all remote HTTP flavors do. Keyed off the registry, not the string.
fn entry_requires_key(id: &str) -> bool {
    id != "ollama-local"
}
```

In `select_provider`, before the `match family` block, add an early return for the no-key local case:

```rust
    // ollama-local: usable without a key (localhost, no auth). Construct directly.
    if zoid_provider::model::canonical_id(&config.provider) == "ollama-local" {
        let base_url = effective_base_url(config);
        return (
            Arc::new(zoid_provider::ollama::OllamaProvider::new(String::new()).with_base_url(base_url)),
            "ollama",
            true, // no key required → treat as ready
        );
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid ollama_local_needs_no_key && cargo build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(provider): ollama-local constructs without a key (localhost)"
```

---

### Task 14: `AgentUpdate::ModelsFetched` + fetch spawn on model-picker-open

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`AgentUpdate` enum, ~line 61)
- Modify: `crates/zoid/src/main.rs` (`ui_rx` recv arm ~line 791; the `ConfigDrillOpen`/`ConfigPickerSelect` model-open paths from Task 7)

**Interfaces:**
- Consumes: `Provider::list_models` (Task 12); the `ui_tx: mpsc::Sender<AgentUpdate>` the main loop already owns (same one `run_agent_turn` uses)
- Produces: `AgentUpdate::ModelsFetched(Vec<String>)`; a helper `spawn_model_fetch(provider: Arc<dyn Provider>, ui_tx)`; the model-picker-open paths call it.

- [ ] **Step 1: Add the variant + handler**

In `crates/zoid/src/agent.rs`, add to `pub enum AgentUpdate`:

```rust
    /// Live model list fetched for the config/quick-switch picker.
    ModelsFetched(Vec<String>),
```

In `crates/zoid/src/main.rs`, add a match arm in the `Some(update) = ui_rx.recv()` block (near line 791, alongside `AgentUpdate::Appended` etc.):

```rust
                    AgentUpdate::ModelsFetched(models) => {
                        // Replace an OPEN model picker's options with the live list.
                        // Ignore if empty (keep fallback) or if a model picker isn't open.
                        if !models.is_empty() && app.shell.config_picker_open() {
                            let on_model = current_config_field(app).map(|(l, _)| l) == Some("model");
                            if on_model {
                                let cur = app.config.model.clone();
                                app.shell.config_picker = models
                                    .into_iter()
                                    .map(|m| zoid_tui::config_view::PickOption {
                                        is_current: m == cur,
                                        id: m.clone(),
                                        label: m,
                                        detail: String::new(),
                                        selectable: true,
                                    })
                                    .collect();
                                app.shell.config_picker_sel = app.shell.config_picker
                                    .iter().position(|o| o.is_current).unwrap_or(0);
                            }
                        }
                    }
```

- [ ] **Step 2: Add the spawn helper**

Add to `crates/zoid/src/main.rs` (near `select_provider`). Use the same `ui_tx` clone pattern the code already uses to hand a sender to `run_agent_turn` (grep `ui_tx` / `ui.clone()` for the exact handle):

```rust
/// Spawn a background fetch of the active provider's model list; result is
/// delivered as `AgentUpdate::ModelsFetched`. Non-fatal: errors → empty list
/// (the picker keeps its fallback). 
fn spawn_model_fetch(
    provider: std::sync::Arc<dyn Provider>,
    ui_tx: tokio::sync::mpsc::Sender<zoid_tui_agent_update_path>, // use the real AgentUpdate sender type in scope
) {
    tokio::spawn(async move {
        let models = provider.list_models().await.unwrap_or_default();
        let _ = ui_tx.send(zoid::agent::AgentUpdate::ModelsFetched(models)).await;
    });
}
```

> Implementer: the sender type is whatever `run_agent_turn` is passed (an `mpsc::Sender<AgentUpdate>`); `AgentUpdate` lives in this crate (`crate::agent::AgentUpdate`). Replace the placeholder type/path with the real ones from the surrounding code. `Provider` is already imported in `main.rs`.

- [ ] **Step 3: Trigger the fetch when a model picker opens**

In the Task 7 handlers (`ConfigDrillOpen` for the `model` field, and `ConfigPickerSelect` after a provider commit opens the model picker), after populating `app.shell.config_picker` with the fallback and setting `config_col = Picker`, add:

```rust
                    spawn_model_fetch(app.provider.clone(), ui_tx.clone());
```

(`ui_tx` is the sender in scope in `handle_action` / the event loop — thread it in if `handle_action` doesn't already receive it; grep how `AgentUpdate::Appended` is sent from within action handling for the existing handle.)

- [ ] **Step 4: Build + manual smoke**

Run: `cargo build && cargo test -p zoid`
Expected: builds; existing tests green. (The live fetch is exercised by manual smoke — env `OLLAMA_API_KEY` set, open settings → model picker → observe the list populate from `/api/tags`.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(config): live model fetch (AgentUpdate::ModelsFetched) refreshing the open picker"
```

---

### Task 15: API-key gate in the cascade

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (a `config_key_prompt: Option<&'static str>` env-name field on `ShellState`)
- Modify: `crates/zoid-tui/src/route.rs` (key-entry routing)
- Modify: `crates/zoid/src/main.rs` (`ConfigPickerSelect` provider branch: gate before fetch; commit the key to the secret store)

**Interfaces:**
- Consumes: `entry_requires_key` (Task 13); `select_provider`'s `has_key` return; `SecretStore::set/status`; the masked inline-edit buffer (`config_edit` + `FieldKind::Secret` masking).
- Produces: `ShellState.config_key_prompt: Option<&'static str>` (the env name being entered, e.g. `"OLLAMA_API_KEY"`); when `Some`, col 2 shows a masked key entry and `Enter` commits it to the secret store.

- [ ] **Step 1: Write the failing test (gate decision)**

Add to `crates/zoid/src/main.rs` tests:

```rust
    #[test]
    fn key_env_for_family() {
        assert_eq!(key_env_for("anthropic-api"), Some("ANTHROPIC_API_KEY"));
        assert_eq!(key_env_for("ollama-cloud"), Some("OLLAMA_API_KEY"));
        assert_eq!(key_env_for("ollama-local"), None); // no key needed
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid key_env_for_family`
Expected: FAIL — `key_env_for` not found.

- [ ] **Step 3: Implement `key_env_for` + gate the provider-select path**

Add to `crates/zoid/src/main.rs`:

```rust
/// The secret env name a provider id needs, or `None` if it needs no key.
fn key_env_for(id: &str) -> Option<&'static str> {
    if !entry_requires_key(id) {
        return None;
    }
    match zoid_provider::model::entry(id).map(|e| e.family) {
        Some("anthropic") => Some("ANTHROPIC_API_KEY"),
        _ => Some("OLLAMA_API_KEY"),
    }
}
```

In the `ConfigPickerSelect` provider branch (Task 7), after `apply_config_write(app, "provider", …)` + `apply_config_write(app, "base_url", …)`, insert the gate before opening the model picker:

```rust
                    // Key gate: if this provider needs a key we don't have, prompt first.
                    let needs = key_env_for(&id).filter(|env| {
                        app.secrets.as_ref().map(|s| {
                            use zoid_core::secret::SecretStore;
                            matches!(s.status(env), zoid_core::secret::SecretStatus::NotSet)
                        }).unwrap_or(true)
                    });
                    if let Some(env) = needs {
                        app.shell.config_key_prompt = Some(env);
                        app.shell.config_edit = Some(String::new());
                        app.shell.config_picker.clear();
                        app.shell.config_col = zoid_tui::state::ConfigCol::Fields;
                    } else {
                        // (existing auto-advance-to-model + spawn_model_fetch path)
                    }
```

Move the existing "auto-advance to model + open picker + spawn fetch" code into the `else` branch.

- [ ] **Step 4: Route + commit the key entry**

In `route.rs route_config_key`, when `state.config_key_prompt.is_some()` and `config_edit.is_some()`, route `Enter → ConfigCommitEdit`, `Esc → ConfigCancelEdit`, chars/backspace as normal (the existing editing block already does this; just ensure the key-prompt state routes through it — it will, since `config_edit.is_some()`).

In `main.rs`, extend `ConfigCommitEdit` (main.rs:1295): if `app.shell.config_key_prompt` is `Some(env)`, write the buffer to the secret store instead of TOML, clear the prompt, re-run selection, and advance to the model fetch:

```rust
            if let Some(env) = app.shell.config_key_prompt.take() {
                if let (Some(s), Some(buf)) = (&app.secrets, app.shell.config_edit.clone()) {
                    use zoid_core::secret::SecretStore;
                    if let Err(e) = s.set(env, buf.trim()) {
                        eprintln!("zoid: secret set failed for {env}: {e}");
                    }
                }
                app.shell.config_edit = None;
                refresh_config_sections(app);
                // Re-select with the new key, then advance to model fetch.
                let (provider, name, has_key) = select_provider(&app.config, &app.secrets);
                app.provider = provider;
                app.shell.provider = provider_label(name, has_key);
                if let Some(mi) = app.shell.config_sections.get(app.shell.config_section)
                    .and_then(|sec| sec.rows.iter().position(|r| r.label == "model")) {
                    app.shell.config_field = mi;
                }
                app.shell.config_picker =
                    zoid_tui::config_view::model_options(&app.config.provider, &app.config.model);
                app.shell.config_picker_sel = 0;
                app.shell.config_col = if app.shell.config_picker.is_empty() {
                    zoid_tui::state::ConfigCol::Fields
                } else {
                    zoid_tui::state::ConfigCol::Picker
                };
                spawn_model_fetch(app.provider.clone(), ui_tx.clone());
                return Ok(false); // handled; skip the normal edit-commit path below
            }
```

Add `config_key_prompt: Option<&'static str>` to `ShellState` + `new()` default `None` (state.rs), and have `render_config` show a masked key-entry row (label = the env name, value masked) when `config_key_prompt.is_some()` — reuse the `FieldKind::Secret` masking already in the field renderer.

- [ ] **Step 5: Build + manual smoke**

Run: `cargo test -p zoid key_env_for_family && cargo build && cargo test`
Expected: PASS. Manual smoke: unset `ANTHROPIC_API_KEY`, select `anthropic-api` → key prompt appears → enter key → model list fetches.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/render.rs crates/zoid/src/main.rs
git commit -m "feat(config): API-key gate — prompt + store key before model fetch"
```

---

### Task 16: Full-suite green + fmt/clippy + changelog

**Files:**
- Modify: `CHANGELOG.md`
- Modify: any snapshot files needing final acceptance.

- [ ] **Step 1: Run the whole suite**

Run: `cargo test`
Expected: all green. Fix any remaining references to removed symbols (`FieldKind::Cycle`, `ConfigCycle`, `KNOWN_PROVIDERS`) surfaced here.

- [ ] **Step 2: Lint + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all`
Expected: clean.

- [ ] **Step 3: Add a CHANGELOG entry**

Add under a new unreleased section in `CHANGELOG.md`:

```markdown
## Unreleased

Settings redesign.
- Full-screen three-column settings (sections · fields · contextual picker) replacing the cramped card; baseline 160×40 with graceful degradation.
- Visible provider/model picker (Miller-column cascade) replacing the blind cycle; selecting a provider seeds `base_url` from the registry and jumps to model selection.
- Transport-aware provider registry: `ollama-local` / `ollama-cloud` split, `anthropic-api`, plus `[planned]` `anthropic-cli` / `anthropic-sdk` seam entries. Legacy `ollama`/`anthropic` ids alias to the new canonical ids.
- Live model discovery: the model picker fetches available models from the provider (Ollama `/api/tags`, Anthropic `/v1/models`), falling back to the registry list offline. Selecting a key-requiring provider prompts for the API key before fetching.
- `Alt+P` quick-switch overlay for changing provider + model mid-session.
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(settings): full suite green, clippy/fmt clean, changelog"
```

---

## Self-Review

**Spec coverage:**
- §4.1 full-screen shell → Task 8. §4.2 three columns → Tasks 8/9. §4.3 cascade (provider→auto-jump→model, direct model) → Tasks 6/7. §4.4 registry struct + ids + aliases + single-source endpoints → Tasks 1/2/3. §4.5 transport-adaptive connection field + seeding/provenance → Tasks 4/7. §4.6 other sections inline → preserved (Task 6 keeps Text/Uint/Bool/Secret inline). §4.7 ALT+P → Tasks 10/11. §4.8 keybindings → Tasks 6/10. §4.9 degradation → Task 9. Migration alias → Task 1. `[planned]` visible-not-selectable → Tasks 1/4/6 (skip logic) /7/8.
- CLI `command` config storage is intentionally deferred with the CLI impl (spec §7); Task 4 renders the `command` label using the base_url slot so the seam shows, no new config field.
- §4.10 dynamic model discovery → Tasks 12 (`list_models` seam + parsers) / 14 (`ModelsFetched` + fetch spawn). §4.11 API-key gate → Tasks 13 (`ollama-local` no-key) / 15 (key prompt + secret store). Registry `models` as fallback → Task 1 (list) + Task 14 (live overrides when non-empty).

**Placeholder scan:** Persistence uses the real `apply_config_write(app, key, value, false)` (`main.rs:1021`) throughout Tasks 7/11 — no invented helper. The one deliberately-prose step is Task 11 Step 3 (`render_provider_switch`), which reuses the concrete per-option line-building from Task 8 (extract a shared `picker_lines(opts, sel, active) -> Vec<Line>` and call it from both renderers). All other steps carry complete code.

**Type consistency:** `PickOption` fields (`id: String`, `label`, `detail`, `selectable`, `is_current`) are consistent across Tasks 4/5/6/7/8/11. `ConfigCol::{Fields, Picker}` consistent Tasks 5/6/7/8. `Transport`/`Status`/`ProviderEntry`/`canonical_id`/`entry`/`default_base_url`/`models_for`/`selectable` consistent Tasks 1/2/3/4. `base_url_write_for(&str) -> TomlValue` consistent Tasks 7/11. Config persistence is `apply_config_write(app, &str, TomlValue, bool)` in Tasks 7/11.
