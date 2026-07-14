# MCP Catalog Entries (Spec 2.5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a third catalog plugin kind `mcp` that installs one stdio MCP server by merging a server block into a `.mcp.json` behind an exact-command confirm gate.

**Architecture:** A new `kind = ["mcp"]` manifest carries one inline `[mcp.servers.<name>]` table (no `[source]`/`[mode]`). Selecting an mcp row in the `:plugin` overlay async-fetches its `<id>.toml`, populates a kind-aware confirm (command/args/env-warnings/target), and on `y` writes atomically into the user (default) or project `.mcp.json` — skip-on-collision, order-preserving. It bypasses the upstream-tree fetch and the Effect gate entirely.

**Tech Stack:** Rust workspace (crates `zoid-plugin`, `zoid-mcp`, `zoid-tui`, `zoid`); `toml`/`serde_json` (with `preserve_order`); `ratatui`/`crossterm` TUI; `tokio` async; `tempfile` for atomic writes.

## Global Constraints

- `schema == 1` only; an mcp manifest's `kind` MUST equal `["mcp"]` exactly (mutually exclusive with `mode`/`skills`).
- Exactly **one** `[mcp.servers.<name>]` per mcp manifest (multi-server is Spec 3). stdio only — a server requires a non-empty `command`; no `type`/`url`.
- `${VAR}` placeholders are written **verbatim**; zoid never expands them on write and never prompts for or persists secret values.
- The `.mcp.json` write is **atomic** (temp file in the same dir + `rename`) and **order-preserving** (`serde_json/preserve_order`); a name collision is **skipped**, never overwritten; a malformed target file **aborts** the write.
- Default write target is **user** (`resolve_config_dir(...).join("mcp.json")`); project is `current_dir().join(".mcp.json")`.
- `zoid-tui` MUST NOT depend on `zoid`/`zoid-mcp`/`zoid-plugin` types — overlay state carries plain `String`/enum shapes the bin maps into (mirrors `McpStatusRow`/`PluginCatalogRow`).
- Never log secrets or `env` values. Commit messages MUST NOT include any `Co-Authored-By`/co-author trailer.
- Post-install activation is a **restart hint** (no in-session hot-connect).

---

### Task 1: `zoid-plugin` — `[mcp]` manifest parse + validate

**Files:**
- Modify: `crates/zoid-plugin/src/manifest.rs`

**Interfaces:**
- Consumes: existing `PluginManifest`, `parse_manifest`, `validate` (`manifest.rs`).
- Produces:
  - `pub struct McpServerSpec { pub command: String, pub args: Vec<String>, pub env: BTreeMap<String, String> }`
  - `pub struct McpManifest { pub servers: BTreeMap<String, McpServerSpec> }`
  - New field `pub mcp: Option<McpManifest>` on `PluginManifest`.
  - `validate()` accepts `kind == ["mcp"]` with exactly one server; rejects mixed kinds, empty/multi servers, `[source]`/`[mode]` on mcp, and a server missing `command`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/zoid-plugin/src/manifest.rs`:

```rust
    const MCP_GOOD: &str = r#"
[plugin]
id = "github"
schema = 1
kind = ["mcp"]
name = "GitHub MCP"
description = "GitHub over MCP"

