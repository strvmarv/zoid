# Local Model Auto-Provisioning Design

## Vision

`zoid --local qwythos hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M`
downloads, configures, and runs a local model — no manual Ollama install, no
manual `ollama pull`, no manual `ollama create` with the right chat template.
One command, and zoid handles the rest. The user can then switch between local
and cloud models mid-session the same way they switch between any two models.

This is the stopgap (Ollama as the runtime). The long-term goal (zoid bundles
its own inference engine, no Ollama needed) is designed for but deferred: the
registry carries a `runtime` field that today is always `ollama`, and will
accept `embedded` when an `EmbeddedProvider` impl is added.

## Problem

Today, running a local model with zoid requires:
1. Install Ollama (platform-specific, with edge cases like the Arch pacman
   conflict on `/usr/share/ollama`)
2. `ollama pull hf.co/...` (manual, no progress in zoid)
3. `ollama create qwythos -f Modelfile` with a hand-written chat template and
   stop tokens (the raw HF pull gets a bare `{{ .Prompt }}` template that
   produces worse output — this is the difference between `qwythos:latest`
   (usable) and `hf.co/...:Q4_K_M` (bare default))
4. Manually edit `config.toml` (provider, model, num_ctx)
5. Know your VRAM limits to pick the right `num_ctx` (from benchmarking —
   qwythos at 96K fits in 12GB VRAM at ~23 tok/s; 128K spills to CPU and
   collapses to ~4.6 tok/s)

Each step is a friction point. The user is doing work that zoid can do.

## Architecture

### 1. Db-backed local model registry (cloud table stays compiled)

The compiled `MODEL_CAPS` and `PROVIDERS` tables stay in `zoid-model` — they
are the runtime lookup for **cloud** models. `zoid-model` is a dependency-free
leaf crate by design (`Cargo.toml` has zero `[dependencies]`); both
`zoid-provider` and `zoid-tui` consume it without coupling to `zoid-core` or
SQLite. Moving the cloud table into the db would either drag SQLite into the
TUI's dependency graph or move `model_info()` across crate boundaries (touching
13+ call sites). Neither is warranted — cloud models don't change at runtime.

A new `local_models` table in the zoid SQLite db holds **local model entries
only** — curated (seeded by zoid) and user-defined. The bin merges the two
sources at runtime: cloud lookups hit the static `MODEL_CAPS` table (unchanged);
local lookups hit the db. `model_info()` in `zoid-model` stays byte-identical;
the bin's `select_provider` and `resolve_thinking` consult the db for local
model ids and fall back to the static table for cloud.

**Schema:**

```
local_models table:
  id              TEXT PRIMARY KEY   -- "qwythos" (also the Ollama tag name for local rows)
  display_name    TEXT NOT NULL       -- friendly name for the TUI
  provider        TEXT NOT NULL       -- "ollama-local" (future: other local runtimes)
  runtime         TEXT NOT NULL       -- "ollama" | "embedded" (future)
  source          TEXT NOT NULL       -- "curated" (seeded by zoid) | "user" (user-added)
  download_source TEXT NOT NULL       -- HF URL or Ollama registry tag
  quant           TEXT                -- "Q4_K_M", null if unknown
  modelfile       TEXT                -- full Ollama Modelfile body (template, stop tokens, params)
  context_window  INTEGER             -- trained context window
  thinking        TEXT                -- "None" | "Toggle" | "Budget" | "Adaptive" | "ToggleWithEffort"
  thinking_wire   TEXT                -- "None" | "Anthropic" | "DeepSeek" | "OpenAI" | "Ollama"
  max_output      INTEGER             -- 0 = unlimited
  tools           INTEGER             -- boolean
  prompt_cache    INTEGER             -- boolean
  num_ctx         INTEGER             -- recommended num_ctx for this model
  vram_curve      TEXT                -- JSON array of {num_ctx, vram_mb} pairs, null if unknown
  schema_version  INTEGER             -- row version (see versioning below)
```

**Design decisions in the schema:**

