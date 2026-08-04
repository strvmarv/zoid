# Onboarding: first-run LLM connection wizard — design

**Source:** brainstorming session (user: gomanjoe) — guiding first-time users to
set up their first LLM connection.

**Problem.** A first-time user who launches zoid with no provider configured and
no API key sees the same "Try one of these to get started" empty-state prompts
as a user with a working connection. Their first message silently hits the
offline `FakeProvider` (the binary always runs via `select_provider`'s fallback).
There is no guidance to set up a connection first. The existing empty-state
onboarding (`onboarding.rs`, spec `2026-07-06-empty-state-guidance-design.md`)
knows nothing about connection readiness — it keys only on
`first_time_user = sessions.is_empty()`.

**Goal.** A first-run **wizard** — a dedicated full-screen overlay — that guides
the user through choosing a provider and entering an API key, fires only when
there is no working connection, and writes the result back through the existing
config + secret-store paths. After completing it, the user lands in the normal
empty-state chat with a live connection.

---

## 1. Scope

### In scope

- A new `Overlay::Onboarding` variant + full-screen wizard view
  (`render_onboarding` in `render.rs`).
- A 2–3 step linear wizard: **Provider → API key → Model** (step 3 only if the
  chosen provider has >1 registry model).
- An **"unconfigured" sentinel**: compiled default for `provider` becomes empty
  string (was `"ollama"`).
- A **wizard gate** predicate: `first_time_user && (provider empty || (provider
  requires key && no key found))`, with `ollama-local` exempt.
- A new `key_url: Option<&'static str>` field on `ProviderEntry` (the
  key-acquisition URL shown in the API-key step).
- Boot-time orchestration: when the gate fires, open the overlay and seed
  wizard state.
- Config write-back through existing `set_in_toml` + `SecretStore::set` +
  `select_provider` re-selection (no new write paths).
- `insta` snapshot tests for each wizard step at two widths.

### Out of scope

- **Reachability probing.** No TCP/HTTP probe of any provider. Readiness is
  "provider chosen + key present (for key-requiring providers)." `ollama-local`
  is assumed correct; its local troubleshooting is the user's responsibility.
- **Returning-user treatment.** A returning user with a broken/missing key does
  not get the wizard. The gate includes `first_time_user`. A lighter inline hint
  for returning users with a missing key is a future enhancement, not this spec.
- **Reopening the wizard mid-session.** After `Esc` (skip), the wizard does not
  re-fire within the same session. It re-fires on the next launch if the gate
  still holds. No `:onboarding` command to reopen manually in v1.
- **A "wizard seen" persistence flag.** The gate itself is the persistence — no
  separate flag is stored.
- **Multi-provider configuration.** The wizard configures exactly one provider +
  key. Multi-provider profiles / switching presets are out of scope.

---

## 2. The sentinel: representing "unconfigured"

### Current state

The compiled default (`Config::default()` in `crates/zoid-core/src/config.rs`)
is:

```rust
provider: "ollama".to_string(),   // canonical_id("ollama") → "ollama-cloud"
model: String::new(),             // empty → provider picks its default
base_url: None,
```

`"ollama"` canonicalizes to `"ollama-cloud"` — a key-requiring provider. With no
`OLLAMA_API_KEY`, `select_provider` falls back to `FakeProvider` and reports
`has_key = false`. So a fresh launch is silently "offline" but `provider` is
non-empty — "unconfigured" is not representable.

### Change

The compiled default for `provider` becomes **empty string**:

```rust
provider: String::new(),   // empty = unconfigured (was "ollama".to_string())
model: String::new(),      // unchanged (already empty)
base_url: None,            // unchanged
```

**Empty `provider` = "no provider chosen."** No new enum, no `Option<String>`.
The wizard writes a non-empty provider id on completion; until then `provider`
stays empty. `select_provider` already handles an unknown/empty provider id
gracefully (family lookup fails → `FakeProvider` fallback).

### Migration for existing users

There is **no silent migration** — we do not auto-write `"ollama-local"` for
anyone. On next launch:

- A user who never set `provider` in any TOML layer now has `provider = ""`
  (the new default). The gate fires (first-time-user branch or, if they have
  sessions, the `first_time_user` check fails and no wizard appears — they keep
  using the `FakeProvider` fallback until they `:config`).
- A user who explicitly set `provider = "anthropic-api"` (or any value) in
  their TOML keeps that value. The gate fires for them only if
  `first_time_user && no key found`.
- A user who relied on the old `"ollama"` default and has no prior sessions will
  see the wizard once, pick a provider (possibly `ollama-local`), and continue.
  This is an acceptable one-time friction for a clean "unconfigured" state.

**What `canonical_id` does with `""`:** `canonical_id("")` returns `""` (the
`other => other` arm). `entry("")` returns `None`. `select_provider`'s
`canonical_id(&config.provider) == "ollama-local"` check is false, the family
match falls through to the `_ =>` arm, `key_for("OLLAMA_API_KEY")` is likely
`None`, and `FakeProvider` is returned. All safe — no panic, no bad state.

---

## 3. The gate

A single pure predicate:

```rust
/// True when the onboarding wizard should be shown at startup. Pure; no IO.
///
/// - `first_time_user`: from `sessions.is_empty()` at boot.
/// - `config`: the resolved Config.
/// - `has_key`: the third return of `select_provider` — whether a credential
///   was found for the active provider (true for keyless `ollama-local`).
/// - `secrets_available`: whether the encrypted secret store opened successfully.
///   If false, the wizard cannot function (step 2 writes to it) and must not
///   fire — the user is directed to `:config` via the normal empty state.
fn wizard_needed(
    first_time_user: bool,
    config: &Config,
    has_key: bool,
    secrets_available: bool,
) -> bool {
    if !first_time_user || !secrets_available {
        return false;
    }
    let canon = zoid_provider::model::canonical_id(&config.provider);
    if canon == "ollama-local" {
        return false; // keyless local — assumed correct, never probed
    }
    if config.provider.trim().is_empty() {
        return true; // sentinel: no provider chosen
    }
    // provider is set + requires a key + key not found → misconfigured
    !has_key
}
```

### Where it is checked

1. **At boot**, in `run()`, after `select_provider` returns
   `(provider, label, has_key)` and `first_time_user` is known. If true, set
   `shell.overlay = Overlay::Onboarding` and seed `OnboardingState` (see §6).
2. **After the wizard completes** (writes config): the bin calls
   `select_provider` again with the new config. If the gate now evaluates false
   (provider set + key present, or `ollama-local`), the overlay closes and the
   user lands in the empty-state chat. (The wizard's own commit handler closes
   the overlay; the re-check is implicit — the wizard only completes when a
   valid state is reached.)
3. **After `Esc` (skip):** the overlay closes (`Overlay::None`), wizard state is
   dropped, and the user lands in the empty-state chat. The gate is **not**
   re-evaluated mid-session. On the next launch, the gate re-evaluates from
   scratch; if the user skipped without configuring, it fires again.

### What `has_key` means

It is the exact boolean `select_provider` already returns at boot — whether a
credential was found for the active provider. For `ollama-local`, it is `true`
(no key required). For key-requiring providers, it is `true` only if the env var
or encrypted-store key is present. No new secret-store probe is introduced.

---

## 4. Wizard steps and state machine

### Steps

| Step | When | Asks | Input | On commit |
|------|------|------|-------|-----------|
| **1. Provider** | Always | "Choose your LLM provider" | Pick-list from `config_view::provider_options("")` | Write `provider` to user-global TOML. If `ollama-local` → DONE (skip key + model). Else → step 2. |
| **2. API key** | Only if provider requires a key | "Enter your `{FRIENDLY_NAME}` API key" | Masked free-text. **Non-empty `Enter` only.** Empty `Enter` is a no-op. | Write key to encrypted secret store via `key_env_for`. Clear buffer. → step 3 (if >1 model) or DONE. |
| **3. Model** | Only if chosen provider has >1 registry model | "Pick a model (or accept the default)" | Pick-list from `config_view::model_options(provider_id, "")` + synthetic "use default" row (index 0, selects empty model). | Write `model` to user-global TOML (or leave empty for "use default"). → DONE. |

**Step 3 auto-skip:** If `model::models_for(provider_id).len() <= 1`, step 3 is
skipped silently — `model` stays empty and the provider's default model is used
at runtime (existing behavior). This keeps the wizard to 2 steps for the common
case (e.g. `ollama-cloud` has 1 model, `ollama-local` has 0).

**The "no key, no complete" rule (option 2).** For a key-requiring provider, the
only way to advance past step 2 is to enter a non-empty key. There is no "skip
key" and no "back to step 1." The user's choices are: enter a key, or `Esc` the
whole wizard. This prevents the silent dead-end of a provider configured without
a key. `ollama-local` is the escape hatch for users who have no key and want to
proceed — they pick it in step 1 and skip steps 2–3 entirely.

### State

```rust
// crates/zoid-tui/src/state.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStep {
    Provider,
    ApiKey,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingState {
    pub step: OnboardingStep,
    /// The provider id chosen in step 1 (empty until committed).
    pub chosen_provider: String,
    /// Masked key entry buffer for step 2. Cleared immediately after the key
    /// is written to the secret store — the plaintext is never held longer than
    /// necessary.
    pub key_buffer: String,
    /// Highlighted row in the current step's pick-list (steps 1 and 3).
    pub list_sel: usize,
    /// The pick-list options for the current step (providers in step 1, models
    /// in step 3). Rebuilt on step transition.
    pub options: Vec<crate::config_view::PickOption>,
}
```

### Transitions

```
gate fires → Onboarding { step: Provider, options: provider_options("") }
   │
   ├─ select ollama-local → write provider → DONE (close overlay, re-select)
   ├─ select key-requiring provider → write provider → Onboarding { step: ApiKey }
   │                                                        │
   │                                    ├─ non-empty key + Enter → write key, clear buffer
   │                                    │    → step 3 (if >1 model) or DONE
   │                                    ├─ empty key + Enter → no-op (stay)
   │                                    └─ Esc → skip wizard (overlay closed, state dropped)
   │
   └─ step 3 (if reached):
        ├─ pick "use default" → model stays "" → DONE
        ├─ pick a model → write model → DONE
        └─ Esc → skip wizard
```

**Navigation is strictly forward.** No back button, no retreat to a previous
step. `Esc` at any step skips the whole wizard (closes the overlay). This is
deliberate: it enforces the "no key, no complete" rule (there is no way to
retreat past step 2 to avoid entering a key) and avoids the confusion of what
"going back to step 2" shows (the buffer is cleared after commit).

### Completion (DONE)

1. Close the overlay: `shell.overlay = Overlay::None`, `shell.onboarding = None`.
2. Re-select the provider: `let (provider, label, has_key) =
   select_provider(&app.config, &app.secrets);` — the same call the bin makes at
   startup and after config-screen edits. Swap in the new provider.
3. The next frame, `proj.msgs.is_empty()` is true and `first_time_user` is still
   true, so the existing empty-state intercept fires — the user sees the normal
   onboarding prompts ("explain this codebase", etc.) with a working connection.

---

## 5. Full-screen view layout and rendering

### Overlay integration

A new variant on the `Overlay` enum (`crates/zoid-tui/src/state.rs`):

```rust
pub enum Overlay {
    None,
    // ... existing variants ...
    Onboarding,
}
```

Rendered in `render_shell` (`render.rs`) alongside the other overlays, full-frame
like `render_config`:

```rust
} else if state.overlay == Overlay::Onboarding {
    render_onboarding(frame, state, frame.area());
}
```

### Layout

A single-column centered card (the wizard is linear, not a browser — no
left-rail/three-column split like the config screen):

```
╭─────────────────────────────────────────────────────────────╮
│  zoid · setup                                                │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Welcome to zoid — let's connect your first LLM.             │
│                                                              │
│  ● 1 — Choose your provider                                  │
│     › ollama · local          http://localhost:11434         │
│       ollama · cloud          https://ollama.com             │
│       anthropic · api key     https://api.anthropic.com      │
│       opencode · go           https://opencode.ai/zen/go     │
│       zai · coding plan       https://api.z.ai               │
│       opencode · zen          https://opencode.ai/zen        │
│                                                              │
│  ☐ 2 — API key            (dim, locked)                      │
│  ☐ 3 — Model              (dim, locked)                      │
│                                                              │
├─────────────────────────────────────────────────────────────┤
│  ↑↓ move · Enter select · Esc skip setup                     │
╰─────────────────────────────────────────────────────────────╯
```

**Structure:**
- **Outer card**: rounded border, `color::CHAT_ACCENT`, title `" zoid · setup "`.
- **Intro line**: "Welcome to zoid — let's connect your first LLM." in
  `color::TXT`. Constant across all steps.
- **Step rail**: all three steps listed vertically, always visible. The active
  step is expanded (shows its pick-list or input inline); inactive steps are
  collapsed to a single dim line. This gives the user a sense of progress and
  what's ahead.
- **Footer**: keybind hints, same style as `render_config`'s footer.

**Step glyphs** (from the visual-language table in `docs/ux/README.md`):
- `●` active step (accent)
- `✓` completed step (ok green)
- `☐` pending/locked step (dim)

### Per-step rendering

**Step 1 — Provider (pick-list):**
`config_view::provider_options("")` rows, rendered like the config screen's
picker: highlighted row in accent with a `›` marker (`glyph::USER_TURN`), detail
line (endpoint) in dim. `ollama-local` is first (registry order) with
`(no key needed)` appended to its detail. `Up`/`Down` moves, `Enter` selects.

**Step 2 — API key (masked free-text):**

```
  ● 2 — API key
     Enter your Anthropic API key
     ┌──────────────────────────────────────────┐
     │ sk-ant-••••••••••••••••••                  │
     └──────────────────────────────────────────┘
     Get one at https://console.anthropic.com/settings/keys
```

A single-line input box (reusing the input-rendering idiom from `render_input`),
masked with `•` per char. A help line below shows the chosen provider's
`key_url`. `Enter` commits (non-empty only — empty `Enter` is a no-op).
`Backspace` deletes. `Esc` → skip whole wizard. The friendly provider name
("Anthropic") comes from the registry `display` field.

**Step 3 — Model (pick-list):**
Same pick-list rendering as step 1, with `config_view::model_options(provider_id,
"")` plus a synthetic "use default" row at index 0 (selectable, selects empty
model). If the provider has ≤1 registry model, this step is skipped entirely.

### The `render_onboarding` function

```rust
// crates/zoid-tui/src/render.rs
pub fn render_onboarding(frame: &mut Frame, state: &ShellState, area: Rect) {
    let onb = match &state.onboarding {
        Some(o) => o,
        None => return,
    };
    // Outer card (rounded, accent border, " zoid · setup " title) + footer split
    //   — same pattern as render_config.
    // Intro line.
    // Step rail: render all 3 steps with ●/✓/☐ glyphs; expand the active one.
    //   - Provider/Model: pick-list rows (PickOption), highlighted = list_sel.
    //   - ApiKey: masked input box + key_url help line.
    // Footer: "↑↓ move · Enter select · Esc skip setup"
    //   (step 2 footer: "Enter submit · Backspace delete · Esc skip setup")
}
```

**Width/degrade:** The card uses `frame.area()`. At the 100×24 floor (~51 content
cols), the pick-list detail (endpoint URL) truncates via the existing
`fit`/`truncate` helpers (same as `overview.rs`); the masked input box shrinks
to fit. The layout never overflows or panics.

### Snapshot tests

Following the `overview.rs` pattern: `insta` snapshots at two widths (160×40
baseline, 100×24 floor), one per step:
- `onboarding_step_provider` — step 1 expanded, steps 2–3 locked.
- `onboarding_step_api_key` — step 2 expanded, step 1 complete (✓), step 3
  locked. Shows a masked buffer.
- `onboarding_step_model` — step 3 expanded, steps 1–2 complete.

Each builds a `ShellState` with `overlay = Overlay::Onboarding` and the
appropriate `OnboardingState`, renders via `render_onboarding`, and snapshots.
These live in `render.rs`'s test module alongside the existing config/palette
snapshots.

---

## 6. New registry field: `key_url`

A per-provider URL for the API-key step's help line.

```rust
// crates/zoid-model/src/lib.rs
pub struct ProviderEntry {
    pub id: &'static str,
    pub display: &'static str,
    pub family: &'static str,
    pub transport: Transport,
    pub models: &'static [&'static str],
    pub status: Status,
    /// URL the onboarding wizard's API-key step links to for acquiring a key.
    /// `None` for keyless providers (ollama-local).
    pub key_url: Option<&'static str>,
}
```

Values (to confirm exact landing pages during implementation):

| Provider id | `key_url` |
|---|---|
| `ollama-local` | `None` |
| `ollama-cloud` | `Some("https://ollama.com")` |
| `opencode-go` | `Some("https://opencode.ai")` |
| `anthropic-api` | `Some("https://console.anthropic.com/settings/keys")` |
| `zai-coding-plan` | `Some("https://z.ai")` |
| `opencode-zen` | `Some("https://opencode.ai")` |

A provider with `key_url: None` is keyless — the step-1→DONE short-circuit
handles it, so the wizard never reaches step 2 for a keyless provider. Adding a
future keyless provider works without wizard changes.

---

## 7. Key routing, write-back, and bin-side orchestration

### Config write-back

The wizard writes to **user-global TOML** (`~/.config/zoid/config.toml`), the
same default target as the config screen. It reuses existing machinery — no new
write path.

**Step 1 commit (provider selected):**

```rust
let provider_id = onb.options[onb.list_sel].id.clone();
// existing set_in_toml + write to user-global config.toml
write_config_field("provider", TomlValue::Str(provider_id.clone()));
app.config.provider = provider_id.clone();
```

**Step 2 commit (key entered, non-empty):**

```rust
let key_env = key_env_for(&onb.chosen_provider)
    .expect("key-requiring provider has a key env");
secrets.as_ref().unwrap().set(key_env, &onb.key_buffer)?;
onb.key_buffer.clear(); // plaintext not held after write
```

The key goes to the `SecretStore` (encrypted DB), never to TOML — same rule as
the config screen's secret editing. `key_env_for` (`main.rs`) maps provider
family → env var name and is reused as-is, so the wizard stays in sync with
`select_provider`'s key lookup automatically. No new mapping table.

**Step 3 commit (model selected):**

```rust
let model = if onb.list_sel == 0 {
    String::new() // "use default" row → empty → provider picks its default
} else {
    onb.options[onb.list_sel].id.clone()
};
write_config_field("model", TomlValue::Str(model.clone()));
app.config.model = model;
```

**DONE:** close overlay, re-select provider (see §4 "Completion").

### Key routing

A new branch in `route.rs`'s key dispatcher, mirroring the config overlay's
routing. When `state.overlay == Overlay::Onboarding`:

```rust
pub fn route_onboarding_key(state: &ShellState, key: KeyEvent) -> Action {
    let onb = match &state.onboarding {
        Some(o) => o,
        None => return Action::Noop,
    };
    match onb.step {
        OnboardingStep::Provider | OnboardingStep::Model => match key.code {
            KeyCode::Up => Action::OnboardingMove(-1),
            KeyCode::Down => Action::OnboardingMove(1),
            KeyCode::Enter => Action::OnboardingSelect,
            KeyCode::Esc => Action::OnboardingAbort,
            _ => Action::Noop,
        },
        OnboardingStep::ApiKey => match key.code {
            KeyCode::Enter => Action::OnboardingSubmitKey,
            KeyCode::Backspace => Action::OnboardingKeyBackspace,
            KeyCode::Esc => Action::OnboardingAbort,
            KeyCode::Char(c) => Action::OnboardingKeyChar(c),
            _ => Action::Noop,
        },
    }
}
```

New `Action` variants:

```rust
OnboardingMove(i16),        // up/down in pick-list steps
OnboardingSelect,           // Enter in step 1 or 3
OnboardingSubmitKey,        // Enter in step 2 (non-empty only)
OnboardingKeyChar(char),    // typed char in step 2
OnboardingKeyBackspace,     // backspace in step 2
OnboardingAbort,            // Esc — skip wizard
```

The bin's `handle_action` processes these:
- `OnboardingSelect` — reads the selected option, writes config, advances the
  step (ollama-local → DONE; key-requiring → step 2; model step → DONE).
- `OnboardingSubmitKey` — validates non-empty (no-op if empty), writes to the
  secret store, clears the buffer, advances to step 3 (or DONE if ≤1 model).
- `OnboardingMove` — moves `list_sel`, wrapping via the existing `palette::nav`.
- `OnboardingKeyChar` / `OnboardingKeyBackspace` — mutate `key_buffer`.
- `OnboardingAbort` — `shell.overlay = Overlay::None`, `shell.onboarding = None`.

### Boot-time orchestration

In `run()`, after `select_provider` and `first_time_user` are computed:

```rust
let (provider, provider_label, has_key) = select_provider(&config, &secrets);
// ... existing startup ...
let first_time_user = sessions.is_empty();

if wizard_needed(first_time_user, &config, has_key, secrets.is_some()) {
    shell.overlay = zoid_tui::Overlay::Onboarding;
    shell.onboarding = Some(OnboardingState {
        step: OnboardingStep::Provider,
        chosen_provider: String::new(),
        key_buffer: String::new(),
        list_sel: 0,
        options: zoid_tui::config_view::provider_options(""),
    });
}
```

The wizard state is seeded with the provider pick-list once. Step transitions
rebuild `options` (step 3 repopulates with `model_options(provider_id, "")`).
The per-frame render loop already dispatches on `state.overlay` — adding the
`Onboarding` branch to `render_shell` is the only render-loop change.

---

## 8. Registry-driven content (no hardcoded lists)

The wizard holds **no hardcoded provider or model list**. All content is pulled
from the registry through `config_view`:

- **Step 1 list:** `config_view::provider_options("")` → iterates
  `model::PROVIDERS`.
- **Step 3 list:** `config_view::model_options(provider_id, "")` → iterates
  `model::models_for(provider_id)`.
- **Step 3 auto-skip:** `model::models_for(provider_id).len() <= 1`.

Adding a provider or updating models in the registry (`zoid-model/src/lib.rs`)
automatically updates the wizard's lists. One source of truth, already
maintained.

**The one coupling:** step 2's key routing (which env var to write) uses
`key_env_for` (`main.rs`), which maps provider family → env var name. This is
the same function `select_provider` uses to look up keys, so the wizard and the
provider selector stay in sync. Adding a new provider family requires adding its
family → env mapping to `key_env_for` (already true today for
`select_provider`); the wizard needs no separate mapping.

---

## 9. Edge cases

| Case | Behavior |
|------|----------|
| `Esc` at any step | Overlay closes, wizard state dropped, user lands in empty-state chat. Gate re-fires on next launch if still unconfigured. |
| `Esc` then restart (nothing configured) | Gate re-fires → wizard appears again. |
| User picks `ollama-local` in step 1 | Steps 2–3 skipped, DONE immediately. No key required, no probe. |
| User completes wizard, then deletes their key | Within the session: no wizard (gate not re-checked). On next launch: `first_time_user` is now false (they have a session) → gate false → no wizard. They configure via `:config`. (Returning-user hint is out of scope.) |
| Empty `Enter` in step 2 | No-op. The user must enter a non-empty key or `Esc`. |
| Provider with ≤1 registry model | Step 3 skipped; `model` stays empty (provider default used at runtime). |
| Very narrow terminal (100×24 floor) | Card degrades: pick-list detail truncates, input box shrinks. No overflow, no panic. Covered by the floor snapshot test. |
| `config.provider` set in env (`ZOID_PROVIDER`) | Env shadows TOML. If the env value is a key-requiring provider with no key, the gate fires (first-time-user branch) — `has_key` is false. The wizard's step-1 commit writes to TOML, but the env value still shadows at read time. The user would need to unset the env var. This is an existing config-precedence behavior, not a wizard bug; the wizard writes the user's intent to TOML as designed. |
| Secret store failed to open at boot | `secrets` is `None`, so `secrets_available` is false → `wizard_needed` returns false → no wizard. The user is directed to `:config` via the normal empty state (where the same limitation exists — the secret store is needed to save a key). |

---

## 10. Testing

### Unit tests (pure, no terminal)

1. **`wizard_needed` predicate** (`main.rs` or a new pure helper module):
   - first-time + empty provider + secrets available → true
   - first-time + key-requiring provider + no key + secrets available → true
   - first-time + `ollama-local` → false (exempt)
   - first-time + key-requiring provider + key present → false
   - returning user + empty provider → false
   - returning user + no key → false
   - first-time + empty provider + secrets NOT available → false (no wizard
     when the secret store failed to open)

2. **`OnboardingState` transitions** (in a pure test module or via
   `handle_action` with a test `App`):
   - step 1 select `ollama-local` → overlay closes, provider written.
   - step 1 select key-requiring → step 2, provider written, options rebuilt.
   - step 2 empty `Enter` → no-op (stays in step 2).
   - step 2 non-empty `Enter` → key written, buffer cleared, step 3 (or DONE).
   - step 3 "use default" → model empty, DONE.
   - step 3 pick model → model written, DONE.
   - `Esc` at any step → overlay closed, state dropped.

3. **`canonical_id("")`** returns `""` — confirm the sentinel doesn't trip the
   legacy alias mapping (regression guard).

### Snapshot tests (`render.rs`)

- `onboarding_step_provider` (160×40 + 100×24)
- `onboarding_step_api_key` (160×40 + 100×24)
- `onboarding_step_model` (160×40 + 100×24)

Each asserts no line exceeds the content width and the visual structure matches
the golden snapshot.

### Existing tests unaffected

- The `onboarding.rs` empty-state tests (`empty_state_lines`) are unchanged —
  they test the *post-wizard* empty state, which still fires when
  `first_time_user && msgs.is_empty()` and no overlay is open.
- Config screen tests (`config_view`) are unchanged — the wizard reuses
  `provider_options` / `model_options` without modifying them.
- The `Config::default()` change (`provider: ""` instead of `"ollama"`) may
  affect tests that assert `c.provider == "ollama"` — there is one
  (`config.rs:242`). It must be updated to assert `c.provider.is_empty()`.

---

## 11. Change summary

| Change | File | Size |
|--------|------|------|
| Sentinel default | `crates/zoid-core/src/config.rs` (`Config::default()`: `provider: String::new()`) | ~1 line |
| Fix test asserting old default | `crates/zoid-core/src/config.rs` (test: `c.provider == "ollama"` → `is_empty()`) | ~1 line |
| `key_url` field on `ProviderEntry` | `crates/zoid-model/src/lib.rs` (struct + 6 registry entries) | ~10 lines |
| `Overlay::Onboarding` variant | `crates/zoid-tui/src/state.rs` | ~1 line |
| `OnboardingStep` + `OnboardingState` | `crates/zoid-tui/src/state.rs` | ~20 lines |
| `render_onboarding` | `crates/zoid-tui/src/render.rs` (+ overlay dispatch branch) | ~120 lines |
| `route_onboarding_key` + new `Action` variants | `crates/zoid-tui/src/route.rs` | ~40 lines |
| `wizard_needed` predicate | `crates/zoid/src/main.rs` (or a pure helper) | ~15 lines |
| Boot-time orchestration | `crates/zoid/src/main.rs` (in `run()`, after `select_provider`) | ~10 lines |
| `handle_action` for onboarding actions | `crates/zoid/src/main.rs` | ~60 lines |
| Snapshot tests | `crates/zoid-tui/src/render.rs` (tests) | ~80 lines |
| Unit tests (gate, transitions) | `crates/zoid/src/main.rs` + `crates/zoid-tui/src/state.rs` | ~80 lines |

**No signature changes** to `select_provider`, `config_view::provider_options`,
`config_view::model_options`, `key_env_for`, or the existing config write
machinery. The wizard composes existing primitives.

---

## 12. Architecture diagram

```
                    boot (run())
                         │
           select_provider → (provider, label, has_key)
           first_time_user = sessions.is_empty()
                         │
                wizard_needed(first_time, config, has_key, secrets.is_some())?
                    │              │
                   yes             no
                    │              │
         overlay = Onboarding    normal empty-state
         seed OnboardingState      onboarding
                    │
         ┌──────────┴───────────┐
         │  render_onboarding   │  ← render.rs, full-frame card
         │  (step rail + active │
         │   step content)      │
         └──────────┬───────────┘
                    │
         route_onboarding_key → Action::Onboarding*
                    │
         handle_action:
           OnboardingSelect   → write provider (set_in_toml)
           OnboardingSubmitKey→ write key (SecretStore::set), clear buffer
           OnboardingMove     → move list_sel
           OnboardingAbort    → close overlay
                    │
         step transitions (strictly forward):
           Provider → (ollama-local: DONE) | (key-requiring: ApiKey)
           ApiKey   → (non-empty: write, → Model or DONE) | (empty: no-op)
           Model    → (pick: write, DONE) | (use default: DONE)
                    │
         DONE: overlay = None, onboarding = None
               select_provider(new config) → live connection
               next frame: empty-state onboarding (prompts) with working LLM
```