[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "${GITHUB_TOKEN}" }
"#;

    #[test]
    fn parses_and_validates_an_mcp_manifest() {
        let m = parse_manifest(MCP_GOOD).unwrap();
        m.validate().unwrap();
        assert_eq!(m.kind, vec!["mcp".to_string()]);
        assert!(m.source.is_none() && m.mode.is_none());
        let mcp = m.mcp.as_ref().unwrap();
        assert_eq!(mcp.servers.len(), 1);
        let s = mcp.servers.get("github").unwrap();
        assert_eq!(s.command, "npx");
        assert_eq!(s.args, vec!["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(s.env.get("GITHUB_TOKEN").unwrap(), "${GITHUB_TOKEN}");
    }

    #[test]
    fn rejects_mcp_mixed_with_other_kinds() {
        let src = MCP_GOOD.replace(r#"kind = ["mcp"]"#, r#"kind = ["mcp", "skills"]"#);
        let m = parse_manifest(&src).unwrap();
        assert!(m.validate().unwrap_err().contains("mcp"));
    }

    #[test]
    fn rejects_mcp_without_a_server() {
        let src = "\n[plugin]\nid = \"x\"\nschema = 1\nkind = [\"mcp\"]\nname = \"X\"\ndescription = \"d\"\n";
        let m = parse_manifest(src).unwrap();
        assert!(m.validate().unwrap_err().contains("server"));
    }

    #[test]
    fn rejects_mcp_with_more_than_one_server() {
        let src = format!("{MCP_GOOD}\n[mcp.servers.second]\ncommand = \"foo\"\n");
        let m = parse_manifest(&src).unwrap();
        assert!(m.validate().unwrap_err().contains("one server"));
    }

    #[test]
    fn rejects_mcp_with_source_or_mode() {
        let src = format!("{MCP_GOOD}\n[source]\nrepo = \"a/b\"\nref = \"s\"\nsubtree = \"x\"\n");
        let m = parse_manifest(&src).unwrap();
        assert!(m.validate().unwrap_err().contains("source"));
    }

    #[test]
    fn rejects_mcp_server_missing_command() {
        // `command` is required by the RawMcpServer serde shape → parse error.
        let src = "\n[plugin]\nid=\"x\"\nschema=1\nkind=[\"mcp\"]\nname=\"X\"\ndescription=\"d\"\n[mcp.servers.s]\nargs=[\"a\"]\n";
        assert!(parse_manifest(src).is_err());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-plugin --lib manifest 2>&1 | tail -20`
Expected: FAIL — `McpManifest`/`mcp` field don't exist; validate doesn't reject mcp cases.

- [ ] **Step 3: Add the public types + raw serde shapes + parse wiring**

In `crates/zoid-plugin/src/manifest.rs`, add `use std::collections::BTreeMap;` at the top if absent. Add the public types near `ModeRecipe`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpManifest {
    pub servers: BTreeMap<String, McpServerSpec>,
}
```

Add the field to `PluginManifest` (after `pub mode: Option<ModeRecipe>,`):

```rust
    pub mcp: Option<McpManifest>,
```

Add raw serde shapes near `RawMode`:

```rust
#[derive(Deserialize)]
struct RawMcp {
    #[serde(default)]
    servers: BTreeMap<String, RawMcpServer>,
}

#[derive(Deserialize)]
struct RawMcpServer {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}
```

Add `mcp: Option<RawMcp>` to `RawManifest`:

```rust
#[derive(Deserialize)]
struct RawManifest {
    plugin: RawPlugin,
    source: Option<RawSource>,
    mode: Option<RawMode>,
    mcp: Option<RawMcp>,
    #[serde(default)]
    install: Vec<RawEffect>,
}
```

In `parse_manifest`, add to the returned `PluginManifest { ... }` literal (after the `mode: raw.mode.map(...)` block):

```rust
        mcp: raw.mcp.map(|m| McpManifest {
            servers: m
                .servers
                .into_iter()
                .map(|(name, s)| {
                    (
                        name,
                        McpServerSpec { command: s.command, args: s.args, env: s.env },
                    )
                })
                .collect(),
        }),
```

- [ ] **Step 4: Add the `mcp` validate arm**

Replace the kind-check loop in `validate()` so mcp is exclusive, and add the mcp-specific checks. The current loop is:

```rust
        for k in &self.kind {
            if k != "mode" && k != "skills" {
                return Err(format!(
                    "plugin '{}' declares unsupported kind '{}' (v1 supports 'mode' and 'skills')",
                    self.id, k
                ));
            }
        }
        if self.kind.iter().any(|k| k == "mode") && self.mode.is_none() {
```

Replace it with:

```rust
        let is_mcp = self.kind.iter().any(|k| k == "mcp");
        if is_mcp {
            // mcp is not composable with the tree-materializing kinds; the
            // install dispatch can only route one way.
            if self.kind != ["mcp"] {
                return Err(format!(
                    "plugin '{}' mixes 'mcp' with other kinds; 'mcp' must be the only kind",
                    self.id
                ));
            }
            if self.source.is_some() || self.mode.is_some() {
                return Err(format!(
                    "plugin '{}' is kind 'mcp' and must not declare [source] or [mode]",
                    self.id
                ));
            }
            match self.mcp.as_ref().map(|m| m.servers.len()) {
                Some(1) => {}
                _ => {
                    return Err(format!(
                        "plugin '{}' (kind 'mcp') must declare exactly one server",
                        self.id
                    ));
                }
            }
            return Ok(());
        }
        for k in &self.kind {
            if k != "mode" && k != "skills" {
                return Err(format!(
                    "plugin '{}' declares unsupported kind '{}' (v1 supports 'mode', 'skills', 'mcp')",
                    self.id, k
                ));
            }
        }
        if self.kind.iter().any(|k| k == "mode") && self.mode.is_none() {
```

(Leave the rest of `validate` — the `[mode]` table check and `Ok(())` — unchanged.)

Any existing test or bundled manifest that builds a `PluginManifest` literal now needs `mcp: None`. Search and fix: `rg -n "install:\s*Vec::new\(\)|PluginManifest \{" crates/` — add `mcp: None,` to each literal (notably `crates/zoid-plugin/src/bundled.rs` if it builds a literal; if it calls `parse_manifest`, nothing to change).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-plugin --lib 2>&1 | tail -8`
Expected: PASS — all new mcp tests green; existing `parses_a_good_manifest`, `accepts_skills_kind_without_mode_table`, `rejects_unknown_kind` still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-plugin/src/manifest.rs crates/zoid-plugin/src/bundled.rs
git commit -m "feat(zoid-plugin): [mcp] manifest kind — parse + exclusive-kind validate"
```

---

### Task 2: `zoid-mcp` — atomic, order-preserving `merge_server`

**Files:**
- Modify: `crates/zoid-mcp/src/config.rs`
- Modify: `crates/zoid-mcp/Cargo.toml`

**Interfaces:**
- Consumes: existing `pub struct McpServerConfig { command, args, env }` (`config.rs:5`).
- Produces:
  - `pub enum MergeOutcome { Inserted, SkippedExisting }`
  - `pub fn merge_server(path: &Path, name: &str, server: &McpServerConfig) -> anyhow::Result<MergeOutcome>`

- [ ] **Step 1: Enable `preserve_order` and promote `tempfile` to a runtime dep**

In `crates/zoid-mcp/Cargo.toml`, change the `serde_json` line and add `tempfile` under `[dependencies]`:

```toml
serde_json = { workspace = true, features = ["preserve_order"] }
tempfile = { workspace = true }
```

(`tempfile` is already a `[dev-dependencies]` entry; keep that line too — Cargo dedupes. `preserve_order` unifies workspace-wide; Step 5 verifies nothing depends on `serde_json::Value` alphabetical ordering.)

- [ ] **Step 2: Write the failing tests**

Add a `#[cfg(test)] mod merge_tests` at the bottom of `crates/zoid-mcp/src/config.rs`:

```rust
#[cfg(test)]
mod merge_tests {
    use super::*;

    fn cfg(cmd: &str) -> McpServerConfig {
        McpServerConfig {
            command: cmd.into(),
            args: vec!["-y".into()],
            env: BTreeMap::from([("TOKEN".to_string(), "${TOKEN}".to_string())]),
        }
    }

    #[test]
    fn inserts_into_missing_file_creating_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mcp.json");
        let out = merge_server(&path, "github", &cfg("npx")).unwrap();
        assert!(matches!(out, MergeOutcome::Inserted));
        let back = parse_mcp_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].0, "github");
        // ${VAR} written verbatim, not expanded.
        assert_eq!(back[0].1.env.get("TOKEN").unwrap(), "${TOKEN}");
    }

    #[test]
    fn preserves_siblings_and_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        // Hand-written, deliberately non-alphabetical order.
        std::fs::write(&path, "{\n  \"mcpServers\": {\n    \"zeta\": { \"command\": \"z\" },\n    \"alpha\": { \"command\": \"a\" }\n  }\n}\n").unwrap();
        merge_server(&path, "github", &cfg("npx")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Original siblings kept in original (non-alphabetical) order; new one appended.
        let zi = text.find("zeta").unwrap();
        let ai = text.find("alpha").unwrap();
        let gi = text.find("github").unwrap();
        assert!(zi < ai && ai < gi, "order not preserved: {text}");
    }

    #[test]
    fn skips_existing_name_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "{ \"mcpServers\": { \"github\": { \"command\": \"mine\" } } }").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let out = merge_server(&path, "github", &cfg("npx")).unwrap();
        assert!(matches!(out, MergeOutcome::SkippedExisting));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "must not rewrite on skip");
    }

    #[test]
    fn aborts_on_malformed_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "not json at all").unwrap();
        assert!(merge_server(&path, "github", &cfg("npx")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json at all", "must not clobber");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p zoid-mcp --lib merge_tests 2>&1 | tail -20`
Expected: FAIL — `merge_server`/`MergeOutcome` undefined.

- [ ] **Step 4: Implement `merge_server`**

Add to `crates/zoid-mcp/src/config.rs` (top-level, after `parse_mcp_json`). Add `use std::io::Write;` at the top of the file:

```rust
/// Outcome of a `merge_server` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
    Inserted,
    SkippedExisting,
}

/// Additively merge one named stdio server into the `.mcp.json` at `path`.
/// Atomic (temp file + rename) and order-preserving. Skips an existing name
/// without writing; aborts (never clobbers) a malformed target file. `${VAR}`
/// placeholders in `server.env` are written verbatim.
pub fn merge_server(
    path: &Path,
    name: &str,
    server: &McpServerConfig,
) -> anyhow::Result<MergeOutcome> {
    use serde_json::{Map, Value};

    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} is not valid JSON: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(anyhow::anyhow!("cannot read {}: {}", path.display(), e.kind())),
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} root is not a JSON object", path.display()))?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} 'mcpServers' is not a JSON object", path.display()))?;

    if servers.contains_key(name) {
        return Ok(MergeOutcome::SkippedExisting);
    }

    // Build the server object with a stable key order (command, args, env).
    let mut sv = Map::new();
    sv.insert("command".into(), Value::String(server.command.clone()));
    sv.insert(
        "args".into(),
        Value::Array(server.args.iter().cloned().map(Value::String).collect()),
    );
    let mut env = Map::new();
    for (k, v) in &server.env {
        env.insert(k.clone(), Value::String(v.clone()));
    }
    sv.insert("env".into(), Value::Object(env));
    servers.insert(name.to_string(), Value::Object(sv));

    let mut text = serde_json::to_string_pretty(&root)?;
    text.push('\n');

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    // Atomic: write a temp file in the SAME directory, then rename over the target.
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("atomic rename failed: {e}"))?;
    Ok(MergeOutcome::Inserted)
}
```

- [ ] **Step 5: Run tests + verify no ordering regression**

Run: `cargo test -p zoid-mcp 2>&1 | tail -8`
Expected: PASS (merge tests + existing config/discover tests).

Run the whole workspace once to confirm `preserve_order` didn't break any `serde_json::Value` consumer:
`cargo test --workspace 2>&1 | tail -6`
Expected: all green. (If any test asserted alphabetical `Value` key order, it lives outside mcp and must be reconciled — none is expected.)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-mcp/src/config.rs crates/zoid-mcp/Cargo.toml Cargo.lock
git commit -m "feat(zoid-mcp): merge_server — first .mcp.json writer (atomic, order-preserving, skip-on-collision)"
```

---

### Task 3: `zoid-tui` — confirm state machine (loading + mcp confirm + target)

**Files:**
- Modify: `crates/zoid-tui/src/state.rs`
- Modify: `crates/zoid-tui/src/route.rs`

**Interfaces:**
- Consumes: existing `PluginCatalogState`, `CatalogMode`, `route_plugin_catalog_key` (`route.rs:402`), `Action` enum.
- Produces:
  - `CatalogMode { List, ConfirmLoading, Confirm }`
  - `pub struct McpConfirm { pub server_name: String, pub command: String, pub args: Vec<String>, pub env: Vec<McpEnvEntry>, pub target: McpTarget }`
  - `pub struct McpEnvEntry { pub key: String, pub value: String, pub unset: bool }`
  - `pub enum McpTarget { User, Project }`
  - `PluginCatalogState` fields `mcp: Option<McpConfirm>`, `confirm_error: Option<String>`, and helpers `begin_confirm_loading`, `set_mcp_confirm`, `set_confirm_error`, `toggle_target`.
  - `Action::CatalogTargetToggle`.

- [ ] **Step 1: Write the failing state-machine tests**

Add to the `#[cfg(test)] mod tests` in `crates/zoid-tui/src/state.rs`:

```rust
    #[test]
    fn mcp_confirm_flow_and_target_toggle() {
        let mut s = PluginCatalogState::loading();
        s.rows = vec![PluginCatalogRow {
            id: "github".into(), name: "GitHub".into(), kind_label: "mcp".into(),
            description: "d".into(), source_label: String::new(), license: None,
        }];
        s.status = CatalogStatus::Ready;
        s.begin_confirm_loading();
        assert_eq!(s.mode, CatalogMode::ConfirmLoading);
        assert!(s.mcp.is_none() && s.confirm_error.is_none());

        s.set_mcp_confirm(McpConfirm {
            server_name: "github".into(), command: "npx".into(),
            args: vec!["-y".into()],
            env: vec![McpEnvEntry { key: "TOKEN".into(), value: "${TOKEN}".into(), unset: true }],
            target: McpTarget::User,
        });
        assert_eq!(s.mode, CatalogMode::Confirm);
        assert_eq!(s.mcp.as_ref().unwrap().target, McpTarget::User);
        s.toggle_target();
        assert_eq!(s.mcp.as_ref().unwrap().target, McpTarget::Project);

        s.back_to_list();
        assert_eq!(s.mode, CatalogMode::List);
        assert!(s.mcp.is_none() && s.confirm_error.is_none());
    }

    #[test]
    fn confirm_error_sets_confirm_mode() {
        let mut s = PluginCatalogState::loading();
        s.status = CatalogStatus::Ready;
        s.begin_confirm_loading();
        s.set_confirm_error("boom".into());
        assert_eq!(s.mode, CatalogMode::Confirm);
        assert_eq!(s.confirm_error.as_deref(), Some("boom"));
        assert!(s.mcp.is_none());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid-tui --lib mcp_confirm_flow 2>&1 | tail -15`
Expected: FAIL — `ConfirmLoading`, `McpConfirm`, helpers undefined.

- [ ] **Step 3: Add the types + `CatalogMode` variant + state fields + helpers**

In `crates/zoid-tui/src/state.rs`, change `CatalogMode`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogMode {
    List,
    ConfirmLoading,
    Confirm,
}
```

Add near `PluginCatalogRow`:

```rust
/// Write target for an mcp-server install. `User` (default) writes the global
/// `<config>/mcp.json`; `Project` writes the repo's tracked `cwd/.mcp.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTarget {
    User,
    Project,
}

/// One env var of an mcp-server confirm card. `value` is the raw manifest
/// value (may contain `${VAR}` — written verbatim on install); `unset` flags
/// that a referenced `${VAR}` is absent from the current environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpEnvEntry {
    pub key: String,
    pub value: String,
    pub unset: bool,
}

/// The command card shown when confirming an `mcp`-kind install. Mapped by the
/// bin from a fetched `PluginManifest` (zoid-tui holds no plugin/mcp types).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfirm {
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<McpEnvEntry>,
    pub target: McpTarget,
}
```

Add two fields to `PluginCatalogState`:

```rust
    /// Some when confirming an mcp row whose manifest has been fetched.
    pub mcp: Option<McpConfirm>,
    /// Some when the mcp manifest fetch failed (rendered in Confirm mode).
    pub confirm_error: Option<String>,
```

Update `loading()` to initialize them:

```rust
    pub fn loading() -> Self {
        Self {
            rows: vec![],
            cursor: 0,
            mode: CatalogMode::List,
            status: CatalogStatus::Loading,
            mcp: None,
            confirm_error: None,
        }
    }
```

Update `back_to_list` to clear the mcp confirm state:

```rust
    pub fn back_to_list(&mut self) {
        self.mode = CatalogMode::List;
        self.mcp = None;
        self.confirm_error = None;
    }
```

Add helpers in the same `impl PluginCatalogState`:

```rust
    /// mcp row selected: enter the loading pane while the manifest fetches.
    pub fn begin_confirm_loading(&mut self) {
        self.mode = CatalogMode::ConfirmLoading;
        self.mcp = None;
        self.confirm_error = None;
    }

    /// The fetched manifest resolved into a command card.
    pub fn set_mcp_confirm(&mut self, confirm: McpConfirm) {
        self.mcp = Some(confirm);
        self.confirm_error = None;
        self.mode = CatalogMode::Confirm;
    }

    /// The manifest fetch failed; show the error in the confirm pane.
    pub fn set_confirm_error(&mut self, msg: String) {
        self.mcp = None;
        self.confirm_error = Some(msg);
        self.mode = CatalogMode::Confirm;
    }

    /// Toggle the write target on the active mcp confirm (no-op otherwise).
    pub fn toggle_target(&mut self) {
        if let Some(m) = self.mcp.as_mut() {
            m.target = match m.target {
                McpTarget::User => McpTarget::Project,
                McpTarget::Project => McpTarget::User,
            };
        }
    }
```

Find the other `PluginCatalogState { ... }` literal (a test constructor around `state.rs:1114`) and add `mcp: None, confirm_error: None,`.

- [ ] **Step 4: Add the `CatalogTargetToggle` action + route the new modes**

In `crates/zoid-tui/src/route.rs`, add to the `Action` enum next to the other `Catalog*` variants:

```rust
    /// `:plugin catalog` overlay, mcp Confirm mode: `u`/`p` — toggle the write target.
    CatalogTargetToggle,
```

Replace `route_plugin_catalog_key` (`route.rs:402`) with:

```rust
fn route_plugin_catalog_key(state: &ShellState, key: KeyEvent) -> Action {
    let mode = state.plugin_catalog.as_ref().map(|c| c.mode);
    match mode {
        Some(crate::state::CatalogMode::Confirm) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::CatalogConfirmYes,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::CatalogConfirmNo,
            KeyCode::Char('u') | KeyCode::Char('U') | KeyCode::Char('p') | KeyCode::Char('P') => {
                Action::CatalogTargetToggle
            }
            _ => Action::Noop,
        },
        // While the manifest is fetching, only Esc (cancel) is live.
        Some(crate::state::CatalogMode::ConfirmLoading) => match key.code {
            KeyCode::Esc => Action::CatalogConfirmNo,
            _ => Action::Noop,
        },
        _ => match key.code {
            KeyCode::Up => Action::CatalogMove(-1),
            KeyCode::Down => Action::CatalogMove(1),
            KeyCode::Enter => Action::CatalogEnterConfirm,
            KeyCode::Esc => Action::CloseOverlay,
            _ => Action::Noop,
        },
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib 2>&1 | tail -8`
Expected: PASS. (`render.rs`'s exhaustive `match cat.mode` will now FAIL TO COMPILE — that is expected and fixed in Task 4. If you need a green checkpoint before Task 4, temporarily this task's crate won't build; commit the state+route together with Task 4, OR add the render arm now. Per the plan, commit state+route here and let Task 4's reviewer see the compile gap closed — see note.)

> Compile note: adding `ConfirmLoading` makes `render_plugin_catalog_overlay`'s `match cat.mode` non-exhaustive, so `zoid-tui` won't compile until Task 4. To keep each commit compiling, **fold Steps of Task 4 into this commit** OR add a minimal `CatalogMode::ConfirmLoading => {}` arm now and flesh it out in Task 4. Choose the minimal-arm approach: add to the `match cat.mode` in `render.rs` a temporary `CatalogMode::ConfirmLoading => {}` so the crate compiles; Task 4 replaces it.

Apply the temporary arm, then re-run:
Run: `cargo test -p zoid-tui --lib 2>&1 | tail -6`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/render.rs
git commit -m "feat(zoid-tui): mcp confirm state machine — ConfirmLoading + McpConfirm + target toggle"
```

---

### Task 4: `zoid-tui` — render the mcp confirm, loading, and fetch-failed panes

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` (`render_plugin_catalog_overlay`, `render.rs:1146`)

**Interfaces:**
- Consumes: Task 3's `CatalogMode::ConfirmLoading`, `McpConfirm`, `McpEnvEntry`, `McpTarget`, `PluginCatalogState.mcp`/`.confirm_error`.

- [ ] **Step 1: Replace the temporary `ConfirmLoading` arm + extend `Confirm`**

In `render_plugin_catalog_overlay`, replace the `match cat.mode { ... }` body. Keep the existing `List` arm verbatim. Replace the temporary `ConfirmLoading` arm and the `Confirm` arm with:

```rust
        CatalogMode::ConfirmLoading => {
            frame.render_widget(
                Paragraph::new("Fetching manifest…")
                    .alignment(ratatui::layout::Alignment::Center),
                inner,
            );
        }
        CatalogMode::Confirm => {
            if let Some(err) = &cat.confirm_error {
                frame.render_widget(
                    Paragraph::new(format!("fetch failed: {err}"))
                        .style(Style::new().fg(color::ERROR)),
                    inner,
                );
            } else if let Some(mcp) = &cat.mcp {
                let cmd = if mcp.args.is_empty() {
                    mcp.command.clone()
                } else {
                    format!("{} {}", mcp.command, mcp.args.join(" "))
                };
                let mut lines = vec![
                    Line::from(Span::styled(mcp.server_name.clone(), Style::new().fg(color::TXT))),
                    Line::from(Span::styled(cmd, Style::new().fg(color::DIM))),
                ];
                for e in &mcp.env {
                    let mut spans = vec![Span::styled(
                        format!("env: {} = {}", e.key, e.value),
                        Style::new().fg(color::DIM),
                    )];
                    if e.unset {
                        spans.push(Span::styled("  ⚠ not set", Style::new().fg(color::ERROR)));
                    }
                    lines.push(Line::from(spans));
                }
                let (u, p) = match mcp.target {
                    crate::state::McpTarget::User => ("[u] user", " p  project"),
                    crate::state::McpTarget::Project => (" u  user", "[p] project"),
                };
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("target: {u} / {p}   (u/p to change)"),
                    Style::new().fg(color::DIM),
                )));
                lines.push(Line::from(Span::styled(
                    "Install this MCP server? [y/N]",
                    Style::new().fg(color::CHAT_ACCENT),
                )));
                frame.render_widget(Paragraph::new(lines), inner);
            } else if let Some(row) = cat.selected() {
                let license = row.license.as_deref().unwrap_or("(none)");
                let lines = vec![
                    Line::from(Span::styled(row.name.clone(), Style::new().fg(color::TXT))),
                    Line::from(Span::styled(row.source_label.clone(), Style::new().fg(color::DIM))),
                    Line::from(Span::styled(
                        format!("kind: {}", row.kind_label),
                        Style::new().fg(color::DIM),
                    )),
                    Line::from(Span::styled(
                        format!("license: {license}"),
                        Style::new().fg(color::DIM),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Install this pack? [y/N]",
                        Style::new().fg(color::CHAT_ACCENT),
                    )),
                ];
                frame.render_widget(Paragraph::new(lines), inner);
            }
        }
