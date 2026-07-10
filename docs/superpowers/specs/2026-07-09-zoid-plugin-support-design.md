# zoid Plugin Support (v1) — Design

- **Date:** 2026-07-09
- **Status:** Approved (brainstorming); pending implementation plan
- **Scope:** First-class plugin installation for zoid, using Superpowers as the pilot. Replaces the bespoke `superpowers_install.rs` recipe with a declarative, manifest-driven installer. MCP install is deferred but seamed.

## 1. Motivation

zoid has two install paths today:

1. **Model-driven wizard** — `:mode import <github-url>` fetches a repo tree, the LLM proposes a `ModeMapping`, the user approves, and `mode_wizard::materialize` writes files plus a `.zoid-provenance.json` sidecar.
2. **Bespoke deterministic installer** — `crates/zoid/src/superpowers_install.rs` hardcodes one repo's recipe (pinned SHA, "loader `SKILL.md` → `mode.md`", `strip_prefix "skills/"`, a mechanically generated overlay body) and calls the same `materialize`.

The bespoke path means "how to install Superpowers" lives in zoid's Rust. Adding another plugin means another pull request against zoid. The goal is to **move the recipe out of code and into data**: a plugin ships (or zoid bundles) a declarative `.zoid/plugin.toml` manifest, and zoid becomes a generic plugin host that reads it. This simplifies the Superpowers install and aligns zoid with the plugin ecosystem.

## 2. Decisions (locked during brainstorming)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Manifest source | **Hybrid** resolution: repo `.zoid/` → zoid-bundled manifest → model-driven wizard fallback |
| 2 | Install scope | **v1 = scope 2** (mode materialization + fixed effect vocabulary). **End goal = full plugin support (scope 3)**; defer unbuilt pieces behind explicit typed seams |
| 3 | Command surface | New `:plugin install <id\|url>` verb; Superpowers becomes a bundled manifest; delete the bespoke recipe; `:mode install superpowers` becomes an alias |
| 4 | Trust model | **Gate on danger, not source.** Classify each effect's risk; prompt only for Dangerous effects, regardless of bundled vs fetched. Provenance is shown, not gating |
| 5 | Manifest format | **TOML** (`.zoid/plugin.toml`) — matches zoid's `config.toml` idiom (layered TOML, unknown keys warn not fail) |
| 6 | Code placement | New pure **`zoid-plugin`** crate (schema/effects/resolution, IO-free) + bin installer (`plugin_install.rs`) reusing `github_fetch` + `mode_wizard::materialize` |

## 3. Architecture

### 3.1 Crate layout

New crate `zoid-plugin` holds the pure, IO-free core (unit-testable exactly like `zoid-core::wizard`):

```
zoid-plugin/src/
  manifest.rs   — PluginManifest schema, parse_toml, validate()
  effect.rs     — Effect enum + RiskTier + classification
  resolve.rs    — ManifestSource resolution decision (pure: inputs → chosen source)
  plan.rs       — InstallPlan builder (manifest + scan → plan), reused mode-mapping logic
  lib.rs
```

The bin does effectful orchestration:

```
crates/zoid/src/plugin_install.rs — fetch, apply, write provenance;
                                     reuses github_fetch + mode_wizard::materialize
```

This preserves zoid's existing pure/effectful split: `zoid-plugin` never touches the filesystem or network, so the schema + planning layer is deterministically testable.

### 3.2 Manifest schema (`.zoid/plugin.toml`, `schema = 1`)

```toml
[plugin]
id      = "superpowers"      # stable identity; install dir + provenance key
schema  = 1                  # forward-compat; unknown minor keys warn, not fail
kind    = ["mode"]           # artifact types this plugin installs (v1: only "mode")
name    = "Superpowers"      # display
description = "Skill-driven agent workflows"

[source]                     # provenance anchor
repo   = "obra/superpowers"  #   bundled manifests carry this;
ref    = "d884ae04edebef577e82ff7c4e143debd0bbec99"   # repo-.zoid/ manifests MAY omit it
subtree = "skills"           #   (the repo being pointed at IS the source)

[mode]                       # the "mode" artifact handler's recipe
loader  = "using-superpowers/SKILL.md"   # becomes mode.md overlay (relative to subtree)
map     = { strip_prefix = "" }          # path rewrite for materialized files
body    = "from-skill-frontmatter"       # how to synthesize the overlay body

[[install]]                  # ordered post-materialize effects (fixed vocabulary)
effect = "activate"
[[install]]
effect = "onboarding_hint"
text   = "Superpowers installed — skills auto-load before work."
```

