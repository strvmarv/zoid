# P1a — Async Chat Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the P0 spine into a real **streaming chatbot in the TUI** — talk to Claude, watch tokens stream in live, multi-line input — with no tools yet.

**Architecture:** Introduces the spec's async substrate (§3, §4.1): the synchronous P0 loop becomes a `tokio` event loop using `crossterm`'s `EventStream` + `tokio::select!`; the SQLite store moves behind a **single-writer actor** (a dedicated OS thread owning the `Connection`, reached over a `tokio::sync::mpsc` channel) so blocking SQLite never blocks the runtime. The `Provider` trait is redesigned to **stream** `ProviderEvent`s into an `mpsc` sink; a real `AnthropicProvider` (reqwest + SSE) lands behind it, with the deterministic `FakeProvider` kept as the offline test vehicle. Partial model output is persisted incrementally as `ModelDelta` events (§11), and the `Transcript` projection folds delta runs into assistant turns.

**Tech Stack:** Rust 2021 · `tokio` (rt-multi-thread, macros, sync, time) · `crossterm` (event-stream) · `reqwest` (rustls, json, stream) + `eventsource-stream` + `futures-util` · `tui-textarea` · `ratatui` · `rusqlite` (bundled) · `serde`/`serde_json` · `async-trait` · test: `proptest`, `insta`, `tempfile`.

**Builds on (merged P0, `main`):**
- `zoid_core::event` — `Event { id: Ulid, parent: Option<Ulid>, branch: BranchId, ts: i64, kind: EventKind, tokens: Option<TokenStat> }`; `EventKind::{UserMessage{text}, AssistantMessage{text}}`; `Event::new(id, parent, ts, kind)`; `BranchId(pub String)` (Default `"main"`); `TokenStat{input,output,cached:u64}`.
- `zoid_core::projection` — `Role::{User,Assistant}`, `Turn{role,text}`, `transcript(&[Event]) -> Vec<Turn>`.
- `zoid_core::store` — `EventStore::{open(&str), append(&Event), load_all() -> Vec<Event>}` (sync, owns `Connection`).
- `zoid_provider` — `ProviderEvent::{TextDelta(String), Done}`, `Provider::stream(&self,&str)->Vec<ProviderEvent>` (async_trait), `FakeProvider`. **This crate is rewritten in this plan.**
- `zoid_tui` — `tokens::{glyph,color}` (single source), `chat::render_chat(&mut Frame, &[Turn])`.
- `zoid` bin — synchronous `main` (boot→load→fold→render→quit on `q`/`Ctrl-C`).

## Global Constraints

