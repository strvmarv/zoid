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
empty-state chat with a provider and key *configured*. The wizard guarantees
key **presence**, not key **correctness** — a mistyped key completes the wizard
and fails on the first message with a provider-side auth error (no
reachability probing; see §1 Out of scope).

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

> **Invariant — overloaded sentinel value.** Empty string already means
> "provider picks its default model" for the `model` field (`config.rs:200`).
> It now *also* means "no provider chosen" for the `provider` field. Two fields,
> one sentinel, two meanings — the distinction is purely positional. Every
> reader of `config.provider` that checks `is_empty()` must know it means
> "unconfigured," not "use default." To keep this from rotting, the
> `Config::default()` line carries a comment:
> ```rust
> provider: String::new(), // empty = unconfigured (see onboarding wizard gate)
> ```

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

**Env-var interaction (must document).** A user with a stale `OLLAMA_API_KEY`
exported in their shell environment gets a subtle surprise: with the empty
default, `select_provider` resolves family `"ollama"` (the `_ =>` fallback at
`main.rs:1101`) and finds the ambient key — so it constructs a *live*
`OllamaProvider` for `ollama-cloud`, not `FakeProvider`. That user is silently
routed to a cloud provider they never chose. The wizard's step-1 commit writes
the user's explicit choice to TOML, but the ambient env var continues to
shadow it at read time until unset. This is existing config-precedence
behavior, not a wizard bug — but the wizard is the one UI surface that could
detect the shadow (the config screen already marks rows `env_shadowed`,
`config_view.rs:151/159`) and warn. See §5 (step-1 env-shadow hint) and §9
(edge case).

**The silent-no-improvement population.** A returning user (has sessions) who
was running on the old `"ollama"` default with no key — silently on
`FakeProvider` the whole time — is *not* reached by the wizard: they have
sessions so `first_time_user` is false, the gate returns false, and they stay
on `FakeProvider`. Their experience is unchanged (still broken). The wizard
does not fix their situation; they must `:config` manually. A lighter inline
hint for returning users with a missing key is the correct long-term fix (out
of scope, §1) but the spec names this population explicitly so it isn't
forgotten.

**What `canonical_id` does with `""`:** `canonical_id("")` returns `""` (the
`other => other` arm). `entry("")` returns `None`. `select_provider`'s
`canonical_id(&config.provider) == "ollama-local"` check is false, the family
match falls through to the `_ =>` arm, `key_for("OLLAMA_API_KEY")` is likely
`None`, and `FakeProvider` is returned. All safe — no panic, no bad state. The
gate's `ollama-local` exemption (§3) is placed *before* the empty-provider
check; its correctness depends on `canonical_id("") != "ollama-local"` (true
via the `other => other` arm). This ordering invariant is pinned by a test
(§10).

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
   │                                    └─ Esc → retreat to step 1 (see §4 Navigation)
   │
   └─ step 3 (if reached):
        ├─ pick "use default" → model stays "" → DONE
        ├─ pick a model → write model → DONE
        └─ Esc → skip wizard