- **`thinking_wire` is a column.** The companion thinking-capability spec
  shipped `ThinkingWireShape` (`None | Anthropic | DeepSeek | OpenAI |
  Ollama`) — `openai_compat.rs` and `openai_responses.rs` switch on it to
  distinguish DeepSeek from OpenAI (both are `ToggleWithEffort` on the
  `thinking` axis). Dropping it makes the db unable to reconstruct `ModelInfo`
  for those models. All seven `ModelInfo` fields are represented.
- **`modelfile` replaces `chat_template` + `stop_tokens`.** An Ollama Modelfile
  is a single text block with the template, stop tokens, system prompt, and
  parameters. Storing the full Modelfile body (1:1 with `POST /api/create`) is
  more future-proof than normalizing fields zoid doesn't own — every future
  Modelfile field (temperature, rope_scaling) would otherwise require a schema
  migration.
- **`vram_curve` replaces `vram_min_mb` / `vram_rec_mb`.** A single `num_ctx`
  + single `vram_rec_mb` can't represent the context ladder (96K at 10GB, 128K
  at 12GB, 32K at 7GB) that `recommend_model` needs. A JSON array of
  `{num_ctx, vram_mb}` pairs lets `recommend_model` find the highest `num_ctx`
  that fits the detected VRAM — the actual logic from the eval session.
- **`capabilities` is not a column.** The discrete columns (`thinking`,
  `tools`) are the source of truth for logic; the daemon's raw capabilities
  array is not persisted (see "Fetched values stay in-memory" below). Storing
  both would invite drift.
- **`id` is the Ollama tag for local rows.** `select_provider` reads
  `config.model` (the tag) and passes it to the provider. For local rows, the
  registry id and the wire tag are the same string. Cloud models keep their
  ids in `MODEL_CAPS` (e.g. `glm-5.2:cloud`), which is not an Ollama tag.

**Seeding and versioning:**

Versioning follows the existing `store.rs` house style (`ALTER TABLE` +
`PRAGMA user_version`), not a per-row `schema_version` column. The per-row
`schema_version` in the schema above is the *entry* version — used to decide
whether a curated entry should be updated on upgrade (if the seed's version is
higher than the row's, update the curated row; leave `source = "user"` rows
untouched regardless).

At first boot (or upgrade), zoid runs a seed step:
- Table doesn't exist → create it, seed with curated local entries (qwythos
  only to start).
- `PRAGMA user_version` is older → run a migration (add new curated entries,
  update changed curated entries by comparing per-row `schema_version`, leave
  `source = "user"` entries untouched).
- User-defined entries (`source = "user"`) are never overwritten by seeding.

The curated seed data lives in `zoid-model` as a `const` array — it's the seed
source, not the runtime lookup.

**Runtime lookup:**

The bin resolves model capabilities at two points:
1. **Boot** — `spawn_model_info_fetch` calls `/api/show` and stores the result
   in `app.fetched_model_info` (in-memory, exactly as the companion spec
   designed). This is the *runtime overlay* — preferred over both the static
   table and the db at read time.
2. **Turn** — `resolve_thinking` reads `app.fetched_model_info` first (in-memory
   overlay), then falls back to the db (for local ids) or the static table
   (for cloud ids). `model_info()` in `zoid-model` is unchanged for cloud
   lookups; the bin adds a db-lookup path for local ids.

**Fetched values stay in-memory — never persisted to the db.**

The companion spec's safety contract is preserved: `fetch_model_info` returns
a lenient, in-memory `ModelInfo` stored on `app.fetched_model_info`. It does
**not** write the db. Writing the daemon's report back to the db would risk
corrupting curated entries (a partial `/api/show` response — e.g. context
window present but capabilities array absent — would overwrite a known-good
`thinking: Toggle` with `None`), introduce a second writer racing the event
log on the same SQLite connection, and make the daemon's report durable across
reboots even when the daemon is down on the next boot (losing the static
fallback). The db row is the *curated/user* truth; the daemon fetch is a
*runtime overlay*, merged at read time, never persisted. If a user-defined
model's discovered capabilities should survive a reboot, that happens as an
explicit, gated write in the `--local` flow (step 6), not in the background
fetch. Curated entries are write-protected from `fetch_model_info` under all
paths.

