# P4d · ④ Object-First Verbs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Invert prose-first chat for the common case — let the user pick an **object** in the session (a file, an error, a tree-sitter symbol) and choose an **agent verb scoped to it** from a menu, composing the right prompt automatically.

**Architecture:** A pure object model (`crates/zoid-tui/src/objects.rs`) extracts selectable `Obj`s from the `ChatMsg` stream — files and errors from tool results, **symbols via `zoid-syntax` (P4a)** from file contents. A pure verb table maps each object kind to scoped verbs and composes the prompt. The UI reuses the proven palette/overlay infrastructure as a **two-step picker**: `^O` opens an object overlay → pick an object → a verb overlay → pick a verb. Per the 2026-06-30 decision, a chosen verb is **queued, not dispatched**: it composes the scoped prompt into the input box (ready to send) with a "queued for P5" hint; the actual subagent dispatch lands in **P5**.

**Tech Stack:** Rust 2021, ratatui 0.29 (`TestBackend`/`insta`). **Depends on P4a** (`zoid-syntax` symbols). Independent of P4b/P4c (no motion/zoom coupling).

## Global Constraints

- **Crates & dep direction:** the object model + verb table are **pure** functions in `crates/zoid-tui/src/objects.rs` (consume `ChatMsg` + `zoid-syntax`; no clocks, no `Frame`). Overlay state on `ShellState`; routing in `route.rs`; rendering in `render.rs`; the queue side-effect in the `zoid` bin.
- **Verbs are QUEUED, not dispatched (P4d scope decision, 2026-06-30):** picking a verb composes the scoped prompt into the input box (the "copies prompt" behavior) and shows a transient "queued · runs as a subagent in P5" hint. **No event is recorded, no agent turn is spawned, no subagent runtime is touched** — that is P5's job. This keeps P4d a pure-UI feature.
- **Object kinds in scope:** **File, Error, Symbol**. Diff-hunk and test objects are deferred (no diff drawer / test-detection exists yet) — do not add them.
- **Reuse, don't reinvent (DRY):** the object and verb overlays render through a shared list-overlay helper modeled on the existing `render_palette`; navigation reuses `palette::nav`. Do not fork a parallel palette implementation.
- **Design tokens (spec §16):** any new glyph/color is a token in `tokens.rs` mirrored in `docs/ux/README.md`. Reuse existing glyphs (`glyph::OPEN ▤` for files, `glyph::WARNING ⚠` for errors, `glyph::EDIT ●` or `glyph::RECIPE ▷` for symbols/verbs). ASCII punctuation exempt.
- **UX testing is multi-width:** the object overlay and verb overlay each add `TestBackend`+`insta` snapshots at **both 100×24 and 140×24**, plus `preview.rs` scenes. The object/verb pure functions have unit tests.
- **TDD, DRY, YAGNI, frequent commits. No `Co-Authored-By` / co-author trailer** (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit. Accept snapshots with `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot` and review `.snap` fidelity before committing.

---

### Task 1: Object model — `ObjectKind`, `Obj`, `selectable_objects()`

**Files:**
- Create: `crates/zoid-tui/src/objects.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (`pub mod objects;`)
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: `zoid_core::projection::ChatMsg`, `zoid_syntax::{symbols, Language}` (P4a).
- Produces:
  - `enum ObjectKind { File, Error, Symbol }` (`Debug, Clone, Copy, PartialEq, Eq`).
  - `struct Obj { pub kind: ObjectKind, pub label: String, pub target: String, pub context: String }` (`Debug, Clone, PartialEq, Eq`). `label` is the menu display; `target` is the prompt subject (path / error text / symbol name); `context` is the owning file (for symbols) or empty.
  - `fn selectable_objects(msgs: &[ChatMsg]) -> Vec<Obj>` — files (from file tool-results, newest content wins per path), errors (from errored tool results), and symbols (extracted from each file result via `zoid-syntax`). Deterministic order: files, then symbols, then errors.

- [ ] **Step 1: Write the failing tests**

`crates/zoid-tui/src/objects.rs` (test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::projection::{ChatMsg, ToolCallRef};

    fn call(id: &str, args: &str) -> ToolCallRef {
        ToolCallRef { id: id.into(), name: "read_file".into(), args: args.into() }
    }

    fn seeded() -> Vec<ChatMsg> {
        vec![
            ChatMsg::User("read the ast".into()),
            ChatMsg::Assistant { text: String::new(), tool_calls: vec![call("c1", r#"{"path":"src/ast.rs"}"#)] },
            ChatMsg::ToolResult { id: "c1".into(), name: "read_file".into(), output: "fn parse() {}\nstruct Ast {}\n".into(), is_error: false },
            ChatMsg::Assistant { text: String::new(), tool_calls: vec![ToolCallRef { id: "c2".into(), name: "shell".into(), args: r#"{"command":"cargo test"}"#.into() }] },
            ChatMsg::ToolResult { id: "c2".into(), name: "shell".into(), output: "FAILED\n[exit 1]".into(), is_error: true },
        ]
    }

    #[test]
    fn extracts_file_symbol_and_error_objects() {
        let objs = selectable_objects(&seeded());
        // a File object for src/ast.rs
        assert!(objs.iter().any(|o| o.kind == ObjectKind::File && o.target == "src/ast.rs"));
        // Symbol objects parse, scoped to the file
        assert!(objs.iter().any(|o| o.kind == ObjectKind::Symbol && o.target == "parse" && o.context == "src/ast.rs"));
        assert!(objs.iter().any(|o| o.kind == ObjectKind::Symbol && o.target == "Ast"));
        // an Error object for the failed shell call
        assert!(objs.iter().any(|o| o.kind == ObjectKind::Error));
    }

    #[test]
    fn empty_conversation_has_no_objects() {
        assert_eq!(selectable_objects(&[]), Vec::new());
    }

    #[test]
    fn non_file_tool_results_make_no_file_object() {
        let msgs = vec![
            ChatMsg::Assistant { text: String::new(), tool_calls: vec![ToolCallRef { id: "c1".into(), name: "shell".into(), args: r#"{"command":"ls"}"#.into() }] },
            ChatMsg::ToolResult { id: "c1".into(), name: "shell".into(), output: "a\nb".into(), is_error: false },
        ];
        let objs = selectable_objects(&msgs);
        assert!(objs.iter().all(|o| o.kind != ObjectKind::File));
    }

    #[test]
    fn rereading_a_file_does_not_duplicate_symbols() {
        // read → (edit) → re-read of the same path: one File object, and symbols
        // come once from the LATEST content (not first+latest stacked).
        let msgs = vec![
            ChatMsg::Assistant { text: String::new(), tool_calls: vec![call("c1", r#"{"path":"src/ast.rs"}"#)] },
            ChatMsg::ToolResult { id: "c1".into(), name: "read_file".into(), output: "fn old() {}\n".into(), is_error: false },
            ChatMsg::Assistant { text: String::new(), tool_calls: vec![call("c2", r#"{"path":"src/ast.rs"}"#)] },
            ChatMsg::ToolResult { id: "c2".into(), name: "read_file".into(), output: "fn renamed() {}\n".into(), is_error: false },
        ];
        let objs = selectable_objects(&msgs);
        assert_eq!(objs.iter().filter(|o| o.kind == ObjectKind::File).count(), 1, "one File per path");
        let syms: Vec<&str> = objs.iter().filter(|o| o.kind == ObjectKind::Symbol).map(|o| o.target.as_str()).collect();
        assert_eq!(syms, vec!["renamed"], "symbols come once, from the latest read");
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib objects`
Expected: compile error — module/types undefined.

- [ ] **Step 3: Implement**

`crates/zoid-tui/src/objects.rs`:

```rust
//! Object-first selection model (spec ④). Pure extraction of selectable
//! objects — files, tree-sitter symbols (via zoid-syntax, P4a), and errors —
//! from the conversation. `zoid-tui` renders these into a picker; the verb
//! table (Task 2) maps each to scoped agent verbs.

use std::collections::HashMap;
use zoid_core::projection::ChatMsg;
use zoid_syntax::{symbols, Language};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    File,
    Error,
    Symbol,
}

/// A selectable object. `target` is the prompt subject; `context` names the
/// owning file for symbols (empty otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obj {
    pub kind: ObjectKind,
    pub label: String,
    pub target: String,
    pub context: String,
}

/// Pull a file path out of a tool call's JSON args.
fn path_arg(args_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args_json).ok()?;
    for key in ["path", "file_path", "file"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Files, symbols, then errors — deterministic and de-duplicated by path.
pub fn selectable_objects(msgs: &[ChatMsg]) -> Vec<Obj> {
    // id → path, from assistant tool calls.
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

    let mut files: Vec<Obj> = Vec::new();
    let mut errors: Vec<Obj> = Vec::new();
    let mut seen_paths: Vec<String> = Vec::new();
    // Latest tool-result body per path. Symbols are extracted from this AFTER the
    // pass — emitting them inside the loop would duplicate (and mix stale + fresh)
    // symbols whenever a file is read more than once (the read→edit→re-read cycle).
    let mut latest_output: HashMap<String, String> = HashMap::new();

    for m in msgs {
        if let ChatMsg::ToolResult { id, name, output, is_error } = m {
            if *is_error {
                errors.push(Obj {
                    kind: ObjectKind::Error,
                    label: format!("error: {name}"),
                    target: output.lines().next().unwrap_or("").to_string(),
                    context: String::new(),
                });
                continue;
            }
            if let Some(path) = id_path.get(id.as_str()) {
                if !seen_paths.contains(path) {
                    seen_paths.push(path.clone());
                    files.push(Obj {
                        kind: ObjectKind::File,
                        label: path.clone(),
                        target: path.clone(),
                        context: String::new(),
                    });
                }
                // newest content wins per path.
                latest_output.insert(path.clone(), output.clone());
            }
        }
    }

    // Symbols once per unique path, in file (first-seen) order, from latest content.
    let mut syms: Vec<Obj> = Vec::new();
    for path in &seen_paths {
        if let Some(output) = latest_output.get(path) {
            for s in symbols(output, Language::from_path(path)) {
                syms.push(Obj {
                    kind: ObjectKind::Symbol,
                    label: format!("{}  ({path})", s.name),
                    target: s.name,
                    context: path.clone(),
                });
            }
        }
    }

    files.into_iter().chain(syms).chain(errors).collect()
}
```

In `crates/zoid-tui/src/lib.rs`: add `pub mod objects;` and `pub use objects::{selectable_objects, Obj, ObjectKind};`.

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-tui --lib objects`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/objects.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): object model — selectable files/symbols/errors from conversation (④)"
```

---

### Task 2: Verb table + scoped prompt composer

**Files:**
- Modify: `crates/zoid-tui/src/objects.rs`
- Test: inline `mod tests`.

**Interfaces:**
- Consumes: `ObjectKind`, `Obj`.
- Produces:
  - `fn verbs_for(kind: ObjectKind) -> &'static [&'static str]` — File → `["explain", "summarize", "find usages"]`; Symbol → `["explain", "find references", "add test"]`; Error → `["explain", "fix"]`.
  - `fn verb_prompt(verb: &str, obj: &Obj) -> String` — composes the scoped prompt.

- [ ] **Step 1: Write the failing tests**

In `crates/zoid-tui/src/objects.rs` `mod tests`:

```rust
#[test]
fn verbs_are_scoped_to_kind() {
    assert!(verbs_for(ObjectKind::Error).contains(&"fix"));
    assert!(verbs_for(ObjectKind::Symbol).contains(&"add test"));
    assert!(verbs_for(ObjectKind::File).contains(&"explain"));
}

#[test]
fn verb_prompt_scopes_to_the_object() {
    let sym = Obj { kind: ObjectKind::Symbol, label: "parse  (src/ast.rs)".into(), target: "parse".into(), context: "src/ast.rs".into() };
    let p = verb_prompt("explain", &sym);
    assert!(p.contains("parse"));
    assert!(p.contains("src/ast.rs"));

    let file = Obj { kind: ObjectKind::File, label: "src/ast.rs".into(), target: "src/ast.rs".into(), context: String::new() };
    assert!(verb_prompt("summarize", &file).contains("src/ast.rs"));

    let err = Obj { kind: ObjectKind::Error, label: "error: shell".into(), target: "FAILED".into(), context: String::new() };
    assert!(verb_prompt("fix", &err).to_lowercase().contains("fix"));
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib objects::tests::verb`
Expected: FAIL — `verbs_for`/`verb_prompt` undefined.

- [ ] **Step 3: Implement**

In `crates/zoid-tui/src/objects.rs`:

```rust
/// Agent verbs scoped to an object kind (spec ④).
pub fn verbs_for(kind: ObjectKind) -> &'static [&'static str] {
    match kind {
        ObjectKind::File => &["explain", "summarize", "find usages"],
        ObjectKind::Symbol => &["explain", "find references", "add test"],
        ObjectKind::Error => &["explain", "fix"],
    }
}

/// Compose the scoped prompt a verb would run against an object. In P4d this
/// text is placed in the input box (queued); P5 dispatches it to a subagent.
pub fn verb_prompt(verb: &str, obj: &Obj) -> String {
    match obj.kind {
        ObjectKind::File => format!("{verb} the file `{}`", obj.target),
        ObjectKind::Symbol => format!("{verb} `{}` in `{}`", obj.target, obj.context),
        ObjectKind::Error => format!("{verb} this error: {}", obj.target),
    }
}
```

- [ ] **Step 4: Run to confirm pass**

Run: `cargo test -p zoid-tui --lib objects`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/objects.rs
git commit -m "feat(tui): scoped verb table + prompt composer (④)"
```

---

### Task 3: Overlay state + routing + render (object picker → verb picker) + snapshots

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (`Overlay::Objects`/`Verbs`, `ObjectState`)
- Modify: `crates/zoid-tui/src/route.rs` (`Action` variants + routing)
- Modify: `crates/zoid-tui/src/layout.rs` (reserve the centered overlay rect for `Objects`/`Verbs`)
- Modify: `crates/zoid-tui/src/render.rs` (two overlays via a shared list helper)
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (object + verb overlay snapshots) and `examples/preview.rs`
- Test: inline route + layout tests + snapshots.

**Interfaces:**
- Consumes: `objects::{selectable_objects, verbs_for, Obj, ObjectKind}`, `palette::nav`.
- Produces:
  - `Overlay::Objects`, `Overlay::Verbs` variants.
  - `struct ObjectState { pub obj_selected: usize, pub verb_selected: usize }` (`Debug, Clone, Default, PartialEq, Eq`) + `ShellState.objects: ObjectState`.
  - `Action::OpenObjects, ObjectMove(i32), ObjectPick, VerbMove(i32), VerbPick`.
  - `close_overlay()` also resets `objects`.

- [ ] **Step 1: Write the failing route tests**

In `crates/zoid-tui/src/route.rs` `mod tests`:

```rust
#[test]
fn ctrl_o_opens_object_overlay() {
    let s = ShellState::new();
    assert_eq!(route_key(&s, key(KeyCode::Char('o'), KeyModifiers::CONTROL)), Action::OpenObjects);
}

#[test]
fn object_overlay_navigates_and_picks() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Objects;
    assert_eq!(route_key(&s, key(KeyCode::Down, KeyModifiers::NONE)), Action::ObjectMove(1));
    assert_eq!(route_key(&s, key(KeyCode::Up, KeyModifiers::NONE)), Action::ObjectMove(-1));
    assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::ObjectPick);
    assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::CloseOverlay);
}