```

**Navigation: forward by default, Esc-retreat from step 2.** Steps advance
forward only (no back button). `Esc` behavior is step-dependent:

- **Step 1 (Provider):** `Esc` skips the whole wizard (closes the overlay,
  drops state). There's nothing to retreat to.
- **Step 2 (API key):** `Esc` **retreats to step 1**, it does *not* abort the
  wizard. This is the escape hatch for a keyless user who picked a key-requiring
  provider — they land back on the provider list and can pick `ollama-local`
  (no key needed) or a different provider. The step-1 provider write stays
  (already in TOML); re-picking overwrites it. The "no key, no complete" rule
  is preserved: there is still no way to *complete* the wizard without a key
  for a key-requiring provider — you can only retreat and choose differently.
- **Step 3 (Model):** `Esc` skips the whole wizard. The provider + key are
  already written; the user lands in empty-state chat with a working connection
  and the provider's default model.

This avoids the trap the original "strictly forward, Esc-abort" design created:
a keyless user on step 2 was stuck (enter a key they don't have, or lose their
step-1 choice entirely). Now `Esc` from step 2 is a graceful "go back and pick
differently."

> **Why not a back button?** A dedicated `Back`/`Left` key would need to handle
> step 3 → step 2 retreat, where the key buffer was cleared on commit — showing
> an empty input where the user already entered a key is confusing. Esc-retreat
> is scoped to step 2 only (the one place a user gets trapped), and step 2's
> buffer is only cleared *on commit*, so retreating from an uncommitted step 2
> just drops the in-progress buffer, which is fine.

### Completion (DONE)

1. Close the overlay: `shell.overlay = Overlay::None`, `shell.onboarding = None`.
2. The provider re-selection has **already happened** — the wizard's step-1
   and step-3 commits call `apply_config_write` (see §7), which writes TOML,
   reloads the full layered config, refreshes provenance, and re-selects the
   provider (`main.rs:4198`) on every call. The step-2 key write goes to the
   `SecretStore`, which the *next* `apply_config_write` call (step 3 or, if
   step 3 is skipped, a final no-op reload) picks up. No separate
   `select_provider` call is needed at DONE — using one would re-mutate
   `app.provider`/`app.shell.provider` a second time.
3. If step 3 is skipped (≤1 model), the last config write was step 1's
   `apply_config_write` — but that happened *before* the key was written in
   step 2, so it re-selected with `has_key = false`. To ensure the key takes
   effect, the wizard does one final `apply_config_write(app, "model",
   TomlValue::Unset, false)` (a no-op write that triggers reload + re-select,
   now with the key present) before closing. This mirrors the config screen's
   pattern of re-evaluating after a secret write.
4. The next frame, `proj.msgs.is_empty()` is true and `first_time_user` is still
   true (frozen at boot — see §10 invariant), so the existing empty-state
   intercept fires — the user sees the normal onboarding prompts ("explain
   this codebase", etc.) with a configured provider + key.

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

Adding this variant forces **three** compiler-checked integration points (all
exhaustive `match` arms — the compiler rejects a missing arm, so these are
impossible to ship broken, but the spec lists them so the implementer knows
upfront):

1. **`render_shell` dispatch** (`render.rs`, the overlay `if`/`else if` chain
   at `render.rs:235`): add the `Onboarding` branch calling `render_onboarding`
   full-frame.
   ```rust
   } else if state.overlay == Overlay::Onboarding {
       render_onboarding(frame, state, frame.area());
   }
   ```
2. **`layout.rs:219`** (the overlay-rect exhaustive match — the comment at
   `layout.rs:215` says "every overlay must declare its modal-rect policy
   here"): `Onboarding` draws full-frame like `Config`/`ProviderSwitch`, so it
   joins the `None` arm:
   ```rust
   Overlay::Config | Overlay::ProviderSwitch | Overlay::Onboarding | Overlay::None => None,
   ```
3. **`route_paste`** (`route.rs:170`, exhaustive `match state.overlay`): step 2
   is a free-text API-key field — users **paste** keys (they're long, copied
   from a dashboard). The existing `PasteTarget` enum (`route.rs:152`) needs a
   new variant, and the wizard must route paste into `key_buffer` on step 2:
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum PasteTarget {
       // ... existing ...
       OnboardingKey,
       None,
   }

   // in route_paste, the overlay match:
   Overlay::Onboarding => {
       return match &state.onboarding {
           Some(o) if o.step == OnboardingStep::ApiKey => PasteTarget::OnboardingKey,
           _ => PasteTarget::None, // steps 1, 3 are pick-lists — paste drops
       };
   }
   ```
   The bin's paste handler then appends to `onboarding.key_buffer`. This matches
   the config screen's `PasteTarget::ConfigEdit` pattern (`route.rs:179`) —
   the wizard claims to mirror the config screen, so paste support is
   consistency, not an enhancement.

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

**Env-shadow warning (step 1 only, conditional).** If `app.prov.provider ==
Source::Env` at boot (a `ZOID_PROVIDER` env var is set), render a dim warning
line above the pick-list:

```
  ⚠ ZOID_PROVIDER is set to "{value}" — your choice here writes to TOML
    but won't take effect until you unset it.
```

This uses the existing `Provenance.provider` field (already computed at boot)
and `color::WARN` for the glyph. The warning is the wizard's one chance to catch
a shadowed config before the user completes and is surprised on next launch.

**Step 2 — API key (masked free-text):**