- **Edition 2021**; the workspace already pins `edition = "2021"` in `[workspace.package]`.
- **No co-author / "Generated with" trailers** on any commit (user's `~/CLAUDE.md`).
- **Single static binary stays the goal:** all new transitive native deps must build statically. Use **`rustls`**, never OpenSSL/native-tls — add `reqwest` with `default-features = false, features = ["json","stream","rustls-tls"]`. No `wasmtime`/`tree-sitter` in the build (still deferred).
- **`rusqlite` keeps the `bundled` feature** (no system SQLite).
- **`zoid-core` stays clock-free and pure:** `ts` is always an injected `i64` parameter; no `SystemTime`/`Instant`/`Ulid::new()` inside `zoid-core`. Real time and ULID generation happen only in the `zoid` binary.
- **Append-only:** never UPDATE/DELETE events; never mutate an already-persisted `Event`. Token accounting persisted on `Event.tokens` stays **deferred to P3** — in P1a `tokens` remains `None` (the schema field is retained, behavior phased).
- **Design tokens are the single source** for every glyph/color (spec §16); render code reads from `zoid_tui::tokens`, never hardcoded literals (except pure layout strings).
- **Determinism for tests:** all automated tests use `FakeProvider` (offline, scripted). The real `AnthropicProvider` is **not** unit-tested (needs network + key); it is covered by build + the pure request/SSE-parser unit tests.
- **Release profile** (already set in `[profile.release]`: `opt-level="z"`, `lto=true`, `strip=true`, `panic="abort"`, `codegen-units=1`) must remain intact.
- **Every new screen state** ships an `insta` snapshot rendered via `ratatui::TestBackend`, bound to the `docs/ux/` visual language (§16).
- **TDD throughout:** write the failing test → run-fails → minimal impl → run-passes → commit. Tasks whose deliverable is inherently non-unit-testable (the real provider's network call; the interactive loop) state that explicitly and rely on build + the pure tests they wrap.

---

### Task 1: Workspace dependencies + spikes exclusion

Adds every new dependency P1a needs to `[workspace.dependencies]`, turns on the `crossterm` `event-stream` feature and the broader `tokio` feature set, and explicitly excludes the `spikes/` directory from the workspace (it declares banned deps — `wasmtime`/`tree-sitter` — and is only safe today because Cargo auto-excludes non-members; make it explicit, per the P0 final review).

No crate consumes the new deps yet, so this task's only verification is that the workspace still builds. (Adding entries to `[workspace.dependencies]` does **not** cause unused-dependency warnings; only a crate's own `[dependencies]` referencing an unused crate would.)

**Files:**
- Modify: `Cargo.toml` (workspace root)

**Interfaces:**
- Consumes: nothing.
- Produces: workspace dependency entries `tokio` (expanded features), `crossterm` (event-stream), `reqwest`, `eventsource-stream`, `futures-util`, `tui-textarea`, available to later tasks via `{ workspace = true }`.

- [ ] **Step 1: Update the workspace manifest**

Edit `Cargo.toml` so the `[workspace]`, `[workspace.dependencies]` sections read exactly as below (the `[workspace.package]` and `[profile.release]` sections are unchanged; leave them as-is). The two changes vs P0 are: the `[workspace.exclude]` line, the expanded `tokio` features, the `event-stream` feature on `crossterm`, and the four new dependencies.

```toml
[workspace]
members = ["crates/zoid-core", "crates/zoid-provider", "crates/zoid-tui", "crates/zoid"]
exclude = ["spikes"]
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
crossterm = { version = "0.28", features = ["event-stream"] }
async-trait = "0.1"
tokio = { version = "1", features = ["macros", "rt", "rt-multi-thread", "sync", "time"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "rustls-tls"] }
eventsource-stream = "0.2"
futures-util = "0.3"
tui-textarea = "0.7"
proptest = "1"
insta = "1"
tempfile = "3"
```

- [ ] **Step 2: Verify the workspace still builds**

Run: `cargo build`
Expected: PASS — all 4 crates compile unchanged; the new workspace deps are downloaded/resolved but not yet referenced. No new warnings.

- [ ] **Step 3: Verify the existing suite is still green**

Run: `cargo test`
Expected: PASS — 11 tests, 0 failed (unchanged from P0).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml
git commit -m "build: P1a workspace deps (tokio/reqwest/sse/tui-textarea) + exclude spikes"
```

---

### Task 2: `ModelDelta` event variant

Adds the streaming-output event kind. Per spec §5/§11, partial model output is persisted incrementally as `ModelDelta` events so a dropped connection leaves a resumable log.

**Files:**
- Modify: `crates/zoid-core/src/event.rs`

**Interfaces:**
- Consumes: existing `EventKind`.
- Produces: `EventKind::ModelDelta { text: String }` (a new variant alongside `UserMessage`/`AssistantMessage`).

- [ ] **Step 1: Write the failing test**

Add this test to the existing `#[cfg(test)] mod tests` block in `crates/zoid-core/src/event.rs` (keep the existing `event_json_round_trips` and `event_new_defaults` tests):

```rust
    #[test]
    fn model_delta_round_trips() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let ev = Event::new(id, None, 42, EventKind::ModelDelta { text: "tok".into() });
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
        assert!(matches!(back.kind, EventKind::ModelDelta { .. }));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-core model_delta_round_trips`
Expected: FAIL — `no variant named ModelDelta`.

- [ ] **Step 3: Add the variant**

In `crates/zoid-core/src/event.rs`, extend the `EventKind` enum (add the third variant; leave the derives and the other variants unchanged):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
    ModelDelta { text: String },
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core`
Expected: PASS — all `zoid-core` tests including `model_delta_round_trips`. (The `transcript` projection still compiles because Task 3 has not yet made its match exhaustive over `ModelDelta` — **wait**: adding a variant makes the existing non-exhaustive `match` in `projection.rs` a compile error. That is expected and is fixed in Task 3. If `cargo test -p zoid-core` fails to **compile** `projection.rs` here, that is the signal to proceed to Task 3; the `event.rs` unit test itself is correct.)

> Implementer note: because `projection::transcript` matches `EventKind` exhaustively, this task and Task 3 are a tightly-coupled pair — adding the variant breaks the projection's `match`. To keep each commit compiling, **fold the minimal projection fix into this task if needed**: if `cargo test -p zoid-core` does not compile after Step 3, add a temporary `EventKind::ModelDelta { .. } => continue,` arm (skip deltas) to `transcript` so the crate compiles and `model_delta_round_trips` passes, then Task 3 replaces that arm with the real fold. Confirm `cargo test -p zoid-core` is green before committing.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/projection.rs
git commit -m "feat(core): add ModelDelta event variant for streamed output"
```

---

### Task 3: Fold `ModelDelta` runs in the `Transcript` projection

Teaches `transcript` to collapse a run of consecutive `ModelDelta` events into a single assistant `Turn` (concatenated text), while `AssistantMessage` and `UserMessage` still map to one turn each. This is the read-side of incremental streaming: the UI re-folds the growing log each frame and sees one assistant turn growing token by token.

**Files:**
- Modify: `crates/zoid-core/src/projection.rs`

**Interfaces:**
- Consumes: `event::{Event, EventKind}`.
- Produces: updated `transcript(&[Event]) -> Vec<Turn>` (same signature) with `ModelDelta`-run folding.

- [ ] **Step 1: Write the failing tests**

Add these tests to the existing `#[cfg(test)] mod tests` block in `crates/zoid-core/src/projection.rs` (keep `maps_events_to_turns_in_order` and `transcript_is_deterministic`). Add a `delta` helper next to the existing `user`/`asst` helpers:

```rust
    fn delta(id: u128, text: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::ModelDelta { text: text.into() })
    }

    #[test]
    fn consecutive_deltas_fold_into_one_assistant_turn() {
        let events = vec![user(1, "hi"), delta(2, "he"), delta(3, "ll"), delta(4, "o")];
        let turns = transcript(&events);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0], Turn { role: Role::User, text: "hi".into() });
        assert_eq!(turns[1], Turn { role: Role::Assistant, text: "hello".into() });
    }

    #[test]
    fn delta_run_ends_at_next_user_message() {
        let events = vec![user(1, "a"), delta(2, "x"), delta(3, "y"), user(4, "b"), delta(5, "z")];
        let turns = transcript(&events);
        assert_eq!(turns, vec![
            Turn { role: Role::User, text: "a".into() },
            Turn { role: Role::Assistant, text: "xy".into() },
            Turn { role: Role::User, text: "b".into() },
            Turn { role: Role::Assistant, text: "z".into() },
        ]);
    }

    #[test]
    fn assistant_message_and_delta_run_are_separate_turns() {
        let events = vec![asst(1, "full"), delta(2, "d1"), delta(3, "d2")];
        let turns = transcript(&events);
        assert_eq!(turns, vec![
            Turn { role: Role::Assistant, text: "full".into() },
            Turn { role: Role::Assistant, text: "d1d2".into() },
        ]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core consecutive_deltas_fold_into_one_assistant_turn`
Expected: FAIL — either a compile error from the temporary skip-arm added in Task 2, or an assertion failure (deltas dropped, only 1 turn).

- [ ] **Step 3: Rewrite `transcript`**

Replace the `transcript` function in `crates/zoid-core/src/projection.rs` with this (the `Role`/`Turn` types above it are unchanged):

```rust
/// The Transcript projection: a pure fold over the event log into ordered turns.
/// A run of consecutive `ModelDelta` events collapses into a single assistant
/// `Turn` (concatenated text); `UserMessage`/`AssistantMessage` each map to one
/// turn. Pure: no I/O, no clock.
pub fn transcript(events: &[Event]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut pending: Option<String> = None;

    fn flush(pending: &mut Option<String>, turns: &mut Vec<Turn>) {
        if let Some(text) = pending.take() {
            turns.push(Turn { role: Role::Assistant, text });
        }
    }

    for e in events {
        match &e.kind {
            EventKind::UserMessage { text } => {
                flush(&mut pending, &mut turns);
                turns.push(Turn { role: Role::User, text: text.clone() });
            }
            EventKind::AssistantMessage { text } => {
                flush(&mut pending, &mut turns);
                turns.push(Turn { role: Role::Assistant, text: text.clone() });
            }
            EventKind::ModelDelta { text } => {
                pending.get_or_insert_with(String::new).push_str(text);
            }
        }
    }
    flush(&mut pending, &mut turns);
    turns
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core`
Expected: PASS — all projection tests (old + 3 new) and the rest of `zoid-core`.

- [ ] **Step 5: Add a determinism property test for delta folding**

Add to the same `proptest! { ... }` block in `projection.rs` (alongside `transcript_is_deterministic`):

```rust
        #[test]
        fn delta_fold_is_deterministic(frags in proptest::collection::vec("[a-z]{0,6}", 0..30)) {
            let events: Vec<Event> = frags.iter().enumerate()
                .map(|(i, t)| delta(i as u128 + 1, t))
                .collect();
            let once = transcript(&events);
            prop_assert_eq!(&once, &transcript(&events));
            // A non-empty delta run folds to exactly one assistant turn.
            if !events.is_empty() {
                prop_assert_eq!(once.len(), 1);
                prop_assert_eq!(&once[0].text, &frags.concat());
            }
        }
```

- [ ] **Step 6: Run the property test**

Run: `cargo test -p zoid-core delta_fold_is_deterministic`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/projection.rs
git commit -m "feat(core): fold ModelDelta runs into one assistant turn in transcript"
```

---

### Task 4: Single-writer session store actor (`SessionHandle`)

Wraps the synchronous `EventStore` in the spec's actor (§3, §4.1): a dedicated OS thread owns the `Connection` and serializes all access; async callers reach it over a `tokio::sync::mpsc` channel and await replies via `oneshot`. This is what lets the async UI loop persist events without blocking the runtime on SQLite.

**Files:**
- Create: `crates/zoid-core/src/session.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod session;`)
- Modify: `crates/zoid-core/Cargo.toml` (add `tokio` dep + dev-dep)

**Interfaces:**
- Consumes: `event::Event`, `store::EventStore`.
- Produces:
  - `SessionHandle` (`Clone`) — a handle to the store actor.
  - `SessionHandle::spawn(path: &str) -> anyhow::Result<SessionHandle>` — opens the store and spawns the writer thread.
  - `async fn SessionHandle::append(&self, event: Event) -> anyhow::Result<()>`.
  - `async fn SessionHandle::snapshot(&self) -> anyhow::Result<Vec<Event>>` — an ordered immutable copy of the log.

- [ ] **Step 1: Add tokio to `zoid-core`'s manifest**

Edit `crates/zoid-core/Cargo.toml` to add `tokio` as a dependency (the `sync` feature is what the actor needs; the workspace entry already includes it) and a dev-dependency for `#[tokio::test]`:

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
tokio = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/zoid-core/src/session.rs` with only the test module first:

```rust
use crate::event::Event;
use crate::store::EventStore;
use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use ulid::Ulid;

    #[tokio::test]
    async fn appends_then_snapshots_in_order() {
        let handle = SessionHandle::spawn(":memory:").unwrap();
        let e1 = Event::new(Ulid::from(1u128), None, 10, EventKind::UserMessage { text: "q".into() });
        let e2 = Event::new(Ulid::from(2u128), Some(Ulid::from(1u128)), 20,
            EventKind::ModelDelta { text: "a".into() });
        handle.append(e1.clone()).await.unwrap();
        handle.append(e2.clone()).await.unwrap();

        let snap = handle.snapshot().await.unwrap();
        assert_eq!(snap, vec![e1, e2]);
    }

    #[tokio::test]
    async fn clone_shares_the_same_actor() {
        let handle = SessionHandle::spawn(":memory:").unwrap();
        let e1 = Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "x".into() });
        handle.clone().append(e1.clone()).await.unwrap();
        let snap = handle.snapshot().await.unwrap();
        assert_eq!(snap, vec![e1]);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p zoid-core appends_then_snapshots_in_order`
Expected: FAIL — `SessionHandle` not found (and the test module won't compile).

- [ ] **Step 4: Implement the actor**

Add this above the `#[cfg(test)]` block in `crates/zoid-core/src/session.rs`:

```rust
/// Commands accepted by the single-writer store actor.
enum Cmd {
    Append { event: Box<Event>, reply: oneshot::Sender<Result<()>> },
    Snapshot { reply: oneshot::Sender<Vec<Event>> },
}

/// A cloneable handle to the single-writer event-store actor (spec §4.1).
///
/// The store's `rusqlite::Connection` is blocking, so it lives on a dedicated
/// OS thread that serializes every append/read. Async callers send commands
/// over an `mpsc` channel and await the reply via `oneshot`, so SQLite work
/// never blocks the tokio runtime.
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<Cmd>,
}

impl SessionHandle {
    /// Open the store at `path` and spawn its writer thread. Errors if the
    /// store cannot be opened (the open happens on the caller so the error
    /// surfaces synchronously).
    pub fn spawn(path: &str) -> Result<SessionHandle> {
        let store = EventStore::open(path)?;
        let (tx, mut rx) = mpsc::channel::<Cmd>(64);
        std::thread::spawn(move || {
            // `blocking_recv` is valid here: this thread is not a tokio worker.
            while let Some(cmd) = rx.blocking_recv() {
                match cmd {
                    Cmd::Append { event, reply } => {
                        let _ = reply.send(store.append(&event));
                    }
                    Cmd::Snapshot { reply } => {
                        let snap = store.load_all().unwrap_or_default();
                        let _ = reply.send(snap);
                    }
                }
            }
        });
        Ok(SessionHandle { tx })
    }

    /// Durably append one event. Awaits the actor's confirmation.
    pub async fn append(&self, event: Event) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Append { event: Box::new(event), reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// An ordered, immutable snapshot of the full log.
    pub async fn snapshot(&self) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Snapshot { reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))
    }
}
```

- [ ] **Step 5: Register the module**

In `crates/zoid-core/src/lib.rs`, add `pub mod session;` alongside the existing module declarations (keep `event`, `projection`, `store`, and the `smoke` block):

```rust
pub mod event;
pub mod projection;
pub mod session;
pub mod store;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p zoid-core`
Expected: PASS — both new `session` tests plus all prior `zoid-core` tests. (`:memory:` persists across the two calls because the actor keeps one long-lived connection.)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/session.rs crates/zoid-core/src/lib.rs crates/zoid-core/Cargo.toml
git commit -m "feat(core): single-writer session store actor (SessionHandle)"
```

---

### Task 5: Redesign the `Provider` seam for streaming + the streaming `FakeProvider`

Rewrites `zoid-provider` from P0's "return a `Vec`" shape into a real streaming seam: the provider pushes `ProviderEvent`s into an `mpsc` sink as they arrive, and takes a structured `CompletionRequest` (conversation + model). Adds the `Usage`/`Error` variants the P0 final review flagged. The `FakeProvider` becomes the deterministic, offline test vehicle for the whole agent loop.

**Files:**
- Modify: `crates/zoid-provider/Cargo.toml`
- Modify: `crates/zoid-provider/src/lib.rs` (full rewrite of the seam; the `anthropic` submodule is added in Tasks 6–8)

**Interfaces:**
- Consumes: nothing from sibling crates (the provider seam stays self-contained — its own `MsgRole`/`Usage`, no `zoid-core` dependency — so the future plugin/provider surface is decoupled).
- Produces:
  - `MsgRole::{User, Assistant}` (`Copy`).
  - `Message { role: MsgRole, content: String }`.
  - `Usage { input_tokens: u64, output_tokens: u64 }` (`Copy, Default`).
  - `ProviderEvent::{TextDelta(String), Usage(Usage), Done, Error(String)}`.
  - `CompletionRequest { model: String, system: Option<String>, messages: Vec<Message>, max_tokens: u32 }`.
  - `trait Provider: Send + Sync { async fn stream(&self, req: &CompletionRequest, sink: tokio::sync::mpsc::Sender<ProviderEvent>) -> anyhow::Result<()> }`.
  - `FakeProvider { scripted: Vec<ProviderEvent> }` + `FakeProvider::new(Vec<ProviderEvent>)`.

- [ ] **Step 1: Update the provider manifest**

Edit `crates/zoid-provider/Cargo.toml`:

```toml
[package]
name = "zoid-provider"
version = "0.0.0"
edition.workspace = true

[dependencies]
async-trait = { workspace = true }
anyhow = { workspace = true }
tokio = { workspace = true }
serde_json = { workspace = true }
reqwest = { workspace = true }
eventsource-stream = { workspace = true }
futures-util = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