#[test]
fn verb_overlay_navigates_and_picks() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Verbs;
    assert_eq!(route_key(&s, key(KeyCode::Down, KeyModifiers::NONE)), Action::VerbMove(1));
    assert_eq!(route_key(&s, key(KeyCode::Enter, KeyModifiers::NONE)), Action::VerbPick);
    // Esc steps BACK to the object picker, not all the way out.
    assert_eq!(route_key(&s, key(KeyCode::Esc, KeyModifiers::NONE)), Action::VerbBack);
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui --lib route::tests` (the `::object` prefix alone would skip `verb_overlay_navigates_and_picks` — run the whole `route::tests` module)
Expected: FAIL — variants/actions undefined.

- [ ] **Step 3: Implement state**

In `crates/zoid-tui/src/state.rs`:
- Add to `enum Overlay`: `Objects,` and `Verbs,`.
- Add the state struct (near `PaletteState`):

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectState {
    pub obj_selected: usize,
    pub verb_selected: usize,
}
```

- Add `pub objects: ObjectState,` to `ShellState`; initialize `objects: ObjectState::default(),` in `new()`.
- In `close_overlay`, also reset it: add `self.objects = ObjectState::default();`.

- [ ] **Step 4: Implement routing**

In `crates/zoid-tui/src/route.rs`:
- Add to `enum Action`: `OpenObjects, ObjectMove(i32), ObjectPick, VerbMove(i32), VerbPick, VerbBack,`.
- In `route_key`, extend the overlay-capture block at the top:

```rust
    match state.overlay {
        Overlay::Palette => return route_palette_key(key),
        Overlay::CommandLine => return route_cmdline_key(state, key),
        Overlay::Objects => return route_objects_key(key),
        Overlay::Verbs => return route_verbs_key(key),
        Overlay::None => {}
    }
```

- Add `^O` to the global combos (after the `^P` check):

```rust
    if ctrl(&key, 'o') {
        return Action::OpenObjects;
    }
```

- Add the two overlay routers:

```rust
fn route_objects_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::ObjectPick,
        KeyCode::Up => Action::ObjectMove(-1),
        KeyCode::Down => Action::ObjectMove(1),
        _ => Action::Noop,
    }
}

fn route_verbs_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::VerbBack, // step back to the object picker, not fully out
        KeyCode::Enter => Action::VerbPick,
        KeyCode::Up => Action::VerbMove(-1),
        KeyCode::Down => Action::VerbMove(1),
        _ => Action::Noop,
    }
}
```

- In `route_mouse`, the overlay-dismiss guard already returns `CloseOverlay` for any click/scroll when `state.overlay != Overlay::None` — that now covers Objects/Verbs too. No change needed.

Run: `cargo test -p zoid-tui --lib route::tests::object`
Expected: PASS.

- [ ] **Step 4b: Reserve the overlay rect in `layout.rs` (without this the picker is invisible)**