```
  ● 2 — API key
     Enter your Anthropic API key
     ┌──────────────────────────────────────────┐
     │ sk-ant-••••••••••••••••••                  │
     └──────────────────────────────────────────┘
     Get one at https://console.anthropic.com/settings/keys
     No key? Press Esc to choose a different provider.
```

A single-line input box (reusing the input-rendering idiom from `render_input`),
masked with `•` per char. A help line below shows the chosen provider's
`key_url`. A second help line — "No key? Press Esc to choose a different
provider." in `color::DIM` — surfaces the escape hatch (§4 Navigation: Esc
from step 2 retreats to step 1, not aborts). `Enter` commits (non-empty only —
empty `Enter` is a no-op). `Backspace` deletes. `Esc` → retreat to step 1.
The friendly provider name ("Anthropic") comes from the registry `display`
field.

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
    // Footer: step-dependent keybind hints:
    //   step 1: "↑↓ move · Enter select · Esc skip setup"
    //   step 2: "Enter submit · Backspace delete · Esc back to provider"
    //   step 3: "↑↓ move · Enter select · Esc skip setup"
}
```

**Width/degrade:** The card uses `frame.area()`. At the 100×24 floor, the
content width is approximately `100 - 2 (border) - 2 (padding) ≈ 96` columns at
160 width; the 100×24 floor yields ~51 content columns after the card border
and inner padding. (For reference, `render_config`'s three-column threshold is
`RAIL_W + FIELDS_W + PICKER_MIN = 82` at `render.rs:1418`, but the wizard is
single-column so that threshold doesn't apply — the floor is just "card border +
padding fits.") The pick-list detail (endpoint URL) truncates via the existing
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

## 6. New registry field: `key_url` — and consolidating `entry_requires_key`

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

### Consolidating `entry_requires_key`

`key_url: None` is exactly the "keyless" predicate that `entry_requires_key`
(`main.rs:1046`, currently a hardcoded `id != "ollama-local"`) is hand-coding.
The two must agree. Rather than maintain two tables, **derive
`entry_requires_key` from the registry**:

```rust
fn entry_requires_key(id: &str) -> bool {
    zoid_provider::model::entry(id)
        .map(|e| e.key_url.is_some())
        .unwrap_or(true) // unknown provider → assume key required (safe default)
}
```

This makes `key_url` the single source of truth for "does this provider need a
key." Adding a keyless provider means setting `key_url: None` in the registry —
`entry_requires_key` follows automatically, and `key_env_for`'s
`!entry_requires_key(id) → None` early return picks it up.

### The `key_env_for` / `key_url` lockstep invariant

`key_env_for` (`main.rs:1051`) maps provider family → env var name with a
`_ => Some("OLLAMA_API_KEY")` default arm. A future provider with
`key_url: Some(...)` but no `key_env_for` arm would hit the `_ =>` default and
silently write the key to `OLLAMA_API_KEY` — the wrong env var. This is a
latent footgun. Two safeguards:

1. The wizard's step-2 commit uses `key_env_for(&onb.chosen_provider)` — if it
   returns `None` for a provider the wizard thinks is key-requiring (because
   `key_url: Some`), that's a contradiction. The wizard should **never reach
   step 2 for a provider where `key_env_for` returns `None`** — the step-1
   commit's `ollama-local` short-circuit handles the keyless case, and any
   other keyless provider (future `key_url: None`) short-circuits the same way.
2. A test (§10) asserts that every `PROVIDERS` entry with `key_url: Some` has
   a `key_env_for` arm returning `Some` (i.e., no key-requiring provider falls
   through to the `_ => OLLAMA_API_KEY` default by accident).

The `expect` in the step-2 commit code (§7) is safe because the wizard only
reaches step 2 for providers where `entry_requires_key` is true (i.e.,
`key_url: Some`), and the lockstep test guarantees those providers have a
`key_env_for` arm.

---

## 7. Key routing, write-back, and bin-side orchestration

### Config write-back

The wizard writes to **user-global TOML** (`~/.config/zoid/config.toml`), the
same default target as the config screen. It reuses the existing
`apply_config_write` helper (`main.rs:4144`) — the same function the config
screen's `ConfigPickerSelect` uses. `apply_config_write` does the TOML write
(via `write_config_file` → `set_in_toml`) *plus* the full config reload,
provenance refresh, provider re-selection (`select_provider` at `main.rs:4198`),
and model-info fetch. **No new write path, no new helper.** This keeps the
in-memory `app.config`/`app.prov` and the on-disk TOML in sync within the
session — critical so a later `:config` doesn't show stale provenance.

**Step 1 commit (provider selected):**

```rust
let provider_id = onb.options[onb.list_sel].id.clone();
// Mirror ConfigPickerSelect (main.rs:4974): write provider, seed base_url
// from the registry, clear model (Unset) to avoid an incompatible
// provider+model pair. All three go through apply_config_write so the full
// reload + re-select happens on each.
apply_config_write(app, "provider", TomlValue::Str(provider_id.clone()), false);
apply_config_write(app, "base_url", base_url_write_for(&provider_id), false);
apply_config_write(app, "model", TomlValue::Unset, false);
```

This matches the config screen exactly (`main.rs:4977–4985`). `base_url` is
seeded from the registry default (`base_url_write_for`, `main.rs:4134`) so the
endpoint is materialized in TOML for provenance honesty; `model` is cleared so
a stale model from a prior provider doesn't survive into the new one.

**Step 1 short-circuit (ollama-local / keyless):** If the chosen provider is
keyless (`entry_requires_key` is false, i.e., `key_url: None`), the wizard is
DONE — the three `apply_config_write` calls above have already re-selected the
provider with `has_key = true`. No step 2, no step 3.

**Step 2 commit (key entered, non-empty):**

```rust
let key_env = key_env_for(&onb.chosen_provider)
    .expect("wizard only reaches step 2 for key-requiring providers; \
             lockstep test (§10) guarantees a key_env_for arm");