### 2. Wizard-level setup (hardware detection + Ollama install)

Two new bin-level modules (not skills — they run before the agent loop, which
needs a model running to invoke skills):

**`crates/zoid/src/local_setup.rs`** — hardware detection + Ollama lifecycle.

- `detect_hardware()` → `HardwareProfile`: runs `nvidia-smi` (GPU model, VRAM),
  `lscpu` (cores), `free -h` (RAM). Falls back gracefully if commands are absent
  (CPU-only machine, no nvidia-smi). Returns:
  `{ gpu: Option<GpuInfo>, vram_mb: Option<u32>, cpu_cores: u32, ram_gb: u32 }`.

- `detect_platform()` → `Platform`: detects OS and package manager. Returns
  `Linux { distro }`, `MacOS`, or `Windows`.

- `ensure_ollama(profile, platform)` → `OllamaStatus`: checks `which ollama`,
  checks `curl localhost:11434/api/tags`. If not installed, installs via the
  platform-appropriate method. If installed but not running, starts the daemon.
  Returns: `{ installed: bool, running: bool, version: Option<String> }`.

- `recommend_model(profile, registry)` → `ModelRecommendation`: matches the
  hardware profile against the registry's local model entries (which carry
  `vram_curve`). Finds the highest `num_ctx` that fits the detected VRAM from
  the curve's `{num_ctx, vram_mb}` pairs. Returns the best-fit model + quant +
  recommended `num_ctx`. This is the logic from the eval session — "12GB VRAM →
  qwythos Q4_K_M at 96K" — codified against registry data.

**Wizard integration** — the existing onboarding wizard gains a new branch:

1. "Connect to a cloud provider" (existing — enter API key, pick model)
2. "Run a local model" (new) → `detect_hardware()` → show profile →
   `ensure_ollama()` → if install needed, show progress → `recommend_model()`
   → offer recommended model (or picker from compatible entries) → pull via
   Ollama API with progress → create local tag with modelfile → write
   registry entry → set config → agent loop starts

**Paths for existing users.** The wizard only fires for first-time users
(`wizard_needed`, main.rs:1051 — gated on `first_time_user`). Existing users
who already have a cloud provider configured reach the local-model path via:

- **`zoid --local <name> <source>`** (phase 3) — the primary path for existing
  users. Runs from the terminal, same pull/create/registry/config flow as the
  wizard but without the TUI orchestration.
- **`:config` provider picker** — when the user switches to `ollama-local` and
  picks a local model tag that isn't yet pulled, `:config` offers to pull it
  (calling the same `ensure_ollama` + pull + create path). This makes the
  mid-session switch in §6 work without requiring an out-of-band `ollama pull`.

### 3. Cross-platform Ollama install

**Linux:**
- Official: `curl -fsSL https://ollama.com/install.sh | sh` — detects arch
  (amd64/arm64), downloads binary to `/usr/local/bin`, sets up systemd service.
- Package-manager installs (pacman, apt) are detected first — if Ollama is
  already installed via a package manager, offer to upgrade via that manager
  instead of running the script over it.
- The pacman conflict (`/usr/share/ollama exists in filesystem`) is a
  post-install transaction error, not a pre-flight detection — zoid parses
  pacman's stderr on failure. Offer: (1) upgrade via package manager, (2)
  stop the existing service + remove old package + install via script, (3)
  manual instructions. Note: stopping the service before removal avoids
  systemd unit-name conflicts between the script's service and the
  package's service.

**macOS:**
- Prefer `brew install ollama` if Homebrew is available (non-interactive, works
  in terminal). Start via `brew services start ollama`.
- **Headless macOS** (server, CI): `brew services` may error or no-op (no
  launchd GUI session). Fall back to `ollama serve &` in a detached process.
