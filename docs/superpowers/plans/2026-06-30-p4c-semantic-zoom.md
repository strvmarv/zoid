# P4c · ① Semantic Zoom Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the conversation an **altitude** — a discrete three-level zoom (Summary / Normal / Detail) — driven by `Ctrl-scroll` and keys, with structural per-turn summaries when zoomed out, code-aware highlighting when zoomed in, and an animated fold/unfold transition.

**Architecture:** A pure `zoom` projection in `zoid-core` folds the existing `ChatMsg` stream into one **structural** `TurnDigest` per turn (no model calls). `zoid-tui` adds a `Zoom { Summary, Normal, Detail }` altitude on `ShellState` and a `ChatView` per-frame view-model the bin assembles; the renderer dispatches on altitude — Summary renders digests, Normal is today's conversation, Detail expands tool output and **highlights code via `zoid-syntax` (P4a)**. Altitude changes **animate** via a progressive line-reveal using `motion` (P4b). `Ctrl-scroll`/keys cycle altitude.

**Tech Stack:** Rust 2021, ratatui 0.29 (`TestBackend`/`insta`), proptest. **Depends on P4a** (`zoid-syntax`, `zoid_tui::highlight_lines`) **and P4b** (`zoid_tui::motion`).

## Global Constraints

- **Crates & dep direction:** the zoom projection is a **pure** function over `ChatMsg` in `crates/zoid-core/src/zoom.rs` (no ratatui). Altitude state + rendering live in `zoid-tui`. Code highlighting reuses `zoid-tui::highlight_lines` (P4a); animation reuses `zoid-tui::motion` (P4b). No cycles.
- **Summaries are STRUCTURAL/deterministic (P4c scope decision, 2026-06-30):** zoomed-out summaries are computed from the event/`ChatMsg` structure — `> {user headline}`, `~ {n} tools · {n} files`, error flag — **never** an LLM call. This keeps zoom instant and snapshot-testable. (LLM summaries are post-v1.)
- **Discrete three-level altitude:** `Zoom { Summary, Normal, Detail }`. `Normal` is the existing conversation render (unchanged content). No continuous zoom.
- **Design tokens (spec §16):** any new glyph (e.g. a turn marker) or color must be a token in `tokens.rs` and mirrored in `docs/ux/README.md`. Reuse existing glyphs where possible (`glyph::USER_TURN ›`, `glyph::EDIT ●`, `glyph::COLLAPSED ▸`). ASCII punctuation is exempt.
- **UX testing is multi-width:** the altitude render adds `TestBackend`+`insta` snapshots at **both 100×24 and 140×24** for Summary and Detail, plus `preview.rs` scenes. The zoom projection is a pure function with its own unit + proptest coverage. The **transition animation is NOT snapshot-coverable** (spec §13) — verified by the pure reveal-count test + manual/gif.
- **TDD, DRY, YAGNI, frequent commits. No `Co-Authored-By` / co-author trailer** (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit. Accept snapshots with `INSTA_UPDATE=always cargo test -p zoid-tui --test <file>` and review `.snap` fidelity before committing.

---

### Task 1: `zoom` projection — `TurnDigest`, `digests()`

**Files:**
- Create: `crates/zoid-core/src/zoom.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod zoom;`)
- Test: inline `mod tests` + a `proptest!` block.

**Interfaces:**
- Consumes: `projection::ChatMsg`, `economy::tool_path` (`pub(crate)`, same crate).
- Produces:
  - `struct TurnDigest { pub headline: String, pub tools: usize, pub files: usize, pub has_error: bool }` (`Debug, Clone, PartialEq, Eq`).
  - `fn digests(msgs: &[ChatMsg]) -> Vec<TurnDigest>` — one per turn. A turn starts at each `ChatMsg::User` (or a leading `Assistant` when the log starts mid-turn). `headline` is the user text (trimmed to 60 chars) or, for an assistant-led turn, the assistant text; `tools` counts tool calls; `files` counts tool calls whose args carry a path; `has_error` is set by any errored tool result in the turn.

- [ ] **Step 1: Write the failing tests**

`crates/zoid-core/src/zoom.rs` (test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{ChatMsg, ToolCallRef};
    use proptest::prelude::*;

    fn call(name: &str, args: &str) -> ToolCallRef {
        ToolCallRef { id: String::new(), name: name.into(), args: args.into() }
    }

    #[test]
    fn one_digest_per_turn_with_counts() {
        let msgs = vec![
            ChatMsg::User("fix the parser bug".into()),
            ChatMsg::Assistant {
                text: "looking".into(),
                tool_calls: vec![
                    call("read_file", r#"{"path":"src/ast.rs"}"#), // file
                    call("shell", r#"{"command":"cargo test"}"#),  // not a file
                ],
            },
            ChatMsg::ToolResult { id: String::new(), name: "read_file".into(), output: "fn parse() {}".into(), is_error: false },
            ChatMsg::ToolResult { id: String::new(), name: "shell".into(), output: "boom".into(), is_error: true },
            ChatMsg::User("thanks".into()),
        ];
        let d = digests(&msgs);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].headline, "fix the parser bug");
        assert_eq!(d[0].tools, 2);
        assert_eq!(d[0].files, 1);
        assert!(d[0].has_error);
        assert_eq!(d[1].headline, "thanks");
        assert_eq!(d[1].tools, 0);
        assert!(!d[1].has_error);
    }

    #[test]
    fn assistant_led_log_starts_a_turn() {
        let msgs = vec![ChatMsg::Assistant { text: "hello there".into(), tool_calls: vec![] }];
        let d = digests(&msgs);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].headline, "hello there");
    }

    #[test]
    fn long_headline_is_trimmed() {
        let long = "x".repeat(200);
        let d = digests(&[ChatMsg::User(long)]);
        assert!(d[0].headline.chars().count() <= 60);
    }

    #[test]
    fn empty_log_has_no_digests() {
        assert_eq!(digests(&[]), Vec::new());
    }

    proptest! {
        #[test]
        fn never_panics_and_one_digest_per_user(n in 0usize..30) {
            let msgs: Vec<ChatMsg> = (0..n).map(|i| ChatMsg::User(format!("turn {i}"))).collect();
            prop_assert_eq!(digests(&msgs).len(), n);
        }
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-core zoom`
Expected: compile error — module/types undefined.

- [ ] **Step 3: Implement**

`crates/zoid-core/src/zoom.rs`:

```rust
//! The semantic-zoom projection (spec §4.1/①): fold the conversation into one
//! *structural* digest per turn for the zoomed-out altitude. Deterministic — no
//! model calls. `zoid-tui` renders these as one-line summaries.

use crate::economy::tool_path;
use crate::projection::ChatMsg;

/// A one-line structural summary of a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDigest {
    pub headline: String,
    pub tools: usize,
    pub files: usize,
    pub has_error: bool,
}