secrets
    .as_ref()
    .expect("wizard gate guarantees secrets available")
    .set(key_env, &onb.key_buffer)?;
onb.key_buffer.clear(); // plaintext not held after write
```

The key goes to the `SecretStore` (encrypted DB), never to TOML — same rule as
the config screen's secret editing. The `expect`s are safe: the gate checked
`secrets.is_some()` at boot, and the lockstep test guarantees every
`key_url: Some` provider has a `key_env_for` arm. After the write, the next
`apply_config_write` (step 3, or the final reload if step 3 is skipped) picks
up the key via `select_provider`'s `key_for` lookup.

**Step 3 commit (model selected):**

```rust
let model = if onb.list_sel == 0 {
    String::new() // "use default" row → empty → provider picks its default
} else {
    onb.options[onb.list_sel].id.clone()
};
apply_config_write(app, "model", TomlValue::Str(model), false);
```

If step 3 is skipped (≤1 model), the wizard does one final
`apply_config_write(app, "model", TomlValue::Unset, false)` before closing — a
no-op write that triggers the reload + re-select, now picking up the step-2
key. See §4 "Completion."

**DONE:** close overlay (`overlay = None`, `onboarding = None`). No separate
`select_provider` call — the last `apply_config_write` already did it.

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
            // Step 1: Esc skips the whole wizard. Step 3: Esc skips the wizard
            // (provider + key already written; default model used).
            KeyCode::Esc => Action::OnboardingAbort,
            _ => Action::Noop,
        },
        OnboardingStep::ApiKey => match key.code {
            KeyCode::Enter => Action::OnboardingSubmitKey,
            KeyCode::Backspace => Action::OnboardingKeyBackspace,
            // Step 2: Esc RETREATS to step 1 (not abort) — the escape hatch
            // for a keyless user who picked a key-requiring provider.
            KeyCode::Esc => Action::OnboardingBack,
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
OnboardingBack,             // Esc in step 2 — retreat to step 1
OnboardingAbort,            // Esc in step 1/3 — skip wizard
```

The bin's `handle_action` processes these:
- `OnboardingSelect` — reads the selected option, runs the step-1 commit
  (three `apply_config_write` calls) or step-3 commit, advances the step
  (ollama-local/keyless → DONE; key-requiring → step 2; model step → DONE).
- `OnboardingSubmitKey` — validates non-empty (no-op if empty), writes to the
  secret store, clears the buffer, advances to step 3 (or DONE if ≤1 model).
