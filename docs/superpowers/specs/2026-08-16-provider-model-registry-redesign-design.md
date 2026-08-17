# Provider/Model Registry Redesign — Design

**Date:** 2026-08-16
**Status:** Design (approved for planning)

## Problem

The static provider/model registry in `crates/zoid-model/src/lib.rs` is the
single source of truth for "what providers/models exist and what they can do,"
but it is maintained by hand-editing Rust consts across **five** places that
must stay in sync:

| Location | What | Count |
|---|---|---|
| `PROVIDERS[].models` | per-provider model id lists | varies |
| `ZEN_MODEL_IDS` | Zen gateway id list | 52 |
| `MODEL_CAPS` | per-model capabilities | ~60 |
| `opencode_go.rs::GO_MODELS` | wire-shape routing | 13 |
| `opencode_zen.rs::ZEN_MODELS` | wire-shape routing | 52 |

Plus `main.rs`'s `family`-based `match` arms for provider selection and
`key_env_for`. A model added to one list but not the others silently misroutes
(falls back to `OpenAIChat` with only a `tracing::warn`) or falls back to 32k
caps.

Two concrete bugs already exist:

1. **`claude-sonnet-4-6` appears in `MODEL_CAPS` twice** — once at 1M context
   (the `anthropic-api` entry) and once at 200K (the Zen Anthropic-Messages
   group). Lookup is case-insensitive and returns the first match, so the 1M
   entry silently shadows the 200K one. The registry cannot express
   "Claude via Zen is 200K but via Anthropic is 1M."