> Note: `reqwest`, `eventsource-stream`, and `futures-util` are unused until Tasks 6–8 add the `anthropic` module. To avoid an `unused-crate-dependencies` lint in the interim, this task's `lib.rs` (Step 2) declares `pub mod anthropic;` and Task 6 creates the file; if you implement strictly task-by-task and the module file does not yet exist, temporarily comment the `pub mod anthropic;` line and the three deps until Task 6 — but the cleaner path is to do Tasks 5→8 before running a full `cargo build`, committing each. Either way each task's own `cargo test -p zoid-provider` must pass.

- [ ] **Step 2: Write the failing test**

Replace the entire contents of `crates/zoid-provider/src/lib.rs` with the seam + a test (the `anthropic` module referenced here is created in Tasks 6–8):

```rust
//! The LLM provider seam: a streaming, tool-agnostic interface plus a
//! deterministic `FakeProvider` for offline tests. The real `AnthropicProvider`
//! lives in the `anthropic` submodule. The seam is intentionally self-contained
//! (no dependency on `zoid-core`) so the provider/plugin surface stays decoupled.

pub mod anthropic;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: MsgRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    TextDelta(String),
    Usage(Usage),
    Done,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stream the assistant's response to `req` by sending ordered
    /// `ProviderEvent`s into `sink`. Returns when the stream ends (the sink is
    /// dropped on return). Transport errors are reported as a final
    /// `ProviderEvent::Error` rather than an `Err` where possible.
    async fn stream(&self, req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()>;
}

/// A deterministic, offline provider that replays a scripted event list.
pub struct FakeProvider {
    pub scripted: Vec<ProviderEvent>,
}

impl FakeProvider {
    pub fn new(scripted: Vec<ProviderEvent>) -> Self {
        Self { scripted }
    }
}

#[async_trait]
impl Provider for FakeProvider {
    async fn stream(&self, _req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        for ev in &self.scripted {
            if sink.send(ev.clone()).await.is_err() {
                break; // receiver gone
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_streams_scripted_events_in_order() {
        let script = vec![
            ProviderEvent::TextDelta("hel".into()),
            ProviderEvent::TextDelta("lo".into()),
            ProviderEvent::Usage(Usage { input_tokens: 3, output_tokens: 2 }),
            ProviderEvent::Done,
        ];
        let provider = FakeProvider::new(script.clone());
        let req = CompletionRequest {
            model: "fake".into(),
            system: None,
            messages: vec![Message { role: MsgRole::User, content: "hi".into() }],
            max_tokens: 64,
        };
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&req, tx).await.unwrap();

        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert_eq!(got, script);
    }
}
```

- [ ] **Step 3: Create a minimal `anthropic` module stub so the crate compiles**

So this task compiles independently, create `crates/zoid-provider/src/anthropic.rs` with the request builder's eventual home left empty for now — but to keep `reqwest`/`eventsource-stream`/`futures-util` referenced (avoiding the unused-dep lint) **defer the real content to Tasks 6–8** and write a temporary stub:

```rust
//! The real streaming Anthropic provider (reqwest + SSE). Built up across
//! Tasks 6–8: request body (Task 6), SSE parsing (Task 7), the provider +
//! selection (Task 8).
```

> If `cargo build` reports unused `reqwest`/`eventsource-stream`/`futures-util` at this point, that is expected and resolved by Task 8; it is a warning, not an error, and does not block `cargo test -p zoid-provider`. Do **not** add `#![allow(unused_crate_dependencies)]`. Proceed through Task 8 promptly; the final whole-branch review requires a warning-free build.

- [ ] **Step 4: Run test to verify it fails, then passes**

Run: `cargo test -p zoid-provider fake_streams_scripted_events_in_order`
Expected: FIRST FAIL if you ran it before Step 2's code compiled (old `Vec`-shaped API); after Step 2+3 it PASSES.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/Cargo.toml crates/zoid-provider/src/lib.rs crates/zoid-provider/src/anthropic.rs
git commit -m "feat(provider): streaming Provider seam + streaming FakeProvider"
```

---

### Task 6: Anthropic request body builder (pure)

The pure half of the real provider: turn a `CompletionRequest` into the exact JSON body the Anthropic Messages API expects (`stream: true`). Fully unit-testable, no network.

**Files:**
- Modify: `crates/zoid-provider/src/anthropic.rs`

**Interfaces:**
- Consumes: `crate::{CompletionRequest, Message, MsgRole}`.
- Produces: `pub fn request_body(req: &CompletionRequest) -> serde_json::Value`; `pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6"`.

- [ ] **Step 1: Write the failing test**

Replace the stub contents of `crates/zoid-provider/src/anthropic.rs` with the doc comment + a test module:

```rust
//! The real streaming Anthropic provider (reqwest + SSE).
//! Task 6: request body. Task 7: SSE parsing. Task 8: the provider + selection.

use crate::{CompletionRequest, Message, MsgRole};

/// Default model when `$ZOID_MODEL` is unset (latest Claude Sonnet).
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_messages_body_with_stream_flag() {
        let req = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![
                Message { role: MsgRole::User, content: "hi".into() },
                Message { role: MsgRole::Assistant, content: "hello".into() },
            ],
            max_tokens: 1024,
        };
        let body = request_body(&req);
        assert_eq!(body, json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 1024,
            "stream": true,
            "messages": [
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ],
        }));
    }

    #[test]
    fn includes_system_when_present() {
        let req = CompletionRequest {
            model: "m".into(),
            system: Some("be terse".into()),
            messages: vec![Message { role: MsgRole::User, content: "x".into() }],
            max_tokens: 8,
        };
        let body = request_body(&req);
        assert_eq!(body["system"], json!("be terse"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-provider builds_messages_body_with_stream_flag`
Expected: FAIL — `request_body` not found.

- [ ] **Step 3: Implement `request_body`**

Add above the `#[cfg(test)]` block in `crates/zoid-provider/src/anthropic.rs`:

```rust
use serde_json::{json, Value};

/// Build the Anthropic Messages API request body for a streaming completion.
pub fn request_body(req: &CompletionRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            json!({
                "role": match m.role { MsgRole::User => "user", MsgRole::Assistant => "assistant" },
                "content": m.content,
            })
        })
        .collect();

    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "messages": messages,
    });
    if let Some(sys) = &req.system {
        body["system"] = json!(sys);
    }
    body
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS — both new body tests plus `fake_streams_scripted_events_in_order`.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic.rs
git commit -m "feat(provider): Anthropic request body builder (pure)"
```

---

### Task 7: Anthropic SSE event parsing (pure)

The other pure half: map one Anthropic SSE frame (`event:` type + `data:` JSON) to an optional `ProviderEvent`. This is the highest-value provider test — feed recorded frames, assert the event sequence — with zero network.

**Files:**
- Modify: `crates/zoid-provider/src/anthropic.rs`

**Interfaces:**
- Consumes: `crate::{ProviderEvent, Usage}`.
- Produces: `pub fn parse_event(event_type: &str, data: &str) -> Option<ProviderEvent>`.

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `#[cfg(test)] mod tests` block in `crates/zoid-provider/src/anthropic.rs`:

```rust
    #[test]
    fn parses_text_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(parse_event("content_block_delta", data), Some(ProviderEvent::TextDelta("Hello".into())));
    }

    #[test]
    fn parses_message_stop_as_done() {
        assert_eq!(parse_event("message_stop", r#"{"type":"message_stop"}"#), Some(ProviderEvent::Done));
    }

    #[test]
    fn parses_message_delta_usage() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#;
        assert_eq!(parse_event("message_delta", data),
            Some(ProviderEvent::Usage(Usage { input_tokens: 0, output_tokens: 12 })));
    }

    #[test]
    fn parses_message_start_input_usage() {
        let data = r#"{"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1}}}"#;
        assert_eq!(parse_event("message_start", data),
            Some(ProviderEvent::Usage(Usage { input_tokens: 7, output_tokens: 0 })));
    }

    #[test]
    fn parses_error() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(parse_event("error", data), Some(ProviderEvent::Error("Overloaded".into())));
    }

    #[test]
    fn ignores_unhandled_frames() {
        assert_eq!(parse_event("ping", "{}"), None);
        assert_eq!(parse_event("content_block_start", r#"{"type":"content_block_start"}"#), None);
        assert_eq!(parse_event("content_block_stop", r#"{"type":"content_block_stop"}"#), None);
    }

    #[test]
    fn malformed_data_yields_none_not_panic() {
        assert_eq!(parse_event("content_block_delta", "not json"), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-provider parses_text_delta`
Expected: FAIL — `parse_event` not found.

- [ ] **Step 3: Implement `parse_event`**

Add above the `#[cfg(test)]` block in `crates/zoid-provider/src/anthropic.rs` (the file already has `use crate::{...}` and `use serde_json::{json, Value}` — extend the `crate` import to include `ProviderEvent` and `Usage`):

```rust
use crate::{ProviderEvent, Usage};

/// Map one Anthropic SSE frame to a `ProviderEvent`. Unhandled or malformed
/// frames return `None` (the caller skips them). Never panics.
pub fn parse_event(event_type: &str, data: &str) -> Option<ProviderEvent> {
    match event_type {
        "content_block_delta" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let text = v.get("delta")?.get("text")?.as_str()?;
            Some(ProviderEvent::TextDelta(text.to_string()))
        }
        "message_start" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let input = v.get("message")?.get("usage")?.get("input_tokens")?.as_u64()?;
            Some(ProviderEvent::Usage(Usage { input_tokens: input, output_tokens: 0 }))
        }
        "message_delta" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let output = v.get("usage")?.get("output_tokens")?.as_u64()?;
            Some(ProviderEvent::Usage(Usage { input_tokens: 0, output_tokens: output }))
        }
        "message_stop" => Some(ProviderEvent::Done),
        "error" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let msg = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            Some(ProviderEvent::Error(msg.to_string()))
        }
        _ => None,
    }
}
```

> Implementer note: the existing `use crate::{CompletionRequest, Message, MsgRole};` line from Task 6 stays; either merge the new names into it (`use crate::{CompletionRequest, Message, MsgRole, ProviderEvent, Usage};`) or add the second `use crate::{ProviderEvent, Usage};` line — both compile; prefer merging into one import.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-provider`
Expected: PASS — all SSE-parse tests + body tests + fake test.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-provider/src/anthropic.rs
git commit -m "feat(provider): Anthropic SSE frame parsing (pure)"
```

---

### Task 8: `AnthropicProvider` (reqwest + SSE) + provider selection

Wires the pure halves into a real streaming provider, and adds a selection helper so the binary uses Anthropic when `ANTHROPIC_API_KEY` is set and the offline `FakeProvider` otherwise. The network path is **not** unit-tested (needs a key + network); it is covered by the build plus the Task 6/7 pure tests it composes.

**Files:**
- Modify: `crates/zoid-provider/src/anthropic.rs`

**Interfaces:**
- Consumes: `crate::{Provider, ProviderEvent, CompletionRequest}`, `request_body`, `parse_event`.
- Produces:
  - `AnthropicProvider` + `AnthropicProvider::new(api_key: String) -> Self`.
  - `impl Provider for AnthropicProvider`.
  - `pub fn default_provider() -> std::sync::Arc<dyn Provider>` — Anthropic if `ANTHROPIC_API_KEY` is set & non-empty, else a canned `FakeProvider`.

- [ ] **Step 1: Implement the provider and selection helper**

Add above the `#[cfg(test)]` block in `crates/zoid-provider/src/anthropic.rs`:

```rust
use crate::{FakeProvider, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Streaming Anthropic Messages API provider.
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(&self, req: &crate::CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body(req))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let _ = sink.send(ProviderEvent::Error(format!("HTTP {status}: {text}"))).await;
            return Ok(());
        }

        let mut stream = resp.bytes_stream().eventsource();
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if let Some(pe) = parse_event(&event.event, &event.data) {
                        let is_done = matches!(pe, ProviderEvent::Done);
                        if sink.send(pe).await.is_err() {
                            break; // receiver gone
                        }
                        if is_done {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Pick the provider from the environment: real Anthropic when
/// `ANTHROPIC_API_KEY` is set & non-empty, otherwise an offline fake that
/// echoes a canned reply (so the binary runs without a key).
pub fn default_provider() -> Arc<dyn Provider> {
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => Arc::new(AnthropicProvider::new(key)),
        _ => Arc::new(FakeProvider::new(vec![
            ProviderEvent::TextDelta("(no ANTHROPIC_API_KEY — offline echo) ".into()),
            ProviderEvent::TextDelta("hello from zoid's fake provider.".into()),
            ProviderEvent::Done,
        ])),
    }
}
```

- [ ] **Step 2: Verify the whole provider crate builds warning-free**

Run: `cargo build -p zoid-provider`
Expected: PASS, **zero warnings** — `reqwest`/`eventsource-stream`/`futures-util` are now all referenced.

- [ ] **Step 3: Run the provider test suite**