- `OnboardingMove` — moves `list_sel`, **skipping non-selectable rows** (same
  loop pattern as `ConfigPickerMove` at `main.rs:4950–4956`, not `palette::nav`'s
  simpler wrap — `provider_options` marks `Status::Planned` rows
  `selectable: false`, and a future `Planned` provider would land the cursor on
  a non-selectable row with `palette::nav`).
- `OnboardingKeyChar` / `OnboardingKeyBackspace` — mutate `key_buffer`.
- `OnboardingBack` — step 2 → step 1: set `onb.step = OnboardingStep::Provider`,
  rebuild `onb.options = provider_options("")`, reset `onb.list_sel` to the
  previously-chosen provider (so the user sees their last selection highlighted).
  The step-1 provider write stays in TOML; re-picking overwrites it.
- `OnboardingAbort` — `shell.overlay = Overlay::None`, `shell.onboarding = None`.

### `ShellState` field

`ShellState` gains a new field (`crates/zoid-tui/src/state.rs`):

```rust
/// The onboarding wizard state, or `None` when the wizard isn't open. Set at
/// boot by the gate; cleared on completion or abort. Defaults `None` so tests
/// and examples that don't set it get no wizard.
pub onboarding: Option<OnboardingState>,
```

`ShellState::new()` (the constructor used at boot, `main.rs:2436`) sets
`onboarding: None`. The boot-time orchestration below sets it to `Some(...)`
when the gate fires. `OnboardingState` derives `Clone + Debug + PartialEq + Eq`
(§4); `PickOption` already derives these (`config_view.rs:21`), so the field
composes.

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

**The coupling:** step 2's key routing (which env var to write) uses
`key_env_for` (`main.rs`), which maps provider family → env var name. This is
the same function `select_provider` uses to look up keys, so the wizard and the
provider selector stay in sync. `entry_requires_key` is now derived from
`key_url` (§6), so adding a keyless provider means setting `key_url: None` in
the registry — both `entry_requires_key` and the wizard's step-1 short-circuit
follow automatically. Adding a new *key-requiring* provider family requires
adding its family → env mapping to `key_env_for` (already true today for
`select_provider`); the lockstep test (§10) guards against a missing arm. The
wizard needs no separate mapping.

---

## 9. Edge cases

| Case | Behavior |
|------|----------|
| `Esc` at step 1 (Provider) | Skips the whole wizard. Overlay closes, wizard state dropped, user lands in empty-state chat. Gate re-fires on next launch if still unconfigured. |
| `Esc` at step 2 (API key) | **Retreats to step 1** (not abort). The user lands back on the provider list with their previous selection highlighted. This is the escape hatch for a keyless user who picked a key-requiring provider — they can pick `ollama-local` or a different provider. The step-1 provider write stays in TOML; re-picking overwrites it. |
| `Esc` at step 3 (Model) | Skips the wizard. Provider + key are already written; the user lands in empty-state chat with a working connection and the provider's default model. |
| `Esc` then restart (nothing configured) | Gate re-fires → wizard appears again. |
| User picks `ollama-local` (or any keyless provider) in step 1 | Steps 2–3 skipped, DONE immediately. No key required, no probe. |
| User completes wizard with a mistyped/wrong key | The wizard guarantees key **presence**, not correctness. `has_key` is true (non-empty string), the overlay closes, and the first message fails with a provider-side auth error (e.g. 401). No reachability probe is run (out of scope, §1). |
| User completes wizard, then deletes their key | Within the session: no wizard (gate not re-checked). On next launch: `first_time_user` is now false (they have a session) → gate false → no wizard. They configure via `:config`. (Returning-user hint is out of scope.) |
| Empty `Enter` in step 2 | No-op. The user must enter a non-empty key, `Esc` to retreat, or (no other escape). |
| Paste into step 2 | Pasted text appends to `key_buffer` (via `PasteTarget::OnboardingKey`, §5). Paste into steps 1/3 drops (pick-lists; `PasteTarget::None`). |
| Provider with ≤1 registry model | Step 3 skipped; `model` stays empty (provider default used at runtime). A final `apply_config_write("model", Unset)` triggers the reload that picks up the step-2 key. |
| Very narrow terminal (100×24 floor) | Card degrades: pick-list detail truncates, input box shrinks. No overflow, no panic. Covered by the floor snapshot test. |
| `config.provider` set in env (`ZOID_PROVIDER`) | Env shadows TOML. If the env value is a key-requiring provider with no key, the gate fires (first-time-user branch) — `has_key` is false. The wizard's step-1 screen shows an env-shadow warning (§5). The step-1 commit writes to TOML, but the env value still shadows at read time until unset. The user would need to unset the env var. This is existing config-precedence behavior, not a wizard bug; the wizard warns and writes the user's intent to TOML as designed. |
| Ambient `OLLAMA_API_KEY` with empty `provider` default | `select_provider` resolves family `"ollama"` (the `_ =>` fallback) and finds the ambient key → constructs a live `OllamaProvider` for `ollama-cloud`. A first-time user with `OLLAMA_API_KEY` set and empty `provider` has `has_key = true`, but the gate still fires (empty-provider check precedes `!has_key`). The wizard's step-1 env-shadow warning covers this. A *returning* user in this state is silently on `ollama-cloud` (no wizard, first_time_user false) — named in §2 as the silent-no-improvement population. |
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
   - step 1 select `ollama-local` (or any keyless provider) → overlay closes,
     provider + base_url written, model cleared.
   - step 1 select key-requiring → step 2, provider + base_url written, model
     cleared, options rebuilt for step 2.
   - step 2 empty `Enter` → no-op (stays in step 2).
   - step 2 non-empty `Enter` → key written, buffer cleared, step 3 (or DONE
     if ≤1 model).
   - step 2 `Esc` → **retreats to step 1** (not abort): `step = Provider`,
     `options` rebuilt, `list_sel` reset to previously-chosen provider. Overlay
     stays open.
   - step 3 "use default" → model empty, DONE.
   - step 3 pick model → model written, DONE.
   - step 1 `Esc` → overlay closed, state dropped (abort).
   - step 3 `Esc` → overlay closed, state dropped (abort; provider + key kept).