```

- [ ] **Step 2: Verify it compiles + existing render tests pass**

Run: `cargo test -p zoid-tui --lib 2>&1 | tail -6`
Expected: PASS. Exhaustive `match cat.mode` now has all three real arms.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(zoid-tui): render mcp confirm card, loading, and fetch-failed panes"
```

---

### Task 5: `zoid` — confirm-time manifest fetch (carrier + map + id-guarded apply)

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`AgentUpdate`)
- Modify: `crates/zoid/src/main.rs` (`map_catalog_entries`, `CatalogEnterConfirm` handler, recv dispatch, new fns)

**Interfaces:**
- Consumes: Task 1 (`PluginManifest.mcp`, `McpManifest`, `McpServerSpec`), Task 3 (`begin_confirm_loading`, `set_mcp_confirm`, `set_confirm_error`, `McpConfirm`, `McpEnvEntry`, `McpTarget`, `CatalogMode`).
- Produces: `AgentUpdate::McpManifestFetched { id: String, res: Result<zoid_plugin::manifest::PluginManifest, String> }`; `spawn_mcp_manifest_fetch`; `apply_mcp_manifest_fetched`; mcp-including catalog filter.

- [ ] **Step 1: Add the carrier update variant**

In `crates/zoid/src/agent.rs`, add to `AgentUpdate` (after `CatalogLoaded`):