Everything `superpowers_install.rs` hardcodes today (pinned SHA, loader→`mode.md`, `strip_prefix`, frontmatter-generated body) becomes these fields. `manifests/superpowers.toml` ships inside zoid as the bundled copy.

**Schema notes:**

- `kind` is an **array** so a future plugin declaring `["mode","commands","mcp"]` needs no schema break; v1 errors on any kind but `"mode"`.
- `[source]` is **optional for repo-supplied manifests** — when the manifest lives inside the repo being fetched, the repo+ref already is the provenance. Bundled manifests must carry it because they reference a repo they do not live in.
- `body = "from-skill-frontmatter"` selects the mechanical overlay-body generator (name+description list, alphabetical, loader excluded). It is an enum of body strategies, not free text, so it stays deterministic.

### 3.3 Effect model & risk classification (`effect.rs`)

```rust
pub enum Effect {
    Activate,                                     // set the new mode active
    OnboardingHint { text: String },              // status/onboarding line
    SetConfig { key: String, value: TomlValue },  // write a config.toml key
    // --- seams (declared, v1 rejects at validate) ---
    // InstallMcp { .. }, RunShell { .. }, RegisterCommand { .. }
}

pub enum RiskTier { Safe, Dangerous }

impl Effect {
    pub fn risk(&self) -> RiskTier {
        match self {
            Effect::Activate | Effect::OnboardingHint { .. } => RiskTier::Safe,
            Effect::SetConfig { key, .. } => classify_config_key(key),
        }
    }
}
```

- **Materialize is Safe.** It only writes inside `<cfg>/modes/<plugin-id>/`, the sandbox `materialize` + `rollback` already enforce.
- Install proceeds **without a wall for Safe-only plans** and **prompts only when the plan contains a Dangerous effect** — bundled or fetched alike. Provenance (repo/ref/SHA) is shown in a one-line summary either way; it is informational, not a gate.
- `classify_config_key` is **fail-closed**: an allowlist of known-safe keys (`skills.source_dirs`, `modes.source_dirs`) is Safe; everything else is Dangerous. `provider`, `base_url`, `[approval]`, and secrets-adjacent keys are never silently written.

### 3.4 Source resolution (`resolve.rs`, pure)

```
:plugin install <id|url>
  id matches bundled?          → BundledManifest
  url given → fetch tree:
     repo has .zoid/plugin.toml → RepoManifest (reuse the fetched tree)
     else bundled-for-url       → BundledManifest
     else                       → fall back to model-driven wizard (existing path)
```

The decision function is pure (inputs → chosen `ManifestSource`); the fetch it depends on is performed by the bin and passed in, keeping `resolve.rs` IO-free and table-testable.

### 3.5 Installer flow (`plugin_install.rs`, bin)

```
1. RESOLVE source (resolve.rs).
2. FETCH artifact tree (github_fetch::fetch_tree) at manifest [source].ref.
   (repo-manifest case reuses the tree already fetched during resolution.)
3. PLAN (plan.rs): manifest + scan → InstallPlan { mode_mapping, effects, provenance_preview }.
   - reuse generic mode-mapping logic (loader→mode.md, map, body strategy)
   - validate: unknown kind → UnsupportedKind; unknown effect → error; flag Dangerous effects
4. CONFIRM: plan has any Dangerous effect → approval overlay (reuse detailed_approval_summary);
            else → apply directly with a one-line provenance summary.
5. APPLY (off-thread; AgentUpdate back to main loop, mirroring today's SuperpowersScan path):
   a. materialize(mode_mapping) into <cfg>/modes/<id>  (clean-slate remove_dir_all first)
   b. run effects in order (activate, config.set, onboarding_hint)
   c. write plugin provenance sidecar
   d. rebuild registry, set_active if Activate, sync_mode_mirror
```

Step 5 reuses the exact async orchestration used today for `SuperpowersScan` (`tokio::spawn` → `AgentUpdate` → apply on main loop), so the TUI threading model is unchanged — only the body of `finish_install` becomes generic.

### 3.6 Provenance & lifecycle (`.zoid-plugin.json`, `schema = 1`)