const HEADLINE_MAX: usize = 60;

fn trim_headline(s: &str) -> String {
    let one_line = s.lines().next().unwrap_or("").trim();
    if one_line.chars().count() > HEADLINE_MAX {
        let head: String = one_line.chars().take(HEADLINE_MAX.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        one_line.to_string()
    }
}

/// One `TurnDigest` per turn. Turns start at each `User` message; a log that
/// opens with assistant/tool content starts an implicit turn.
pub fn digests(msgs: &[ChatMsg]) -> Vec<TurnDigest> {
    let mut out: Vec<TurnDigest> = Vec::new();
    let mut cur: Option<TurnDigest> = None;

    for m in msgs {
        match m {
            ChatMsg::User(text) => {
                if let Some(d) = cur.take() {
                    out.push(d);
                }
                cur = Some(TurnDigest {
                    headline: trim_headline(text),
                    tools: 0,
                    files: 0,
                    has_error: false,
                });
            }
            ChatMsg::Assistant { text, tool_calls } => {
                let d = cur.get_or_insert_with(|| TurnDigest {
                    headline: trim_headline(text),
                    tools: 0,
                    files: 0,
                    has_error: false,
                });
                if d.headline.is_empty() {
                    d.headline = trim_headline(text);
                }
                d.tools += tool_calls.len();
                d.files += tool_calls.iter().filter(|c| tool_path(&c.args).is_some()).count();
            }
            ChatMsg::ToolResult { is_error, .. } => {
                let d = cur.get_or_insert_with(|| TurnDigest {
                    headline: String::new(),
                    tools: 0,
                    files: 0,
                    has_error: false,
                });
                d.has_error |= *is_error;
            }
        }
    }
    if let Some(d) = cur.take() {
        out.push(d);
    }
    out
}
```

In `crates/zoid-core/src/lib.rs`: add `pub mod zoom;`.

> `economy::tool_path` is `pub(crate)` (added in P3 T4) — reachable from `zoom.rs` since both are in `zoid-core`. If it is not `pub(crate)`, widen it to `pub(crate)` (do not make it `pub`).

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-core zoom`
Expected: PASS (4 tests + proptest).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/zoom.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): zoom projection — structural per-turn digests (①)"
```

---

### Task 2: `Zoom` altitude on `ShellState` + mutators

**Files:**
- Modify: `crates/zoid-tui/src/state.rs`
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `enum Zoom { Summary, Normal, Detail }` (`Debug, Clone, Copy, PartialEq, Eq`) in `state.rs`.
  - `ShellState.zoom: Zoom` (defaults `Zoom::Normal` in `new()`).
  - `ShellState::zoom_in(&mut self)` — toward `Detail` (`Summary→Normal→Detail`, saturating).
  - `ShellState::zoom_out(&mut self)` — toward `Summary` (`Detail→Normal→Summary`, saturating).

- [ ] **Step 1: Write the failing tests**

In `crates/zoid-tui/src/state.rs` `mod tests`:

```rust
#[test]
fn zoom_defaults_to_normal() {
    assert_eq!(ShellState::new().zoom, Zoom::Normal);
}

#[test]
fn zoom_in_out_saturate_at_ends() {
    let mut s = ShellState::new(); // Normal
    s.zoom_out();
    assert_eq!(s.zoom, Zoom::Summary);
    s.zoom_out();
    assert_eq!(s.zoom, Zoom::Summary); // saturates
    s.zoom_in();
    assert_eq!(s.zoom, Zoom::Normal);
    s.zoom_in();
    assert_eq!(s.zoom, Zoom::Detail);
    s.zoom_in();
    assert_eq!(s.zoom, Zoom::Detail); // saturates
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib state::tests::zoom`
Expected: FAIL — `Zoom`/`zoom`/`zoom_in` undefined.

- [ ] **Step 3: Implement**

In `crates/zoid-tui/src/state.rs`, add the enum (near `Focus`):

```rust
/// Conversation altitude (spec ① semantic zoom). `Normal` is the default
/// turn-by-turn view; `Summary` collapses each turn to a one-line digest;
/// `Detail` expands tool output with code highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Summary,
    Normal,
    Detail,
}
```

Add the field to `ShellState` (after `reduced_motion` from P4b, or after `branch` if P4b not yet merged): `pub zoom: Zoom,`. Initialize in `new()`: `zoom: Zoom::Normal,`. Add the mutators in `impl ShellState`:

```rust
    /// Increase detail (Summary → Normal → Detail), saturating.
    pub fn zoom_in(&mut self) {
        self.zoom = match self.zoom {
            Zoom::Summary => Zoom::Normal,
            Zoom::Normal | Zoom::Detail => Zoom::Detail,
        };
    }

    /// Decrease detail (Detail → Normal → Summary), saturating.
    pub fn zoom_out(&mut self) {
        self.zoom = match self.zoom {
            Zoom::Detail => Zoom::Normal,
            Zoom::Normal | Zoom::Summary => Zoom::Summary,
        };
    }