```rust
    /// A confirm-time fetch of an mcp plugin's `<id>.toml` finished. Tagged with
    /// `id` so a stale fetch (user navigated to another row) is dropped — same
    /// stale-drop discipline as `ModelsFetched`/`PluginScan`. Populates the
    /// already-open confirm; it does NOT install.
    McpManifestFetched {
        id: String,
        res: Result<zoid_plugin::manifest::PluginManifest, String>,
    },
```

- [ ] **Step 2: Write the id-guard unit test**

The guard logic is pure enough to test on `ShellState`. Add to `crates/zoid/src/main.rs`'s test module (find `mod tests` / `#[cfg(test)]`; if the guard predicate is factored into a free fn it is directly testable). Factor the guard into a free fn and test it:

```rust
#[cfg(test)]
mod mcp_confirm_guard_tests {
    use zoid_tui::state::{CatalogMode, PluginCatalogRow, PluginCatalogState};

    // mirrors the guard used in apply_mcp_manifest_fetched
    fn accepts(cat: &PluginCatalogState, arrived_id: &str) -> bool {
        cat.mode == CatalogMode::ConfirmLoading
            && cat.selected().map(|r| r.id.as_str()) == Some(arrived_id)
    }

    fn row(id: &str) -> PluginCatalogRow {
        PluginCatalogRow {
            id: id.into(), name: id.into(), kind_label: "mcp".into(),
            description: String::new(), source_label: String::new(), license: None,
        }
    }

    #[test]
    fn drops_result_for_a_row_the_user_left() {
        let mut cat = PluginCatalogState::loading();
        cat.rows = vec![row("a"), row("b")];
        cat.cursor = 1; // now on "b"
        cat.begin_confirm_loading();
        assert!(accepts(&cat, "b"));
        assert!(!accepts(&cat, "a")); // stale fetch for "a" dropped
    }

    #[test]
    fn drops_when_not_loading() {
        let mut cat = PluginCatalogState::loading();
        cat.rows = vec![row("a")];
        assert!(!accepts(&cat, "a")); // mode == List
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p zoid --lib mcp_confirm_guard 2>&1 | tail -12`
Expected: FAIL to compile until `begin_confirm_loading` etc. are in scope (they are, from Task 3) — if Task 3 is merged this test compiles and passes trivially; its purpose is to lock the guard semantics the real `apply_mcp_manifest_fetched` must mirror. If it passes immediately, that is fine — proceed.

