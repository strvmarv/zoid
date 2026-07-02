# Configuration Screen & Config System — Design

> **Spec set.** Assumes the **core-architecture** doc (event log, store, sessions,
> §7.1 configuration) and the **chat-mode** doc (economy ⑤, palette, overlays).
> This doc covers the in-app **configuration screen** and the **config system**
> that backs it. It **implements §7.1** (TOML files + precedence) and **amends**
> its secrets rule to add an encrypted-DB secret store. The **model registry**
> (`2026-07-01-model-registry.md`) is referenced, not restated.

**Goal:** Replace zoid's scattered env-var-only configuration with (1) a layered
config system of record and (2) a first-class, full-screen configuration screen
reached from the palette — so provider/model, economy policy, interface, and API
keys are viewable and editable in-app, with clear provenance.

**Architecture (3 sentences):** A pure `Config` value is assembled by merging
ordered layers (compiled defaults → user-global TOML → project TOML → local TOML
→ `ZOID_*` env → CLI), each field tracking which layer supplied its effective
value (provenance). Secrets never enter TOML; they live encrypted in `zoid.db`
under a `SecretStore` seam (env var still wins at read time). A two-pane
full-screen overlay renders the resolved config with provenance tags and edits
write back to the active target (user-global by default; repo override on
demand), applying live where possible.

**Tech stack:** Rust; `toml` + `serde` for files; a pure-Rust AEAD
(`chacha20poly1305`) + `rand` for secret encryption; existing `rusqlite` store;
`ratatui` for the screen; `insta` snapshots.

## Global Constraints

- **Secrets never in committed or file config.** API keys are read only from
  env vars or the encrypted DB store — never written to any `*.toml`. This
  amends core §7.1 (which listed env / gitignored `config.local.toml` / keyring)
  to add **encrypted `zoid.db`** as a sanctioned home (consistent: `zoid.db` is
  user-global and never in a repo).
- **Tokens, not dollars.** No pricing/cost config or display (core §Non-Goals).
- **§16 design tokens.** No literal glyphs/hex outside `zoid-tui/src/tokens.rs`
  in rendered UI.
- **Single static binary.** New crypto deps must be pure-Rust (no OpenSSL).
- **User-global is the base; repo config is an optional override** that exists
  only when created.

---

## 1. Config model & precedence

### 1.1 The `Config` value

A single serializable struct grouping the v1 settings (grouped to match the
screen's sections). Illustrative shape:

```
Config
  provider: { name: String, base_url: Option<String> }    // "ollama" | "anthropic" | ...
  model:    String
  economy:  { context_ceiling: Option<u64>,               // None → model registry default
              auto_evict_cold: bool,
              compact_threshold_pct: u8,                   // 0–100; e.g. 80
              token_ceiling: Option<u64> }                 // governor; None = off
  interface:{ reduced_motion: bool }
```

- Secrets are **not** in `Config` (see §3).
- `context_ceiling: None` means "defer to the model registry / `context_ceiling()`
  helper" so the two systems compose rather than fight.

### 1.2 Layers & precedence (low → high)

Exactly core §7.1:

1. compiled defaults
2. user-global `~/.config/zoid/config.toml` (`$XDG_CONFIG_HOME` honored) — **the base/home**
3. project `./.zoid/config.toml`
4. local `./.zoid/config.local.toml` (gitignored)
5. `ZOID_*` environment variables (existing: `ZOID_MODEL`, `ZOID_CONTEXT_CEILING`, `ZOID_REDUCED_MOTION`)
6. CLI flags (only those that already exist; not expanded here)

### 1.3 Loading & merge (pure + IO split)

- **Pure core** (`zoid-core::config`): given N parsed layer values (as partials)
  + the resolution order, produce `(Config, Provenance)`. Deterministic, unit-tested,
  no IO. Each layer is a "partial config" (all fields optional); merge = last
  non-None wins, recording the source layer per field into `Provenance`.
- **IO shell** (`zoid` bin): resolve paths, read files (tolerant: missing file =
  empty layer; parse error = surfaced, non-fatal, falls back to lower layers),
  read env, then call the pure merge.
- **Provenance** = per-field enum `{ Default, UserGlobal, Project, Local, Env, Cli }`,
  consumed by the screen for its tags and the `[env] ⚠` shadow indicator.

### 1.4 Write-back

- Writing a field serializes the **single changed layer's** TOML (user-global by
  default), preserving the rest of that file. Editing never rewrites a file it
  didn't target.
- Active write target defaults to **user-global**; the `r` action writes the
  current value to the **project** `./.zoid/config.toml` (creating it if absent).
- Env-shadowed fields: editing is allowed (writes the file), but the screen keeps
  the `[env] ⚠` marker so it's clear the running value won't change until the env
  var is unset. No silent no-op.

## 2. Applying settings (live vs restart)

On save, apply to the running app where cheap; otherwise mark as "next launch."

| Setting | Apply |
|---|---|
| `interface.reduced_motion` | live (instant) |
| `economy.*` | live — feeds the running `ContextPolicy` (see §5) |
| `model`, `provider`, `base_url` | live — next turn uses the new value |
| DB path (not editable here) | out of scope |

## 3. Secrets — encrypted DB store

### 3.1 `SecretStore` seam

```
trait SecretStore {
    fn get(&self, key: &str) -> Option<String>;   // resolved value or None
    fn set(&self, key: &str, val: &str) -> Result<()>;
    fn clear(&self, key: &str) -> Result<()>;
    fn status(&self, key: &str) -> SecretStatus;   // Set{source: Env|Stored} | NotSet
}
```

- **Read precedence:** env var wins → else decrypt from DB. Keeps CI/headless and
  the current workflow working untouched.