```

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-tui --lib state::tests::zoom`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/state.rs
git commit -m "feat(tui): Zoom altitude on ShellState + zoom_in/zoom_out (①)"
```

---

### Task 3: Altitude render — `ChatView` + `conversation_view` + code-aware Detail + snapshots

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (`ChatView`, `conversation_view`)
- Modify: `crates/zoid-tui/src/render.rs` (`render_shell` takes `&ChatView`)
- Modify: `crates/zoid-tui/examples/preview.rs` + `crates/zoid-tui/tests/shell_snapshot.rs` (build a `ChatView`)
- Create: snapshots in `crates/zoid-tui/tests/shell_snapshot.rs` (Summary & Detail @100/@140)
- Test: inline unit test + snapshots.

**Interfaces:**
- Consumes: `zoid_core::zoom::{digests, TurnDigest}`, `zoid_core::projection::ChatMsg`, `zoid_syntax::{Language, fold_regions}` (P4a) + the highlight helper imported internally as `crate::syntax_view::highlight_lines` (same fn, re-exported as `zoid_tui::highlight_lines` for external callers — `chat.rs` lives inside `zoid-tui`, so it uses the crate-relative path), `state::Zoom`.
- Produces:
  - `struct ChatView { pub zoom: Zoom, pub caret_on: bool, pub reveal: Option<usize> }` (per-frame view-model the bin assembles; `reveal` caps the number of conversation lines shown — `None` = all; used by the Task 5 transition).
  - `fn conversation_view(msgs: &[ChatMsg], view: &ChatView, streaming: bool) -> Vec<Line<'static>>` — dispatches on `view.zoom`.
  - `render_shell(frame, state, economy, msgs, input, streaming, view: &ChatView)` — the trailing `caret_on: bool` from P4b is **migrated into** `ChatView`.

> **Param consolidation:** P4b added `caret_on: bool` to `render_shell`. This task folds it into `ChatView` (carrying zoom + caret + reveal), so the signature stays a single view param rather than accreting positional bools.

- [ ] **Step 1: Write the failing unit test**

In `crates/zoid-tui/src/chat.rs` (test module — add one if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Zoom;
    use zoid_core::projection::{ChatMsg, ToolCallRef};

    // The Assistant carries a tool_call whose id matches the ToolResult and whose
    // args name a `.rs` path — so `detail_lines` resolves id→path→Language::Rust
    // and actually highlights (without this, id_path is empty and Detail silently
    // falls back to PlainText, the exact gap that made the old fixture useless).
    // The body is multi-line so collapse-to-signatures (Task 3b) has something to fold.
    fn seeded() -> Vec<ChatMsg> {
        vec![
            ChatMsg::User("fix the parser bug".into()),
            ChatMsg::Assistant {
                text: "on it".into(),
                tool_calls: vec![ToolCallRef {
                    id: "c1".into(),
                    name: "read_file".into(),
                    args: r#"{"path":"src/parser.rs"}"#.into(),
                }],
            },
            ChatMsg::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "fn parse(s: &str) -> u32 {\n    let n = 42;\n    n\n}\n".into(),
                is_error: false,
            },
            ChatMsg::User("thanks".into()),
        ]
    }

    fn view(zoom: Zoom) -> ChatView {
        ChatView { zoom, caret_on: true, reveal: None }
    }

    #[test]
    fn summary_collapses_to_one_line_per_turn() {
        let lines = conversation_view(&seeded(), &view(Zoom::Summary), false);
        // two turns → two digest lines
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn detail_highlights_file_tool_results() {
        use crate::tokens::color;
        let lines = conversation_view(&seeded(), &view(Zoom::Detail), false);
        // A keyword (`fn`/`let`) must carry the syntax keyword color — proves the
        // id→path→Rust resolution fired and highlighting actually ran, rather than
        // silently falling back to PlainText (which colors everything TXT).
        let has_keyword_color = lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.style.fg == Some(color::SYN_KEYWORD)));
        assert!(has_keyword_color, "Detail must highlight the Rust tool-result body");
    }

    #[test]
    fn normal_matches_conversation_lines() {
        let msgs = seeded();
        let normal = conversation_view(&msgs, &view(Zoom::Normal), false);
        let baseline = conversation_lines(&msgs, false, true);
        assert_eq!(normal.len(), baseline.len());
    }

    #[test]
    fn reveal_caps_line_count() {
        let mut v = view(Zoom::Normal);
        v.reveal = Some(1);
        let lines = conversation_view(&seeded(), &v, false);
        assert_eq!(lines.len(), 1);
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib chat`
Expected: compile error — `ChatView`/`conversation_view` undefined.

- [ ] **Step 3: Implement the view-model + dispatch**

In `crates/zoid-tui/src/chat.rs`, add the imports and types:

```rust
use crate::state::Zoom;
use zoid_core::zoom::{digests, TurnDigest};
use zoid_syntax::Language;
use crate::syntax_view::highlight_lines;
```

```rust
/// Per-frame conversation view-model the bin assembles: altitude + caret blink
/// + an optional reveal cap (for the zoom transition animation, P4c Task 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatView {
    pub zoom: Zoom,
    pub caret_on: bool,
    pub reveal: Option<usize>,
}

/// Build the conversation lines at the requested altitude, capped to
/// `view.reveal` lines when set.
pub fn conversation_view(msgs: &[ChatMsg], view: &ChatView, streaming: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = match view.zoom {
        Zoom::Summary => digest_lines(&digests(msgs)),
        Zoom::Normal => conversation_lines(msgs, streaming, view.caret_on)
            .into_iter()
            .map(own_line)
            .collect(),
        Zoom::Detail => detail_lines(msgs),
    };
    if let Some(n) = view.reveal {
        lines.truncate(n);
    }
    lines
}

