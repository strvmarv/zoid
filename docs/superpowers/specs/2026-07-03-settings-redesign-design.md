# Settings Redesign — full-screen Miller-column config + transport-aware provider registry

Date: 2026-07-03
Status: Design (approved in brainstorm; awaiting spec review → writing-plans)
Supersedes: `2026-07-01-config-screen-design.md` (layout), extends `2026-07-01-model-registry.md` (registry shape)

## 1. Problem

The current settings overlay (`render_config`, `crates/zoid-tui/src/render.rs:656`) is a small, content-sized centered card with a **stacked** layout: all section titles print on top, then the active section's fields below ("section over detail"). It floors at 40 columns and never grows to use the screen, so it looks cramped on a wide terminal. Provider selection is a hidden `FieldKind::Cycle` — you step blindly through values with Left/Right and can't see the options.

We want:
- A roomier, **side-by-side** layout (section rail beside the detail, not above it).
- A **visible** provider/model picker instead of a blind cycle — "fit more columns."
- The model registry to own each provider's default endpoint and populate it on select (user overridable).
- Explicit provider flavors: local vs cloud Ollama, and the groundwork for three Anthropic transports (API key / CLI / SDK).

## 2. Goals / Non-goals

### In scope (this spec → one plan)
- Full-screen, three-column settings page (Sections | Fields | contextual picker), baseline **160×40**, degrades gracefully below.
- Provider/model **Miller-column cascade**: focusing `provider` pops a picker; selecting one seeds the connection field and auto-advances to `model`; `model` is also directly focusable.
- Registry generalization in `zoid-provider/src/model.rs`: entries become structured records carrying a **transport** and its default connection value.
- Real, selectable entries: `ollama-local`, `ollama-cloud`, `anthropic-api`.
- **Transport-adaptive connection field** (`base_url` for HTTP, `command` for CLI) with registry-default **seeding** + `[user]` override, using the existing provenance model.
- `anthropic-cli` and `anthropic-sdk` present as **`[planned]`** entries — visible, annotated, not selectable.
- **ALT+P quick-switch**: the same picker component in a compact floating card, both provider + model visible, for fast mid-work switching.
- Legacy config alias map so existing configs (`provider = "ollama"` / `"anthropic"`) keep working.

### Out of scope (separate future specs)
- Implementing the `anthropic-cli` subprocess provider (zoid-as-orchestrator over Claude Code) and `anthropic-sdk`. This spec only builds the seam + `[planned]` entries; a `[planned]` entry lights up by flipping status and adding a `Provider` impl — **no settings-UI rework**.
- Pricing, model routing, and any Economy-section behavior changes.

## 3. Current state (what exists)

- `config_view.rs` — pure view-model: `Section { title, rows: Vec<FieldRow> }`, `FieldRow { label, value, kind, source, env_shadowed }`, `FieldKind { Text, Uint, Bool, Cycle(&[&str]), Secret }`. `build_sections` produces Provider & Model / Economy / Interface / Secrets.
- `render.rs:656 render_config` — builds one `Vec<Line>` (titles, blank, active-section rows, footer) into a single `Paragraph` in a centered card.
- `state.rs ShellState` — `config_section: usize`, `config_field: usize`, `config_edit: Option<String>`, `config_sections: Vec<Section>`.
- `route.rs route_config_key` — Tab/BackTab switch section; Left/Right change value (bool toggle / `Cycle` step via `ConfigCycle(dir)`); Up/Down move field; Enter edits.
- `zoid-provider/src/model.rs` — `KNOWN_PROVIDERS = ["ollama", "anthropic"]`, `models_for(provider)`, `model_info(model) -> ModelInfo { context_window }` (string-match stub). Default endpoints are **hardcoded in the provider structs** (`ollama.rs:147 "https://ollama.com"`, `anthropic.rs:107 "https://api.anthropic.com"`) — not in the registry.

## 4. Design

### 4.1 Shell — full-screen page
Settings render into `frame.area()` (whole frame) rather than a centered card. Baseline **160×40**; below baseline it degrades gracefully and never blanks (same principle as the rail-fit allocator: collapse by priority, keep something visible). A full-frame `Clear` precedes the draw (as today).

### 4.2 Layout — three columns
`Layout::horizontal` splits the inner area into up to three columns:

1. **Sections rail** — Provider & Model / Economy / Interface / Secrets. Active section marked with the accent marker; others dim.
2. **Fields** — the active section's `FieldRow`s: label (left), value (stretched), provenance tag pinned right, env-shadow warning glyph. This is today's row rendering, moved into a column.
3. **Contextual picker** — present **only** when a *list-valued* field (provider or model) is focused. Lists the choices for that field, current value marked with `●`, highlighted row uses the selection background. Non-list fields (Text/Uint/Bool/Secret) never spawn column 3 — they edit inline in column 2.