- [ ] **Step 4: Include mcp in the catalog filter**

In `crates/zoid/src/main.rs`, `map_catalog_entries` (`main.rs:4895`), change the filter:

```rust
        .filter(|e| e.kind.iter().any(|k| k == "mode" || k == "skills" || k == "mcp"))
```

- [ ] **Step 5: Add the fetch spawn, the env-warn helper, and the id-guarded apply**

Add these free functions in `crates/zoid/src/main.rs` near `spawn_catalog_load`:

```rust
/// True if `value` references at least one `${VAR}` whose variable is unset.
/// A literal (no `${}`) is never flagged.
fn env_ref_unset(value: &str, get: &dyn Fn(&str) -> Option<String>) -> bool {
    let mut rest = value;
    while let Some(pos) = rest.find("${") {
        let after = &rest[pos + 2..];
        if let Some(end) = after.find('}') {
            if get(&after[..end]).is_none() {
                return true;
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    false
}

/// Confirm-time async fetch of an mcp plugin's `<id>.toml`. Sends
/// `McpManifestFetched`; the main loop applies it under the id guard.
fn spawn_mcp_manifest_fetch(app: &App, id: String) {
    let ui_tx = app.ui_tx.clone();
    tokio::spawn(async move {
        let res: Result<zoid_plugin::manifest::PluginManifest, String> = async {
            let body = zoid::catalog::fetch_text(&zoid::catalog::catalog_manifest_url(&id))
                .await
                .map_err(|e| format!("catalog manifest fetch failed: {e}"))?;
            let manifest = zoid_plugin::manifest::parse_manifest(&body)?;
            manifest.validate()?;
            Ok(manifest)
        }
        .await;
        let _ = ui_tx
            .send(zoid::agent::AgentUpdate::McpManifestFetched { id, res })
            .await;
    });
}

/// Apply a confirm-time manifest fetch to the open overlay — but only if the
/// overlay is still the catalog, still ConfirmLoading, and still on the SAME
/// row id (else the user navigated away → drop, protecting consent integrity).
fn apply_mcp_manifest_fetched(
    app: &mut App,
    id: String,
    res: Result<zoid_plugin::manifest::PluginManifest, String>,
) {
    use zoid_tui::state::{CatalogMode, McpConfirm, McpEnvEntry, McpTarget};
    let matches = app.shell.overlay == zoid_tui::state::Overlay::PluginCatalog
        && app.shell.plugin_catalog.as_ref().map_or(false, |c| {
            c.mode == CatalogMode::ConfirmLoading
                && c.selected().map(|r| r.id.as_str()) == Some(id.as_str())
        });
    if !matches {
        return;
    }
    let Some(cat) = app.shell.plugin_catalog.as_mut() else {
        return;
    };
    match res {
        Ok(manifest) => {
            let server = manifest.mcp.as_ref().and_then(|m| m.servers.iter().next());
            let Some((name, spec)) = server else {
                cat.set_confirm_error("manifest declares no server".into());
                return;
            };
            let env = spec
                .env
                .iter()
                .map(|(k, v)| McpEnvEntry {
                    key: k.clone(),
                    value: v.clone(),
                    unset: env_ref_unset(v, &|x| std::env::var(x).ok()),
                })
                .collect();
            cat.set_mcp_confirm(McpConfirm {
                server_name: name.clone(),
                command: spec.command.clone(),
                args: spec.args.clone(),
                env,
                target: McpTarget::User, // default: user scope (safe)
            });
        }
        Err(e) => cat.set_confirm_error(e),
    }
}
```