/// One digest line per turn: `› {headline}   ~ {tools}t · {files}f [⚠]`.
fn digest_lines(ds: &[TurnDigest]) -> Vec<Line<'static>> {
    ds.iter()
        .map(|d| {
            let mut spans = vec![
                Span::styled(format!("{} ", glyph::USER_TURN), Style::new().fg(color::CHAT_ACCENT)),
                // `.40` precision truncates to 40 chars; width 40 pads short ones —
                // a HEADLINE_MAX(60) headline can't blow past the column and misalign
                // the `~ Nt · Nf` field in the 140-col snapshot.
                Span::styled(format!("{:<40.40} ", d.headline), Style::new().fg(color::TXT)),
                Span::styled(format!("~ {}t · {}f", d.tools, d.files), Style::new().fg(color::DIM)),
            ];
            if d.has_error {
                spans.push(Span::styled(format!(" {}", glyph::WARNING), Style::new().fg(color::ERROR)));
            }
            Line::from(spans)
        })
        .collect()
}

/// Detail altitude: the normal view, but file tool-results are rendered with
/// syntax highlighting (Ⓡ3, P4a). The file's language is inferred from the
/// originating tool call's path (correlated by id).
fn detail_lines(msgs: &[ChatMsg]) -> Vec<Line<'static>> {
    use std::collections::HashMap;
    // id → file path, from assistant tool calls.
    let mut id_path: HashMap<&str, String> = HashMap::new();
    for m in msgs {
        if let ChatMsg::Assistant { tool_calls, .. } = m {
            for c in tool_calls {
                if let Some(p) = path_arg(&c.args) {
                    id_path.insert(c.id.as_str(), p);
                }
            }
        }
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    for m in msgs {
        match m {
            ChatMsg::ToolResult { id, name, output, is_error } if !*is_error => {
                let header = Span::styled(
                    format!("  {} {}", glyph::PASS, name),
                    Style::new().fg(color::DIM),
                );
                out.push(Line::from(vec![header]));
                let lang = id_path.get(id.as_str()).map(|p| Language::from_path(p)).unwrap_or(Language::PlainText);
                out.extend(highlight_lines(output, lang));
            }
            other => out.extend(conversation_lines(std::slice::from_ref(other), false, true).into_iter().map(own_line)),
        }
    }
    out
}

/// Extract a file path from a tool call's JSON args (mirrors core's tool_path,
/// kept local so chat.rs stays render-side).
fn path_arg(args_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args_json).ok()?;
    for key in ["path", "file_path", "file"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Convert a borrowed Line into an owned ('static) one by cloning span content.
fn own_line(l: Line) -> Line<'static> {
    Line::from(
        l.spans
            .into_iter()
            .map(|s| Span::styled(s.content.into_owned(), s.style))
            .collect::<Vec<_>>(),
    )
}
```

> `conversation_lines` returns `Vec<Line<'a>>` (borrowing `msgs`); `conversation_view` returns `'static` lines so the bin can build them inside the draw closure without lifetime friction. `own_line` does the cheap clone. Keep `conversation_lines` as the Normal-altitude source of truth (DRY) — `conversation_view` wraps it, never reimplements it.

- [ ] **Step 4: Migrate `render_shell` to `&ChatView`**

In `crates/zoid-tui/src/render.rs`, change the signature (replacing P4b's `caret_on: bool`):

```rust
use crate::chat::{conversation_view, ChatView};

pub fn render_shell(
    frame: &mut Frame,
    state: &ShellState,
    economy: &EconomyView,
    msgs: &[ChatMsg],
    input: &TextArea<'_>,
    streaming: bool,
    view: &ChatView,
) {
```

and the Chat conversation render:

```rust
        Mode::Chat => {
            let body = conversation_view(msgs, view, streaming);
            frame.render_widget(Paragraph::new(body).scroll((state.conversation_scroll, 0)), layout.conversation);
        }
```

(Remove the now-unused `conversation_lines` import from `render.rs` if present.)

- [ ] **Step 5: Update callers**

In `crates/zoid-tui/tests/shell_snapshot.rs`, replace the trailing `, true` (P4b's caret bool) with `, &normal_view()` at **both** `render_shell(...)` call sites — there are two: (1) the `draw_econ` helper used by most tests, and (2) the standalone inline call inside the `economy_drawer_selection_highlights_only_when_rail_focused` test (not routed through any helper). Missing the second site breaks the build. Add a helper:

```rust
use zoid_tui::chat::ChatView;
use zoid_tui::state::Zoom;

fn normal_view() -> ChatView {
    ChatView { zoom: Zoom::Normal, caret_on: true, reveal: None }
}
```

In `crates/zoid-tui/examples/preview.rs`, likewise pass `&ChatView { zoom: Zoom::Normal, caret_on: true, reveal: None }` (import `zoid_tui::chat::ChatView` and `zoid_tui::state::Zoom`).

- [ ] **Step 6: Add Summary & Detail snapshots**

In `crates/zoid-tui/tests/shell_snapshot.rs`, add a draw helper plus a Detail-bearing fixture, then four snapshots. The P3 `seeded()` in this file has **no** `ToolResult`, so Detail rendered against it would be a plain conversation — the snapshot would silently bake a frame that proves nothing. Add `seeded_detail()` with a matched tool-call/result pair (id + `.rs` path) so the Detail snapshots actually show highlighted, collapsed code:

```rust
use zoid_core::projection::ToolCallRef;

fn seeded_detail() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User("show me parse".into()),
        ChatMsg::Assistant {
            text: "reading it".into(),
            tool_calls: vec![ToolCallRef {
                id: "c1".into(),
                name: "read_file".into(),
                args: r#"{"path":"src/parser.rs"}"#.into(),
            }],
        },
        ChatMsg::ToolResult {
            id: "c1".into(),
            name: "read_file".into(),
            output: "fn parse(s: &str) -> u32 {\n    let n = 42;\n    n\n}\n".into(),
            is_error: false,
        },
    ]
}

