# P0 — Spine & Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up zoid's event-sourced spine and the UX-fidelity test pipeline end-to-end on trivial data — a Cargo workspace where `zoid` boots to an empty Chat frame, persists an append-only event log to SQLite, and replays it on the next launch.

**Architecture:** A Cargo workspace of four crates. `zoid-core` holds the pure event-sourced core (immutable `Event` log, a `rusqlite` single-writer store, and pure projections like `transcript`). `zoid-provider` defines the LLM provider seam plus a deterministic `FakeProvider`. `zoid-tui` holds the single design-tokens module and ratatui render functions. `zoid` is the binary that wires them: open the store, load + fold the log, render the Chat frame, quit on a key. Tests prove the two riskiest bets early — the event-sourced round-trip (`proptest` + a SQLite round-trip) and the fidelity pipeline (`ratatui::TestBackend` + `insta` snapshots).

**Tech Stack:** Rust (edition 2021), `ratatui` + `crossterm` (TUI), `rusqlite` (bundled/static SQLite), `serde`/`serde_json`, `ulid`, `anyhow`; dev: `proptest`, `insta`, `tempfile`, `tokio` (provider tests only), `async-trait`.

## Global Constraints

- **Rust edition `2021`** for every crate.
- **No `wasmtime`, no `tree-sitter`, no `tokio` in the binary** in P0 — `wasmtime` is deferred to the plugin phase (spec §3); `tree-sitter` lands P4; the binary's event loop is synchronous in P0 (tokio/streaming arrives P1). `tokio` appears only as a **dev-dependency** of `zoid-provider`.
- **`rusqlite` uses the `bundled` feature** — SQLite is statically compiled in, preserving the single-binary goal (spec §3).
- **`ts` is injected, never read from an ambient clock inside `zoid-core`** — pure code takes timestamps as parameters (spec §5; keeps the core property-testable).
- **One design-tokens module is the single source for all glyphs/colors** (`zoid-tui::tokens`); every render reads from it (spec §16). Values copied verbatim from `docs/ux/README.md`.
- **Snapshots are UTF-8** — the buffer contains box-drawing and glyph chars (`⎇ › ▌`); `insta` writes/reads UTF-8 by default, keep it that way so snapshots don't diff across machines.
- **Event schema encodes the full vision; behavior is phased** (spec §5) — keep `parent`, `branch`, and `tokens` fields even though P0 writes a single linear branch and `tokens: None`.
- **Commits:** after each task; branch is `main`; **never add a `Co-Authored-By` or any co-author trailer** (repo `CLAUDE.md`).
- **Release profile** (size-optimized single binary): `opt-level="z"`, `lto=true`, `strip=true`, `panic="abort"`, `codegen-units=1`.

## File Structure

```
Cargo.toml                              # workspace + shared deps + release profile
.gitignore                             # (append /.zoid/)
crates/
  zoid-core/
    Cargo.toml
    src/lib.rs                          # re-exports: event, projection, store
    src/event.rs                        # Event, EventKind, BranchId, TokenStat
    src/projection.rs                   # Role, Turn, transcript()
    src/store.rs                        # EventStore (rusqlite single-writer)
    tests/round_trip.rs                 # persist → reopen → replay (tempfile)
  zoid-provider/
    Cargo.toml
    src/lib.rs                          # Provider trait, ProviderEvent, FakeProvider
  zoid-tui/
    Cargo.toml
    src/lib.rs                          # re-exports: tokens, chat
    src/tokens.rs                       # glyph + color design tokens (single source)
    src/chat.rs                         # render_chat()
    tests/chat_snapshot.rs              # TestBackend + insta snapshots
  zoid/
    Cargo.toml
    src/main.rs                         # boot → load → fold → render → quit
```

Each file has one responsibility. `event.rs` owns the data shape; `projection.rs` owns reads over it; `store.rs` owns persistence; `tokens.rs` is the lone style source; `chat.rs` is the lone Chat renderer; `main.rs` is glue only.

---

### Task 1: Workspace scaffold + `zoid-core` skeleton (prove the build + test harness)