Column 3 is contextual: it shows **providers** when `provider` is focused and **models** when `model` is focused. There is no persistent 4-wide grid — the picker follows focus.

### 4.3 Provider & Model — cascade behavior
- **Focus `provider` → Enter/Right**: pops the provider picker (col 3) listing all registry entries. Each row shows `id`, transport kind, and its endpoint/command; `[planned]` entries render dim with a `[planned]` tag and are **skipped by cursor movement** (visible, not selectable). Current provider marked `●`.
- **Select a provider (Enter on an available entry)**: sets `provider`, **seeds the connection field** (base_url or command) from the registry default tagged `[default]`, and **auto-advances focus to `model`**, popping the models picker in col 3. No default *model* is guessed — `model` shows `— choose`.
- **Focus `model` directly**: pops the models picker (`models_for(provider)`) with no need to touch provider first. Selecting sets `model`.
- **Left/Esc** in a picker collapses column 3 back to the field list.

### 4.4 Registry model (`zoid-provider/src/model.rs`)
Generalize the flat `KNOWN_PROVIDERS` list into structured entries:

```
enum Transport {
    Http { default_base_url: &'static str },
    Cli  { default_command: &'static str },
    Sdk,
}
enum Status { Available, Planned }
struct ProviderEntry {
    id:        &'static str,   // stable key, hyphenated family-variant slug
    display:   &'static str,   // e.g. "anthropic · Claude Code CLI"
    family:    &'static str,   // "ollama" | "anthropic"
    transport: Transport,
    models:    &'static [&'static str],
    status:    Status,
}
```

Entries:
| id | family | transport | connection default | models | status |
|----|--------|-----------|--------------------|--------|--------|
| `ollama-local` | ollama | Http | `http://localhost:11434` | local tags | Available |
| `ollama-cloud` | ollama | Http | `https://ollama.com` | `glm-5.2:cloud` | Available |
| `anthropic-api` | anthropic | Http | `https://api.anthropic.com` | `claude-sonnet-4-6`, `claude-opus-4-8` | Available |
| `anthropic-cli` | anthropic | Cli | `claude` | claude models (via `--model`) | Planned |
| `anthropic-sdk` | anthropic | Sdk | — | claude models | Planned |

**Naming convention**: hyphenated `family-variant` slugs. Code reads the **struct fields** (`family`, `transport`), never substring-parses the id — the slug is a stable key + human-readable label. Hyphen (not colon) to avoid collision with model tags like `glm-5.2:cloud` and to stay env/config-friendly.

**Single source of truth**: the registry owns default endpoints. The provider constructors (`OllamaProvider::new`, `AnthropicProvider::new`) read their default from the registry rather than hardcoding it, so settings and the actual HTTP call can never disagree. (`with_base_url` override behavior is unchanged.)