`layout::compute` only fills `layout.palette` when `state.overlay == Overlay::Palette`. The Step 5 render dispatch guards each new overlay with `if let Some(p) = layout.palette { … }`, so for `Objects`/`Verbs` that rect is `None` and **both render branches are dead code** — the picker draws nothing, and the Step 6 snapshots would silently bake a blank-overlay frame as the baseline. Widen the condition. In `crates/zoid-tui/src/layout.rs`, change:

```rust
    let palette = if state.overlay == Overlay::Palette {
        Some(centered(area, 72, 18))
    } else {
        None
    };
```

to:

```rust
    let palette = if matches!(state.overlay, Overlay::Palette | Overlay::Objects | Overlay::Verbs) {
        Some(centered(area, 72, 18))
    } else {
        None
    };
```

(The object/verb pickers reuse the same centered rect as the palette — same geometry, same hit area.) Add a test in `layout.rs` `mod tests` alongside `palette_rect_only_when_overlay_active`:

```rust
#[test]
fn overlay_rect_present_for_object_and_verb_pickers() {
    for ov in [Overlay::Objects, Overlay::Verbs] {
        let mut s = ShellState::new();
        s.overlay = ov;
        assert!(compute(area(100, 24), &s).palette.is_some(), "{ov:?} needs a rect");
    }
}
```