Establishes the workspace, the release profile, the shared dependency table, and a compiling `zoid-core` with one trivial passing test — so every later task has a known-good `cargo test` baseline.

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/zoid-core/Cargo.toml`
- Create: `crates/zoid-core/src/lib.rs`
- Modify: `.gitignore` (append `/.zoid/`)

**Interfaces:**
- Consumes: nothing.
- Produces: the workspace + `zoid-core` crate that later tasks add modules to.

- [ ] **Step 1: Write the workspace root `Cargo.toml`**

```toml
[workspace]
members = ["crates/zoid-core", "crates/zoid-provider", "crates/zoid-tui", "crates/zoid"]
resolver = "2"

[workspace.package]
edition = "2021"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ulid = { version = "1", features = ["serde"] }
rusqlite = { version = "0.32", features = ["bundled"] }
anyhow = "1"
ratatui = "0.29"
crossterm = "0.28"
async-trait = "0.1"
proptest = "1"
insta = "1"
tempfile = "3"
tokio = { version = "1", features = ["macros", "rt"] }

[profile.release]
opt-level = "z"
lto = true
strip = true
panic = "abort"
codegen-units = 1
```

- [ ] **Step 2: Write `crates/zoid-core/Cargo.toml`**

```toml
[package]
name = "zoid-core"
version = "0.0.0"
edition.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
ulid = { workspace = true }
rusqlite = { workspace = true }
anyhow = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 3: Write the minimal `crates/zoid-core/src/lib.rs` with one test**

```rust
//! zoid-core — the event-sourced spine: an append-only log, a SQLite store,
//! and pure projections over the log.

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
```

- [ ] **Step 4: Append `/.zoid/` to `.gitignore`**

Add this line to `.gitignore` (the binary writes its session DB under `./.zoid/`):

```gitignore
# zoid local session data
/.zoid/
```

- [ ] **Step 5: Run the build and test to verify the harness**

