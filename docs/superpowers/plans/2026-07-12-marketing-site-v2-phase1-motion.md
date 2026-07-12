# Marketing Site v2 — Phase 1 (Real-Frame Motion) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the real-frame motion pipeline end-to-end on one story — render the context-economy scene as an animated, pixel-accurate sequence on a standalone preview page — so Phase 2 can rewrite the site on a trusted foundation.

**Architecture:** Fix the `buffer_to_html` converter so wide glyphs occupy exact character-cell widths (kills scrollbar drift). Enrich the context-economy scene fixture so a captured frame reads as real usage. Add an ordered frame *sequence* for that scene, a capture script that writes numbered HTML fragments, and a tiny standalone preview page whose ~1 KB vanilla player cycles the fragments (reduced-motion + off-screen aware). The live `public/index.html` is **not** touched in Phase 1.

**Tech Stack:** Rust (crate `zoid-tui`, `web-capture` feature, `ratatui` `TestBackend`, `unicode-width`), POSIX `sh` + `awk` capture/assembly scripts, vanilla HTML/CSS/JS preview page.

## Global Constraints

- **No new dependencies.** `unicode-width = "0.2"` is already a `zoid-tui` dep; use it. No crates added.
- **Converter code stays behind the `web-capture` feature.** It is never compiled into the product binary. The `web_capture` example already declares `required-features = ["web-capture"]`.
- **Capture dimensions must be ≥ 160×40.** `zoid_tui::layout::{MIN_WIDTH=160, MIN_HEIGHT=40}`; below that `render_shell` draws only a "too small" message. Default/all captures in this plan use **160×40**. (This is why the legacy `frames/` are stale — `capture.sh` used 140×24.)
- **Do not run or revive `public/build.sh`.** It is guarded/disabled; running it would `cp template.html index.html` and clobber the hand-authored live page.
- **Phase 1 never edits `public/index.html`.** Only new files are added: `public/preview.template.html`, `public/preview.html` (generated), `public/capture-preview.sh`, `public/assemble-preview.sh`, and `public/frames/context-economy/*.html` (generated).
- **No internal leakage in any published copy** (per `AGENTS.md`): no crate names, algorithms, or file paths in visitor-facing text. (Applies to the preview page's copy.)
- **Commit messages:** no `Co-Authored-By`/co-author trailer (maintainer's global instruction).
- **Verify honestly:** run the exact commands shown and confirm output before checking a step.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `crates/zoid-tui/src/web_capture.rs` (modify) | Buffer→HTML converter; wide-glyph fixed-width fix + tests | 1 |
| `crates/zoid-tui/examples/scenes/mod.rs` (modify) | Scene fixtures; enrich `economy`, add tasks, add sequence + shared render helper | 2, 3 |
| `crates/zoid-tui/examples/web_capture.rs` (modify) | CLI: single still (back-compat) + `--count` / `--frame` sequence modes | 4 |
| `public/capture-preview.sh` (create) | Render `frames/context-economy/NN.html` from the sequence | 4 |
| `public/preview.template.html` (create) | Standalone preview page: site tokens + player CSS/JS + marker slot | 5 |
| `public/assemble-preview.sh` (create) | Inline captured fragments into the template → `public/preview.html` | 5 |
| `public/preview.html` (generated) | The assembled, self-contained preview page | 5, 6 |

---

### Task 1: Fidelity fix — wide glyphs get explicit cell width

**Files:**
- Modify: `crates/zoid-tui/src/web_capture.rs` (rewrite `buffer_to_html`; add tests in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn buffer_to_html(buf: &ratatui::buffer::Buffer) -> String` — **unchanged signature**; output now wraps every glyph whose `UnicodeWidthStr::width() >= 2` in `<span style="display:inline-block;width:{w}ch;text-align:center;…">`. Normal (width-1) runs are still coalesced by `(fg,bg)`.

- [ ] **Step 1: Write the failing tests**

Add these two tests inside the existing `mod tests` block in `crates/zoid-tui/src/web_capture.rs`:

```rust
    #[test]
    fn wide_glyph_gets_explicit_cell_width() {
        // A 2-cell emoji must be emitted as a fixed 2ch inline-block so the
        // browser reserves exactly its terminal width regardless of font advance
        // (otherwise every column to its right — worst of all the scrollbar —
        // drifts). ratatui reserves the continuation cell, so use a width-2 buffer.
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf.set_string(0, 0, "📦", Style::default().fg(Color::Rgb(0x58, 0xa6, 0xff)));
        let html = buffer_to_html(&buf);
        assert!(
            html.contains("display:inline-block;width:2ch"),
            "expected fixed 2ch width for the wide glyph; got: {html}"
        );
        assert!(html.contains("📦"));
    }

    #[test]
    fn wide_glyph_does_not_shift_following_text() {
        // "📦" at col 0 (occupies cols 0-1), "x" at col 2. The wide glyph is
        // isolated in its own fixed-width span; the trailing normal run is intact.
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_string(0, 0, "📦", Style::default());
        buf.set_string(2, 0, "x", Style::default());
        let html = buffer_to_html(&buf);
        assert!(html.contains("width:2ch"));
        assert!(html.trim_end().ends_with("x</pre>"), "got: {html}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --features web-capture web_capture:: 2>&1 | tail -20`
Expected: FAIL — the current output has no `display:inline-block;width:2ch` (the wide glyph is emitted inside a coalesced run).

- [ ] **Step 3: Rewrite `buffer_to_html` with two free helpers**

Replace the entire `buffer_to_html` function (and the closure it uses) with the version below. This swaps the borrow-heavy inner closure for two free functions and adds the wide-glyph branch. Keep the file's top `use` lines (`Buffer`, `Color`, `Modifier`, `write!`, `UnicodeWidthStr`) and the `css` / `push_escaped` helpers exactly as they are.

```rust
/// Emit a run of normal-width cells sharing one (fg,bg) style.
fn write_run(out: &mut String, fg: &Option<String>, bg: &Option<String>, run: &str) {
    if run.is_empty() {
        return;
    }
    let mut style = String::new();
    if let Some(fg) = fg {
        let _ = write!(style, "color:{fg};");
    }
    if let Some(bg) = bg {
        let _ = write!(style, "background:{bg};");
    }
    if style.is_empty() {
        out.push_str(run);
    } else {
        let _ = write!(out, "<span style=\"{}\">{}</span>", style.trim_end_matches(';'), run);
    }
}

/// Emit a single wide (≥2 cell) glyph as a fixed-width inline-block, so the
/// browser reserves exactly its terminal cell count and the grid can't drift.
fn write_wide(out: &mut String, fg: &Option<String>, bg: &Option<String>, w: u16, glyph: &str) {
    let mut style = format!("display:inline-block;width:{w}ch;text-align:center;");
    if let Some(fg) = fg {
        let _ = write!(style, "color:{fg};");
    }
    if let Some(bg) = bg {
        let _ = write!(style, "background:{bg};");
    }
    let _ = write!(out, "<span style=\"{}\">{}</span>", style.trim_end_matches(';'), glyph);
}

/// Convert a rendered buffer into a colored `<pre>` mirroring the terminal grid.
pub fn buffer_to_html(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::from("<pre class=\"tui\">");
    for y in area.y..area.y + area.height {
        // Rows are *separated* by `\n`, not terminated (no trailing blank line).
        if y > area.y {
            out.push('\n');
        }
        let mut x = area.x;
        // Open normal-run state for this row.
        let mut run = String::new();
        let mut run_fg: Option<String> = None;
        let mut run_bg: Option<String> = None;
        let mut run_open = false;

        while x < area.x + area.width {
            let cell = &buf[(x, y)];
            let (mut fg, mut bg) = (cell.fg, cell.bg);
            if cell.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let fg = css(fg);
            let bg = css(bg);
            let sym = cell.symbol();
            // Advance by the glyph's display width so a 2-col glyph skips its
            // reserved continuation cell (ratatui leaves it blank).
            let w = sym.width().max(1) as u16;

            if w >= 2 {
                // Flush any pending normal run, then emit the wide glyph alone.
                write_run(&mut out, &run_fg, &run_bg, &run);
                run.clear();
                run_open = false;
                let mut esc = String::new();
                push_escaped(&mut esc, sym);
                write_wide(&mut out, &fg, &bg, w, &esc);
            } else {
                if !run_open || run_fg != fg || run_bg != bg {
                    write_run(&mut out, &run_fg, &run_bg, &run);
                    run.clear();
                    run_fg = fg.clone();
                    run_bg = bg.clone();
                    run_open = true;
                }
                push_escaped(&mut run, sym);
            }
            x += w;
        }
        write_run(&mut out, &run_fg, &run_bg, &run);
    }
    out.push_str("</pre>");
    out
}
```

- [ ] **Step 4: Run the full converter test set to verify pass**

Run: `cargo test -p zoid-tui --features web-capture web_capture:: 2>&1 | tail -20`
Expected: PASS — the two new tests plus the pre-existing `emits_rgb_span_and_escapes_html`, `reversed_swaps_fg_and_bg`, `rows_are_separated_not_terminated` all pass (the rewrite preserves their behavior for normal-width glyphs).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/web_capture.rs
git commit -m "fix(web-capture): give wide glyphs explicit cell width (no grid drift)"
```

---

### Task 2: Enrich the context-economy scene fixture

**Files:**
- Modify: `crates/zoid-tui/examples/scenes/mod.rs`

**Interfaces:**
- Consumes: `scene(name)` (existing, returns `(ShellState, Vec<ChatMsg>, EconomyView)`), `render_shell` (existing).
- Produces:
  - `fn seeded_tasks() -> Vec<zoid_core::tasks::TaskItem>` — two tasks (one Done, one Active).
  - `fn scene_tasks(name: &str) -> Vec<zoid_core::tasks::TaskItem>` — `seeded_tasks()` for `"economy"`/`"context-economy"`, else empty.
  - `fn render_one(state: &ShellState, msgs: &[ChatMsg], economy: &EconomyView, tasks: &[zoid_core::tasks::TaskItem], w: u16, h: u16) -> Buffer` — shared single-frame renderer.
  - `render_shell_scene(name, w, h)` now populates session fields for `economy` (via `scene`) and passes real tasks.

The `economy` arm of `scene()` is enriched; `scene()`'s **signature is unchanged** (so `preview.rs`, which calls `scenes::scene`, keeps compiling).

- [ ] **Step 1: Write the failing test**

Add this test to the bottom of `crates/zoid-tui/examples/scenes/mod.rs`, in a new test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn economy_scene_is_populated() {
        // The captured hero scene must read as real usage: a named session with
        // real token/cache/ctx numbers and a couple of tasks — not empty rails.
        let (s, _msgs, _econ) = scene("economy");
        assert!(!s.session_name.is_empty(), "session should be named");
        assert!(s.session_tokens > 0, "session tokens should be non-zero");
        assert!(s.ctx_used > 0 && s.ctx_ceiling > s.ctx_used, "ctx should be seeded");
        assert_eq!(scene_tasks("economy").len(), 2, "two tasks expected");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --features web-capture --example web_capture economy_scene_is_populated 2>&1 | tail -20`
Expected: FAIL — `scene_tasks` does not exist yet (compile error), and the `economy` `ShellState` fields are still their `ShellState::new()` defaults.

- [ ] **Step 3: Add the fixtures and enrich the `economy` arm**

In `crates/zoid-tui/examples/scenes/mod.rs`, add these two functions just above `pub fn scene(`:

```rust
/// A short, realistic task list for the hero scene's Tasks drawer.
fn seeded_tasks() -> Vec<zoid_core::tasks::TaskItem> {
    use zoid_core::tasks::{TaskItem, TaskStatus};
    vec![
        TaskItem {
            text: "reproduce the 500".into(),
            status: TaskStatus::Done,
        },
        TaskItem {
            text: "patch the unwrapped lookup".into(),
            status: TaskStatus::Active,
        },
    ]
}

/// Tasks a scene renders into the Tasks drawer (empty for scenes without tasks).
fn scene_tasks(name: &str) -> Vec<zoid_core::tasks::TaskItem> {
    match name {
        "economy" | "context-economy" => seeded_tasks(),
        _ => vec![],
    }
}
```

Then replace the `economy` arm inside `scene()`:

```rust
        "economy" => {
            // Populate the right-rail widgets so the frame reads as real usage.
            s.session_name = "diagnose 500".into();
            s.model = "glm-5.2".into();
            s.provider = "ollama".into();
            s.duration = "12m".into();
            s.session_tokens = 48_200;
            s.cached_tokens = 31_040;
            s.cache_supported = true;
            s.ctx_used = 18_000;
            s.ctx_ceiling = 128_000;
            s.repo_name = "api".into();
            s.branch = "main".into();
            s.changes_added = 24;
            s.changes_removed = 6;
            s.changes_files = 3;
            s.tasks_len = 2;
            return (s, seeded(), seeded_economy());
        }
```

- [ ] **Step 4: Extract `render_one` and thread tasks through `render_shell_scene`**

Replace the existing `render_shell_scene` function with a shared helper plus a thin wrapper:

```rust
/// Render one frame (state + messages + economy + tasks) to a cloned buffer.
#[allow(dead_code)]
pub fn render_one(
    state: &ShellState,
    msgs: &[ChatMsg],
    economy: &EconomyView,
    tasks: &[zoid_core::tasks::TaskItem],
    w: u16,
    h: u16,
) -> Buffer {
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let view = ChatView {
        zoom: state.zoom,
        caret_on: true,
        reveal: None,
        tz_offset_secs: 0,
    };
    terminal
        .draw(|f| {
            render_shell(f, state, economy, msgs, None, tasks, &input, false, &view);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Render a shell scene and return a clone of the rendered buffer.
#[allow(dead_code)]
pub fn render_shell_scene(name: &str, w: u16, h: u16) -> Buffer {
    let (state, msgs, economy) = scene(name);
    render_one(&state, &msgs, &economy, &scene_tasks(name), w, h)
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p zoid-tui --features web-capture --example web_capture economy_scene_is_populated 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Confirm the shared scenes module still compiles for `preview`**

Run: `cargo build -p zoid-tui --features web-capture --example preview 2>&1 | tail -5`
Expected: builds cleanly (`scene()`'s signature is unchanged; `preview.rs:41` still binds a 3-tuple).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/examples/scenes/mod.rs
git commit -m "feat(web-capture): enrich context-economy scene (populated rail + tasks)"
```

---

### Task 3: Frame sequence for the context-economy story

**Files:**
- Modify: `crates/zoid-tui/examples/scenes/mod.rs`

**Interfaces:**
- Consumes: `scene`, `seeded`, `empty_economy`, `seeded_economy`, `render_one`, `scene_tasks` (Task 2).
- Produces:
  - `pub fn scene_seq(name: &str) -> Vec<(ShellState, Vec<ChatMsg>, EconomyView)>` — for `"context-economy"`, four states progressively revealing the seeded turn (economy fills at the compaction step); any other name yields a single frame `vec![scene(name)]`.
  - `pub fn render_shell_scene_seq(name: &str, w: u16, h: u16) -> Vec<Buffer>` — renders each state via `render_one`.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `scenes/mod.rs`:

```rust
    #[test]
    fn context_economy_sequence_reveals_the_turn() {
        // Four frames: user prompt → +searching → +compaction → +answer.
        let seq = scene_seq("context-economy");
        assert_eq!(seq.len(), 4, "expected a 4-frame reveal");
        assert_eq!(seq[0].1.len(), 1, "frame 0 shows only the user prompt");
        assert_eq!(seq[3].1.len(), 4, "final frame shows the whole turn");
        // The rail fills once compaction happens (frame 2 onward).
        assert!(seq[0].2.items.is_empty(), "frame 0 rail empty");
        assert!(!seq[2].2.items.is_empty(), "frame 2 rail populated");

        // And each frame renders to a buffer at the required min size.
        let frames = render_shell_scene_seq("context-economy", 160, 40);
        assert_eq!(frames.len(), 4);
    }
```

> Note: `EconomyView` exposes `items` (a `Vec`) used above. If the field is named differently, adjust the two `.items` assertions to the actual public accessor on `EconomyView` — check `crates/zoid-tui/src/economy_view.rs:101`. The reveal-length assertions are the primary check and do not depend on that.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --features web-capture --example web_capture context_economy_sequence 2>&1 | tail -20`
Expected: FAIL — `scene_seq` / `render_shell_scene_seq` do not exist (compile error).

- [ ] **Step 3: Implement the sequence functions**

Add to `scenes/mod.rs`, just below `render_shell_scene`:

```rust
/// The context-economy story as an ordered set of states. Reuses the enriched
/// `economy` ShellState and progressively reveals the seeded turn; the context
/// rail fills once the compaction event lands (frame 2), so the player animates
/// "work happens → context becomes a managed, measured resource".
#[allow(dead_code)]
pub fn scene_seq(name: &str) -> Vec<(ShellState, Vec<ChatMsg>, EconomyView)> {
    match name {
        "context-economy" => {
            // The enriched right-rail state, reused for every frame.
            let base = || {
                let (s, _m, _e) = scene("economy");
                s
            };
            let turn = seeded(); // [user, assistant+search, compacted result, answer]
            vec![
                (base(), turn[..1].to_vec(), empty_economy()),
                (base(), turn[..2].to_vec(), empty_economy()),
                (base(), turn[..3].to_vec(), seeded_economy()),
                (base(), turn[..4].to_vec(), seeded_economy()),
            ]
        }
        _ => vec![scene(name)],
    }
}

/// Render every frame of a sequence to cloned buffers.
#[allow(dead_code)]
pub fn render_shell_scene_seq(name: &str, w: u16, h: u16) -> Vec<Buffer> {
    let tasks = scene_tasks(name);
    scene_seq(name)
        .into_iter()
        .map(|(state, msgs, economy)| render_one(&state, &msgs, &economy, &tasks, w, h))
        .collect()
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid-tui --features web-capture --example web_capture context_economy_sequence 2>&1 | tail -20`
Expected: PASS. (If the `.items` accessor name differs, fix per the Step 1 note and re-run.)

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/examples/scenes/mod.rs
git commit -m "feat(web-capture): add context-economy frame sequence"
```

---

### Task 4: `web_capture` sequence CLI + capture script

**Files:**
- Modify: `crates/zoid-tui/examples/web_capture.rs`
- Create: `public/capture-preview.sh`

**Interfaces:**
- Consumes: `scenes::render_shell_scene`, `scenes::render_shell_scene_seq`, `scenes::scene_seq`, `zoid_tui::web_capture::buffer_to_html`.
- Produces: CLI modes — `web_capture <scene> [w] [h]` (still, default 160×40); `web_capture --count <scene>` (prints frame count); `web_capture --frame <i> <scene> [w] [h]` (prints frame *i*'s HTML). Script writes `public/frames/context-economy/NN.html`.

- [ ] **Step 1: Rewrite `main` in `crates/zoid-tui/examples/web_capture.rs`**

Replace the whole `fn main()` (keep the `mod scenes;` line and the doc comment):

```rust
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Print the number of frames in a scene's sequence.
        Some("--count") => {
            let name = args.get(1).map(String::as_str).unwrap_or("context-economy");
            println!("{}", scenes::scene_seq(name).len());
        }
        // Print one frame of a scene's sequence: --frame <i> <scene> [w] [h]
        Some("--frame") => {
            let i: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(0);
            let name = args.get(2).map(String::as_str).unwrap_or("context-economy");
            let w: u16 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(160);
            let h: u16 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(40);
            let frames = scenes::render_shell_scene_seq(name, w, h);
            let buf = frames
                .get(i)
                .unwrap_or_else(|| panic!("frame {i} out of range (have {})", frames.len()));
            print!("{}", zoid_tui::web_capture::buffer_to_html(buf));
        }
        // Single still (back-compat): <scene> [w] [h]. Default size ≥ MIN.
        _ => {
            let name = args.first().map(String::as_str).unwrap_or("chat");
            let w: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(160);
            let h: u16 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(40);
            let buf = scenes::render_shell_scene(name, w, h);
            print!("{}", zoid_tui::web_capture::buffer_to_html(&buf));
        }
    }
}
```

- [ ] **Step 2: Verify `--count` prints 4**

Run: `cargo run -q -p zoid-tui --features web-capture --example web_capture -- --count context-economy`
Expected: `4`

- [ ] **Step 3: Verify a frame renders full-shell HTML (not "too small")**

Run: `cargo run -q -p zoid-tui --features web-capture --example web_capture -- --frame 3 context-economy 160 40 | head -c 400`
Expected: starts with `<pre class="tui">`, contains styled spans (e.g. `color:#`), and does **not** contain the too-small message. Sanity-check that a `width:2ch` span appears somewhere in the full output:
`cargo run -q -p zoid-tui --features web-capture --example web_capture -- --frame 3 context-economy 160 40 | grep -c 'width:2ch'` → expect a non-zero count (emoji in the rail).

- [ ] **Step 4: Create `public/capture-preview.sh`**

```sh
#!/bin/sh
# Render the context-economy frame sequence into public/frames/context-economy/.
# Run from repo root: sh public/capture-preview.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/public/frames/context-economy"
RUN="cargo run -q -p zoid-tui --features web-capture --example web_capture --"
mkdir -p "$OUT"
rm -f "$OUT"/*.html
N=$($RUN --count context-economy)
i=0
while [ "$i" -lt "$N" ]; do
  f=$(printf "%02d" "$i")
  $RUN --frame "$i" context-economy 160 40 > "$OUT/$f.html"
  i=$((i + 1))
done
echo "captured $N frames → $OUT"
```

- [ ] **Step 5: Run the capture script and verify output files**

Run: `sh public/capture-preview.sh && ls public/frames/context-economy/`
Expected: prints `captured 4 frames …` and lists `00.html 01.html 02.html 03.html`.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/examples/web_capture.rs public/capture-preview.sh public/frames/context-economy/
git commit -m "feat(site): capture context-economy sequence to HTML frames"
```

---

### Task 5: Preview page — player + assembler

**Files:**
- Create: `public/preview.template.html`
- Create: `public/assemble-preview.sh`
- Generated: `public/preview.html`

**Interfaces:**
- Consumes: `public/frames/context-economy/*.html` (Task 4).
- Produces: `public/preview.html` — a standalone, self-contained page. A `.player` element holds N `.tui-frame` children (each wrapping one captured `<pre>`); a ~1 KB script cycles them, honoring `prefers-reduced-motion` and pausing off-screen.

- [ ] **Step 1: Create `public/preview.template.html`**

Palette/`pre.tui` values are copied from the live `public/index.html` `:root` so the preview reads as zoid. The `<!--SEQ:context-economy-->` line is the single assembly marker.

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>zoid — motion preview (context economy)</title>
<style>
:root{
  --bg:#0d1117; --panel:#161b22; --line:#30363d; --line2:#21262d;
  --txt:#c9d1d9; --muted:#8b949e; --dim:#6e7681; --acc:#58a6ff; --acc2:#79c0ff;
  --mono:"JetBrains Mono","SF Mono",Menlo,Consolas,monospace;
}
*{box-sizing:border-box;}
html,body{margin:0;background:var(--bg);color:var(--txt);font-family:var(--mono);
  font-size:15px;line-height:1.6;-webkit-font-smoothing:antialiased;}
.wrap{max-width:1120px;margin:0 auto;padding:48px 24px;}
h1{font-size:22px;font-weight:600;}
.sub{color:var(--muted);margin:0 0 28px;}
.frame{overflow-x:auto;border:1px solid var(--line);border-radius:10px;background:var(--bg);
  box-shadow:0 8px 40px rgba(0,0,0,.4);}
pre.tui{margin:0;padding:14px 16px;font-family:var(--mono);font-size:12px;line-height:1.5;
  color:var(--txt);background:var(--bg);white-space:pre;}
@media (max-width:640px){pre.tui{font-size:10px;}}

/* Player: exactly one frame visible at a time. */
.player .tui-frame{display:none;}
.player .tui-frame.on{display:block;}
/* Pre-JS / no-JS fallback: show the first frame. */
.player.nojs .tui-frame:first-child{display:block;}
/* Reduced motion: show the final (resolved) frame, no animation. */
@media (prefers-reduced-motion: reduce){
  .player .tui-frame{display:none;}
  .player .tui-frame:last-child{display:block;}
}
</style>
</head>
<body>
<div class="wrap">
  <h1>zoid — motion preview</h1>
  <p class="sub"># context economy · real renderer frames · phase-1 fidelity check</p>
  <div class="frame">
    <div class="player nojs" role="img"
         aria-label="zoid diagnosing a 500: the turn streams in while the context economy rail fills.">
<!--SEQ:context-economy-->
    </div>
  </div>
</div>
<script>
(function(){
  var reduce = window.matchMedia
    && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  document.querySelectorAll('.player').forEach(function(p){
    p.classList.remove('nojs');
    var frames = p.querySelectorAll('.tui-frame');
    if(!frames.length) return;
    if(reduce){ frames[frames.length-1].classList.add('on'); return; }
    var i = 0;
    function show(n){ frames.forEach(function(f,k){ f.classList.toggle('on', k===n); }); }
    show(0);
    var timer = null;
    function tick(){ i = (i+1) % frames.length; show(i); }
    var io = new IntersectionObserver(function(entries){
      entries.forEach(function(e){
        if(e.isIntersecting && !timer){ timer = setInterval(tick, 1400); }
        else if(!e.isIntersecting && timer){ clearInterval(timer); timer = null; }
      });
    }, {threshold:0.25});
    io.observe(p);
  });
})();
</script>
</body>
</html>
```

- [ ] **Step 2: Create `public/assemble-preview.sh`**

Uses the same awk-marker idiom as the (disabled) `build.sh`: replace the marker line with the wrapped frames. Never edits `index.html`.

```sh
#!/bin/sh
# Inline captured context-economy frames into preview.template.html.
# Run from repo root: sh public/assemble-preview.sh  (after capture-preview.sh)
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DIR="$ROOT/public/frames/context-economy"
TPL="$ROOT/public/preview.template.html"
OUT="$ROOT/public/preview.html"
[ -d "$DIR" ] || { echo "run capture-preview.sh first (no frames/)"; exit 1; }

# Wrap each captured <pre> fragment in a .tui-frame div → a temp block.
TMP=$(mktemp)
for f in "$DIR"/*.html; do
  printf '<div class="tui-frame">' >> "$TMP"
  cat "$f" >> "$TMP"
  printf '</div>\n' >> "$TMP"
done

# Replace the single marker line with the frames block.
awk -v marker="<!--SEQ:context-economy-->" -v ff="$TMP" '
  $0 ~ marker { while ((getline line < ff) > 0) print line; next }
  { print }
' "$TPL" > "$OUT"
rm -f "$TMP"
echo "assembled → $OUT"
```

- [ ] **Step 3: Assemble and verify the marker was replaced**

Run: `sh public/assemble-preview.sh && grep -c 'class="tui-frame"' public/preview.html && grep -c 'SEQ:context-economy' public/preview.html`
Expected: `assembled → …`, then `4` (four frame wrappers), then `0` (marker consumed, none left).

- [ ] **Step 4: Commit**

```bash
git add public/preview.template.html public/assemble-preview.sh public/preview.html
git commit -m "feat(site): standalone motion-preview page with frame player"
```

---

### Task 6: End-to-end browser verification (Phase 1 exit gate)

**Files:** none (verification only). Produces a screenshot artifact for review.

**Interfaces:** Consumes `public/preview.html` and the scripts from Tasks 4–5.

- [ ] **Step 1: Regenerate the full pipeline from clean**

Run:
```bash
rm -rf public/frames/context-economy && \
sh public/capture-preview.sh && \
sh public/assemble-preview.sh
```
Expected: `captured 4 frames …` then `assembled → …/public/preview.html`, no errors.

- [ ] **Step 2: Serve the page locally**

Run (background): `python3 -m http.server 8099 --directory public`
Open: `http://localhost:8099/preview.html`
(Agentic workers: use the claude-in-chrome tools — `tabs_create_mcp` then `navigate` to the URL — and `computer`/screenshot to capture the frame. A `file://` open also works if no server is desired.)

- [ ] **Step 3: Verify the fidelity checklist (the Phase 1 exit criteria)**

Confirm by looking at the rendered page:
- [ ] **Scrollbar column is vertically straight** down the right edge of the terminal across every row — no per-row wobble (this is the defect Phase 1 exists to kill).
- [ ] **Right-rail widgets are populated** — repo/branch, a named session with real duration + tok/cac + ctx numbers, the context sparklines, and two tasks. Not empty rails.
- [ ] **The animation loops** through the 4 frames (prompt → searching → compaction → answer) and the context rail fills partway through.
- [ ] **Text is crisp and selectable** (drag-select a line; it selects as text, not an image).
- [ ] **Reduced motion**: with OS "reduce motion" on (or emulated in devtools → Rendering → "Emulate prefers-reduced-motion"), the page shows the final frame **statically**, no cycling.

- [ ] **Step 4: Capture a screenshot for the review record**

Save a screenshot of the running preview (e.g. `public/frames/context-economy/PREVIEW.png` is **not** committed — put it under the scratchpad or attach to the review). This is the evidence the exit gate passed.

- [ ] **Step 5: Stop the local server**

Stop the backgrounded `http.server` (Ctrl-C or kill the job).

- [ ] **Step 6: Record completion**

No code commit. In the execution notes / PR description, state: "Phase 1 exit gate met — scrollbar stable, rail populated, animation + reduced-motion verified in-browser," and attach the screenshot. Phase 2 (full beta-site rewrite) is a separate spec-review → plan cycle.

---

## Self-Review

**Spec coverage** (against `2026-07-12-marketing-site-v2-beta-motion-design.md` §5 + §7 Phase 1):
- §5.1 fidelity fix → Task 1. ✓
- §5.2 richer scenes → Task 2. ✓
- §5.3 frame sequences → Task 3. ✓
- §5.4 web player (reduced-motion, off-screen pause, inert, KB-sized) → Task 5. ✓
- §5.5 additive pipeline (no clobber; `index.html` untouched; marker-inlined) → Tasks 4–5, enforced by Global Constraints. ✓
- §7 Phase 1 exit criterion (pixel-perfect, real-usage, crisp, reduced-motion, publishes end-to-end, browser-verified) → Task 6. ✓
- Phase 2 explicitly deferred. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every command shows expected output. The one conditional (Task 3 `EconomyView.items` accessor) names the exact file to check and keeps a non-dependent primary assertion. ✓

**Type consistency:** `render_one` signature is identical where referenced (Tasks 2 & 3). `scene_tasks` / `scene_seq` / `render_shell_scene_seq` names match across Tasks 2–4. `buffer_to_html` signature unchanged (Task 1). Capture dims are 160×40 everywhere. Frame marker string `<!--SEQ:context-economy-->` is identical in template (Task 5 Step 1) and assembler (Task 5 Step 2). ✓