fn draw_zoom(zoom: Zoom, w: u16, h: u16) -> String {
    let s = ShellState::new();
    let view = ChatView { zoom, caret_on: true, reveal: None };
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_shell(f, &s, &empty_economy(), &seeded_detail(), &input, false, &view))
        .unwrap();
    terminal.backend().to_string()
}

#[test] fn zoom_summary_frame() { insta::assert_snapshot!(draw_zoom(Zoom::Summary, 100, 24)); }
#[test] fn zoom_summary_wide_frame() { insta::assert_snapshot!(draw_zoom(Zoom::Summary, 140, 24)); }
#[test] fn zoom_detail_frame() { insta::assert_snapshot!(draw_zoom(Zoom::Detail, 100, 24)); }
#[test] fn zoom_detail_wide_frame() { insta::assert_snapshot!(draw_zoom(Zoom::Detail, 140, 24)); }
```

(`empty_economy()` already exists in this test file from P3; `ChatMsg` is already imported.) The snapshot is text-only (`to_string()` drops color), so it proves the Detail *structure* (header + collapsed body via Task 3b); the highlight *colors* are proven by the `detail_highlights_file_tool_results` unit test in Task 3 Step 1.

- [ ] **Step 7: Accept snapshots and verify fidelity**

Run: `cargo test -p zoid-tui --lib chat` (unit, must pass) then
Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`
Read the four new `.snap` files: Summary shows one `›`-prefixed digest line per turn with `~ Nt · Nf`; Detail shows the tool-result file body highlighted. Re-run without the env var:
Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/chat.rs crates/zoid-tui/src/render.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/examples/preview.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): zoom altitude render — Summary digests, code-aware Detail; ChatView (①+Ⓡ3)"
```

---

### Task 3b: Detail "collapse to signatures" — fold leaf bodies (Ⓡ3↔① compounding)

Spec §6.4 Ⓡ3 names **"code-aware semantic zoom (collapse to signatures)"** as an explicit deliverable, and P4a hands the substrate (`fold_regions`, now covering function **and** type/impl/trait bodies) to P4c. Task 3 only full-highlights file bodies; this task collapses them so a long file shows its *structure* at Detail altitude, not 500 lines. It swaps one line in `detail_lines` and adds the collapse helper + a new ellipsis token.

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs` (add `glyph::ELLIPSIS` + test assertion)
- Modify: `crates/zoid-tui/src/chat.rs` (`collapse_to_signatures` helper; `detail_lines` calls it)
- Modify: `crates/zoid-tui/tests/snapshots/` (re-accept the two `zoom_detail` snapshots — bodies now collapse)
- Test: inline unit test in `chat.rs`.

**Interfaces:**
- Consumes: `zoid_syntax::{fold_regions, FoldRegion, Language}` (P4a), `crate::syntax_view::highlight_lines`.
- Produces: `pub(crate) fn collapse_to_signatures(source: &str, lang: Language) -> Vec<Line<'static>>`.

- [ ] **Step 1: Add the ellipsis token**

In `crates/zoid-tui/src/tokens.rs`, add to `mod glyph` (no existing token is an ellipsis — `COLLAPSED` is a `▸` disclosure triangle):

```rust
    pub const ELLIPSIS: char = '…';     // collapsed-body marker (① collapse-to-signatures)
```

And in the `tokens` test module (alongside the other `assert_eq!(glyph::…)` checks), add:

```rust
    assert_eq!(glyph::ELLIPSIS, '…');
```

> Also add a row to the `docs/ux/README.md` glyph table for `…` (collapsed body) to keep §16's authoritative table in sync.

- [ ] **Step 2: Write the failing test**

In `crates/zoid-tui/src/chat.rs` `mod tests` (the `seeded()` there has a multi-line `fn parse` body):

```rust
    #[test]
    fn detail_collapses_function_bodies_to_signatures() {
        use crate::tokens::glyph;
        let lines = conversation_view(&seeded(), &view(Zoom::Detail), false);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        assert!(text.iter().any(|t| t.contains("fn parse")), "signature line is kept");
        assert!(text.iter().any(|t| t.contains(glyph::ELLIPSIS)), "body collapses to …");
        assert!(!text.iter().any(|t| t.contains("let n = 42")), "body interior is elided");
    }
```

Run: `cargo test -p zoid-tui --lib chat::tests::detail_collapses_function_bodies_to_signatures`
Expected: FAIL — `collapse_to_signatures` not yet wired (full body still rendered).

- [ ] **Step 3: Implement `collapse_to_signatures` and wire it**

In `crates/zoid-tui/src/chat.rs`, add `use zoid_syntax::{fold_regions, FoldRegion};` (next to the existing `Language`/`highlight_lines` imports) and the helper:

```rust
/// Collapse a code file to signatures: highlight every line, but replace each
/// **leaf** fold body's interior lines with a single `…` marker. "Leaf" = a fold
/// containing no other fold, so a container (`impl`/`mod`) keeps its method
/// signatures while each method/struct/enum leaf body folds. Realizes spec Ⓡ3↔①
/// "collapse to signatures"; uses P4a's `fold_regions` (function + type/impl bodies).
pub(crate) fn collapse_to_signatures(source: &str, lang: Language) -> Vec<Line<'static>> {
    let all = highlight_lines(source, lang); // one Line per source line
    let folds = fold_regions(source, lang);
    if folds.is_empty() {
        return all;
    }
    // 0-based line index of a byte offset = count of '\n' before it.
    let line_of = |byte: usize| {
        source[..byte.min(source.len())].bytes().filter(|&b| b == b'\n').count()
    };
    let is_leaf = |f: &FoldRegion, i: usize| {
        !folds
            .iter()
            .enumerate()
            .any(|(j, g)| j != i && g.start >= f.start && g.end <= f.end)
    };
    let mut elided = vec![false; all.len()];
    for (i, f) in folds.iter().enumerate() {
        if is_leaf(f, i) {
            // Keep the opening (signature) line and the closing line; elide between.
            for ln in (line_of(f.start) + 1)..line_of(f.end) {
                if ln < elided.len() {
                    elided[ln] = true;
                }
            }
        }
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    while i < all.len() {
        if elided[i] {
            out.push(Line::from(Span::styled(
                format!("    {}", glyph::ELLIPSIS),
                Style::new().fg(color::DIM),
            )));
            while i < all.len() && elided[i] {
                i += 1;
            }
        } else {
            out.push(all[i].clone());
            i += 1;
        }
    }
    out
}
```