- [ ] **Step 6: Branch `CatalogEnterConfirm` by kind + dispatch the new update**

Replace the `Action::CatalogEnterConfirm` handler (`main.rs:4532`) with:

```rust
        Action::CatalogEnterConfirm => {
            let sel = app
                .shell
                .plugin_catalog
                .as_ref()
                .and_then(|c| c.selected())
                .map(|r| (r.id.clone(), r.kind_label.clone()));
            if let Some((id, kind)) = sel {
                if kind == "mcp" {
                    if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                        cat.begin_confirm_loading();
                    }
                    spawn_mcp_manifest_fetch(app, id);
                } else if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                    cat.enter_confirm();
                }
            }
        }
```

Add the recv-loop arm next to `CatalogLoaded` (`main.rs:3148`):

```rust
                    zoid::agent::AgentUpdate::McpManifestFetched { id, res } => {
                        apply_mcp_manifest_fetched(app, id, res);
                    }
```

- [ ] **Step 7: Run tests + build**

Run: `cargo test -p zoid --lib mcp_confirm_guard 2>&1 | tail -6` → PASS.
Run: `cargo build -p zoid 2>&1 | tail -4` → Finished (no unused-warning on `spawn_mcp_manifest_fetch`/`apply_mcp_manifest_fetched`; both are now wired).

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): confirm-time mcp manifest fetch — id-guarded carrier + mcp catalog rows"
```

---

### Task 6: `zoid` — install the mcp server (write path + confirm-yes branch)

**Files:**
- Modify: `crates/zoid/src/main.rs` (`CatalogConfirmYes` handler, `CatalogTargetToggle` handler, new `install_mcp_server`)

**Interfaces:**
- Consumes: Task 2 (`zoid_mcp::config::merge_server`, `MergeOutcome`, `McpServerConfig`), Task 3 (`McpConfirm`, `McpTarget`), existing `resolve_config_dir` (`main.rs:65`).

- [ ] **Step 1: Write the target-path + mapping test**

`install_mcp_server`'s side effect is a file write; factor the pure parts (target path + `McpConfirm` → `McpServerConfig`) into testable helpers and test them. Add to `crates/zoid/src/main.rs` tests:

```rust
#[cfg(test)]
mod mcp_install_tests {
    use super::mcp_target_path;
    use zoid_tui::state::McpTarget;