- Fall back to downloading `ollama.dmg` — `hdiutil attach` the dmg, instruct
  user to drag to Applications. zoid cannot complete this step automatically
  (it's a GUI install). zoid polls for `ollama` by re-scanning PATH directories
  (not re-reading the env var — a running process doesn't pick up shell PATH
  changes from `/etc/paths`), with a timeout and a "skip" option.

**Windows:**
- Download and run `OllamaSetup.exe` with silent-install flags:
  `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`. Requires Windows 10+.
- The installer auto-starts the background app. If not running, spawn
  `ollama serve` via `CreateProcess` with `DETACHED_PROCESS` (not Unix `&`).
- **PATH refresh:** the Inno Setup installer updates the system PATH, but a
  running terminal/zoid won't inherit it without a shell restart. zoid
  re-scans PATH directories (same approach as macOS) rather than trusting the
  process env var.

**Daemon version probe (all platforms):**

`ensure_ollama` checks `which ollama` then `curl localhost:11434/api/tags`,
but a dev machine may have multiple Ollama installs (e.g. a package-manager
binary on PATH and a script-installed daemon running on a different binary).
To avoid proceeding against the wrong binary, `ensure_ollama` also probes
`ollama --version` (the binary `which` resolved) and compares it to the
running daemon's `/api/version`. A mismatch is a warning, not a blocker —
the user may have intentionally pinned an older daemon.

### 4. The `--local` CLI flow and model pull

```
zoid --local <friendly_name> <download_source>
```

**The flow:**

1. **Parse args** — `friendly_name` is the registry id. `download_source` is an
   Ollama-pull-compatible reference (HF URL, ollama registry tag, or local path).

2. **Ensure Ollama** — call `ensure_ollama()`. If not installed, the hardware
   detection + install path runs (same as the wizard). If installed and
   running, proceed.

3. **Pull the model** — call `POST /api/pull` with the `download_source`.
   Stream download progress via an `InstallProgress` trait (TUI impl for the
   wizard, stderr impl for terminal-only). Ollama's pull API handles resume,
   blob dedup, and integrity. zoid shows a progress bar.

4. **Check the registry** — does `friendly_name` already exist?
   - **Curated entry exists**: use the known-good metadata (modelfile, thinking,
     context window, vram_curve). Skip introspection on first pull — the
     curated entry is trusted.
   - **No entry**: introspect via `POST /api/show` (now that the model is
     pulled) and populate thinking, context window, tools, prompt_cache. The
     chat template comes from Ollama's auto-generated modelfile — if it's the
     bare `{{ .Prompt }}` default, warn the user that a custom template may be
     needed. Create a user-defined entry (`source = "user"`).

5. **Create the local tag** — call `POST /api/create` with the friendly name
   and the curated modelfile (if curated) or the auto-generated modelfile (if
   user-defined). This is the step that made `qwythos:latest` work vs the raw
   HF pull — the proper `<|im_start|>` template and `<|im_end|>` stop tokens.

6. **Write the registry entry** — insert or update the row in the
   `local_models` table. For curated models, update with any new metadata. For
   user-defined, insert with `source = "user"`. This is the only path that
   writes user-defined capabilities to the db (not the background
   `fetch_model_info`).

7. **Set config** — write `provider = "ollama-local"`,
   `model = "<friendly_name>"` to config.toml. The next `zoid` invocation uses
   this model.

**The `runtime` field:** today every local model entry gets
`runtime = "ollama"`. When the embedded engine is ready, the flow would be
`zoid --local qwythos hf.co/... --runtime embedded` → download GGUF directly,
write `runtime = "embedded"`, and `select_provider` constructs an
`EmbeddedProvider`. The flag for runtime choice is deferred — today it's
always Ollama.

### 5. Curated local model definitions

The curated seed data gains validated local model entries. Start small —
qwythos only (the one zoid has validated end-to-end). Each entry:

```
qwythos (Qwythos-9B-Claude-Mythos-5-1M):
  provider:        ollama-local
  runtime:         ollama
  download_source: hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M
  quant:           Q4_K_M
  modelfile:       |
                   FROM hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M
                   TEMPLATE """{{ if .System }}<|im_start|>system
                   {{ .System }}<|im_end|>{{ end }}<|im_start|>user
                   {{ .Prompt }}<|im_end|>
                   <|im_start|>assistant"""
                   PARAMETER stop <|im_end|>
                   PARAMETER stop <|im_start|>
  context_window:  1048576
  thinking:        Toggle
  thinking_wire:   Ollama
  tools:           true
  prompt_cache:    true
  max_output:      0
  vram_curve:      [{"num_ctx":32768,"vram_mb":7000},{"num_ctx":65536,"vram_mb":8500},{"num_ctx":98304,"vram_mb":10000},{"num_ctx":131072,"vram_mb":12000}]
  num_ctx:         98304  (96K — the sweet spot from the eval session)
```

The `vram_curve` encodes the context ladder from the benchmarking session —
96K at ~9.8GB (91% GPU) as the practical ceiling, 128K at ~12GB spilling to
CPU and collapsing to ~4.6 tok/s, 32K at ~7GB (barely fits).

The curated modelfile bakes in the known-good `<|im_start|>` format so
`zoid --local qwythos hf.co/...` produces a working model on the first try.
User-defined models get Ollama's auto-generated modelfile — if it's the bare
`{{ .Prompt }}` default, zoid warns and suggests the user provide a template.

### 6. Switching between local and cloud mid-session

```
user: :config → switch to ollama-cloud, model glm-5.2:cloud
  → SELECT FROM local_models WHERE id="glm-5.2:cloud" → no local row → static MODEL_CAPS
  → select_provider() — runtime="ollama", provider="ollama-cloud" → OllamaProvider with API key
  → resolve_thinking(config, provenance, provider, model_info.thinking)
  → agent loop continues with cloud model

user: :config → switch to ollama-local, model qwythos
  → SELECT FROM local_models WHERE id="qwythos" → local entry
  → if tag not yet pulled: offer to pull (ensure_ollama + pull + create)
  → select_provider() — runtime="ollama", provider="ollama-local" → OllamaProvider, keyless
  → resolve_thinking(config, provenance, provider, fetched.thinking=Toggle)
  → agent loop continues with local model, think:true
```

## What does not change

- **The `Provider` trait** — still takes a `CompletionRequest` and streams
  `ProviderEvent`s. The `runtime` field tells the bin which provider impl to
  construct (`OllamaProvider` today, `EmbeddedProvider` future).
- **`resolve_thinking`** — stays pure. Its source for local model capabilities
  changes from the static table to the db (for local ids) or stays the static
  table (for cloud ids), but the function body is unchanged. The in-memory
  `fetched_model_info` overlay (companion spec) is still preferred over both.
- **`request_body`** — unchanged. Still trusts `resolve_thinking`'s output.
- **`model_info()` in `zoid-model`** — unchanged for cloud models. The bin
  adds a db-lookup path for local ids but the function itself stays.
- **The wizard's existing cloud-provider branch** — unchanged.
- **`emit_ephemeral` for `ModelThinking`** — unchanged (thinking stays
  in-memory, not persisted to the db — by design).

## What does change (explicitly)

- **`select_provider` (main.rs:1115)** — gains a `runtime` dispatch: reads the
  local model's `runtime` field from the db (defaulting to `"ollama"` for
  cloud models, which have no db row). Today this is always `"ollama"` →
  `OllamaProvider`. When the embedded engine is ready, `runtime = "embedded"`
  → `EmbeddedProvider`. This is the one new dispatch axis the `runtime` field
  introduces; the spec owns it rather than claiming `select_provider` is
  unchanged.