Then, in `detail_lines`, change the file-body render from full highlight to collapsed:

```rust
                let lang = id_path.get(id.as_str()).map(|p| Language::from_path(p)).unwrap_or(Language::PlainText);
                out.extend(collapse_to_signatures(output, lang));
```

(was `out.extend(highlight_lines(output, lang));`). `detail_highlights_file_tool_results` (Task 3) still passes — the `fn parse` signature line keeps its keyword color; only the body interior is elided.

Run: `cargo test -p zoid-tui --lib chat`
Expected: PASS (highlight + collapse tests).

- [ ] **Step 4: Re-accept the Detail snapshots**

The `zoom_detail_frame`/`zoom_detail_wide_frame` snapshots now show the collapsed body. Re-accept and verify:

Run: `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`
Read the two `zoom_detail` `.snap` files: the `fn parse(...) {` signature and `}` remain; the two interior lines are replaced by one `    …`. Re-run without the env var:
Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/chat.rs crates/zoid-tui/tests/snapshots/ docs/ux/README.md
git commit -m "feat(tui): Detail collapse-to-signatures via leaf folds (Ⓡ3↔① spec deliverable)"
```

---

### Task 4: Routing — `Ctrl-scroll` + keys → `ZoomIn`/`ZoomOut`

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (`Action` variants + key/mouse routing)
- Modify: `crates/zoid/src/main.rs` (`handle_action` arms + assemble `ChatView`)
- Test: inline route tests.

**Interfaces:**
- Consumes: `state::Zoom`, `ShellState::{zoom_in, zoom_out}`.
- Produces: `Action::ZoomIn`, `Action::ZoomOut`. Keys (Conversation focus): `=`/`+` → `ZoomIn`, `-`/`_` → `ZoomOut`. Mouse: `Ctrl`+`ScrollUp` → `ZoomIn`, `Ctrl`+`ScrollDown` → `ZoomOut` (plain scroll still scrolls the conversation).

- [ ] **Step 1: Write the failing route tests**

In `crates/zoid-tui/src/route.rs` `mod tests`:

```rust
#[test]
fn zoom_keys_route_in_conversation_focus() {
    let mut s = ShellState::new();
    s.focus = Focus::Conversation;
    assert_eq!(route_key(&s, key(KeyCode::Char('='), KeyModifiers::NONE)), Action::ZoomIn);
    assert_eq!(route_key(&s, key(KeyCode::Char('+'), KeyModifiers::NONE)), Action::ZoomIn);
    assert_eq!(route_key(&s, key(KeyCode::Char('-'), KeyModifiers::NONE)), Action::ZoomOut);
}