    #[test]
    fn user_target_is_config_mcp_json() {
        let p = mcp_target_path(McpTarget::User, std::path::Path::new("/cfg"), std::path::Path::new("/repo"));
        assert_eq!(p, std::path::Path::new("/cfg/mcp.json"));
    }

    #[test]
    fn project_target_is_cwd_dot_mcp_json() {
        let p = mcp_target_path(McpTarget::Project, std::path::Path::new("/cfg"), std::path::Path::new("/repo"));
        assert_eq!(p, std::path::Path::new("/repo/.mcp.json"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p zoid --lib mcp_install_tests 2>&1 | tail -8`
Expected: FAIL — `mcp_target_path` undefined.

- [ ] **Step 3: Implement `mcp_target_path` + `install_mcp_server`**

Add to `crates/zoid/src/main.rs` near `install_plugin`:

```rust
/// Resolve the `.mcp.json` an mcp install writes to. Pure (dirs injected) for tests.
fn mcp_target_path(
    target: zoid_tui::state::McpTarget,
    config_dir: &std::path::Path,
    cwd: &std::path::Path,
) -> std::path::PathBuf {
    match target {
        zoid_tui::state::McpTarget::User => config_dir.join("mcp.json"),
        zoid_tui::state::McpTarget::Project => cwd.join(".mcp.json"),
    }
}

/// Write the confirmed mcp server into the chosen `.mcp.json` (additive, atomic,
/// skip-on-collision) and report the outcome + a restart hint. Uses the carried
/// confirm — never re-enters `install_plugin` (whose catalog id-path requires
/// `[source]`, which an mcp manifest lacks).
fn install_mcp_server(app: &mut App, confirm: &zoid_tui::state::McpConfirm) {
    let config_dir = resolve_config_dir(|k| std::env::var(k).ok());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let path = mcp_target_path(confirm.target, &config_dir, &cwd);

    let server = zoid_mcp::config::McpServerConfig {
        command: confirm.command.clone(),
        args: confirm.args.clone(),
        env: confirm
            .env
            .iter()
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect(),
    };

    let hint = match zoid_mcp::config::merge_server(&path, &confirm.server_name, &server) {
        Ok(zoid_mcp::config::MergeOutcome::Inserted) => format!(
            "✓ wrote '{}' to {} · restart zoid to connect",
            confirm.server_name,
            path.display()
        ),
        Ok(zoid_mcp::config::MergeOutcome::SkippedExisting) => format!(
            "ℹ '{}' already configured in {} — left unchanged",
            confirm.server_name,
            path.display()
        ),
        Err(e) => format!("mcp install failed: {e}"),
    };
    app.shell.status_hint = Some(hint);
}
```

- [ ] **Step 4: Branch `CatalogConfirmYes` + handle `CatalogTargetToggle`**

Replace the `Action::CatalogConfirmYes` handler (`main.rs:4542`) with:

```rust
        Action::CatalogConfirmYes => {
            // mcp path: install from the carried confirm. Must NOT re-enter
            // install_plugin — its catalog id-path requires [source].
            let mcp = app.shell.plugin_catalog.as_ref().and_then(|c| c.mcp.clone());
            if let Some(confirm) = mcp {
                app.shell.plugin_catalog = None;
                app.shell.overlay = Overlay::None;
                install_mcp_server(app, &confirm);
            } else {
                let id = app
                    .shell
                    .plugin_catalog
                    .as_ref()
                    .and_then(|cat| cat.selected())
                    .map(|row| row.id.clone());
                app.shell.plugin_catalog = None;
                app.shell.overlay = Overlay::None;
                if let Some(id) = id {
                    install_plugin(app, id);
                }
            }
        }
```

Add a `CatalogTargetToggle` handler next to the other `Catalog*` arms:

```rust
        Action::CatalogTargetToggle => {
            if let Some(cat) = app.shell.plugin_catalog.as_mut() {
                cat.toggle_target();
            }
        }
```

- [ ] **Step 5: Run tests + full workspace build/test**

Run: `cargo test -p zoid --lib mcp_install_tests 2>&1 | tail -6` → PASS.
Run: `cargo test --workspace 2>&1 | tail -6` → all green.

- [ ] **Step 6: Manual smoke (optional, documents the happy path)**

```bash
# Confirms merge_server end-to-end against a temp file without the TUI.
cargo run -p zoid --quiet -- --version >/dev/null 2>&1 || true
```
(There is no headless install CLI; the smoke path is the workspace tests above. The live path is `:plugin` → select an `[mcp]` row → `y`.)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): install_mcp_server — write .mcp.json from the carried confirm + target toggle"
```

---

## Post-merge follow-up (NOT an SDD task in this private repo)

Once merged, add a first real `mcp` manifest to the **public** `strvmarv/zoid-releases` repo
(`plugins/<id>.toml`, e.g. a `context7`/`github` server) so an mcp row actually appears in the
live catalog. CI (`catalog-index.yml`) regenerates `index.json` — `gen_index.py` already carries
the `kind` array through unchanged, so no generator change is needed. Never leak private internals.

## Global self-review notes (for the executing controller)

- **Spec coverage:** A=Task 1; B=Task 2; C=Task 3 (state) + Task 5 (carrier/apply/id-guard); D=Task 5 (filter/fetch) + Task 6 (install); E=Task 3/4 (render) + Task 6 (target). Error-handling table rows map to Task 1 (validate), Task 2 (merge), Task 5 (fetch-fail/id-drop).
- **Type consistency:** `McpConfirm`/`McpEnvEntry`/`McpTarget` defined in Task 3 are used verbatim in Tasks 4/5/6; `merge_server(path, name, &McpServerConfig)` defined in Task 2 is called in Task 6; `AgentUpdate::McpManifestFetched { id, res }` defined in Task 5 is dispatched in Task 5.
- **Commit-compiles rule:** Task 3 must land the temporary `ConfirmLoading => {}` render arm (Step 5 note) so every commit builds; Task 4 replaces it.