- Backends behind the seam: `EncryptedDb` (v1). Keyring / passphrase-KDF are
  **deferred** (out of scope) but the seam makes them additive.

### 3.2 At-rest encryption (hygiene, not defense-in-depth)

Explicit threat model: this defeats **casual exposure** (`cat`/`grep`/accidental
`git add`/screen-share/backup), **not** a same-uid local attacker (accepted — a
process with your uid already owns env/ssh/etc.).

- **App key:** 32 random bytes generated on first use, stored at
  `~/.local/share/zoid/secret.key` (`$XDG_DATA_HOME` honored) with `0600` perms.
  **Not** in the DB — so a copied/dumped `zoid.db` doesn't carry its own key.
- **Cipher:** XChaCha20-Poly1305 (`chacha20poly1305`), random nonce per value.
- **Schema:** new table `secrets(name TEXT PRIMARY KEY, ciphertext BLOB NOT NULL,
  nonce BLOB NOT NULL, created_ts INTEGER)`.
- The screen shows **status only** (`set ✓` + source, or `not set`); it never
  decrypts a key back to the display.

### 3.3 Provider wiring

`zoid-provider::default_provider()` currently reads `OLLAMA_API_KEY` /
`ANTHROPIC_API_KEY` from env. It gains a `SecretStore` fallback: env → store.
(The provider crate stays IO-light; the bin injects a `SecretStore`
implementation backed by the store + key file.)

## 4. The configuration screen (UI)

- **Presentation:** full-screen overlay (a new `Overlay::Config` or equivalent
  screen state) that replaces the conversation while open; `esc` returns to Chat.
- **Entry:** palette **Settings** group → "Open settings", and `:config`.
- **Layout:** two pane. Left = section nav (`Provider & Model`, `Economy ⑤`,
  `Interface`, `Secrets`); right = fields for the active section + a one-line help
  for the field under the cursor.
- **Per-field:** label, current value (or toggle `[x]/[ ]`), right-aligned
  provenance tag (`[default]`/`[user]`/`[repo]`/`[env]`), with `[env] ⚠` when an
  env var shadows a stored value.
- **Interactions (defaults; tune in use):** `↑↓` move field · `←→` switch section
  · `⏎` edit / store · `space` toggle bool · `x` clear (secrets) · `r` save value
  to repo override · `esc` back. Footer always shows the active write target.
- **Edit affordance:** inline-in-place edit buffer (may revisit a bottom input
  line after dogfooding). Text/number fields accept typed input; bools toggle;
  **`provider` cycles a fixed known set** (`ollama`, `anthropic`) with
  `←→`/`space`. **`model` is free-text** for now — a per-provider model *cycle*
  waits on the model registry (`2026-07-01-model-registry.md`); until it exists
  there is no authoritative list to offer, so typed entry is the honest choice.
- **Rendering:** lives in `zoid-tui` (`render.rs` + a `config_view` model), state
  in `state.rs`, routing in `route.rs`, all glyphs/colors via `tokens.rs`.

## 5. Economy policy activation (fold-in)

Today `main.rs` builds `ContextPolicy::default()` and the drawer is
observability-only (the third audit gap). This design **wires the config's
`economy.*` into the running `ContextPolicy`** so `auto_evict_cold`,
`compact_threshold_pct`, and `token_ceiling` finally take effect — the automated
governor half of active context management. **Out of scope:** the *manual*
pin/evict keystrokes (a separate effort); this design only makes the policy
config-driven and active.

## 6. Components / file map

- `zoid-core::config` — `Config`, partial-layer type, pure merge + `Provenance`. (new)
- `zoid-core::store` — `secrets` table + get/set/clear; settings persistence helpers. (modify)
- `zoid-core::secret` — `SecretStore` trait + `EncryptedDb` impl + key-file mgmt + AEAD. (new)
- `zoid` bin — path resolution, file/env layer loading, CLI merge, `SecretStore` injection, apply-on-save, economy policy wiring. (modify `main.rs`)
- `zoid-provider` — env→store secret fallback in `default_provider`. (modify)
- `zoid-tui` — `Overlay::Config` state, `config_view` model, two-pane render, routing, palette Settings entry + `:config`. (modify `state.rs`/`render.rs`/`route.rs`/`palette.rs`/`command.rs`)

## 7. Testing

- **Pure merge/precedence** (`zoid-core::config`): each layer overrides the one
  below; provenance is recorded correctly; env shadows file.
- **Secrets:** encrypt→decrypt round-trip; env-override precedence; key-file
  created `0600`; missing key file regenerates cleanly; `status` reports source.
- **Loader IO:** missing files = empty layers; malformed TOML falls back without
  crashing.
- **Screen:** `insta` snapshots for each section (incl. an editing state, a
  secrets state, and an `[env] ⚠` shadow); routing tests for open/close and
  save→repo.
- **Economy wiring:** config `economy.*` produces the expected `ContextPolicy`.

## 8. Non-goals (this iteration)

- Keyring and passphrase-KDF secret backends (seam only).
- DB-backed **config** layer (config stays TOML; DB is for secrets + machine state).
- Model registry implementation (separate spec; `context_ceiling: None` composes with it).
- Manual pin/evict keystrokes (economy *manual* control).
- CLI flag expansion, config hot-reload, first-run onboarding wizard, in-place `$EDITOR` launch.

## 9. Open (tune during dogfooding)

- Section-switch keys (`←→` vs `Tab`) and inline vs bottom-line edit.
- **`model` stays free-text** until the model registry lands, at which point it
  becomes a per-provider cycle/pick. (`provider` already cycles the fixed known
  set — decided.)