#[test]
fn ctrl_scroll_zooms_plain_scroll_scrolls() {
    let s = ShellState::new();
    let l = compute(Rect { x: 0, y: 0, width: 100, height: 24 }, &s);
    let ev = |kind, mods| MouseEvent { kind, column: 10, row: 10, modifiers: mods };
    // ctrl + scroll → zoom
    assert_eq!(route_mouse(&s, &l, ev(MouseEventKind::ScrollUp, KeyModifiers::CONTROL)), Action::ZoomIn);
    assert_eq!(route_mouse(&s, &l, ev(MouseEventKind::ScrollDown, KeyModifiers::CONTROL)), Action::ZoomOut);
    // plain scroll → conversation scroll (unchanged)
    assert_eq!(route_mouse(&s, &l, ev(MouseEventKind::ScrollDown, KeyModifiers::NONE)), Action::ScrollConversation(1));
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib route::tests::zoom`
Expected: FAIL — `Action::ZoomIn`/`ZoomOut` undefined.

- [ ] **Step 3: Implement**

In `crates/zoid-tui/src/route.rs`, add to `enum Action`:

```rust
    ZoomIn,
    ZoomOut,
```

In `route_key`, in the `Focus::Conversation` match arm, add zoom keys **before** the `j/k` scroll arms:

```rust
        Focus::Conversation => match key.code {
            KeyCode::Char('=') | KeyCode::Char('+') => Action::ZoomIn,
            KeyCode::Char('-') | KeyCode::Char('_') => Action::ZoomOut,
            KeyCode::Char(':') => Action::OpenCommandLine,
            KeyCode::Char('j') | KeyCode::Down => Action::ScrollConversation(1),
            KeyCode::Char('k') | KeyCode::Up => Action::ScrollConversation(-1),
            KeyCode::Esc => Action::FocusRegion(Focus::Input),
            _ => Action::Noop,
        },
```

In `route_mouse`, handle `Ctrl`+scroll **before** the plain-scroll arms (after the overlay-dismiss block):

```rust
    match m.kind {
        MouseEventKind::ScrollUp if m.modifiers.contains(KeyModifiers::CONTROL) => Action::ZoomIn,
        MouseEventKind::ScrollDown if m.modifiers.contains(KeyModifiers::CONTROL) => Action::ZoomOut,
        MouseEventKind::ScrollDown => Action::ScrollConversation(1),
        MouseEventKind::ScrollUp => Action::ScrollConversation(-1),
        MouseEventKind::Down(MouseButton::Left) => match hit_test(layout, m.column, m.row) {
            Target::DrawerHeader(id) => Action::ToggleDrawer(id),
            Target::Input => Action::FocusRegion(Focus::Input),
            Target::Conversation => Action::FocusRegion(Focus::Conversation),
            Target::None => Action::Noop,
        },
        _ => Action::Noop,
    }
```

- [ ] **Step 4: Wire the bin**

In `crates/zoid/src/main.rs` `handle_action`, add arms (near `ScrollConversation`):

```rust
        Action::ZoomIn => app.shell.zoom_in(),
        Action::ZoomOut => app.shell.zoom_out(),
```

And assemble a `ChatView` in the draw closure (replacing the bare `caret` bool passed in P4b):

```rust
            let elapsed = app.started.elapsed().as_millis() as u64;
            let caret = zoid_tui::motion::caret_on(elapsed, 1000, app.shell.reduced_motion);
            let view = zoid_tui::chat::ChatView { zoom: app.shell.zoom, caret_on: caret, reveal: None };
            render_shell(f, &app.shell, &economy, &msgs, &app.textarea, app.streaming, &view);
```

- [ ] **Step 5: Run to confirm pass**

Run: `cargo test -p zoid-tui --lib route && cargo build -p zoid`
Expected: PASS / clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/route.rs crates/zoid/src/main.rs
git commit -m "feat(tui): zoom routing — Ctrl-scroll + =/- keys → ZoomIn/ZoomOut; bin assembles ChatView (①)"
```

---

### Task 5: Animated fold/unfold transition (motion, P4b)

**Files:**
- Modify: `crates/zoid-tui/src/motion.rs` (`reveal_count`)
- Modify: `crates/zoid/src/main.rs` (track zoom-change instant; drive `reveal`)
- Test: inline `reveal_count` test (motion frames are not snapshot-coverable, spec §13).

**Interfaces:**
- Consumes: `motion::{ease_out_cubic}`, `ShellState.reduced_motion`, `ChatView.reveal`.
- Produces:
  - `fn reveal_count(total: usize, t: f32) -> usize` — eased line count for a fold/unfold; `t<=0 → 0`, `t>=1 → total`.
  - `fn zoom_reveal(total: usize, elapsed_ms: u64, anim_ms: u64, reduced_motion: bool) -> Option<usize>` — the **pure** gate: `None` (no cap, final frame) when `reduced_motion`, `anim_ms == 0`, or `elapsed_ms >= anim_ms`; else `Some(reveal_count(total, elapsed/anim))`. Extracting this keeps the "should we animate / reduced-motion ⇒ instant" decision out of the impure draw closure so it is unit-testable (the determinism gap §13 warns about).
  - Zoom changes animate by revealing conversation lines top-down over `ZOOM_ANIM_MS`; reduced-motion shows all lines instantly.

> A cell buffer cannot cross-fade, so the "fold/unfold animates" requirement (spec §6.2) is realized as a **progressive top-down line reveal** eased by `ease_out_cubic` — visible, cheap, and reusing the P4b tick loop. Reduced-motion resolves it to the final frame immediately (spec §13).

- [ ] **Step 1: Write the failing test**

In `crates/zoid-tui/src/motion.rs` `mod tests`:

```rust
#[test]
fn reveal_count_eases_from_zero_to_total() {
    assert_eq!(reveal_count(10, 0.0), 0);
    assert_eq!(reveal_count(10, 1.0), 10);
    assert_eq!(reveal_count(10, -0.5), 0);  // clamped
    assert_eq!(reveal_count(10, 2.0), 10);  // clamped
    // monotonic non-decreasing
    let mut prev = 0;
    for i in 0..=10 {
        let c = reveal_count(10, i as f32 / 10.0);
        assert!(c >= prev);
        prev = c;
    }
    assert_eq!(reveal_count(0, 0.5), 0); // empty stays empty
}

#[test]
fn zoom_reveal_gates_on_motion_and_completion() {
    // mid-animation → a capped count; the reduced-motion and completion cases
    // resolve to None (no cap = final frame). This is the determinism the inline
    // draw-closure logic couldn't be tested for.
    assert_eq!(zoom_reveal(10, 0, 160, false), Some(0));
    assert_eq!(zoom_reveal(10, 80, 160, false), Some(reveal_count(10, 0.5)));
    assert_eq!(zoom_reveal(10, 160, 160, false), None); // animation complete
    assert_eq!(zoom_reveal(10, 200, 160, false), None); // past the end
    assert_eq!(zoom_reveal(10, 80, 160, true), None);   // reduced-motion → instant final frame
    assert_eq!(zoom_reveal(10, 80, 0, false), None);    // zero duration never divides
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib motion::tests`
Expected: FAIL — `reveal_count`/`zoom_reveal` undefined.

- [ ] **Step 3: Implement `reveal_count`**

In `crates/zoid-tui/src/motion.rs`:

```rust
/// Number of lines to show at eased progress `t` of a fold/unfold reveal.
pub fn reveal_count(total: usize, t: f32) -> usize {
    let eased = ease_out_cubic(t);
    ((total as f32) * eased).round() as usize
}

/// Pure reveal gate: `Some(cap)` while a zoom animation is mid-flight, `None`
/// when no cap should apply (reduced-motion, zero duration, or finished — i.e.
/// show the final frame). Keeps the animate-or-not decision out of the bin's
/// impure draw closure so it is unit-testable (spec §13 determinism).
pub fn zoom_reveal(total: usize, elapsed_ms: u64, anim_ms: u64, reduced_motion: bool) -> Option<usize> {
    if reduced_motion || anim_ms == 0 || elapsed_ms >= anim_ms {
        return None;
    }
    Some(reveal_count(total, elapsed_ms as f32 / anim_ms as f32))
}
```

Add both to the `lib.rs` re-export list: `pub use motion::{caret_on, ease_out_cubic, reveal_count, zoom_reveal, Anim, MOTION_FPS};`.

- [ ] **Step 4: Drive the reveal from the bin clock**

In `crates/zoid/src/main.rs`, add a field to `App`:

```rust
    /// When the altitude last changed, for the fold/unfold reveal (Ⓡ2).
    zoom_changed_at: Option<std::time::Instant>,
```

Initialize `zoom_changed_at: None,`. Set it in the zoom arms:

```rust
        Action::ZoomIn => { app.shell.zoom_in(); app.zoom_changed_at = Some(std::time::Instant::now()); }
        Action::ZoomOut => { app.shell.zoom_out(); app.zoom_changed_at = Some(std::time::Instant::now()); }
```

Add a const near the top of `main.rs`: `const ZOOM_ANIM_MS: u64 = 160;`.

In the draw closure, compute `reveal` from the zoom-change clock and the current altitude's line count:

```rust
            // Measure total lines (which re-runs conversation_view — tree-sitter in
            // Detail) ONLY while a zoom animation is actually in flight; on every
            // ordinary frame `reveal` is None and we skip the second build entirely.
            let reveal = match app.zoom_changed_at {
                Some(t0) if zoom_animating(&app) => {
                    let total_lines = zoid_tui::chat::conversation_view(
                        &msgs,
                        &zoid_tui::chat::ChatView { zoom: app.shell.zoom, caret_on: caret, reveal: None },
                        app.streaming,
                    )
                    .len();
                    zoid_tui::motion::zoom_reveal(
                        total_lines,
                        t0.elapsed().as_millis() as u64,
                        ZOOM_ANIM_MS,
                        app.shell.reduced_motion,
                    )
                }
                _ => None,
            };
            let view = zoid_tui::chat::ChatView { zoom: app.shell.zoom, caret_on: caret, reveal };
            render_shell(f, &app.shell, &economy, &msgs, &app.textarea, app.streaming, &view);
```

Extend the motion-tick guard (P4b) so it also fires during a zoom animation:

```rust
            _ = motion_tick.tick(), if app.streaming || zoom_animating(&app) => { }
```

and add a small helper:

```rust
fn zoom_animating(app: &App) -> bool {
    matches!(app.zoom_changed_at, Some(t0) if t0.elapsed().as_millis() < ZOOM_ANIM_MS as u128)
        && !app.shell.reduced_motion
}
```

- [ ] **Step 5: Add preview scenes + verify**

In `crates/zoid-tui/examples/preview.rs`, add `summary` and `detail` scenes that render the seeded conversation at those altitudes (build the matching `ChatView`).

Run: `cargo test --workspace && cargo clippy --all-targets`
Expected: PASS, zero warnings.

Manual (motion not snapshot-coverable, spec §13):
- `cargo run -p zoid` → `Ctrl-scroll` (or focus the conversation with Tab and press `-`/`=`) flips altitude; lines reveal top-down over ~160 ms.
- `ZOOM_ANIM_MS` reveal completes and the tick loop goes idle.
- `ZOID_REDUCED_MOTION=1 cargo run -p zoid` → altitude flips **instantly** (no reveal).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/motion.rs crates/zoid-tui/src/lib.rs crates/zoid/src/main.rs crates/zoid-tui/examples/preview.rs
git commit -m "feat(zoid): animated zoom fold/unfold via reveal_count + motion tick (①+Ⓡ2)"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] `cargo run -p zoid-tui --example preview -- summary 100 24` / `-- detail 140 24` render the two non-Normal altitudes.
- [ ] Zoom projection is pure (no ratatui in `zoid-core/src/zoom.rs`) and proptest-covered.
- [ ] Summary & Detail snapshots exist at both 100 and 140.
- [ ] Detail highlights file tool-results via `zoid-syntax` (Ⓡ3 compounds with ①) — proven by the `detail_highlights_file_tool_results` unit test (keyword carries `SYN_KEYWORD`), not just a color-blind text snapshot.
- [ ] Detail **collapses leaf bodies to signatures** (T3b): `detail_collapses_function_bodies_to_signatures` asserts the signature is kept, the body interior elided, and the `…` marker present.
- [ ] Transition animation has **no** frame snapshots (spec §13); `reveal_count` and the pure `zoom_reveal` gate are unit-tested and reduced-motion shows the final frame instantly.