Run: `cargo test -p zoid-tui --lib layout`
Expected: PASS (the new test fails before the `matches!` change, passes after).

- [ ] **Step 5: Implement render (shared list overlay)**

In `crates/zoid-tui/src/render.rs`, add a generic list-overlay renderer and the two callers, and dispatch in `render_shell`'s overlay block.

Add to `render_shell`'s overlay section (after the CommandLine arm):

```rust
    } else if state.overlay == Overlay::Objects {
        if let Some(p) = layout.palette {
            render_object_overlay(frame, msgs, state, p);
        }
    } else if state.overlay == Overlay::Verbs {
        if let Some(p) = layout.palette {
            render_verb_overlay(frame, msgs, state, p);
        }
    }
```

Add the renderers (reusing the palette rect + a shared list helper):

```rust
fn list_overlay(frame: &mut Frame, area: Rect, title: String, rows: &[String], selected: usize) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(title, Style::new().fg(color::TXT)));
    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    frame.render_widget(block, area);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let style = if i == selected {
                Style::new().fg(color::TXT).bg(color::SEL_BG)
            } else {
                Style::new().fg(color::TXT)
            };
            Line::from(Span::styled(format!(" {r}"), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_object_overlay(frame: &mut Frame, msgs: &[ChatMsg], state: &ShellState, area: Rect) {
    use crate::objects::selectable_objects;
    let objs = selectable_objects(msgs);
    let sel = crate::palette::nav(state.objects.obj_selected, 0, objs.len());
    let rows: Vec<String> = objs.iter().map(object_row).collect();
    let rows = if rows.is_empty() { vec!["(no objects yet)".to_string()] } else { rows };
    list_overlay(frame, area, format!(" {} select object ", glyph::OPEN), &rows, sel);
}

fn render_verb_overlay(frame: &mut Frame, msgs: &[ChatMsg], state: &ShellState, area: Rect) {
    use crate::objects::{selectable_objects, verbs_for};
    let objs = selectable_objects(msgs);
    let sel_obj = crate::palette::nav(state.objects.obj_selected, 0, objs.len());
    let (title, rows) = match objs.get(sel_obj) {
        Some(o) => (
            format!(" {} verbs · {} ", glyph::RECIPE, o.label),
            verbs_for(o.kind).iter().map(|v| v.to_string()).collect::<Vec<_>>(),
        ),
        None => (" verbs ".to_string(), vec!["(no object)".to_string()]),
    };
    let sel = crate::palette::nav(state.objects.verb_selected, 0, rows.len());
    list_overlay(frame, area, title, &rows, sel);
}

fn object_row(o: &crate::objects::Obj) -> String {
    use crate::objects::ObjectKind;
    let g = match o.kind {
        ObjectKind::File => glyph::OPEN,
        ObjectKind::Symbol => glyph::EDIT,
        ObjectKind::Error => glyph::WARNING,
    };
    format!("{g} {}", o.label)
}
```

