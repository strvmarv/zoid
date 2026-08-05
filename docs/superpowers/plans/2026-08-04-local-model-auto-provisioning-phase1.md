# Local Model Auto-Provisioning — Phase 1: Local Models DB Table

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `local_models` SQLite table to the zoid db, seed it with a curated qwythos entry, and version it with `PRAGMA user_version`. Nothing reads from the table yet — phase 1 ships with zero behavior change.

**Architecture:** The `local_models` table lives in the existing zoid SQLite db (`zoid-core::store::EventStore`). The seed data (curated local model definitions) lives in `zoid-model` as a `const` array — the same crate that holds `MODEL_CAPS`. The bin calls a new `seed_local_models` function at boot, which creates the table (if absent) and seeds/updates curated entries. User-defined entries are never overwritten.

**Tech Stack:** Rust, `rusqlite` (already a zoid-core dep), `serde_json` for the `vram_curve` column.

---

## Handoff Context

**Companion spec:** `docs/superpowers/specs/2026-08-04-local-model-auto-provisioning-design.md` (commit `aee629e`). Read §1 ("Db-backed local model registry") for the full schema design and the crate-boundary reasoning. This plan implements phase 1 only.

**Status:** planned, not started. No code written.

### Why the cloud table stays compiled (not in the db)