3. **`canonical_id("")`** returns `""` — confirm the sentinel doesn't trip the
   legacy alias mapping (regression guard). Also assert
   `canonical_id("") != "ollama-local"` — this is the ordering invariant the
   gate's `ollama-local` exemption depends on (the exemption check precedes
   the empty-provider check and relies on the empty sentinel not aliasing to
   `ollama-local`).

4. **`key_url` / `entry_requires_key` / `key_env_for` lockstep:** For every
   `PROVIDERS` entry with `key_url: Some(...)`, assert `entry_requires_key(id)`
   is true and `key_env_for(id)` returns `Some`. For every entry with
   `key_url: None`, assert `entry_requires_key(id)` is false and `key_env_for`
   returns `None`. This guards against a future provider with `key_url: Some`
   falling through `key_env_for`'s `_ => OLLAMA_API_KEY` default (which would
   silently write the key to the wrong env var).

5. **`first_time_user` frozen invariant:** Add a comment at `main.rs:2470`
   (`shell.first_time_user = first_time_user;`) noting that the value is
   deliberately frozen at boot and the onboarding wizard + post-wizard
   empty-state copy depend on it not being recomputed mid-session. (No test —
   it's a documentation invariant, not a runtime check.)

### Snapshot tests (`render.rs`)

- `onboarding_step_provider` (160×40 + 100×24)
- `onboarding_step_api_key` (160×40 + 100×24)
- `onboarding_step_model` (160×40 + 100×24)

Each asserts no line exceeds the content width and the visual structure matches
the golden snapshot.

### Existing tests affected

- The `onboarding.rs` empty-state tests (`empty_state_lines`) are unchanged —
  they test the *post-wizard* empty state, which still fires when
  `first_time_user && msgs.is_empty()` and no overlay is open.
- The `Config::default()` change (`provider: ""` instead of `"ollama"`) affects
  tests that assert `c.provider == "ollama"` — there is one (`config.rs:242`).
  It must be updated to assert `c.provider.is_empty()`.
- **`config_view` tests must be run, not assumed unaffected.** `config_view.rs`
  tests call `Config::default()` (e.g. `config_view.rs:304`) and `build_sections`
  does `model::entry(&cfg.provider)` (`config_view.rs:123`) to pick the
  `connection_row`. With `provider = ""`, `entry("")` is `None`, so the
  `connection_row` match hits the `_ =>` arm (renders a `base_url` row) — *same
  arm as today* (today `entry("ollama")` → `ollama-cloud` → Http → also
  `base_url` row). So the structure is the same, but the test should be run to
  confirm no assertion on the provider label breaks. Don't claim immunity;
  verify.

---

## 11. Change summary

| Change | File | Size |
|--------|------|------|
| Sentinel default + invariant comment | `crates/zoid-core/src/config.rs` (`Config::default()`: `provider: String::new()` + `// empty = unconfigured` comment) | ~2 lines |
| Fix test asserting old default | `crates/zoid-core/src/config.rs` (test: `c.provider == "ollama"` → `is_empty()`) | ~1 line |
| `key_url` field on `ProviderEntry` | `crates/zoid-model/src/lib.rs` (struct + 6 registry entries) | ~10 lines |
| Derive `entry_requires_key` from `key_url` | `crates/zoid/src/main.rs` (`entry_requires_key` body → `entry(id).map(\|e\| e.key_url.is_some())`) | ~3 lines |
| `Overlay::Onboarding` variant | `crates/zoid-tui/src/state.rs` | ~1 line |
| `OnboardingStep` + `OnboardingState` + `ShellState.onboarding` field | `crates/zoid-tui/src/state.rs` (struct, enum, field + `new()` default) | ~25 lines |
| `render_onboarding` + `render_shell` dispatch branch | `crates/zoid-tui/src/render.rs` | ~120 lines |
| `layout.rs` overlay-rect arm | `crates/zoid-tui/src/layout.rs` (add `Onboarding` to the `None` arm) | ~1 line |
| `route_onboarding_key` + new `Action` variants (incl. `OnboardingBack`) | `crates/zoid-tui/src/route.rs` | ~45 lines |
| `PasteTarget::OnboardingKey` + `route_paste` arm | `crates/zoid-tui/src/route.rs` | ~10 lines |
| `wizard_needed` predicate | `crates/zoid/src/main.rs` (or a pure helper) | ~15 lines |
| Boot-time orchestration | `crates/zoid/src/main.rs` (in `run()`, after `select_provider`) | ~10 lines |
| `handle_action` for onboarding actions (uses `apply_config_write`) | `crates/zoid/src/main.rs` | ~70 lines |
| `first_time_user` frozen-invariant comment | `crates/zoid/src/main.rs` (`shell.first_time_user = ...`) | ~1 line |
| Snapshot tests | `crates/zoid-tui/src/render.rs` (tests) | ~80 lines |
| Unit tests (gate, transitions, lockstep, `canonical_id`) | `crates/zoid/src/main.rs` + `crates/zoid-tui/src/state.rs` + `crates/zoid-model/src/lib.rs` | ~100 lines |

**No signature changes** to `select_provider`, `config_view::provider_options`,
`config_view::model_options`, `key_env_for`, `apply_config_write`, or
`base_url_write_for`. The wizard composes existing primitives. The three
compiler-enforced integration points (`render_shell`, `layout.rs`,
`route_paste`) are mandatory follow-ons of adding the `Overlay::Onboarding`
variant — the compiler rejects a missing arm, so they can't ship broken.

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
         │  (step rail + active │   (also: layout.rs rect arm, route_paste arm)
         │   step content)      │
         └──────────┬───────────┘
                    │
         route_onboarding_key → Action::Onboarding*
                    │
         handle_action (uses apply_config_write — no new write path):
           OnboardingSelect    → apply_config_write(provider + base_url + model-Unset)
           OnboardingSubmitKey → SecretStore::set(key), clear buffer
           OnboardingMove      → move list_sel (skip non-selectable)
           OnboardingBack      → step 2 → step 1 (Esc retreat; options rebuilt)
           OnboardingAbort     → close overlay (step 1/3 Esc)
                    │
         step transitions:
           Provider → (keyless: DONE) | (key-requiring: ApiKey)
           ApiKey   → (non-empty: write, → Model or DONE) | (empty: no-op)
                    │ (Esc: → back to Provider)
           Model    → (pick: apply_config_write(model), DONE) | (use default: DONE)
                    │
         DONE: overlay = None, onboarding = None
               (last apply_config_write already re-selected the provider)
               next frame: empty-state onboarding (prompts) with configured LLM
```