(Import what you need at the top of `render.rs`: `glyph` is already imported via `tokens`.)

- [ ] **Step 6: Add snapshots**

In `crates/zoid-tui/tests/shell_snapshot.rs`, add a seeded conversation with a file + error, then snapshots for both overlays @100/@140:

```rust
fn seeded_objects() -> Vec<ChatMsg> {
    use zoid_core::projection::ToolCallRef;
    vec![
        ChatMsg::Assistant { text: String::new(), tool_calls: vec![ToolCallRef { id: "c1".into(), name: "read_file".into(), args: r#"{"path":"src/ast.rs"}"#.into() }] },
        ChatMsg::ToolResult { id: "c1".into(), name: "read_file".into(), output: "fn parse() {}\nstruct Ast {}\n".into(), is_error: false },
        ChatMsg::ToolResult { id: "c2".into(), name: "shell".into(), output: "FAILED\n".into(), is_error: true },
    ]
}

fn draw_overlay(overlay: zoid_tui::Overlay, w: u16, h: u16) -> String {
    let mut s = ShellState::new();
    s.overlay = overlay;
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_shell(f, &s, &empty_economy(), &seeded_objects(), &input, false, &normal_view()))
        .unwrap();
    terminal.backend().to_string()
}

#[test] fn object_overlay_frame() { insta::assert_snapshot!(draw_overlay(zoid_tui::Overlay::Objects, 100, 24)); }
#[test] fn object_overlay_wide_frame() { insta::assert_snapshot!(draw_overlay(zoid_tui::Overlay::Objects, 140, 24)); }
#[test] fn verb_overlay_frame() { insta::assert_snapshot!(draw_overlay(zoid_tui::Overlay::Verbs, 100, 24)); }
#[test] fn verb_overlay_wide_frame() { insta::assert_snapshot!(draw_overlay(zoid_tui::Overlay::Verbs, 140, 24)); }
```

> `normal_view()` and `empty_economy()` come from P4c/P3 in this test file. If P4c is not yet merged on this branch, build the `ChatView`/economy inline. Export `Overlay` from `zoid_tui` if not already (`pub use state::Overlay`) — it is re-exported in `lib.rs`.

- [ ] **Step 6b: Add the preview scenes**

The Files list promises `examples/preview.rs` scenes; wire them. `preview.rs`'s `scene(name)` returns `(ShellState, Vec<ChatMsg>, EconomyView)` and `main()` renders via `render_shell`, which draws overlays straight from `state.overlay` — so a scene only needs to set the overlay and seed a conversation. Add a `seeded_objects()` fixture to `preview.rs` (same shape as the snapshot one) and two arms to `scene()`:

```rust
        "objects" => {
            s.overlay = Overlay::Objects;
            return (s, seeded_objects(), empty_economy());
        }
        "verbs" => {
            s.overlay = Overlay::Verbs;
            return (s, seeded_objects(), empty_economy());
        }
```

Update the module doc-comment scene list (line 7 of `preview.rs`) to include `objects, verbs`. (`Overlay` is already imported in `preview.rs`.)

Run: `cargo run -p zoid-tui --example preview -- objects 100 24` and `-- verbs 140 24` — confirm the picker overlay actually draws (this is the manual proof that the Step 4b layout-rect fix worked end-to-end).

- [ ] **Step 7: Accept snapshots and verify**