2. **Operational metadata is a property of the (provider, model) pair, not the
   model alone.** The same model id routes through different wire protocols
   depending on the gateway (`minimax-m3` is `Anthropic` on opencode-go but
   `OpenAIChat` on opencode-zen), and the same model has different effective
   context windows on different providers (Claude Sonnet 4.6 is 1M on Anthropic
   direct but 128K through GitHub Copilot's LM API).

The existing `refreshing-provider-models` skill is a "teach an agent to
hand-edit Rust" recipe — exactly the drift-prone manual procedure this redesign
replaces.

## Goals

1. **Single source of truth** — one data file of `(provider, model)` rows, each
   carrying caps + wire shape + provenance.
2. **Runtime-loadable** — user edits take effect without a rebuild.
3. **A refresh tool** — regenerate wire-derived rows from live endpoints,
   replacing the manual skill.
4. **Gemini as a first-class provider** — with wire-derived caps.
5. **Unify local-model provisioning** with the same caps type, dropping the
   parallel string schema and the seed-only SQLite table.

## Non-goals

- No cost/pricing fields (the economy is token-denominated by explicit spec
  choice; see `2026-07-01-model-registry.md`).
- No config-file/TOML *user* model definitions beyond the user-file override
  mechanism described here.
- No change to the `Provider` trait's streaming contract.

---

## 1. Data model & file format

Two files, merged at load:

- **`models.toml`** (shipped) — provider metadata + curated `static` models.
  Replaced wholesale on upgrade; never edited by the tool or user in normal
  operation.
- **`models.user.toml`** (user) — tool-generated (`wire`) and human-added
  (`user`) rows, merged over the shipped file. Never touched by upgrades.

### TOML shape (nested)

```toml
[[provider]]
id = "opencode-zen"
display = "opencode · zen"
family = "opencode-zen"
transport = { kind = "http", default_base_url = "https://opencode.ai/zen" }
status = "available"
key_url = "https://opencode.ai"
key_env = "OPENCODE_GO_API_KEY"

  [[provider.model]]
  id = "claude-sonnet-4-5"
  display = "Claude Sonnet 4.5"
  wire_shape = "anthropic-messages"
  source = "static"
  default = true
  context_window = 200_000
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "none"
  thinking_wire = "none"

  [[provider.model]]
  id = "minimax-m3"
  wire_shape = "openai-chat"
  source = "static"
  context_window = 200_000
  # ...
```

### Field semantics

- **`wire_shape`** — per-model field; union of leaf clients: `openai-chat`,
  `anthropic-messages`, `openai-responses`, `google-gemini`, `ollama`. Collapses
  `GO_MODELS`/`ZEN_MODELS` into the registry. Only meaningful for composite
  providers (opencode-go, opencode-zen) that route by model; for single-shape
  providers (ollama-local, anthropic-api, zai, gemini-api) it is present but
  ignored (the provider is constructed directly, not routed). The `ollama`
  value is carried for completeness/self-documentation on `ollama-local` rows
  but has no routing enum — Ollama is a standalone `OllamaProvider`, never
  routed through a composite.
- **`source`** — `static` | `wire` | `user`. In the shipped file it is always
  `static`; in the user file it is `wire` (tool-generated) or `user`
  (human-added).
- **`default`** — optional `bool` on a model row; marks the provider's default
  model. At most one `default = true` per provider; if none, the first row in
  shipped order is the default (with a warning). Survives merge/reorder, and a
  user row may override the default by setting `default = true`. **Merge rule:**
  if a user-file row sets `default = true` on a model while the shipped file
  already has `default = true` on a *different* model, the user-file default
  wins and the shipped default is demoted (so the post-merge registry has
  exactly one default per provider).
- **`display`** — optional human-readable model name (e.g. "Claude Sonnet 4.5").
  Falls back to the raw `id` when absent. Preserves the `display_name` that
  `LocalModelSeed` currently carries for local models.
- **`key_env`** — the secret env var name for the provider. Moves the
  `key_env_for()` mapping out of `main.rs`'s `match` arms and into the provider
  entry, so adding a provider no longer requires editing `main.rs`. Keyless
  providers (`ollama-local`) **omit** the key (or set it to the empty string);
  the parser treats both as `None`, mirroring the `key_url` rule.
- **`family`** — retained for display/grouping only (e.g. the picker's grouping
  and the `provider_label` drawer). It is no longer read for routing after the
  `select_provider`/`provider_for_id` `match` arms are replaced by
  `key_env` + `wire_shape`.
- **`thinking`** — `none` | `toggle` | `toggle-with-effort` | `budget` |
  `adaptive`.
- **`thinking_wire`** — `none` | `anthropic` | `deepseek` | `openai` | `ollama`.
- **`hidden`** — optional `bool` on a user-file row; hides a shipped model from
  the picker without removing it from the registry.
- **`key_url`** — the onboarding key-acquisition URL. Keyless providers
  (`ollama-local`) **omit** the key (or set it to the empty string); the parser
  treats both as `None`. Key-requiring providers must set a non-empty URL.
- **Local-model provisioning fields** (`runtime`, `download_source`, `quant`,
  `modelfile`, `num_ctx`, `vram_curve`) are optional keys, only meaningful on
  `ollama-local` rows. `schema_version` is **dropped** — it existed solely to
  drive the SQLite seed-update mechanism, which is removed (§5).

### Lookup semantics

- **Model ids** are compared **case-insensitively** (preserving today's
  `model_info` behavior); **provider ids** are compared **exactly** after
  `canonical_id` alias resolution.
- **`default_model(provider)`** returns the id of the row with `default = true`,
  else the first row in shipped order (with a warning). This replaces the
  "first entry = default" comment that lived in the deleted `ZEN_MODEL_IDS`
  const.
- **`default_model()`** (the env-driven, no-argument form) composes two steps:
  (1) env selects the provider (`OLLAMA_API_KEY` → `ollama-cloud`, else
  `anthropic-api`), then (2) `default_model(provider)` returns that provider's
  default. To preserve today's defaults, the shipped `models.toml` must carry
  `default = true` on `glm-5.2:cloud` (under `ollama-cloud`) and on
  `claude-sonnet-4-6` (under `anthropic-api`).

### Merge semantics

- User-file rows override shipped rows by `(provider.id, model.id)`.
- A user row may add new providers/models.
- `hidden = true` on a user row hides the matching shipped model.
- Duplicate `(provider.id, model.id)` within a single file is a parse error
  (no silent shadowing).

---

## 2. Crate layout & runtime loading

Three crates:

- **`zoid-model`** (dependency-free) — keeps the pure types: `ModelInfo`,
  `ProviderEntry`, `Transport`, `Status`, `ThinkingSupport`,
  `ThinkingWireShape`, a new `WireShape`, plus a `Registry` *data* struct (no
  I/O). The `&'static` consts (`PROVIDERS`, `MODEL_CAPS`, `ZEN_MODEL_IDS`,
  `DEFAULT_MODEL_INFO`) are **removed** — they become the shipped `models.toml`.

- **`zoid-registry`** (new) — depends on `zoid-model` + `toml`. Owns:
  - TOML parsing (shipped + user files) and the merge.
  - `Registry::load(paths) -> Result<Registry>`.
  - The refresh tool's fetch + reconcile logic (a testable library).
  - A thin CLI shim (the `refresh-models` entry point).

- **`zoid` / `zoid-core`** — load a `Registry` once at startup and thread it
  (as `Arc<Registry>`) into provider selection and model lookup.

### Owned-type migration (required)

The current types are `Copy` and borrow `&'static str` (`Transport::Http {
default_base_url: &'static str }`, `ProviderEntry`'s `id`/`display`/`family`/
`models`/`key_url`, and `ModelInfo` is `Copy`). A runtime-loaded TOML registry
cannot populate `&'static str`, so these become **owned**:

- `ProviderEntry` / `Transport` / `ModelInfo` fields become `String` /
  `Vec<String>` / `Option<String>`; the `Copy` derive is dropped (they become
  `Clone`).
- Lookup methods (`entry`, `models_for`, `default_base_url`, `model_info`,
  `selectable`) become methods on `Registry` returning `&str` / `&ModelInfo`
  borrowed from `&self` (the registry lives for the process lifetime, held in
  `Arc<Registry>`).
- Functions that currently return `&'static str` become owned:
  - `select_provider`'s `provider_name` → `String` (or `Arc<str>`).
  - `key_env_for` → `Option<String>`.
  - `default_model` → `String` (or `Arc<str>`).
  - `canonical_id` stays `&str`-in/`&str`-out where it only maps a borrowed
    input to a static alias; otherwise it returns owned.
- `default_provider()` already returns `Arc<dyn Provider>` (owned) and needs no
  string-ownership change; its only change is to consult the registry for the
  default provider *id* rather than hardcoding env-var branches.

This is a larger, more invasive change than "thread `Arc<Registry>`" suggests,
and it is the bulk of Phase 2. The spec's "never a broken tree" claim (§6)
depends on doing this migration deliberately, not incidentally.

### Loading flow

1. At startup, `zoid` calls `zoid_registry::load(shipped_path, user_path)`.
2. A missing user file is treated as empty (shipped file alone is valid).
3. The merged `Registry` is held in app state and passed to `select_provider`,
   `provider_for_id`, and model-lookup paths.
4. `zoid-provider`'s `stream()` routing reads `wire_shape` from the `Registry`.
   Each composite provider holds an `Arc<Registry>` and looks up the shape
   itself, so the `Provider` trait signature stays stable.

### What gets deleted

- `zoid-model/src/lib.rs` consts (`PROVIDERS`, `ZEN_MODEL_IDS`, `MODEL_CAPS`,
  `DEFAULT_MODEL_INFO`) → replaced by the TOML + a `Registry` default.
- `opencode_go.rs::GO_MODELS` and `opencode_zen.rs::ZEN_MODELS` routing tables →
  replaced by `wire_shape` lookups.
- `main.rs`'s `key_env_for` `match` arms and the `family`-based `match` in
  `select_provider`/`provider_for_id` → replaced by `key_env` + `wire_shape`
  from the registry.
- `zoid-provider/src/lib.rs::default_model()` / `default_provider()` → rewritten
  to consult the registry (env-driven *provider* selection stays; the *model*
  default comes from the `default = true` flag).

---

## 3. The refresh tool

**Form:** a library in `zoid-registry` (`refresh::reconcile`) plus a thin CLI
shim exposed as a `zoid` subcommand (`zoid refresh-models`) — no second binary.

**Behavior:**

1. **Fetch** live model lists from each provider that has a key, using the
   endpoint/auth table already documented in the existing skill:
   - Ollama: `GET {base}/api/tags` (Bearer), parse `.models[].name`.
   - Anthropic: `GET {base}/v1/models` (`x-api-key` + `anthropic-version`),
     parse `.data[].id`.
   - OpenAI-compat (opencode-go, opencode-zen, zai): `GET {base}{prefix}/models`
     (Bearer), parse `.data[].id`. ZAI uses `path_prefix=""`.
   - Gemini: **two fetches** — `GET {base}/v1/models` for the id list (parse
     `.models[].name`, the existing `list_models` shape) and
     `GET {base}/v1beta/models` for caps (parse `inputTokenLimit` /
     `outputTokenLimit`). These are distinct endpoints with distinct response
     shapes; the caps fetch is a new dedicated unit, not a reuse of
     `list_models`.
2. **Reconcile** against the merged registry:
   - **Add** new models as `source = "wire"` rows in the user file.
   - **Update** existing `wire` rows whose caps changed (Ollama `/api/show`,
     Gemini `/v1beta/models`).
   - **Remove** `wire` rows whose model id is absent from the live list.
   - **Report** (never delete) `static`/`user` rows absent from live.
3. **Write** results to `models.user.toml`, preserving human `user` rows and
   only touching `wire` rows.

### Wire-derived caps scope

- **Ollama** — `/api/show` returns `context_length` (already implemented in
  `fetch_model_info`); the tool reuses this.
- **Gemini** — `/v1beta/models` returns `inputTokenLimit`/`outputTokenLimit`
  (new dedicated fetcher + parser).
- **Anthropic / OpenAI-compat / OpenCode / ZAI** — list endpoints return only
  ids, no caps; these stay `static`/curated. The tool still fetches their id
  lists to detect additions/removals, but because it cannot derive caps, it
  **reports** new models (and absent `static` models) for a human to act on —
  it does not auto-add them. The human then adds a new model as a `static` row
  in the shipped file (or a `user` row in the user file) with researched caps.

**`wire` rows only ever exist for Ollama and Gemini** — the only two providers
with wire-derived caps. The "remove absent `wire` rows" step therefore applies
only to those two providers; Anthropic/OpenAI-compat/OpenCode/ZAI never have
`wire` rows to remove (their models are all `static`/`user`).

### Idempotency & safety

- Re-running refresh is idempotent: it re-derives `wire` rows and leaves
  `static`/`user` rows alone.
- A provider with no key is skipped (nothing to fetch with).
- A fetch error for one provider does not abort the run — it is reported and
  the other providers still reconcile.

### Key handling

Resolves keys through the **same `env → EncryptedDb` precedence as
`select_provider`** (env wins, then the encrypted secret store). The
`zoid refresh-models` subcommand runs inside the main binary, so it reuses the
existing `resolve_secret_key_path` / `EncryptedDb::open` path — no new key
plumbing, and no drift from the runtime selection logic. The env var name for
each provider comes from the `key_env` field on its registry entry (not
hardcoded). A provider with no key in either source is skipped and reported.

### Skill repurposing

The `refreshing-provider-models` built-in skill is **repurposed** (not dropped)
into a slim skill that instructs the agent to run `zoid refresh-models` for the
user and report the diff — a pointer to the tool, not a recipe for hand-editing
code. Rationale: not every model can be trusted to reach for the tool on its
own; explicit instructions to assist the agent in assisting the user are
warranted.

---

## 4. Gemini as a first-class provider

**Registry entry (shipped `models.toml`):**

```toml
[[provider]]
id = "gemini-api"
display = "gemini · api key"
family = "gemini"
transport = { kind = "http", default_base_url = "https://generativelanguage.googleapis.com" }
status = "available"
key_url = "https://aistudio.google.com/app/apikey"
key_env = "GEMINI_API_KEY"
```

**Models:** the three already in `MODEL_CAPS` (`gemini-3.5-flash`,
`gemini-3.1-pro`, `gemini-3-flash`) as `static` rows, plus the refresh tool can
pull the full live list from `/v1/models` as `wire` rows.

**Wire shape:** `google-gemini` — the existing `google_gemini.rs` leaf already
implements `stream()`/`list_models()` and is wired into Zen routing, so
surfacing it as a first-class provider is mostly registry + selection plumbing,
not new wire code.

**Three Gemini endpoints (do not conflate):**

- `GET {base}/v1/models` — model **id list** (`.models[].name`); the existing
  `GoogleGeminiProvider::list_models` shape, used for add/remove detection.
- `GET {base}/v1beta/models` — model **caps** (`inputTokenLimit` /
  `outputTokenLimit`); a new dedicated fetcher + parser for the refresh tool.
- `POST {base}/v1/models/{model}:streamGenerateContent` — the **streaming**
  endpoint, already implemented in the leaf (note the `v1` segment, matching
  the existing `google_gemini.rs` path builder).

**Caps:** the three shipped Gemini models are `static` (hand-researched), but
because `/v1beta/models` returns `inputTokenLimit`/`outputTokenLimit`, the
refresh tool can update their caps and add additional live models as `wire`
rows — the one new wire-derived caps source beyond Ollama.

**Selection wiring:** `select_provider`/`provider_for_id` become registry-driven
via `key_env` + `wire_shape`, so no new `match` arm is needed. `key_env_for`
resolves `GEMINI_API_KEY` from the `key_env` field.

**Key generation:** Google AI Studio → "Get API key" → create a key in a
project → a `GEMINI_API_KEY`-style string usable against
`generativelanguage.googleapis.com`. Exact steps at implementation time.

---

## 5. Local-model unification (drop SQLite, read TOML)

Fold local models into the TOML registry, drop the SQLite `local_models` table,
and read provisioning directly from the TOML.

**Shape:** `ollama-local` provider rows carry optional provisioning fields
alongside caps:

```toml
[[provider]]
id = "ollama-local"
# ... transport, status, key_url = none ...

  [[provider.model]]
  id = "qwythos"
  display = "Qwythos 9B (Claude Mythos 5, 1M)"
  wire_shape = "ollama"
  source = "static"
  context_window = 1_048_576
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "toggle"
  thinking_wire = "ollama"
  # local-only provisioning fields:
  runtime = "ollama"
  download_source = "hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M"
  quant = "Q4_K_M"
  modelfile = """..."""
  num_ctx = 98_304
  vram_curve = """[...]"""
```

**What changes:**

- `LocalModelSeed` (in `local_seed.rs`) is **removed**; its fields become
  optional keys on `ollama-local` model rows. `display_name` becomes the
  `display` field; `schema_version` is dropped (it only drove the SQLite
  seed-update mechanism, which is removed).
- The `ModelInfo` caps are shared — no more parallel string `thinking` /
  `thinking_wire` schema.
- `store.rs::seed_local_models` and the `local_models` table are **removed**;
  provisioning reads the merged `Registry`'s `ollama-local` rows directly.
- The `source = "user"` SQLite column semantics are replaced by the TOML
  `source` field (`user` rows in `models.user.toml`).

**Verified safe:** the `local_models` table is seed-only — the code comments
state "Phase 1: nothing reads the table yet," and there is no per-install state
(downloaded/installed flag, local tag). Dropping it loses nothing.

**Ownership guarantee:** the "user-defined rows never overwritten" guarantee
moves from the DB to the file-ownership split (shipped vs user file) and the
`source` field — stronger, since user rows live in a file the tool and upgrades
never touch.

---

## 6. Migration & compatibility

**What breaks:** the `&'static` consts and the `match family` arms are
compile-time. Moving to runtime TOML means every consumer changes. The existing
const-lock tests become tests against the shipped `models.toml`.

**Phased migration (never a broken tree):**

1. **Phase 1 — types + registry crate.** Add `WireShape` to `zoid-model`,
   create `zoid-registry` with the `Registry` struct + TOML parsing + merge.
   Ship `models.toml` as a transcription of today's consts. No behavior change
   yet — the consts still exist.
2. **Phase 2 — switch consumers.** Replace const lookups with `Registry`
   lookups, thread `Arc<Registry>` through `select_provider`/`provider_for_id`,
   replace `GO_MODELS`/`ZEN_MODELS` with `wire_shape` lookups, replace
   `key_env_for`/`family` matches with `key_env`/`wire_shape`, and perform the
   owned-type migration (§2). Delete the consts.
3. **Phase 3 — refresh tool.** Add `refresh::reconcile` + the
   `zoid refresh-models` subcommand. Repurpose the skill to point at the tool.
4. **Phase 4 — Gemini + local-model unification.** Add the `gemini-api`
   provider entry, wire `/v1beta/models` caps, fold local models into the TOML,
   drop `local_seed.rs` + the `local_models` table.

**Compatibility guarantees:**

- The shipped `models.toml` is a semantic transcription of today's consts, so
  Phase 2 is behavior-preserving (same providers, models, caps, defaults).
- **The transcription resolves the `claude-sonnet-4-6` duplicate** by splitting
  it into two `(provider, model)` rows with their correct per-provider windows
  (`anthropic-api` → 1M, `opencode-zen` → 200K). This is the *intended* (not
  accidental) behavior change of Phase 1 — it is the bug fix the redesign
  exists to make.
- Legacy provider id aliases (`ollama` → `ollama-cloud`, `anthropic` →
  `anthropic-api`) are preserved in the registry's `canonical_id` logic.
- `ZOID_CONTEXT_CEILING` env override still wins over the registry (unchanged).
- The selectable-provider count changes from 6 to 7 when `gemini-api` lands
  (Phase 4); the ported `selectable_has_six_providers` test must assert
  membership (including `gemini-api`) rather than a magic count.

---

## 7. Error handling & edge cases

**Load-time:**

- Missing user file → treated as empty (shipped file alone is valid).
- Malformed TOML in either file → fail loudly with a clear path + line error,
  but fall back to the shipped file alone if the *user* file is broken (a bad
  user edit must not brick startup — it is reported and ignored). **Note:** a
  broken user file also drops the user's `hidden`/`user` rows, so hidden models
  re-appear in the picker; the warning must call this out explicitly.
- Unknown enum string (`thinking`, `wire_shape`, `source`) → parse error with
  the offending value named, not a silent default.

**Refresh-time:**

- Provider with no key → skipped, reported.
- One provider's fetch fails → reported, other providers still reconcile.
- A `wire` row's caps can't be derived (endpoint returns no caps) → leave the
  existing row, report "couldn't refresh."
- A `static`/`user` row absent from live → reported, never deleted.
- Concurrent refresh (two processes) → last-writer-wins on the user file;
  acceptable for a manual tool, documented.

**Merge-time:**

- User row overrides shipped row by `(provider.id, model.id)`.
- `hidden = true` on a user row hides the matching shipped model.
- Duplicate `(provider.id, model.id)` within one file → parse error.

**Stale selection (upgrade removed/hid the selected provider or model):**

`config.provider` and `config.model` persist as id strings, but the registry is
now a file that changes on upgrade. When the merged registry no longer contains
the persisted selection (provider removed, model removed, or model `hidden`),
startup must **not** silently fall back to a default — that produces a "ghost"
model with wrong caps (32k `DEFAULT_MODEL_INFO`) or a silently different
provider, and the user is left wondering.

Instead, **hard stop into the quick-switch overlay** (`Overlay::ProviderSwitch`,
the `Alt+P` surface): boot with the quick-switch open and a banner explaining
that the selected provider/model is no longer available. The quick-switch's two
side-by-side panes (providers left, models right) cover both stale cases in one
screen — a removed provider is fixed in the left pane, a removed/hidden model
in the right pane — so no separate "which picker to land on" decision is
needed. No auto-selection, no in-memory fallback, no persisted rewrite.

The quick-switch is preferred over the full settings overlay (`Overlay::Config`)
because it is a focused, purpose-built provider+model surface: the settings
overlay buries the provider/model field behind a sections rail and renders the
picker as a contextual column, whereas the quick-switch drops the user straight
into the exact decision with nothing else in the way.

The banner also names the non-picker recovery paths (edit `config.toml`
directly, or un-hide the model in `models.user.toml`), and the overlay remains
dismissible so the user can choose one of those instead.

**Dismiss path (defined):** if the user dismisses the quick-switch without
picking a valid model, the app runs with a **clearly-labeled offline
`FakeProvider`** (the existing "offline echo" provider) and a persistent banner
"no valid model selected — press Alt+P to choose one." The agent loop is inert
(no real model), but the app stays navigable and the user can re-open the
picker, edit `config.toml`, or un-hide a model at their leisure. This is neither
a silent default nor a trap: the banner is explicit, and nothing is persisted.

This validation runs at load, after the merge, and covers both the
`config.provider` and `config.model` fields (a removed provider implies its
model is also stale). The quick-switch reads `state.switch_providers` /
`state.switch_models`, which the bin seeds from the registry — so those must be
seeded before the overlay opens (the same ordering `Alt+P` already relies on).

---

## 8. Testing strategy

**`zoid-registry` (new crate):**

- TOML parse: valid shipped + user files round-trip into a `Registry`.
- Merge: user overrides shipped by `(provider, model)`; `hidden = true` hides;
  user adds new providers/models.
- Enum string parsing: valid values map correctly; invalid values error with
  the offending name.
- Load: missing user file → shipped alone; malformed user file → fall back to
  shipped + report (and the report names the dropped `hidden`/`user` rows).
- Case-insensitivity: model-id lookup is case-insensitive; provider-id lookup
  is exact after `canonical_id`.
- Default model: `default = true` wins; fallback to first row with a warning
  when absent; a user row can override the default.
- Reconcile: add new wire rows, update changed wire rows, remove absent wire
  rows, report (not delete) static/user, idempotent re-run, key-missing skip,
  per-provider error isolation, and `wire` rows only for Ollama/Gemini.
- Gemini caps parser: `/v1beta/models` `inputTokenLimit`/`outputTokenLimit` →
  `context_window`/`max_output`, distinct from the `/v1/models` id-list parser.

**`zoid-model`:** the pure types + `Registry` data struct compile and are
constructible without I/O (so `zoid-provider`/`zoid-tui` tests can build a
`Registry` in-memory). The owned-type migration (`&'static str` → `String`) is
covered by the compile + the ported lookup tests.

**`zoid-provider`:** wire-shape routing tests assert against a `Registry`
(in-memory) instead of the deleted `GO_MODELS`/`ZEN_MODELS` consts. The existing
routing tests (chat→`/v1/chat/completions`, anthropic→`/v1/messages`,
responses→`/v1/responses`, gemini→`streamGenerateContent`) are preserved, fed
from the registry. `default_model`/`default_provider` are tested against the
registry's `default = true` flag.

**`zoid` / `zoid-core`:** the const-lock tests (`selectable_has_six_providers`,
`opencode_go_model_caps_match_reconciled_table`,
`opencode_zen_caps_match_table`, `key_url_field_present_on_all_providers`,
`models_for_by_id_and_alias`) are ported to assert against the shipped
`models.toml` (loaded in-test), so the invariants survive the migration. The
selectable-provider test asserts membership (including `gemini-api`) rather
than a magic count.

**Gemini:** new tests for the `gemini-api` entry (selectable, key_env,
wire_shape), `/v1beta/models` caps parsing, and selection wiring.

**Local models:** tests that `ollama-local` rows carry provisioning fields and
that provisioning reads from the registry (replacing the deleted
`seed_local_models` tests).

**Stale selection:** tests that a removed/hidden provider or model triggers the
quick-switch recovery path (boot into `Overlay::ProviderSwitch` with a banner,
not a silent fallback), that the switch panes are seeded before the overlay
opens, that a valid selection proceeds normally, and that **dismissing** the
quick-switch without picking runs the offline `FakeProvider` with a persistent
banner (not a silent default).

**Full gate:** `cargo nextest run --workspace --features zoid/local-embed
--no-fail-fast` (per `docs/DEVELOPMENT.md`).