## Error handling

- **Ollama install fails** (pacman conflict, permission denied, network):
  wizard shows the error, offers retry or manual instructions. Doesn't crash
  — user can fall back to a cloud provider.
- **Model pull fails** (network error, HF rate limit): progress bar shows the
  error, offers retry. Partial downloads resume via Ollama's pull API.
- **`/api/show` fails or returns unexpected data**: the in-memory
  `fetched_model_info` overlay stays `None` (companion spec behavior). The
  `local_models` db row keeps its curated/seeded values. Same lenient
  fallback as today — no regression. `fetch_model_info` never writes the db.
- **Hardware detection fails** (no nvidia-smi, headless server):
  `detect_hardware` returns a CPU-only profile. `recommend_model` offers
  models that fit in RAM, warns that CPU inference is slow. No crash.
- **Db seed/migration fails** (corrupt db, schema mismatch): fall back to a
  compiled emergency seed (the same curated data, hardcoded). Log a warning.
  The agent loop still starts.
- **macOS dmg install path** (brew not available): zoid `hdiutil attach`es
  the dmg, instructs the user to drag to Applications. zoid re-scans PATH
  directories (not the env var — a running process doesn't pick up shell PATH
  changes) for `ollama`, with a timeout and a "skip" option.

## Testing

- **`local_setup::detect_hardware`** — unit tests with mocked command output
  (nvidia-smi, lscpu, free). Test CPU-only fallback (no nvidia-smi), GPU
  present, AMD GPU (no nvidia-smi but has /sys/class/drm).