Run: `cargo test -p zoid-provider`
Expected: PASS — all pure tests (body, SSE parse, fake stream). No network test exists by design.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-provider/src/anthropic.rs
git commit -m "feat(provider): AnthropicProvider (reqwest+SSE) + env-based selection"
```

---

### Task 9: Key→action classification (pure input logic)

The pure, terminal-free core of input handling: classify a `crossterm` `KeyEvent` into a `KeyAction` the loop acts on. With a real input box, `q` is now typable, so quit moves to `Ctrl-C`; mode toggle is `Shift-Tab` (`KeyCode::BackTab`); `Enter` submits and `Alt+Enter` inserts a newline; everything else is editing passed to the text area.

**Files:**
- Create: `crates/zoid/src/input.rs`
- Modify: `crates/zoid/src/main.rs` (add `mod input;` — done in Task 11; for this task add it at the top so the test compiles)

**Interfaces:**
- Consumes: `crossterm::event::{KeyCode, KeyEvent, KeyModifiers}`.
- Produces: `pub enum KeyAction { Quit, ToggleMode, Submit, Newline, Edit }`; `pub fn classify(key: KeyEvent) -> KeyAction`.

- [ ] **Step 1: Add the module declaration**

At the top of `crates/zoid/src/main.rs`, add (above the existing `use` lines):

```rust
mod input;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/zoid/src/input.rs`:

```rust
//! Pure key classification for the Chat loop — terminal-free and unit-tested.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Quit,
    ToggleMode,
    Submit,
    Newline,
    Edit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn ctrl_c_quits() {
        assert_eq!(classify(key(KeyCode::Char('c'), KeyModifiers::CONTROL)), KeyAction::Quit);
    }

    #[test]
    fn shift_tab_toggles_mode() {
        assert_eq!(classify(key(KeyCode::BackTab, KeyModifiers::SHIFT)), KeyAction::ToggleMode);
        // crossterm sometimes reports BackTab with no modifier flag
        assert_eq!(classify(key(KeyCode::BackTab, KeyModifiers::NONE)), KeyAction::ToggleMode);
    }

    #[test]
    fn plain_enter_submits_alt_enter_newlines() {
        assert_eq!(classify(key(KeyCode::Enter, KeyModifiers::NONE)), KeyAction::Submit);
        assert_eq!(classify(key(KeyCode::Enter, KeyModifiers::ALT)), KeyAction::Newline);
    }

    #[test]
    fn plain_char_is_edit() {
        assert_eq!(classify(key(KeyCode::Char('q'), KeyModifiers::NONE)), KeyAction::Edit);
        assert_eq!(classify(key(KeyCode::Char('a'), KeyModifiers::NONE)), KeyAction::Edit);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p zoid ctrl_c_quits`
Expected: FAIL — `classify` not found.

- [ ] **Step 4: Implement `classify`**

Add above the `#[cfg(test)]` block in `crates/zoid/src/input.rs`:

```rust
/// Classify a key press for the Chat loop. Order matters: control combos and
/// special keys are matched before falling through to plain editing.
pub fn classify(key: KeyEvent) -> KeyAction {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => KeyAction::Quit,
        (KeyCode::BackTab, _) => KeyAction::ToggleMode,
        (KeyCode::Enter, m) if m.contains(KeyModifiers::ALT) => KeyAction::Newline,
        (KeyCode::Enter, _) => KeyAction::Submit,
        _ => KeyAction::Edit,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid`
Expected: PASS — all `input` tests. (The binary itself is still P0's synchronous `main`; adding `mod input;` does not change its behavior. If `mod input;` triggers a dead-code warning because `classify` is only used by tests until Task 11, add `#[allow(dead_code)]` on the `pub fn classify` for this task only and remove it in Task 11 when the loop calls it — or accept the warning until Task 11. Prefer: leave it; Task 11 consumes it and the final build is warning-free.)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/input.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): pure key→action classification for the chat loop"
```

---

### Task 10: Chat render with input box + streaming caret

Extends `render_chat` to a 4-row layout (title · conversation · input box · status), renders the `tui-textarea` input widget, and shows the streaming caret `▌` after the in-progress assistant text while a response streams. Adds `glyph::SHIFT` to the token set (resolving the P0-deferred `⇧` hardcode) and routes the status bar through it. Updates the two P0 snapshots (layout changed) and adds a streaming-caret snapshot.

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs` (add `glyph::SHIFT`)
- Modify: `crates/zoid-tui/src/chat.rs` (new signature + layout + caret)
- Modify: `crates/zoid-tui/Cargo.toml` (add `tui-textarea` dep)
- Modify: `crates/zoid-tui/tests/chat_snapshot.rs` (new signature; update/add snapshots)

**Interfaces:**
- Consumes: `tokens::{color, glyph}`, `zoid_core::projection::{Role, Turn}`, `tui_textarea::TextArea`, `ratatui::Frame`.
- Produces: `chat::render_chat(frame: &mut Frame, turns: &[Turn], input: &tui_textarea::TextArea<'_>, streaming: bool)` (changed signature — adds `input` and `streaming`; `TextArea` carries a lifetime, so the borrow is `&TextArea<'_>`).

- [ ] **Step 1: Add the `tui-textarea` dependency**

Edit `crates/zoid-tui/Cargo.toml` to add `tui-textarea` under `[dependencies]`:

```toml
[package]
name = "zoid-tui"
version = "0.0.0"
edition.workspace = true

[dependencies]
ratatui = { workspace = true }
zoid-core = { path = "../zoid-core" }
tui-textarea = { workspace = true }

[dev-dependencies]
insta = { workspace = true }
```

- [ ] **Step 2: Add the `SHIFT` glyph token**

In `crates/zoid-tui/src/tokens.rs`, add `SHIFT` to the `glyph` module (keep all existing glyphs):

```rust
    pub const SHIFT: char = '⇧';
```

(Place it alongside the other `pub const ... : char` entries inside `pub mod glyph { ... }`.)

- [ ] **Step 3: Update the snapshot tests to the new signature**

Replace the contents of `crates/zoid-tui/tests/chat_snapshot.rs` with (note the shared `TextArea` construction and the new `streaming_caret_frame` test):

```rust
use ratatui::{backend::TestBackend, Terminal};
use tui_textarea::TextArea;
use zoid_core::projection::{Role, Turn};
use zoid_tui::chat::render_chat;

#[test]
fn empty_chat_frame() {
    let input = TextArea::default();
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &[], &input, false)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn seeded_transcript_frame() {
    let turns = vec![
        Turn { role: Role::User, text: "what's causing the 500?".into() },
        Turn { role: Role::Assistant, text: "an unwrapped lookup in the handler.".into() },
    ];
    let input = TextArea::default();
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &turns, &input, false)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn streaming_caret_frame() {
    let turns = vec![
        Turn { role: Role::User, text: "hi".into() },
        Turn { role: Role::Assistant, text: "thinking".into() },
    ];
    let input = TextArea::default();
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_chat(f, &turns, &input, true)).unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}
```

- [ ] **Step 4: Run the snapshot tests to verify they fail to compile**

Run: `cargo test -p zoid-tui --test chat_snapshot`
Expected: FAIL to compile — `render_chat` takes 2 args, not 4.

- [ ] **Step 5: Rewrite `render_chat`**

Replace the contents of `crates/zoid-tui/src/chat.rs` with:

```rust
use crate::tokens::{color, glyph};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_textarea::TextArea;
use zoid_core::projection::{Role, Turn};

/// Render the Chat surface: title bar, conversation column, input box, status bar.
/// When `streaming` is true, a caret `▌` trails the in-progress assistant text.
pub fn render_chat(frame: &mut Frame, turns: &[Turn], input: &TextArea<'_>, streaming: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // conversation
        Constraint::Length(3), // input box (bordered)
        Constraint::Length(1), // status bar
    ])
    .split(frame.area());

    // Title bar.
    let title = Line::from(vec![
        Span::styled(" zoid ", Style::new().fg(color::TXT).bold()),
        Span::styled("CHAT ", Style::new().fg(color::CHAT_ACCENT).bold()),
        Span::styled(format!("{} main", glyph::BRANCH), Style::new().fg(color::BRANCH)),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    // Conversation.
    let last = turns.len().saturating_sub(1);
    let body: Vec<Line> = if turns.is_empty() {
        vec![Line::styled("  (no messages yet)", Style::new().fg(color::DIM))]
    } else {
        turns
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (prefix, accent) = match t.role {
                    Role::User => (format!("{} ", glyph::USER_TURN), color::CHAT_ACCENT),
                    Role::Assistant => ("zoid ".to_string(), color::DIM),
                };
                let mut text = t.text.clone();
                if streaming && i == last && t.role == Role::Assistant {
                    text.push(glyph::CARET);
                }
                Line::from(vec![
                    Span::styled(prefix, Style::new().fg(accent)),
                    Span::styled(text, Style::new().fg(color::TXT)),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(body), chunks[1]);

    // Input box (bordered text area).
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::DIM))
        .title(Span::styled(" message ", Style::new().fg(color::DIM)));
    frame.render_widget(input_block, chunks[2]);
    let inner = chunks[2].inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    frame.render_widget(input, inner);

    // Status bar.
    let status = Line::from(vec![
        Span::styled(" CHAT ", Style::new().fg(color::CHAT_ACCENT)),
        Span::styled(
            format!("· {}Tab Build · ⏎ send · ^C quit", glyph::SHIFT),
            Style::new().fg(color::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[3]);
}
```

> Implementer note on the `tui-textarea` 0.7 + ratatui 0.29 render call: `&TextArea` implements `ratatui::widgets::Widget`, so `frame.render_widget(input, inner)` compiles (`input: &TextArea`). If the trait bound does not resolve in this version, use `frame.render_widget(input.widget(), inner)` instead — try the `&TextArea` form first; do not add other rendering crates.

- [ ] **Step 6: Run, review, and accept the snapshots**

Run: `cargo test -p zoid-tui --test chat_snapshot`
Expected: FAIL — three new/changed snapshots (`empty_chat_frame`, `seeded_transcript_frame`, `streaming_caret_frame`) are pending.

Inspect each pending `.snap.new` under `crates/zoid-tui/tests/snapshots/` and confirm:
- `empty_chat_frame`: title `zoid CHAT ⎇ main`, `(no messages yet)`, a bordered ` message ` input box, status `CHAT · ⇧Tab Build · ⏎ send · ^C quit`.
- `seeded_transcript_frame`: the two turns, no caret.
- `streaming_caret_frame`: the assistant line ends with `▌`.
- All glyphs (`⎇ › ⇧ ▌`) render as real characters (UTF-8), not replacement chars.

Accept: `cargo insta accept` (if `cargo-insta` is unavailable, rename each `.snap.new` to `.snap`).

Then re-run: `cargo test -p zoid-tui`
Expected: PASS — tokens test + all three snapshots.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/Cargo.toml crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/chat.rs crates/zoid-tui/tests/chat_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): chat input box + streaming caret + SHIFT glyph token"
```

---

### Task 11: The async chat loop (`zoid` binary)

The capstone: rewrite `main.rs` into a `tokio` event loop that streams. It boots via `SessionHandle`, renders with the new `render_chat`, and runs a `tokio::select!` over `crossterm::EventStream` and a provider-delta channel. Submitting a message persists a `UserMessage`, spawns the provider stream, and appends each `TextDelta` as a `ModelDelta` so the transcript grows live. Also folds in the P0-deferred binary cleanups (terminal-restore hardening; `db_path` → `PathBuf` with context).

This task is **integration glue** with no unit test of its own (an interactive TUI loop and a network provider can't be exercised headlessly). It is verified by `cargo build`, the full `cargo test` suite staying green, and a **deferred manual smoke** (no TTY in a subagent).

**Files:**
- Modify: `crates/zoid/src/main.rs` (full rewrite)
- Modify: `crates/zoid/Cargo.toml` (add deps)

**Interfaces:**
- Consumes: `zoid_core::session::SessionHandle`, `zoid_core::event::{Event, EventKind}`, `zoid_core::projection::{transcript, Role}`, `zoid_provider::{Provider, ProviderEvent, CompletionRequest, Message, MsgRole, anthropic}`, `zoid_tui::chat::render_chat`, `crate::input::{classify, KeyAction}`, `tui_textarea::TextArea`.
- Produces: the streaming `zoid` binary.

- [ ] **Step 1: Update the binary manifest**

Edit `crates/zoid/Cargo.toml`:

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
zoid-provider = { path = "../zoid-provider" }
zoid-tui = { path = "../zoid-tui" }
anyhow = { workspace = true }
ratatui = { workspace = true }
crossterm = { workspace = true }
tokio = { workspace = true }
tui-textarea = { workspace = true }
ulid = { workspace = true }
futures-util = { workspace = true }
```

- [ ] **Step 2: Rewrite `main.rs`**

Replace the contents of `crates/zoid/src/main.rs` with (keep the `mod input;` line from Task 9):

```rust
mod input;

use anyhow::{Context, Result};
use crossterm::{
    event::{Event as CEvent, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{prelude::CrosstermBackend, Terminal};
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tui_textarea::TextArea;
use ulid::Ulid;

use input::{classify, KeyAction};
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{transcript, Role};
use zoid_core::session::SessionHandle;
use zoid_provider::anthropic::{default_provider, DEFAULT_MODEL};
use zoid_provider::{CompletionRequest, Message, MsgRole, Provider, ProviderEvent};

const SYSTEM_PROMPT: &str = "You are zoid, a terminal coding assistant. Be concise and precise.";

/// Resolve the session DB path: `$ZOID_DB` if set, else `./.zoid/session.db`.
fn db_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("ZOID_DB") {
        return Ok(PathBuf::from(p));
    }
    let dir = Path::new(".zoid");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("session.db"))
}

/// Wall-clock millis since the epoch — supplied by the binary (core stays clock-free).
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct App {
    session: SessionHandle,
    events: Vec<Event>,
    provider: Arc<dyn Provider>,
    model: String,
    textarea: TextArea<'static>,
    streaming: bool,
}

impl App {
    /// Append an event both durably (session actor) and to the in-memory log
    /// the UI renders from.
    async fn record(&mut self, kind: EventKind) -> Result<()> {
        let ev = Event::new(Ulid::new(), None, now_ms(), kind);
        self.session.append(ev.clone()).await?;
        self.events.push(ev);
        Ok(())
    }

    /// Build the provider request from the current transcript.
    fn request(&self) -> CompletionRequest {
        let messages = transcript(&self.events)
            .into_iter()
            .map(|t| Message {
                role: match t.role {
                    Role::User => MsgRole::User,
                    Role::Assistant => MsgRole::Assistant,
                },
                content: t.text,
            })
            .collect();
        CompletionRequest {
            model: self.model.clone(),
            system: Some(SYSTEM_PROMPT.to_string()),
            messages,
            max_tokens: 4096,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = db_path()?;
    let session = SessionHandle::spawn(path.to_str().context("session DB path is not valid UTF-8")?)?;
    let events = session.snapshot().await?;

    let model = std::env::var("ZOID_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    let mut app = App {
        session,
        events,
        provider: default_provider(),
        model,
        textarea: TextArea::default(),
        streaming: false,
    };

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, &mut app).await;

    // Restore the terminal on every exit path — drive through errors, don't bail.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

async fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let mut term_events = EventStream::new();
    // Long-lived delta channel; each provider turn clones the sender.
    let (delta_tx, mut delta_rx) = mpsc::channel::<ProviderEvent>(256);

    loop {
        let turns = transcript(&app.events);
        terminal.draw(|f| render_chat(f, &turns, &app.textarea, app.streaming))?;

        tokio::select! {
            maybe_term = term_events.next() => {
                match maybe_term {
                    Some(Ok(CEvent::Key(key))) => {
                        match classify(key) {
                            KeyAction::Quit => return Ok(()),
                            KeyAction::ToggleMode => { /* Build mode arrives in P6 — no-op */ }
                            KeyAction::Newline => { app.textarea.insert_newline(); }
                            KeyAction::Edit => { app.textarea.input(key); }
                            KeyAction::Submit => {
                                if app.streaming { continue; } // ignore submits mid-stream
                                let text = app.textarea.lines().join("\n");
                                if text.trim().is_empty() { continue; }
                                app.textarea = TextArea::default();
                                app.record(EventKind::UserMessage { text }).await?;
                                app.streaming = true;

                                let req = app.request();
                                let provider = app.provider.clone();
                                let tx = delta_tx.clone();
                                tokio::spawn(async move {
                                    let _ = provider.stream(&req, tx).await;
                                });
                            }
                        }
                    }
                    Some(Ok(_)) => { /* resize/mouse/etc: redraw on next loop */ }
                    Some(Err(_)) | None => return Ok(()),
                }
            }
            Some(pe) = delta_rx.recv() => {
                match pe {
                    ProviderEvent::TextDelta(s) => {
                        app.record(EventKind::ModelDelta { text: s }).await?;
                    }
                    ProviderEvent::Usage(_) => { /* token ledger lands in P3 */ }
                    ProviderEvent::Error(msg) => {
                        app.record(EventKind::AssistantMessage { text: format!("⚠ {msg}") }).await?;
                        app.streaming = false;
                    }
                    ProviderEvent::Done => { app.streaming = false; }
                }
            }
        }
    }
}
```

- [ ] **Step 3: Verify the whole workspace builds warning-free**

Run: `cargo build`
Expected: PASS — all 4 crates compile, `target/debug/zoid` produced, **zero warnings** (the Task 9 `classify` is now used; remove any temporary `#[allow(dead_code)]` added there).

- [ ] **Step 4: Verify the full test suite is green**

Run: `cargo test`
Expected: PASS across `zoid-core` (event/projection/session/store + round_trip), `zoid-provider` (fake + body + SSE parse), `zoid-tui` (tokens + 3 snapshots), `zoid` (input classify).

- [ ] **Step 5: Manual smoke (deferred — requires a real TTY + optionally an API key)**

> This step is for the human, not the subagent (no TTY headlessly). Document it in the task report as deferred.

```bash
# Offline (fake provider): type a message, press Enter, watch the canned reply stream in.
ZOID_DB=/tmp/zoid-p1a.db cargo run -p zoid
# With a real key: export ANTHROPIC_API_KEY=sk-...  (optionally ZOID_MODEL=claude-sonnet-4-6)
#   then submit a message and watch tokens stream from Claude.
# Alt+Enter = newline; Enter = send; Shift-Tab = (no-op until P6); Ctrl-C = quit.
# Relaunch against the same ZOID_DB and confirm the prior turns replay.
rm -f /tmp/zoid-p1a.db
```

Expected: launches to the empty Chat frame with a bordered input; submitting streams an assistant reply token-by-token with a trailing `▌`; quitting restores the terminal; relaunch replays the persisted conversation.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/Cargo.toml crates/zoid/src/main.rs
git commit -m "feat(zoid): async streaming chat loop (tokio select + session actor)"
```

---

## P1a Definition of Done

- `cargo build` produces `zoid`; `cargo test` is fully green and **warning-free**.
- `zoid` runs an **async** loop (`tokio` + `crossterm::EventStream` + `tokio::select!`); SQLite is behind the single-writer `SessionHandle` actor (no blocking on the runtime).
- A user can type a multi-line message (`tui-textarea`; `Enter` send, `Alt+Enter` newline), and the assistant reply **streams in token-by-token** with a trailing caret `▌`.
- Streamed output is persisted incrementally as `ModelDelta` events; the `Transcript` projection folds delta runs into one assistant turn; relaunching **replays** the conversation.
- The real `AnthropicProvider` (reqwest + rustls + SSE) is used when `ANTHROPIC_API_KEY` is set; otherwise the offline `FakeProvider` echoes a canned reply — the binary always runs.
- `Provider` is a streaming seam (`mpsc` sink + `CompletionRequest`); `ProviderEvent` carries `TextDelta`/`Usage`/`Done`/`Error`; the pure request-builder and SSE-parser are unit-tested.
- Single-static-binary constraints intact: `rustls` (no OpenSSL), `rusqlite` bundled, no `wasmtime`/`tree-sitter`, release profile unchanged, `spikes/` excluded.
- P0-deferred cleanups resolved: `glyph::SHIFT` token (no hardcoded `⇧`), hardened terminal restore, `db_path` → `PathBuf` with context.
- Every screen state carries an `insta` snapshot (empty / seeded / streaming-caret) rendered from the `tokens` single source.

## Self-Review (against spec §3, §4.1, §6.1, §9, §11, §13 + roadmap P1)

- **Spec coverage (P1a slice of P1):** Anthropic SSE streaming provider ✓ (Tasks 5–8); the agent loop ✓ (Task 11, text-only — tool dispatch is P1b); real multi-line input `tui-textarea` ✓ (Tasks 10–11); streaming caret / motion-lite ✓ (Task 10). **Deferred to P1b (by design):** tool-calling, core tools (fs/shell/search) in cwd, inline tool rendering with `→ peek`. **Deferred later:** token ledger/economy (P3) — `Usage` is captured by the seam but not yet persisted/rendered.
- **Async substrate (§3, §4.1):** tokio loop + `EventStream` + `select!` ✓; single-writer store actor with immutable snapshots + channel message-passing ✓ (Task 4) — matches the actor shape the spec mandates to avoid borrow-checker friction.
- **Incremental persistence / resumability (§11):** `ModelDelta` appended per token; crash/relaunch re-folds the log ✓.
- **Clock-free core:** `ts`/`Ulid` injected by the binary only; `zoid-core` adds no clock ✓.
- **Testing strategy (§13):** core projections via unit + proptest ✓; agent loop via `FakeProvider` (offline, scripted) ✓; provider SSE parsing unit-tested on recorded frames ✓; TUI fidelity via `TestBackend` + `insta` ✓; the network path explicitly not unit-tested ✓.
- **Type consistency:** `SessionHandle::{spawn,append,snapshot}` used identically in Task 4 and Task 11; `render_chat(&mut Frame, &[Turn], &TextArea, bool)` identical in Tasks 10 and 11; `Provider::stream(&self, &CompletionRequest, mpsc::Sender<ProviderEvent>)` identical across Tasks 5, 8, 11; `EventKind::ModelDelta { text }` identical across Tasks 2, 3, 11; `classify`/`KeyAction` identical across Tasks 9 and 11.
- **Placeholder scan:** every code step contains complete, compilable code; every run step states the command + expected outcome. The two inherently non-unit-testable deliverables (real network provider; interactive loop) are explicitly flagged and bracketed by the pure tests they compose.
- **Coupling note:** the tight Task 2↔3 pair (adding a variant breaks the exhaustive projection match) is called out in Task 2's note so each commit still compiles.