Run: `cargo test -p zoid-tui --lib && INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`
Read the four `.snap` files: the object overlay lists `▤ src/ast.rs`, `● parse …`, `● Ast …`, `⚠ error: shell`; the verb overlay lists the verbs for the selected object. Re-run without the env var:
Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/layout.rs crates/zoid-tui/src/render.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/examples/preview.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): object/verb picker overlays — ^O, nav, snapshots (④)"
```

---

### Task 4: Bin wiring — queue the verb (compose prompt into input; dispatch deferred to P5)

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (`status_hint` field on `ShellState`)
- Modify: `crates/zoid-tui/src/render.rs` (status bar shows `state.status_hint` when set)
- Modify: `crates/zoid/src/main.rs` (`handle_action` arms)
- Test: inline state test; manual UI flow; the pure tests already cover object/verb/prompt logic.

**Interfaces:**
- Consumes: `zoid_tui::objects::{selectable_objects, verbs_for, verb_prompt}`, `zoid_tui::route::Action`.
- Produces: choosing a verb places the composed prompt in the input box and sets `ShellState.status_hint`, a transient "queued · P5" hint the status bar renders. **No event recorded, no turn spawned.**

> Per the P4d decision, verbs are inert until P5. The most useful inert behavior ("copies prompt") is to seed the input box so the user can review/edit and send manually — P5 will instead dispatch it to a subagent automatically.

- [ ] **Step 1: Add `status_hint` to `ShellState` (not `App`)**

The hint must live on `ShellState`, because `render_shell`/the status renderer only ever receive `&ShellState` (and `&ChatView`) — never `&App`. A field on the bin's `App` would be invisible to the pure renderer. In `crates/zoid-tui/src/state.rs`, add to `ShellState` (near `overlay`):

```rust
    /// Transient one-line hint shown in the status bar (e.g. the ④ "queued · P5"
    /// notice). Cleared on the next submit/keypress by the bin.
    pub status_hint: Option<String>,
```

Initialize `status_hint: None,` in `new()`. Add a test in `mod tests`:

```rust
#[test]
fn new_has_no_status_hint() {
    assert!(ShellState::new().status_hint.is_none());
}
```

- [ ] **Step 2: Implement the action arms**

In `crates/zoid/src/main.rs` `handle_action`, add (near the palette arms):

```rust
        Action::OpenObjects => {
            app.shell.overlay = zoid_tui::Overlay::Objects;
            app.shell.objects = Default::default();
        }
        Action::ObjectMove(d) => {
            let n = zoid_tui::objects::selectable_objects(&conversation(&app.events)).len();
            app.shell.objects.obj_selected = zoid_tui::palette::nav(app.shell.objects.obj_selected, d, n);
        }
        Action::ObjectPick => {
            // Advance to the verb picker — but only if there's actually an object to
            // act on (otherwise the verb picker would show "(no object)").
            if !zoid_tui::objects::selectable_objects(&conversation(&app.events)).is_empty() {
                app.shell.overlay = zoid_tui::Overlay::Verbs;
                app.shell.objects.verb_selected = 0;
            }
        }
        Action::VerbBack => {
            // Step back to the object picker (keeps the object selection).
            app.shell.overlay = zoid_tui::Overlay::Objects;
        }
        Action::VerbMove(d) => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(&app.events));
            let sel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            let n = objs.get(sel).map(|o| zoid_tui::objects::verbs_for(o.kind).len()).unwrap_or(0);
            app.shell.objects.verb_selected = zoid_tui::palette::nav(app.shell.objects.verb_selected, d, n);
        }
        Action::VerbPick => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(&app.events));
            let osel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            if let Some(obj) = objs.get(osel) {
                let verbs = zoid_tui::objects::verbs_for(obj.kind);
                let vsel = zoid_tui::palette::nav(app.shell.objects.verb_selected, 0, verbs.len());
                if let Some(verb) = verbs.get(vsel) {
                    let prompt = zoid_tui::objects::verb_prompt(verb, obj);
                    // Queue (P4d): seed the input; P5 will dispatch to a subagent.
                    app.textarea = TextArea::from(prompt.lines().map(String::from).collect::<Vec<_>>());
                    app.shell.status_hint = Some("queued · runs as a subagent in P5".into());
                    app.shell.focus = zoid_tui::Focus::Input;
                }
            }
            app.shell.close_overlay();
        }