## Self-Review notes (author)

- **Spec coverage (①):** altitude control collapsing/expanding the transcript by meaning — `Zoom` enum + `zoom_in/out` (T2), structural digests (T1), altitude render (T3), Detail **collapse-to-signatures** (T3b), `Ctrl-scroll`/keys (T4), animated fold/unfold (T5). **Structural** summaries per the 2026-06-30 decision (no LLM). Code-aware highlight in Detail (T3) **and** "collapse to signatures" — leaf fold bodies elided to `…` (T3b, consuming P4a's broadened `fold_regions`) — together realize the spec §6.4 Ⓡ3↔① "code-aware semantic zoom (collapse to signatures)" deliverable. (Earlier drafts claimed this under T3 alone; it is genuinely delivered by T3b.)
- **Type consistency:** `ChatView { zoom, caret_on, reveal }` (T3) is the single render param from T3 onward; it absorbs P4b's `caret_on` bool. `conversation_view` wraps `conversation_lines` (Normal) — never reimplements it (DRY). `digests`/`TurnDigest` (T1) consumed by `digest_lines` (T3). `Action::ZoomIn/ZoomOut` (T4) map to `ShellState::zoom_in/zoom_out` (T2). `reveal_count` (T5) feeds `ChatView.reveal` (T3).
- **Dependencies:** consumes P4a (`highlight_lines`/`Language`/`fold_regions` for Detail + collapse) and P4b (`motion::{ease_out_cubic, caret_on}` + the tick loop). `reveal_count` and the pure `zoom_reveal` gate are **defined by this plan** (T5, added into P4b's `motion.rs` and re-export), not consumed from P4b. Sequencing P4a→P4b→P4c is required.
- **§13 honored:** zoom *content* is snapshot-tested (Summary/Detail @100/@140); the *transition* is not — verified by `reveal_count`/`zoom_reveal` unit tests (incl. reduced-motion ⇒ final frame) + manual/gif.