Run: `cargo test -p zoid-core`
Expected: compiles; `1 passed` (`workspace_builds_and_tests_run`). (The other three crates don't exist as packages yet — that's fine; this command targets `zoid-core` only.)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/zoid-core .gitignore
git commit -m "feat(core): workspace scaffold + zoid-core skeleton"
```

---

### Task 2: Event model + serde round-trip

Defines the immutable `Event` and its supporting types exactly as spec §5 describes, and proves it serializes/deserializes losslessly (the format the SQLite store will persist).

**Files:**
- Create: `crates/zoid-core/src/event.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod event;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `BranchId(pub String)` — `Default` = `"main"`.
  - `TokenStat { input: u64, output: u64, cached: u64 }` — `Default`, `Copy`.
  - `EventKind` enum: `UserMessage { text: String }`, `AssistantMessage { text: String }`.
  - `Event { id: Ulid, parent: Option<Ulid>, branch: BranchId, ts: i64, kind: EventKind, tokens: Option<TokenStat> }`.
  - `Event::new(id: Ulid, parent: Option<Ulid>, ts: i64, kind: EventKind) -> Event` (sets `branch = default`, `tokens = None`).

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-core/src/event.rs`:

```rust
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_json_round_trips() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let ev = Event::new(id, None, 1_700_000_000, EventKind::UserMessage { text: "hi".into() });
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
        assert_eq!(back.branch, BranchId::default());
        assert_eq!(back.tokens, None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core event_json_round_trips`
Expected: FAIL — compile errors (`Event`, `EventKind`, `BranchId` not defined).

- [ ] **Step 3: Write minimal implementation**

Add above the `#[cfg(test)]` block in `crates/zoid-core/src/event.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchId(pub String);

impl Default for BranchId {
    fn default() -> Self {
        BranchId("main".to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenStat {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: Ulid,
    pub parent: Option<Ulid>,
    pub branch: BranchId,
    pub ts: i64,
    pub kind: EventKind,
    pub tokens: Option<TokenStat>,
}

impl Event {
    pub fn new(id: Ulid, parent: Option<Ulid>, ts: i64, kind: EventKind) -> Self {
        Event { id, parent, branch: BranchId::default(), ts, kind, tokens: None }
    }
}
```

Add to `crates/zoid-core/src/lib.rs` (above the `smoke` module):

```rust
pub mod event;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core event_json_round_trips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): event model with serde round-trip"
```

---

### Task 3: Transcript projection + proptest determinism

The first projection: a pure fold from the event log into ordered conversation turns. Proves determinism with a property test (the highest-value test class per spec §13).

**Files:**
- Create: `crates/zoid-core/src/projection.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod projection;`)

**Interfaces:**
- Consumes: `event::{Event, EventKind}`.
- Produces:
  - `Role` enum: `User`, `Assistant`.
  - `Turn { role: Role, text: String }`.
  - `transcript(events: &[Event]) -> Vec<Turn>` — pure; preserves event order; one `Turn` per event.

- [ ] **Step 1: Write the failing tests**

Create `crates/zoid-core/src/projection.rs`:

```rust
use crate::event::{Event, EventKind};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use proptest::prelude::*;
    use ulid::Ulid;

    fn user(id: u128, text: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::UserMessage { text: text.into() })
    }
    fn asst(id: u128, text: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::AssistantMessage { text: text.into() })
    }

    #[test]
    fn maps_events_to_turns_in_order() {
        let events = vec![user(1, "q"), asst(2, "a")];
        let turns = transcript(&events);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0], Turn { role: Role::User, text: "q".into() });
        assert_eq!(turns[1], Turn { role: Role::Assistant, text: "a".into() });
    }

    proptest! {
        #[test]
        fn transcript_is_deterministic(texts in proptest::collection::vec("[a-z ]{0,12}", 0..20)) {
            let events: Vec<Event> = texts.iter().enumerate()
                .map(|(i, t)| user(i as u128 + 1, t))
                .collect();
            prop_assert_eq!(transcript(&events), transcript(&events));
            prop_assert_eq!(transcript(&events).len(), events.len());
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core projection`
Expected: FAIL — `transcript`, `Turn`, `Role` not defined.

- [ ] **Step 3: Write minimal implementation**

Add above the `#[cfg(test)]` block in `crates/zoid-core/src/projection.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// The Transcript projection: a pure fold over the event log into ordered turns.
pub fn transcript(events: &[Event]) -> Vec<Turn> {
    events
        .iter()
        .map(|e| match &e.kind {
            EventKind::UserMessage { text } => Turn { role: Role::User, text: text.clone() },
            EventKind::AssistantMessage { text } => Turn { role: Role::Assistant, text: text.clone() },
        })
        .collect()
}
```

Add to `crates/zoid-core/src/lib.rs`:

```rust
pub mod projection;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core projection`
Expected: PASS (both `maps_events_to_turns_in_order` and `transcript_is_deterministic`).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/projection.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): transcript projection + determinism property test"
```

---

### Task 4: SQLite event store + persist/replay round-trip

The append-only `rusqlite` store (single-writer: it owns the `Connection`). An integration test proves the headline P0 capability: append events, reopen the DB, load them back identically, and re-fold to the same transcript.

**Files:**
- Create: `crates/zoid-core/src/store.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod store;`)
- Create: `crates/zoid-core/tests/round_trip.rs`

**Interfaces:**
- Consumes: `event::{Event, EventKind, BranchId}`, `projection::transcript`.
- Produces:
  - `EventStore` (owns a `rusqlite::Connection`).
  - `EventStore::open(path: &str) -> anyhow::Result<EventStore>` — creates the `events` table if absent.
  - `EventStore::append(&self, event: &Event) -> anyhow::Result<()>`.
  - `EventStore::load_all(&self) -> anyhow::Result<Vec<Event>>` — ordered by `id` ascending (ULIDs sort chronologically).

- [ ] **Step 1: Write the failing unit test**

Create `crates/zoid-core/src/store.rs`:

```rust
use crate::event::{BranchId, Event};
use anyhow::Result;
use rusqlite::{params, Connection};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use ulid::Ulid;

    #[test]
    fn append_then_load_round_trips_in_order() {
        let store = EventStore::open(":memory:").unwrap();
        let e1 = Event::new(Ulid::from(1u128), None, 10, EventKind::UserMessage { text: "q".into() });
        let e2 = Event::new(Ulid::from(2u128), Some(Ulid::from(1u128)), 20,
            EventKind::AssistantMessage { text: "a".into() });
        store.append(&e1).unwrap();
        store.append(&e2).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded, vec![e1, e2]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core append_then_load_round_trips_in_order`
Expected: FAIL — `EventStore` not defined.

- [ ] **Step 3: Write minimal implementation**

Add above the `#[cfg(test)]` block in `crates/zoid-core/src/store.rs`:

```rust
/// Single-writer, append-only event log backed by SQLite. The store owns the
/// connection; readers obtain owned `Vec<Event>` snapshots via `load_all`.
pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id     TEXT PRIMARY KEY,
                parent TEXT,
                branch TEXT NOT NULL,
                ts     INTEGER NOT NULL,
                kind   TEXT NOT NULL,
                tokens TEXT
            );",
        )?;
        Ok(EventStore { conn })
    }

    pub fn append(&self, event: &Event) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (id, parent, branch, ts, kind, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.id.to_string(),
                event.parent.map(|p| p.to_string()),
                event.branch.0,
                event.ts,
                serde_json::to_string(&event.kind)?,
                event.tokens.map(|t| serde_json::to_string(&t)).transpose()?,
            ],
        )?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, parent, branch, ts, kind, tokens FROM events ORDER BY id ASC",
        )?;
        // The row closure must return rusqlite::Result, so pull raw columns here
        // and do (fallible) serde decoding outside the closure.
        let raw = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;

        let mut out = Vec::new();
        for r in raw {
            let (id, parent, branch, ts, kind, tokens) = r?;
            out.push(Event {
                id: id.parse()?,
                parent: parent.map(|p| p.parse()).transpose()?,
                branch: BranchId(branch),
                ts,
                kind: serde_json::from_str(&kind)?,
                tokens: tokens.map(|t| serde_json::from_str(&t)).transpose()?,
            });
        }
        Ok(out)
    }
}
```

Add to `crates/zoid-core/src/lib.rs`:

```rust
pub mod store;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-core append_then_load_round_trips_in_order`
Expected: PASS.

- [ ] **Step 5: Write the persist/replay integration test**

Create `crates/zoid-core/tests/round_trip.rs`:

```rust
use tempfile::tempdir;
use ulid::Ulid;
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{transcript, Role, Turn};
use zoid_core::store::EventStore;

#[test]
fn session_persists_and_replays_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.db");
    let path = path.to_str().unwrap();

    // First session: append two turns.
    let e1 = Event::new(Ulid::from(1u128), None, 10, EventKind::UserMessage { text: "hello".into() });
    let e2 = Event::new(Ulid::from(2u128), Some(Ulid::from(1u128)), 20,
        EventKind::AssistantMessage { text: "hi there".into() });
    {
        let store = EventStore::open(path).unwrap();
        store.append(&e1).unwrap();
        store.append(&e2).unwrap();
    } // store dropped — connection closed.

    // Second session: reopen, load, fold. Same events, same transcript.
    let store = EventStore::open(path).unwrap();
    let events = store.load_all().unwrap();
    assert_eq!(events, vec![e1, e2]);

    let turns = transcript(&events);
    assert_eq!(
        turns,
        vec![
            Turn { role: Role::User, text: "hello".into() },
            Turn { role: Role::Assistant, text: "hi there".into() },
        ]
    );
}
```

- [ ] **Step 6: Run the integration test**

Run: `cargo test -p zoid-core --test round_trip`
Expected: PASS — `session_persists_and_replays_across_reopen`.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/store.rs crates/zoid-core/src/lib.rs crates/zoid-core/tests/round_trip.rs
git commit -m "feat(core): sqlite event store + persist/replay round-trip"
```

---

### Task 5: Provider seam + deterministic `FakeProvider`

Defines the LLM provider boundary and a scripted fake that replays a fixed list of events. Establishes the deterministic, offline test pattern the P1 agent loop will rely on (spec §13). The seam returns a `Vec` in P0; P1 replaces it with a real SSE stream.

**Files:**
- Create: `crates/zoid-provider/Cargo.toml`
- Create: `crates/zoid-provider/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `ProviderEvent` enum: `TextDelta(String)`, `Done`.
  - `trait Provider { async fn stream(&self, prompt: &str) -> Vec<ProviderEvent>; }` (via `async_trait`).
  - `FakeProvider { scripted: Vec<ProviderEvent> }` with `FakeProvider::new(Vec<ProviderEvent>)`, implementing `Provider`.

- [ ] **Step 1: Write `crates/zoid-provider/Cargo.toml`**

```toml
[package]
name = "zoid-provider"
version = "0.0.0"
edition.workspace = true

[dependencies]
async-trait = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/zoid-provider/src/lib.rs`:

```rust
//! The LLM provider seam. P0 ships only the trait + a deterministic fake;
//! the real streaming Anthropic provider arrives in P1.

use async_trait::async_trait;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_replays_scripted_events_in_order() {
        let script = vec![
            ProviderEvent::TextDelta("hel".into()),
            ProviderEvent::TextDelta("lo".into()),
            ProviderEvent::Done,
        ];
        let provider = FakeProvider::new(script.clone());
        let out = provider.stream("ignored prompt").await;
        assert_eq!(out, script);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-provider`
Expected: FAIL — `ProviderEvent`, `FakeProvider`, `Provider` not defined.

- [ ] **Step 4: Write minimal implementation**

Add above the `#[cfg(test)]` block in `crates/zoid-provider/src/lib.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    TextDelta(String),
    Done,
}

#[async_trait]
pub trait Provider {
    /// Produce the assistant's response to `prompt` as ordered events.
    /// P0 returns the full list; P1 swaps this for a streamed SSE response.
    async fn stream(&self, prompt: &str) -> Vec<ProviderEvent>;
}

pub struct FakeProvider {
    pub scripted: Vec<ProviderEvent>,
}

impl FakeProvider {
    pub fn new(scripted: Vec<ProviderEvent>) -> Self {
        FakeProvider { scripted }
    }
}

#[async_trait]
impl Provider for FakeProvider {
    async fn stream(&self, _prompt: &str) -> Vec<ProviderEvent> {
        self.scripted.clone()
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p zoid-provider`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-provider
git commit -m "feat(provider): provider seam + deterministic FakeProvider"
```

---

### Task 6: `zoid-tui` design-tokens module (single source)

The lone source of glyphs and colors, copied verbatim from the visual-language table in `docs/ux/README.md`. Every render reads from here (spec §16).

**Files:**
- Create: `crates/zoid-tui/Cargo.toml`
- Create: `crates/zoid-tui/src/lib.rs`
- Create: `crates/zoid-tui/src/tokens.rs`

**Interfaces:**
- Consumes: `ratatui::style::Color`.
- Produces:
  - `tokens::glyph::{EDIT, PASS, RUNNING, PENDING, STREAM, BRANCH, BLOCKER, USER_TURN, CARET}: char`.
  - `tokens::color::{CHAT_ACCENT, BUILD_ACCENT, OK, WARN, ERROR, BRANCH, DIM, TXT}: ratatui::style::Color`.

- [ ] **Step 1: Write `crates/zoid-tui/Cargo.toml`**

```toml
[package]
name = "zoid-tui"
version = "0.0.0"
edition.workspace = true

[dependencies]
ratatui = { workspace = true }
zoid-core = { path = "../zoid-core" }

[dev-dependencies]
insta = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/zoid-tui/src/tokens.rs`:

```rust
//! The single source of truth for glyphs and colors (spec §16). Values are
//! copied verbatim from docs/ux/README.md's visual-language table.

use ratatui::style::Color;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_match_visual_language_table() {
        assert_eq!(glyph::BRANCH, '⎇');
        assert_eq!(glyph::USER_TURN, '›');
        assert_eq!(color::CHAT_ACCENT, Color::Rgb(0x58, 0xa6, 0xff));
        assert_eq!(color::OK, Color::Rgb(0x3f, 0xb9, 0x50));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-tui tokens`
Expected: FAIL — `glyph`/`color` modules not defined (and `zoid-tui` has no `lib.rs` yet).

- [ ] **Step 4: Write minimal implementation**

Add above the `#[cfg(test)]` block in `crates/zoid-tui/src/tokens.rs`:

```rust
/// Glyphs (visual-language table, spec §16 / docs/ux/README.md).
pub mod glyph {
    pub const EDIT: char = '●';
    pub const PASS: char = '✓';
    pub const RUNNING: char = '◐';
    pub const PENDING: char = '☐';
    pub const STREAM: char = '⠿';
    pub const BRANCH: char = '⎇';
    pub const BLOCKER: char = '⛔';
    pub const USER_TURN: char = '›';
    pub const CARET: char = '▌';
}

/// Colors (visual-language table, spec §16 / docs/ux/README.md).
pub mod color {
    use ratatui::style::Color;
    pub const CHAT_ACCENT: Color = Color::Rgb(0x58, 0xa6, 0xff);
    pub const BUILD_ACCENT: Color = Color::Rgb(0xe3, 0xb3, 0x41);
    pub const OK: Color = Color::Rgb(0x3f, 0xb9, 0x50);
    pub const WARN: Color = Color::Rgb(0xd2, 0x99, 0x22);
    pub const ERROR: Color = Color::Rgb(0xf8, 0x51, 0x49);
    pub const BRANCH: Color = Color::Rgb(0xbc, 0x8c, 0xff);
    pub const DIM: Color = Color::Rgb(0x6e, 0x76, 0x81);
    pub const TXT: Color = Color::Rgb(0xc9, 0xd1, 0xd9);
}
```

Create `crates/zoid-tui/src/lib.rs`:

```rust
//! zoid-tui — design tokens and ratatui render functions. Every view renders
//! from the `tokens` module (spec §16).

pub mod tokens;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p zoid-tui tokens`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui
git commit -m "feat(tui): design-tokens module (single source of glyphs+colors)"
```

---

### Task 7: Chat empty-frame render + insta snapshot (fidelity pipeline)

The first render function and the first snapshot — proving the `TestBackend` + `insta` fidelity pipeline (spec §16). Renders title bar / conversation / status bar from tokens.

**Files:**
- Create: `crates/zoid-tui/src/chat.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (add `pub mod chat;`)
- Create: `crates/zoid-tui/tests/chat_snapshot.rs`

**Interfaces:**
- Consumes: `tokens::{color, glyph}`, `zoid_core::projection::{Turn, Role}`, `ratatui::Frame`.
- Produces: `chat::render_chat(frame: &mut ratatui::Frame, turns: &[Turn])` — draws a 3-row layout (title, conversation, status); empty conversation shows a dim placeholder.

- [ ] **Step 1: Write the render function**

Create `crates/zoid-tui/src/chat.rs`:

```rust
use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use zoid_core::projection::{Role, Turn};

/// Render the Chat surface: title bar, conversation column, status bar.
pub fn render_chat(frame: &mut Frame, turns: &[Turn]) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // conversation
        Constraint::Length(1), // status bar
    ])
    .split(frame.area());

    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled("CHAT ", Style::new().fg(color::CHAT_ACCENT).bold()),
        Span::styled(format!("{} main", glyph::BRANCH), Style::new().fg(color::BRANCH)),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    let body: Vec<Line> = if turns.is_empty() {
        vec![Line::styled("  (no messages yet)", Style::new().fg(color::DIM))]
    } else {
        turns
            .iter()
            .map(|t| {
                let (prefix, accent) = match t.role {
                    Role::User => (format!("{} ", glyph::USER_TURN), color::CHAT_ACCENT),
                    Role::Assistant => ("zoid ".to_string(), color::DIM),
                };
                Line::from(vec![
                    Span::styled(prefix, Style::new().fg(accent)),
                    Span::styled(t.text.clone(), Style::new().fg(color::TXT)),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(body), chunks[1]);

    let status = Line::from(vec![
        Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(
            format!("· {}Tab Build · q quit", "\u{21e7}"), // ⇧Tab
            Style::new().fg(color::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[2]);
}
```

Add to `crates/zoid-tui/src/lib.rs`:

```rust
pub mod chat;
```

- [ ] **Step 2: Write the snapshot test**

Create `crates/zoid-tui/tests/chat_snapshot.rs`:

```rust
use ratatui::{backend::TestBackend, Terminal};
use zoid_tui::chat::render_chat;

#[test]
fn empty_chat_frame() {
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &[])).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}
```

- [ ] **Step 3: Run the test — it fails because there's no accepted snapshot yet**

Run: `cargo test -p zoid-tui --test chat_snapshot`
Expected: FAIL — insta reports a new snapshot (`empty_chat_frame.snap.new`) and the test fails (no accepted baseline). This is the expected first-run behavior.

- [ ] **Step 4: Review and accept the snapshot, then verify it passes**

Inspect the pending snapshot, then accept it:

```bash
cargo insta accept
```

(If `cargo insta` isn't installed: `cargo install cargo-insta`. Alternatively review the `.snap.new` file by hand and rename it to `.snap`.)

Then re-run:

Run: `cargo test -p zoid-tui --test chat_snapshot`
Expected: PASS — the buffer now matches `crates/zoid-tui/tests/snapshots/chat_snapshot__empty_chat_frame.snap`. The title bar shows `zoid CHAT ⎇ main`, the body shows `(no messages yet)`, the status bar shows `CHAT · ⇧Tab Build · q quit`.

- [ ] **Step 5: Commit (including the accepted snapshot)**

```bash
git add crates/zoid-tui/src/chat.rs crates/zoid-tui/src/lib.rs crates/zoid-tui/tests/chat_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): chat empty-frame render + insta snapshot harness"
```

---

### Task 8: Render a seeded transcript + snapshot (replay → render fidelity)

Proves the full read path — events → `transcript` → rendered frame — with a second snapshot over seeded turns. This is the visual half of "persists & replays a session."

**Files:**
- Modify: `crates/zoid-tui/tests/chat_snapshot.rs` (add a second test)

**Interfaces:**
- Consumes: `zoid_core::projection::{Turn, Role}`, `chat::render_chat`.
- Produces: a snapshot baseline for a two-turn conversation.

- [ ] **Step 1: Add the seeded-transcript snapshot test**

Append to `crates/zoid-tui/tests/chat_snapshot.rs`:

```rust
use zoid_core::projection::{Role, Turn};

#[test]
fn seeded_transcript_frame() {
    let turns = vec![
        Turn { role: Role::User, text: "what's causing the 500?".into() },
        Turn { role: Role::Assistant, text: "an unwrapped lookup in the handler.".into() },
    ];
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &turns)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}
```

- [ ] **Step 2: Run the test — fails on the new (unaccepted) snapshot**

Run: `cargo test -p zoid-tui --test chat_snapshot seeded_transcript_frame`
Expected: FAIL — new snapshot `seeded_transcript_frame.snap.new` generated.

- [ ] **Step 3: Accept the snapshot and verify it passes**

```bash
cargo insta accept
```

Run: `cargo test -p zoid-tui --test chat_snapshot`
Expected: PASS — both `empty_chat_frame` and `seeded_transcript_frame`. The seeded frame shows `› what's causing the 500?` and `zoid an unwrapped lookup in the handler.`.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/tests/chat_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "test(tui): seeded-transcript snapshot (replay->render fidelity)"
```

---

### Task 9: `zoid` binary — boot → load → fold → render → quit

Wires the spine to the screen: open the store at the data path, load + fold the log, render the Chat frame, and quit on `q` / `Ctrl-C`. Synchronous event loop (tokio arrives in P1). Closes out the P0 end-state.

**Files:**
- Create: `crates/zoid/Cargo.toml`
- Create: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `zoid_core::store::EventStore`, `zoid_core::projection::{transcript, Turn}`, `zoid_tui::chat::render_chat`, `crossterm`, `ratatui`.
- Produces: the `zoid` binary.

- [ ] **Step 1: Write `crates/zoid/Cargo.toml`**

```toml
[package]
name = "zoid"
version = "0.0.0"
edition.workspace = true

[[bin]]
name = "zoid"
path = "src/main.rs"

[dependencies]
zoid-core = { path = "../zoid-core" }
zoid-tui = { path = "../zoid-tui" }
anyhow = { workspace = true }
ratatui = { workspace = true }
crossterm = { workspace = true }
```

- [ ] **Step 2: Write `crates/zoid/src/main.rs`**

```rust
use anyhow::Result;
use crossterm::{
    event::{self, Event as CEvent, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::Backend, prelude::CrosstermBackend, Terminal};
use std::io::stdout;
use std::path::Path;
use zoid_core::projection::{transcript, Turn};
use zoid_core::store::EventStore;
use zoid_tui::chat::render_chat;

/// Resolve the session DB path: `$ZOID_DB` if set, else `./.zoid/session.db`.
fn db_path() -> String {
    if let Ok(p) = std::env::var("ZOID_DB") {
        return p;
    }
    let dir = Path::new(".zoid");
    let _ = std::fs::create_dir_all(dir);
    dir.join("session.db").to_string_lossy().into_owned()
}

fn main() -> Result<()> {
    // Boot: open the log, replay it into the current transcript.
    let store = EventStore::open(&db_path())?;
    let turns = transcript(&store.load_all()?);

    // Enter the TUI.
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, &turns);

    // Restore the terminal regardless of how `run` ended.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run<B: Backend>(terminal: &mut Terminal<B>, turns: &[Turn]) -> Result<()> {
    loop {
        terminal.draw(|f| render_chat(f, turns))?;
        if let CEvent::Key(key) = event::read()? {
            let quit = key.code == KeyCode::Char('q')
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
            if quit {
                return Ok(());
            }
        }
    }
}
```

- [ ] **Step 3: Verify the whole workspace builds**

Run: `cargo build`
Expected: all four crates compile; produces `target/debug/zoid`.

- [ ] **Step 4: Verify the full test suite is green**

Run: `cargo test`
Expected: PASS across `zoid-core` (event/projection/store + round_trip), `zoid-provider` (fake), and `zoid-tui` (tokens + both snapshots).

- [ ] **Step 5: Manual smoke — boot, render, persist/replay, quit**

Seed a session, launch, confirm replay, quit:

```bash
# Seed two turns into a throwaway DB via the round-trip test path is automatic;
# for a manual check, run the binary against a temp DB:
ZOID_DB=/tmp/zoid-smoke.db cargo run
# Expect: alt-screen TUI showing "zoid CHAT ⎇ main" and "(no messages yet)".
# Press q to quit; terminal restores cleanly.
# Run it again — it reopens the same DB without error (empty replay).
rm -f /tmp/zoid-smoke.db
```

Expected: launches to the empty Chat frame, `q` exits cleanly with the terminal restored (no raw-mode residue), and re-launch succeeds (proving open→load→render is replay-safe).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid
git commit -m "feat(zoid): boot -> load -> fold -> render -> quit binary"
```

---

## P0 Definition of Done

- `cargo build` produces a `zoid` binary; `cargo test` is fully green.
- `zoid` boots to an empty Chat frame and quits cleanly (`q` / `Ctrl-C`), terminal restored.
- The event log persists to SQLite and **replays identically across reopen** (`round_trip` integration test).
- The event-sourced core is covered by `proptest` (transcript determinism) and the store round-trip.
- The **fidelity pipeline** is live: two committed `insta` snapshots (empty + seeded Chat frame) rendered via `TestBackend` from the `tokens` single source.
- `wasmtime`/`tree-sitter`/runtime-`tokio` are absent from the build; `rusqlite` is `bundled`.

## Self-Review (performed against spec §12 P0 + §4/§5/§16)

- **Spec coverage:** workspace ✓ (Task 1); design-tokens module ✓ (Task 6, spec §16); event log rusqlite append-only ✓ (Task 4); fold engine + Transcript projection ✓ (Task 3); fake provider ✓ (Task 5); bare ratatui shell boot/render/quit ✓ (Task 9); proptest harness ✓ (Task 3); TestBackend/insta harness ✓ (Tasks 7–8); persists & replays ✓ (Task 4 round_trip + Task 9 smoke).
- **Schema-as-vision (spec §5):** `parent`/`branch`/`tokens` retained on `Event` though unused by P0 behavior (Task 2).
- **Injected clock (spec §5):** `ts` is a parameter to `Event::new`; no ambient clock in `zoid-core`.
- **Single-binary constraint (spec §3):** `rusqlite` `bundled`; no `wasmtime`/`tree-sitter`; release profile set in Task 1.
- **Type consistency:** `Event`/`EventKind`/`BranchId`/`TokenStat` (Task 2) used unchanged in store (Task 4) and projection (Task 3); `Turn`/`Role` (Task 3) used unchanged in `render_chat` (Task 7) and tests (Task 8); `render_chat(&mut Frame, &[Turn])` signature identical across Tasks 7–9; `EventStore::{open,append,load_all}` signatures identical across Tasks 4 and 9; `ProviderEvent`/`Provider`/`FakeProvider` self-contained (Task 5).
- **Placeholder scan:** none — every code step contains complete, compilable code; every run step states the exact command and expected outcome.
- **Deferred-correctly:** input handling, streaming, the agent loop, real Anthropic provider, economy, modes beyond an empty Chat frame are all out of P0 scope (P1+).