- **`local_setup::detect_platform`** — unit tests per OS variant.
- **`local_setup::ensure_ollama`** — integration tests against a mock Ollama
  daemon (already-installed + running, installed-but-not-running,
  not-installed). Install-path tests are manual (can't mock a real install in
  CI without containerization).
- **Registry seeding** — test that first boot creates the table + seeds, that
  upgrade runs the migration, that user entries survive re-seeding.
- **`recommend_model`** — pure function tests: 12GB VRAM → qwythos 96K, 24GB →
  higher context, 4GB → no compatible model (warn), CPU-only → RAM-limited.
- **`--local` flow** — integration test: `zoid --local qwythos hf.co/...` with
  a mock Ollama daemon, verify registry entry created, config written, tag
  created with the right template.

## Scope

This is a multi-phase feature. The phases, in order:

1. **Local model db table** — add `local_models` table, seed with curated
   entries (qwythos), version with `PRAGMA user_version`. The static
   `MODEL_CAPS` table stays as-is. Nothing reads from the db yet — genuinely
   no behavior change. This is the foundation — phases 2-4 depend on it.
2. **Local setup module** — `detect_hardware`, `detect_platform`,
   `ensure_ollama`, `recommend_model`. Platform-independent logic with
   platform-specific install impls. Depends on phase 1's `vram_curve` schema.
3. **`--local` CLI flow** — the pull + create + registry-write + config-write
   path. The first user-facing feature.
4. **Wizard integration** — the "Run a local model" wizard branch that
   orchestrates phases 2-3 with TUI progress. Also wires `:config` to provision
   (not just select) local models.

Phase 1 ships alone with no behavior change (table created, nothing reads it).
Phases 2-3 ship together (the setup module powers the CLI flow). Phase 4 is
the polished UX on top.

## Long-term: embedded inference (deferred)

When the embedded engine (mistral.rs or llama.cpp) is ready:
- Add `EmbeddedProvider` impl of the `Provider` trait.
- The `--local` command gains `--runtime embedded` → downloads GGUF directly
  (not via Ollama), writes `runtime = "embedded"` to the `local_models` table.
- `select_provider` reads `runtime = "embedded"` → constructs
  `EmbeddedProvider` instead of `OllamaProvider`.
- The registry schema, wizard, config, and `resolve_thinking` are unchanged —
  they already don't bake in Ollama assumptions.
- **The download path is new work, not a reuse of `zoid-embed`.** The
  `zoid-embed` fetch (`crates/zoid-embed/src/fetch.rs`) downloads three pinned
  files for one embedding model (bge-small-en-v1.5) with fixed sha256s. It is
  not a general GGUF download: no resumable multi-GB files, no HF LFS redirect
  resolution, no quant selection, no rate-limit handling. The embedded path
  needs a new general-purpose weight downloader with progress, resume, and
  HF API integration. The concept of a progress-callback download exists; the
  implementation for GGUF-scale models does not.