```

> Use the bin's actual `conversation` import (it already calls `conversation(&app.events)` in the draw loop). For seeding the input, `tui_textarea::TextArea::from(lines: Vec<String>)` builds a multi-line textarea (the codebase otherwise only uses `TextArea::default()`, so there's no in-repo precedent to copy — `from` is the documented tui-textarea constructor); if the bin wraps `TextArea` differently, match that.

- [ ] **Step 3: Render the hint from `ShellState`**

In `crates/zoid-tui/src/render.rs`, in the status-bar render (the bottom status line that already reads `state`), show `state.status_hint` when `Some` — one line, `color::DIM` (e.g. append it after the mode/branch segment). Because it lives on `ShellState`, the renderer can read it directly; no `App` plumbing.

In `crates/zoid/src/main.rs`, clear it once it has been seen: set `app.shell.status_hint = None` in the `Submit` path (and/or on the next non-verb keypress) so the hint is transient. Keep it minimal — clearing on Submit is enough for v1.

- [ ] **Step 4: Build and verify**

Run: `cargo build -p zoid && cargo test --workspace && cargo clippy --all-targets`
Expected: clean build, full suite green, zero warnings.

Manual:
- `cargo run -p zoid` → after a tool call/file read, press `^O` → object picker; arrow to a symbol; `Enter` → verb picker; arrow to "explain"; `Enter` → the input box now reads `explain \`parse\` in \`src/ast.rs\`` and a "queued · P5" hint shows. Pressing Enter sends it as a normal Chat turn (manual), proving the prompt is well-formed; P5 will instead auto-dispatch.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/render.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): object-verb wiring — pick → compose scoped prompt into input (queued for P5) (④)"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] `^O` opens the object picker; Enter advances to the verb picker; picking a verb seeds the input with the scoped prompt + a "queued · P5" hint.
- [ ] Object/verb extraction is pure (`objects.rs`) and unit-tested; symbols come from `zoid-syntax` (P4a).
- [ ] Object & verb overlay snapshots exist at both 100 and 140.
- [ ] **No** event is recorded and **no** agent turn is spawned by a verb pick (grep `VerbPick` arm — it only touches `textarea`/`status_hint`/overlay). Dispatch is P5.

## Self-Review notes (author)

- **Spec coverage (④):** select an object (file, error, **tree-sitter symbol**) → a menu of agent verbs scoped to it. Object model (T1, symbols via P4a), verb table + prompt (T2), two-step picker UI (T3), queue-on-pick (T4). Diff-hunk/test objects deferred (no diff drawer/test-detection yet) — documented in Global Constraints. Verb **dispatch** deferred to P5 per the 2026-06-30 decision; P4d ships the full selection + scoping UI and composes the exact prompt P5 will run.
- **Known simplification:** all four `SymbolKind`s (`Function/Struct/Enum/Trait`) collapse to a single `ObjectKind::Symbol` with one verb set (`explain / find references / add test`). This is deliberate v1 YAGNI — `zoid-syntax` preserves the full `SymbolKind` (P4a), so a later pass can recover it to offer kind-specific verbs (e.g. "implement trait" vs "add test for fn") without a data change. The mild oddity ("add test `MyTrait`") is accepted for now.
- **Type consistency:** `Obj`/`ObjectKind` (T1) flow into `verbs_for`/`verb_prompt` (T2), the overlays (T3), and the bin queue (T4) unchanged. `ObjectState { obj_selected, verb_selected }` (T3) is navigated with `palette::nav` (reused, DRY) in both routing and the bin. `Action::{OpenObjects, ObjectMove, ObjectPick, VerbMove, VerbPick}` (T3) map 1:1 to the bin arms (T4).
- **Reuse:** overlays render through one `list_overlay` helper modeled on `render_palette`; navigation reuses `palette::nav`; the overlay-dismiss-on-click guard in `route_mouse` already covers the new overlays. No parallel palette fork.
- **Independence / ordering:** P4d's *logic* depends only on P4a (`zoid-syntax` symbols), not on P4b/P4c. But its snapshot helper calls `render_shell` in whatever signature shape the branch is at, and references P4c's `normal_view()`/P3's `empty_economy()`. The intended execution order is P4a→P4b→P4c→P4d, so by the time P4d runs, `render_shell` takes `&ChatView` and `normal_view()` exists. **If P4d is pulled ahead of P4c**, match the current `render_shell` signature (e.g. P4b's trailing `caret_on: bool`) and inline the `ChatView`/economy constructors (noted in T3 Step 6).