**Legacy alias map**: on config load, map deprecated ids to canonical ones so existing configs and `select_provider`'s env defaults keep working:
- `ollama` → `ollama-cloud` (preserves today's behavior, where bare ollama = cloud endpoint)
- `anthropic` → `anthropic-api`

`models_for` and endpoint lookup are keyed by the canonical id. `select_provider` (env-based auto-select) resolves to `ollama-cloud` when `OLLAMA_API_KEY` is set, `anthropic-api` when `ANTHROPIC_API_KEY` is set, else offline echo — unchanged behavior, explicit ids.

### 4.5 Connection field — transport-adaptive + seeding
Column 2's connection row is derived from the selected provider's transport:
- **Http** → label `base_url`, value = config `base_url` or registry default.
- **Cli** → label `command`, value = config command or registry default (`claude`).
- **Sdk** → no connection row.

Seeding uses the existing provenance model (`zoid_core::config::Source`): a value taken from the registry shows `[default]` (`Source::Default`); typing over it flips to `[user]` (`Source::UserGlobal`); clearing it back to empty re-seeds the registry default. `[env]`/`[repo]`/`[local]` provenance and the env-shadow warning are unchanged.

Config gains storage for the CLI `command` alongside `base_url` (or a single generalized "connection override" keyed by transport — decided in the plan; both preserve the provenance tag).

### 4.6 Other sections — inline
Economy, Interface, and Secrets edit **inline in column 2** exactly as today (Uint/Bool/Text/Secret via the existing `config_edit` buffer and Left/Right/Enter handling). They never spawn column 3.

### 4.7 ALT+P quick-switch
A new overlay (`Overlay::ProviderSwitch` or similar) bound to `Alt+P`, available from the chat view. It is a **compact floating card** over dimmed chat that reuses the **same picker component** as settings, but shows **both** provider and model panes at once (no drilling) for speed:
- Left pane: providers (available entries; `[planned]` skipped). Right pane: `models_for(highlighted provider)`.
- `←/→` switch pane, `↑/↓` move, `Enter` applies the provider+model pair (and seeds the connection field, same as settings), `Esc` cancels.
- Same registry + selection logic as settings — one picker component, two entry points and two chromes (full-screen column vs floating two-pane).

### 4.8 Keybindings
Settings: `Tab`/`Shift+Tab` section · `↑/↓` move within the focused column · `→`/`Enter` drill into / select · `←`/`Esc` back (collapse column 3, else close) · typing edits inline text/uint fields.
Quick-switch: `Alt+P` open · `←/→` pane · `↑/↓` move · `Enter` apply · `Esc` cancel.

### 4.9 Graceful degradation below baseline
- If width is too narrow for three columns, column 3 (the picker) renders as a **floating sub-card overlaying column 2** rather than shrinking to nothing — the picker is transient, so overlaying is acceptable and keeps every row legible.
- The sections rail has a minimum width; below it, section titles abbreviate before the layout blanks. Never render an empty/borderless frame.

## 5. Affected code

- `zoid-provider/src/model.rs` — new `Transport`/`Status`/`ProviderEntry`; entry table; canonical-id accessors (`entries()`, `entry(id)`, `models_for(id)`, `default_connection(id)`); legacy alias resolution. `model_info` unchanged here (the context-window hardening is the separate ACM step-0).
- `zoid-provider/src/ollama.rs`, `anthropic.rs` — read default endpoint from the registry instead of hardcoding.
- `zoid-core/src/config.rs` — `select_provider` resolves canonical ids; alias map on load; storage for the transport connection override (`command`) + its provenance.
- `zoid-tui/src/config_view.rs` — Provider & Model section becomes transport-aware: provider/model are list-valued (feed the picker), connection row label/value derives from transport. Add a `FieldKind` (or metadata) marking a field as "opens a picker" vs inline.
- `zoid-tui/src/state.rs` — `ShellState` gains column-focus state (which of Sections/Fields/Picker is active), picker-open flag + picker cursor, and quick-switch overlay state.
- `zoid-tui/src/render.rs` — rewrite `render_config` from a single Paragraph into a three-column `Layout` render; new `render_provider_switch` for the ALT+P card; a shared picker-column/pane helper used by both.
- `zoid-tui/src/route.rs` — expand `route_config_key` for column focus + drill/back + picker selection (replaces blind `ConfigCycle`); add `Alt+P` → open quick-switch and its key routing.
- `zoid-tui/src/lib.rs` / `main.rs` — wire the new overlay + actions (open quick-switch, commit provider/model, seed connection field).

## 6. Testing
- `model.rs` unit tests: entry table integrity, `models_for`/`default_connection` per id, legacy alias resolution (`ollama`→`ollama-cloud`, `anthropic`→`anthropic-api`), `[planned]` entries excluded from selectable iteration.
- `config_view.rs`: Provider & Model section renders the transport-correct connection row (base_url vs command); seeding produces `[default]` provenance; override produces `[user]`.
- `route.rs`: column focus transitions; provider-select seeds connection + auto-advances to model; direct model focus; `[planned]` skipped by cursor; Esc collapse-then-close.
- Snapshot tests (`shell_snapshot`): full-screen settings at 160×40 (provider picker open, model picker open, CLI connection field), and the ALT+P card. Add a below-baseline degradation snapshot (picker overlays col 2).
- Existing config/provider tests updated for canonical ids + registry-sourced endpoints.

## 7. Open items / risks
- **Bare-id migration**: today `provider = "ollama"` means the cloud endpoint; the alias maps it to `ollama-cloud` to preserve behavior. Anyone who *intended* local Ollama with a bare `ollama` + a localhost `base_url` override keeps working because their explicit `base_url` wins (provenance `[user]`). Documented, low risk.
- **Config storage for CLI `command`**: whether to add a dedicated field or a generalized transport-keyed connection override is a plan-level decision; both must carry provenance.
- **`model_info` context-window stub** is intentionally left to the separate ACM step-0 hardening; not touched here beyond keying by canonical id.

## 8. Next
Spec review by user → `superpowers:writing-plans` to produce the phased implementation plan.