`zoid-model` is a dependency-free leaf crate (`Cargo.toml` has zero `[dependencies]`). Both `zoid-provider` and `zoid-tui` consume `model_info()`, `PROVIDERS`, `canonical_id()` from it without coupling to `zoid-core` or SQLite. Moving cloud model lookups into the db would require adding `rusqlite` to `zoid-model` (dragging SQLite into the TUI's dep graph) or moving `model_info()` to `zoid-core` (touching 13+ call sites). Neither is warranted — cloud models don't change at runtime. The `local_models` db table holds **local entries only**; cloud models stay in `MODEL_CAPS`.

### What "no behavior change" means

Phase 1 creates the table and seeds it, but nothing reads from it. The existing `model_info()`, `PROVIDERS`, `canonical_id()`, `resolve_thinking`, and `select_provider` are untouched. A user running zoid after phase 1 sees identical behavior — the table exists in the db but is inert. Phases 2-4 add the readers.

### The schema (from the spec)

```
local_models table:
  id              TEXT PRIMARY KEY   -- "qwythos" (also the Ollama tag name)
  display_name    TEXT NOT NULL
  provider        TEXT NOT NULL       -- "ollama-local"
  runtime         TEXT NOT NULL       -- "ollama" | "embedded" (future)
  source          TEXT NOT NULL       -- "curated" | "user"
  download_source TEXT NOT NULL
  quant           TEXT
  modelfile       TEXT
  context_window  INTEGER
  thinking        TEXT                -- "None" | "Toggle" | "Budget" | etc.
  thinking_wire   TEXT                -- "None" | "Anthropic" | "DeepSeek" | "OpenAI" | "Ollama"
  max_output      INTEGER
  tools           INTEGER             -- boolean
  prompt_cache    INTEGER             -- boolean
  num_ctx         INTEGER
  vram_curve      TEXT                -- JSON: [{"num_ctx":98304,"vram_mb":10000},...]
  schema_version  INTEGER             -- per-row entry version for curated updates
```

Versioning uses `PRAGMA user_version` (database-level), following the existing `store.rs` house style. The per-row `schema_version` is the *entry* version — used to decide whether a curated entry should be updated on upgrade (if the seed's version is higher, update; leave `source = "user"` rows untouched).

---

## Global Constraints

- **`zoid-model` must stay dependency-free.** The curated seed data is a `const` array of plain structs with `&'static str` fields — no `serde`, no `rusqlite`, no deps. The bin maps the seed structs to db rows.
- **User-defined entries (`source = "user"`) are never overwritten by seeding.** A re-seed on upgrade updates only `source = "curated"` rows where the seed's `schema_version` is higher.
- **`fetch_model_info` never writes the db.** The companion thinking-capability spec's in-memory overlay contract is preserved. The db is written only by the `--local` flow (phase 3) and the seed step (this phase).
- **Commit messages: no `Co-Authored-By` or any co-author trailer.**
- **`PRAGMA user_version` for db-level schema versioning**, following `store.rs`'s existing migration style (probe-then-ALTER, no per-row version for table existence).

---

## File Structure

**Created:**
- `crates/zoid-model/src/local_seed.rs` — `LocalModelSeed` struct + `CURATED_LOCAL_MODELS` const array. Pure data, no deps.

**Modified:**
- `crates/zoid-model/src/lib.rs` — `pub mod local_seed;` (one line).
- `crates/zoid-core/src/store.rs` — `seed_local_models` method on `EventStore` (creates table, seeds curated entries, versions with `PRAGMA user_version`).
- `crates/zoid/src/main.rs` — call `seed_local_models` at boot (one line, after `EventStore::open`).

**No new files in the bin or provider crates. No config changes. No UI changes.**

---

### Task 1: `LocalModelSeed` struct + curated qwythos entry

**Files:**
- Create: `crates/zoid-model/src/local_seed.rs`
- Modify: `crates/zoid-model/src/lib.rs` (add `pub mod local_seed;`)

**Interfaces:**
- Produces: `pub struct LocalModelSeed` (plain data, all `&'static str` or integer fields), `pub const CURATED_LOCAL_MODELS: &[LocalModelSeed]`. Task 2 maps these to db rows.

- [ ] **Step 1: Write the seed data file**

Create `crates/zoid-model/src/local_seed.rs`:

```rust
//! Curated local model seed data — the source for `local_models` db rows.
//! Pure data: no deps (this crate is dependency-free by design). The bin maps
//! these structs to SQLite rows at seed time. User-defined entries
//! (`source = "user"`) are never overwritten by re-seeding.

/// One curated local model definition. All fields are `&'static str` or
/// integers so the const array is compile-time constructible with zero deps.
/// `vram_curve` is a JSON string literal — the bin stores it as-is in the db.
pub struct LocalModelSeed {
    pub id: &'static str,
    pub display_name: &'static str,
    pub provider: &'static str,
    pub runtime: &'static str,
    pub download_source: &'static str,
    pub quant: Option<&'static str>,
    pub modelfile: &'static str,
    pub context_window: u64,
    pub thinking: &'static str,
    pub thinking_wire: &'static str,
    pub max_output: u64,
    pub tools: bool,
    pub prompt_cache: bool,
    pub num_ctx: u32,
    pub vram_curve: &'static str,
    pub schema_version: u32,
}

/// The curated local model catalog. Start small — qwythos only (the one zoid
/// has validated end-to-end). Adding more models is incremental: add an entry,
/// bump its `schema_version`, and the seed step on the next boot updates the
/// db row.
pub const CURATED_LOCAL_MODELS: &[LocalModelSeed] = &[
    LocalModelSeed {
        id: "qwythos",
        display_name: "Qwythos 9B (Claude Mythos 5, 1M)",
        provider: "ollama-local",
        runtime: "ollama",
        download_source: "hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M",
        quant: Some("Q4_K_M"),
        modelfile: r#"FROM hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M
TEMPLATE """{{ if .System }}<|im_start|>system
{{ .System }}<|im_end|>{{ end }}<|im_start|>user
{{ .Prompt }}<|im_end|>
<|im_start|>assistant"""
PARAMETER stop <|im_end|>
PARAMETER stop <|im_start|>"#,
        context_window: 1_048_576,
        thinking: "Toggle",
        thinking_wire: "Ollama",
        max_output: 0,
        tools: true,
        prompt_cache: true,
        num_ctx: 98_304,
        vram_curve: r#"[{"num_ctx":32768,"vram_mb":7000},{"num_ctx":65536,"vram_mb":8500},{"num_ctx":98304,"vram_mb":10000},{"num_ctx":131072,"vram_mb":12000}]"#,
        schema_version: 1,
    },
];
```

- [ ] **Step 2: Wire the module into zoid-model**

In `crates/zoid-model/src/lib.rs`, add at the top (after the module doc comment, before `pub struct ModelInfo`):

```rust
pub mod local_seed;
```

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build -p zoid-model`
Expected: compiles cleanly (no deps added, pure data).

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-model/src/local_seed.rs crates/zoid-model/src/lib.rs
git commit -m "feat(model): curated local model seed data (qwythos)

Pure const data in the dep-free leaf crate. The bin maps these structs
to local_models db rows at seed time. No deps added to zoid-model."
```

---

### Task 2: `local_models` table creation + `PRAGMA user_version`

**Files:**
- Modify: `crates/zoid-core/src/store.rs` (add table creation in `EventStore::open`, add `seed_local_models` method)

**Interfaces:**
- Consumes: `zoid_model::local_seed::{LocalModelSeed, CURATED_LOCAL_MODELS}` from Task 1.
- Produces: `pub fn seed_local_models(&self) -> Result<()>` on `EventStore`. Task 3 calls it at boot.

- [ ] **Step 1: Write the failing test**

Add to `crates/zoid-core/src/store.rs` test module (or create a new test file if the module is too large — check first):

```rust
#[test]
fn seed_local_models_creates_table_and_seeds_qwythos() {
    let dir = std::env::temp_dir().join(format!("zoid-seed-test-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = EventStore::open(dir.to_str().unwrap()).unwrap();
    store.seed_local_models().unwrap();

    // Table exists.
    let count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='local_models'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 1, "local_models table must exist");

    // qwythos is seeded.
    let id: String = store.conn.query_row(
        "SELECT id FROM local_models WHERE id = 'qwythos'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(id, "qwythos");

    // source is "curated".
    let source: String = store.conn.query_row(
        "SELECT source FROM local_models WHERE id = 'qwythos'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(source, "curated");
}

#[test]
fn seed_local_models_is_idempotent() {
    let dir = std::env::temp_dir().join(format!("zoid-seed-idempotent-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = EventStore::open(dir.to_str().unwrap()).unwrap();
    store.seed_local_models().unwrap();
    // Seeding again must not duplicate or error.
    store.seed_local_models().unwrap();
    let count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM local_models WHERE id = 'qwythos'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 1, "qwythos must appear exactly once after double-seed");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core --lib seed_local_models -- --nocapture`
Expected: FAIL to compile — `no method named 'seed_local_models' found for struct 'EventStore'`.

- [ ] **Step 3: Write the implementation**

In `crates/zoid-core/src/store.rs`, add inside the `impl EventStore` block (after the `open` method, before `append`):

```rust
    /// Create the `local_models` table (if absent), seed curated entries from
    /// `zoid_model::local_seed::CURATED_LOCAL_MODELS`, and version the schema
    /// with `PRAGMA user_version`. Idempotent: re-running on an existing db
    /// updates curated entries where the seed's `schema_version` is higher,
    /// and leaves `source = "user"` entries untouched. Phase 1: nothing reads
    /// the table yet.
    pub fn seed_local_models(&self) -> Result<()> {
        // Table creation (idempotent).
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS local_models (
                id              TEXT PRIMARY KEY,
                display_name    TEXT NOT NULL,
                provider        TEXT NOT NULL,
                runtime         TEXT NOT NULL,
                source          TEXT NOT NULL,
                download_source TEXT NOT NULL,
                quant           TEXT,
                modelfile       TEXT,
                context_window  INTEGER,
                thinking        TEXT,
                thinking_wire   TEXT,
                max_output      INTEGER,
                tools           INTEGER,
                prompt_cache    INTEGER,
                num_ctx         INTEGER,
                vram_curve      TEXT,
                schema_version  INTEGER
            )",
        )?;

        // Seed curated entries. Upsert: insert if absent, update if the seed's
        // version is higher. Never touch source = "user" rows.
        for seed in zoid_model::local_seed::CURATED_LOCAL_MODELS {
            // Check if the row exists and its source/schema_version.
            let existing: Option<(String, u32)> = self.conn.query_row(
                "SELECT source, COALESCE(schema_version, 0) FROM local_models WHERE id = ?1",
                params![seed.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).ok();

            match existing {
                None => {
                    // Insert new curated entry.
                    self.conn.execute(
                        "INSERT INTO local_models (
                            id, display_name, provider, runtime, source,
                            download_source, quant, modelfile, context_window,
                            thinking, thinking_wire, max_output, tools,
                            prompt_cache, num_ctx, vram_curve, schema_version
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                        params![
                            seed.id,
                            seed.display_name,
                            seed.provider,
                            seed.runtime,
                            "curated",
                            seed.download_source,
                            seed.quant,
                            seed.modelfile,
                            seed.context_window as i64,
                            seed.thinking,
                            seed.thinking_wire,
                            seed.max_output as i64,
                            seed.tools as i64,
                            seed.prompt_cache as i64,
                            seed.num_ctx as i64,
                            seed.vram_curve,
                            seed.schema_version as i64,
                        ],
                    )?;
                }
                Some((source, row_version)) if source == "curated" && seed.schema_version > row_version => {
                    // Update curated entry with newer seed version.
                    self.conn.execute(
                        "UPDATE local_models SET
                            display_name = ?2, provider = ?3, runtime = ?4,
                            download_source = ?5, quant = ?6, modelfile = ?7,
                            context_window = ?8, thinking = ?9, thinking_wire = ?10,
                            max_output = ?11, tools = ?12, prompt_cache = ?13,
                            num_ctx = ?14, vram_curve = ?15, schema_version = ?16
                        WHERE id = ?1",
                        params![
                            seed.id,
                            seed.display_name,
                            seed.provider,
                            seed.runtime,
                            seed.download_source,
                            seed.quant,
                            seed.modelfile,
                            seed.context_window as i64,
                            seed.thinking,
                            seed.thinking_wire,
                            seed.max_output as i64,
                            seed.tools as i64,
                            seed.prompt_cache as i64,
                            seed.num_ctx as i64,
                            seed.vram_curve,
                            seed.schema_version as i64,
                        ],
                    )?;
                }
                _ => {
                    // Row exists as "user" or with >= seed version: leave untouched.
                }
            }
        }

        // Set the db-level schema version for local_models.
        self.conn.pragma_update(None, "user_version", 1)?;

        Ok(())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-core --lib seed_local_models -- --nocapture`
Expected: PASS — both tests pass.

- [ ] **Step 5: Run the full zoid-core suite for regressions**

Run: `cargo test -p zoid-core --lib -- --nocapture`
Expected: PASS — all existing tests still pass (the new table doesn't affect existing tables or queries).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(core): local_models db table + curated seed step

Creates the local_models SQLite table in EventStore::open's db, seeds
curated entries from zoid_model::local_seed, versions with PRAGMA
user_version. Idempotent: re-seed updates curated rows where the seed's
schema_version is higher, leaves user entries untouched. Nothing reads
the table yet — phase 1 is inert."
```

---

### Task 3: Call `seed_local_models` at boot

**Files:**
- Modify: `crates/zoid/src/main.rs` (one call after `EventStore::open`)

**Interfaces:**
- Consumes: `EventStore::seed_local_models` from Task 2.
- Produces: the `local_models` table is created and seeded on every boot. Future phases read from it.

- [ ] **Step 1: Find the boot site where `EventStore::open` is called**

Search: `grep -n 'EventStore::open' crates/zoid/src/main.rs`
This returns the line where the session store opens. The `seed_local_models` call goes immediately after it.

- [ ] **Step 2: Add the boot call**

After the `EventStore::open` call (the exact line from step 1), add:

```rust
    // Seed the local_models table (curated entries from zoid_model). Phase 1:
    // creates the table and seeds it; nothing reads from it yet. Idempotent —
    // re-runs on every boot, updates curated entries if the seed version is
    // higher, leaves user-defined entries untouched.
    if let Err(e) = events.store().seed_local_models() {
        tracing::warn!(error = %e, "failed to seed local_models table");
    }
```

Note: check how `EventStore` is accessed in the boot path — it may be behind a `SessionHandle` or accessed via `events.store()`. Inspect the actual variable name from step 1 and use the right accessor. The call is non-fatal (warn on error) — a seed failure must not prevent zoid from starting.

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build -p zoid`
Expected: compiles cleanly.

- [ ] **Step 4: Run zoid and verify the table exists**

Run: `cargo run --release -- -p zoid -- --yolo` (then quit immediately with Ctrl+C or `:q`)

Then verify:
```bash
sqlite3 ~/.local/share/zoid/zoid.db "SELECT id, source, schema_version FROM local_models;"
```
Expected: `qwythos|curated|1`

- [ ] **Step 5: Run the full bin test suite for regressions**

Run: `cargo test -p zoid -- -- --nocapture`
Expected: PASS — all existing tests still pass (the seed is inert).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(bin): seed local_models table at boot

Calls EventStore::seed_local_models after the db opens. Non-fatal: a
seed failure logs a warning but does not prevent zoid from starting.
Phase 1: the table is created and seeded but nothing reads from it yet."
```

---

## Post-implementation verification (not a task — manual, after all tasks land)

1. `cargo build --release` and run zoid once.
2. `sqlite3 ~/.local/share/zoid/zoid.db ".schema local_models"` — verify the table matches the spec schema.
3. `sqlite3 ~/.local/share/zoid/zoid.db "SELECT id, display_name, thinking, thinking_wire, num_ctx FROM local_models;"` — verify qwythos is seeded with `Toggle` / `Ollama` / `98304`.
4. `sqlite3 ~/.local/share/zoid/zoid.db "SELECT vram_curve FROM local_models WHERE id='qwythos';"` — verify the JSON array has 4 entries (32K/64K/96K/128K).
5. Run zoid a second time — verify no duplicate rows (idempotency): `sqlite3 ~/.local/share/zoid/zoid.db "SELECT COUNT(*) FROM local_models WHERE id='qwythos';"` should be 1.
6. Run zoid with a user-inserted row: `sqlite3 ~/.local/share/zoid/zoid.db "INSERT INTO local_models (id, display_name, provider, runtime, source, download_source, schema_version) VALUES ('test-model', 'Test', 'ollama-local', 'ollama', 'user', 'test', 1);"` — then run zoid — then verify the user row survives: `sqlite3 ~/.local/share/zoid/zoid.db "SELECT source FROM local_models WHERE id='test-model';"` should still be `user`.