The existing per-mode `.zoid-provenance.json` records files only. A plugin install also runs effects, so uninstall must know what to revert. A **superset sidecar** is written alongside the mode:

```jsonc
{
  "schema": 1,
  "plugin": { "id": "superpowers", "manifest_ref": "d884ae0…", "installed_at": "…" },
  "source": { "repo": "obra/superpowers", "ref": "d884ae0…", "subtree": "skills",
              "origin": "bundled" },          // or "repo" / "url"
  "files":  [ /* same per-file sha/snapshot as today, for 3-way update */ ],
  "effects_applied": [
     { "effect": "activate" },
     { "effect": "set_config", "key": "skills.source_dirs",
       "prev": null, "new": ["~/.config/zoid/modes/superpowers"] }  // prev enables clean revert
  ]
}
```

- **`:plugin uninstall <id>`** — revert `effects_applied` in reverse (restore each `prev`), `remove_dir_all` the mode dir, rebuild registry. Recording `prev` makes uninstall truthful (restore the exact prior value) rather than best-effort; a config key the user already had set survives uninstall untouched.
- **`:plugin update <id>`** — reuse the existing `classify_update` 3-way reconciliation on `files`; re-run effects idempotently.

## 4. Command surface

```
:plugin install <id>        # bundled manifest by id  (e.g. superpowers)
:plugin install <url>       # repo .zoid/ → bundled-for-url → wizard fallback
:plugin uninstall <id>
:plugin update <id>
:plugin list                # installed plugins + source/ref + active state
:mode install superpowers   # ALIAS → :plugin install superpowers
```

- New `Command::Plugin(PluginCmd)` variants in `zoid-tui/src/command.rs`, parsed alongside the existing `ModeInstallSuperpowers`.
- The old literal-match `"mode install superpowers"` (`command.rs:78`) is retargeted to emit the alias.
- Palette gains a `plugin install superpowers` row, replacing today's `install superpowers` row (`palette.rs:205`).
- `:plugin list` is a read-only overlay mirroring the existing `Overlay::Mcp` view pattern.

## 5. Superpowers migration

- **New:** `manifests/superpowers.toml` (bundled) carrying pinned SHA `d884ae04edebef577e82ff7c4e143debd0bbec99`, loader, map, body strategy, and `[[install]]` `activate` + `onboarding_hint`.
- **Deleted:** `superpowers_install.rs`'s recipe — `superpowers_mapping`, `generate_mode_body`, the URL/loader consts. The generic mode-mapping + frontmatter-body logic they encoded moves into `zoid-plugin` so it is reusable and manifest-driven.
- **Kept:** the clean-slate + `materialize` reuse from `finish_install`, now called generically.
- **Retargeted:** the onboarding line (`onboarding.rs:26`) points to `:plugin install superpowers`.
- **Net:** ~120 lines of bespoke Rust → one TOML file + a generic installer.

## 6. Deferred seams (explicit, typed — not TODOs)

| Seam | v1 behavior | Later |
|------|-------------|-------|
| `kind` values beyond `"mode"` (`commands`, `agents`, `skills`) | `validate()` returns `UnsupportedKind` | register per-kind handler |
| `Effect::InstallMcp` (overlaps `zoid-mcp`) | parse-reject with a clear message | wire to `McpManager`; classify **Dangerous** |
| `Effect::RunShell` | not in enum | add as **Dangerous** |
| repo `.zoid/` beyond `plugin.toml` (bundled assets) | only `plugin.toml` is read | richer repo layout |

Each is a named variant that fails cleanly: a manifest using a future feature gets "not supported in this zoid version" — never a silent partial install.

## 7. Testing

- **`zoid-plugin` unit tests** (pure, no IO): manifest parse/validate (good + malformed + unknown-key-warns), resolution decision table, effect risk classification (allowlist vs fail-closed), plan generation from a synthetic scan.
- **Bin integration:** reuse the `HttpGithubApi` fake / fixture-tree pattern. A Superpowers fixture asserts the generic installer produces **byte-identical output to today's bespoke installer** (regression guard for the deletion).
- **Provenance round-trip:** install → sidecar written → uninstall reverts `effects_applied` (including a `set_config` `prev` restore) → mode dir gone.

## 8. Out of scope (v1)

- MCP install (seamed only).
- Non-`mode` artifact kinds (commands/agents/skills — seamed only).
- Arbitrary shell effects.
- A remote plugin registry/index (install is by bundled id or explicit URL only).
