# zoid Marketing Teaser Site — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Task 4 (the page itself) additionally uses the **frontend-design** skill for visual execution.

**Goal:** Build a single self-contained, terminal-authentic teaser page for zoid, with real TUI frames rendered by zoid's own renderer, hostable on GitHub Pages with no build step at serve time.

**Architecture:** A reusable `buffer_to_html` converter (in `zoid-tui`, behind a `web-capture` cargo feature so it never enters the product binary) turns a rendered `TestBackend` buffer into a faithful colored HTML `<pre>`. A `web_capture` example renders each marketing scene (reusing `preview.rs`'s fixtures, extracted into a shared `examples/scenes/` module) and prints its fragment. A shell script captures each scene into `public/frames/*.html`, and a build script injects those fragments into `public/template.html` to produce the shipped `public/index.html`.

**Tech Stack:** Rust (ratatui 0.30 / ratatui-core 0.1, `TestBackend`), `unicode-width 0.2` (already a direct dep of `zoid-tui`), plain HTML/CSS/JS (no framework, no SSG, no web-font fetch), POSIX shell for the build.

## Global Constraints

- **Self-contained artifact:** the shipped `public/index.html` MUST have zero external `http(s)://` references — no CDN scripts, external stylesheets, web-font fetches, or remote images. All CSS/JS/frames inlined.
- **No web-font fetch:** font stack is `"JetBrains Mono","SF Mono",Menlo,Consolas,monospace` (system fallback only).
- **Design tokens (verbatim):** bg `#0d1117` · panel `#161b22` · gutter `#0b0e13` · line `#30363d` · line2 `#21262d` · text `#c9d1d9` · muted `#8b949e` · dim `#6e7681` · accent `#58a6ff` · accent2 `#79c0ff` · chip-bg `#0d2a4d` · ok `#3fb950` · warn `#d29922` · error `#f85149` · branch `#bc8cff` · pink `#f778ba`.
- **Tone:** soft "coming soon". No pricing, plans, subscription, or waitlist/data-capture copy. No download links or "get it now" CTA.
- **Honesty rule (Modes frame):** the Shift+Tab mode registry is Slice 3 (designed, not fully built). Capture the closest *current* palette state; never fabricate a frame of unbuilt UI. Copy frames the switch as a near-term capability.
- **Size discipline:** the converter lives behind `web-capture` and MUST NOT be compiled into the `zoid` release binary. Verify the default build's feature set is unchanged.
- **Commits:** conventional-commit messages; **no `Co-Authored-By` / co-author trailer** (per repo CLAUDE.md).

---

### Task 1: `buffer_to_html` converter (feature-gated, in `zoid-tui`)

Pure function that walks a rendered `ratatui` buffer and emits a faithful colored HTML `<pre>`. Lives behind a cargo feature so it is testable but never in the product binary.

**Files:**
- Modify: `crates/zoid-tui/Cargo.toml` (add `[features] web-capture = []`)
- Create: `crates/zoid-tui/src/web_capture.rs`
- Modify: `crates/zoid-tui/src/lib.rs` (feature-gated `pub mod web_capture;`)
- Test: inline `#[cfg(test)] mod tests` in `crates/zoid-tui/src/web_capture.rs`

**Interfaces:**
- Consumes: `ratatui::buffer::Buffer`, `ratatui::style::{Color, Modifier}`, `ratatui::layout::Rect` (re-exported through `ratatui`).
- Produces: `pub fn buffer_to_html(buf: &ratatui::buffer::Buffer) -> String` — a `<pre class="tui">…</pre>` string; each maximal run of cells sharing the same effective (fg,bg) becomes one `<span style="color:#rrggbb;background:#rrggbb">…</span>`; rows separated by `\n`; HTML-special chars escaped; wide glyphs advance by their display width (skipping the reserved continuation cell); `Modifier::REVERSED` swaps fg/bg.

- [ ] **Step 1: Add the cargo feature**

In `crates/zoid-tui/Cargo.toml`, after the `[dependencies]` block and before `[dev-dependencies]`, add:

```toml
[features]
# Opt-in: compiles the buffer→HTML converter used only by the `web_capture`
# example + the marketing site build. Never enabled by the product binary,
# so it costs zero bytes in a default `cargo build -p zoid`.
web-capture = []
```

- [ ] **Step 2: Write the failing test**

Create `crates/zoid-tui/src/web_capture.rs` with the test first:

```rust
//! Faithful buffer → HTML converter for the marketing site (feature `web-capture`).
//! Walks a rendered `TestBackend` buffer and emits a colored `<pre>` that mirrors
//! the terminal grid. Not compiled into the product binary.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn emits_rgb_span_and_escapes_html() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_string(0, 0, "a<b", Style::default().fg(Color::Rgb(0x58, 0xa6, 0xff)));
        let html = buffer_to_html(&buf);
        assert!(html.starts_with("<pre class=\"tui\">"));
        assert!(html.contains("color:#58a6ff"));
        assert!(html.contains("a&lt;b"));
        assert!(html.trim_end().ends_with("</pre>"));
    }

    #[test]
    fn reversed_swaps_fg_and_bg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf.set_string(
            0,
            0,
            "x",
            Style::default()
                .fg(Color::Rgb(0x0d, 0x11, 0x17))
                .bg(Color::Rgb(0x58, 0xa6, 0xff))
                .add_modifier(Modifier::REVERSED),
        );
        let html = buffer_to_html(&buf);
        // After REVERSED swap, the glyph paints in the (former) bg color.
        assert!(html.contains("color:#58a6ff"));
        assert!(html.contains("background:#0d1117"));
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --features web-capture web_capture -- --nocapture`
Expected: FAIL to **compile** — `cannot find function buffer_to_html`.

- [ ] **Step 4: Implement the converter**

Add above the `#[cfg(test)]` module in `crates/zoid-tui/src/web_capture.rs`:

```rust
use std::fmt::Write as _;
use unicode_width::UnicodeWidthStr;

/// Resolve a ratatui `Color` to a CSS hex, or `None` to inherit the `<pre>` default.
/// `Reset`/`Indexed`/named colors inherit — the design tokens are all `Rgb`, and the
/// `<pre>` carries the default text/background, so inheriting is exactly right.
fn css(color: Color) -> Option<String> {
    match color {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        _ => None,
    }
}

fn push_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(ch),
        }
    }
}

/// Convert a rendered buffer into a colored `<pre>` mirroring the terminal grid.
pub fn buffer_to_html(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::from("<pre class=\"tui\">");
    for y in area.y..area.y + area.height {
        let mut x = area.x;
        // Open-span state for the current run.
        let mut run = String::new();
        let mut cur: Option<(Option<String>, Option<String>)> = None;

        let flush = |out: &mut String,
                     run: &mut String,
                     cur: &mut Option<(Option<String>, Option<String>)>| {
            if let Some((fg, bg)) = cur.take() {
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
            } else {
                out.push_str(run);
            }
            run.clear();
        };

        while x < area.x + area.width {
            let cell = &buf[(x, y)];
            let (mut fg, mut bg) = (cell.fg, cell.bg);
            if cell.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let key = (css(fg), css(bg));
            if cur.as_ref() != Some(&key) {
                flush(&mut out, &mut run, &mut cur);
                cur = Some(key);
            }
            let sym = cell.symbol();
            push_escaped(&mut run, sym);
            // Advance by the glyph's display width so a 2-col glyph skips its
            // reserved continuation cell (ratatui leaves it blank).
            let w = sym.width().max(1) as u16;
            x += w;
        }
        flush(&mut out, &mut run, &mut cur);
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}
```

- [ ] **Step 5: Wire the module in `lib.rs`**

In `crates/zoid-tui/src/lib.rs`, add (near the other `pub mod` lines):

```rust
#[cfg(feature = "web-capture")]
pub mod web_capture;
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --features web-capture web_capture`
Expected: PASS (`emits_rgb_span_and_escapes_html`, `reversed_swaps_fg_and_bg`).

- [ ] **Step 7: Verify the default build is unaffected**

Run: `cargo build -p zoid-tui` (no `--features`)
Expected: builds; `web_capture` module is not compiled (feature off). Sanity: `cargo build -p zoid` still succeeds.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/Cargo.toml crates/zoid-tui/src/web_capture.rs crates/zoid-tui/src/lib.rs
git commit -m "feat(tui): feature-gated buffer_to_html converter for marketing frames"
```

---

### Task 2: Extract shared scene fixtures + `render_shell_scene`

Move `preview.rs`'s scene fixtures into a shared example module both examples use, and add a helper that returns the rendered buffer (not just text).

**Files:**
- Create: `crates/zoid-tui/examples/scenes/mod.rs`
- Modify: `crates/zoid-tui/examples/preview.rs` (delete moved fixtures; `mod scenes;`; delegate)

**Interfaces:**
- Produces: `pub fn scene(name: &str) -> (ShellState, Vec<ChatMsg>, EconomyView)` (moved verbatim from `preview.rs`), and `pub fn render_shell_scene(name: &str, w: u16, h: u16) -> ratatui::buffer::Buffer` — renders a shell scene via `render_shell` and returns a clone of the `TestBackend` buffer.
- Consumes (by later task): `web_capture.rs` calls `scenes::render_shell_scene`.

- [ ] **Step 1: Create the shared module**

Create `crates/zoid-tui/examples/scenes/mod.rs`. Move the fixture fns (`seeded`, `seeded_objects`, `empty_economy`, `seeded_economy`) and `scene` **verbatim** from `preview.rs` into it, make them `pub` where needed, and add `render_shell_scene`:

```rust
//! Shared scene fixtures for the `preview` and `web_capture` examples.
//! (Files under `examples/<dir>/` are modules, not example binaries.)

use ratatui::buffer::Buffer;
use ratatui::{backend::TestBackend, Terminal};
use ratatui_textarea::TextArea;
use zoid_core::projection::{ChatMsg, ToolCallRef};
use zoid_tui::chat::ChatView;
use zoid_tui::render_shell;
use zoid_tui::state::{DrawerId, Mode, Overlay, ShellState, Zoom};
use zoid_tui::EconomyView;

// ── moved verbatim from preview.rs ──────────────────────────────────────────
pub fn seeded() -> Vec<ChatMsg> { /* … exact body from preview.rs … */ }
fn seeded_objects() -> Vec<ChatMsg> { /* … exact body … */ }
fn empty_economy() -> EconomyView { /* … exact body … */ }
fn seeded_economy() -> EconomyView { /* … exact body … */ }
pub fn scene(name: &str) -> (ShellState, Vec<ChatMsg>, EconomyView) { /* … exact body … */ }
// ────────────────────────────────────────────────────────────────────────────

/// Render a shell scene and return a clone of the rendered buffer.
pub fn render_shell_scene(name: &str, w: u16, h: u16) -> Buffer {
    let (state, msgs, economy) = scene(name);
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let view = ChatView { zoom: state.zoom, caret_on: true, reveal: None, tz_offset_secs: 0 };
    terminal
        .draw(|f| {
            render_shell(f, &state, &economy, &msgs, None, &[], &input, false, &view);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}
```

> Copy the exact bodies from the current `preview.rs` (Task tool: read `crates/zoid-tui/examples/preview.rs`) — do not paraphrase them.

- [ ] **Step 2: Refactor `preview.rs` to use the shared module**

In `crates/zoid-tui/examples/preview.rs`: delete the moved fns; add `mod scenes;` at the top; replace the local `scene(name)` call in `main` with `scenes::scene(name)`. Keep the `syntax` branch, the ruler, and the `print!("{}", terminal.backend())` Display output unchanged.

- [ ] **Step 3: Verify `preview` still renders identically**

Run: `cargo run -q -p zoid-tui --example preview -- economy 140 24 > /tmp/claude-1000/-home-gomanjoe-source-zoid/0077d3fc-2629-4103-9369-576e739a5fc6/scratchpad/preview_after.txt`
Expected: exit 0; output is a text economy frame identical to before the refactor (spot-check the `context · tokens` sparkline row is present).

- [ ] **Step 4: Run the TUI test suite (guard the refactor)**

Run: `cargo test -p zoid-tui`
Expected: PASS (snapshots unchanged — we only moved example code).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/examples/scenes/mod.rs crates/zoid-tui/examples/preview.rs
git commit -m "refactor(tui): extract shared scene fixtures for example reuse"
```

---

### Task 3: `web_capture` example + capture script

The example renders a scene and prints its HTML fragment; the script captures each marketing scene to `public/frames/`.

**Files:**
- Modify: `crates/zoid-tui/Cargo.toml` (declare the example with `required-features`)
- Create: `crates/zoid-tui/examples/web_capture.rs`
- Create: `public/capture.sh`

**Interfaces:**
- Consumes: `scenes::render_shell_scene` (Task 2), `zoid_tui::web_capture::buffer_to_html` (Task 1).
- Produces: `public/frames/{hero,economy,palette,summary,detail}.html`, each a `<pre class="tui">…</pre>` fragment.

- [ ] **Step 1: Declare the example (feature-gated)**

Append to `crates/zoid-tui/Cargo.toml`:

```toml
[[example]]
name = "web_capture"
required-features = ["web-capture"]
```

- [ ] **Step 2: Write the example**

Create `crates/zoid-tui/examples/web_capture.rs`:

```rust
//! Render a shell scene to a faithful colored HTML fragment for the marketing
//! site. Reuses the shared scene fixtures and the feature-gated converter.
//!
//!   cargo run -p zoid-tui --features web-capture --example web_capture -- [scene] [w] [h]

mod scenes;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map(String::as_str).unwrap_or("chat");
    let w: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(140);
    let h: u16 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(24);
    let buf = scenes::render_shell_scene(name, w, h);
    print!("{}", zoid_tui::web_capture::buffer_to_html(&buf));
}
```

- [ ] **Step 3: Verify a fragment is produced with colored spans**

Run: `cargo run -q -p zoid-tui --features web-capture --example web_capture -- economy 140 24 | head -c 400`
Expected: output begins `<pre class="tui">` and contains at least one `color:#` span (e.g. the accent `#58a6ff` or dim `#6e7681`).

- [ ] **Step 4: Write the capture script**

Create `public/capture.sh` (executable):

```sh
#!/bin/sh
# Capture each marketing scene into public/frames/<scene>.html.
# Run from repo root: sh public/capture.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/public/frames"
mkdir -p "$OUT"
RUN="cargo run -q -p zoid-tui --features web-capture --example web_capture --"

# scene            w    h   → file
$RUN chat    140 24 > "$OUT/hero.html"
$RUN economy 140 24 > "$OUT/economy.html"
$RUN palette 140 24 > "$OUT/palette.html"
$RUN summary  96 20 > "$OUT/summary.html"
$RUN detail   96 20 > "$OUT/detail.html"
echo "captured: $(ls "$OUT")"
```

- [ ] **Step 5: Run the capture script**

Run: `chmod +x public/capture.sh && sh public/capture.sh`
Expected: prints `captured: detail.html economy.html hero.html palette.html summary.html`; each file starts with `<pre class="tui">`.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/Cargo.toml crates/zoid-tui/examples/web_capture.rs public/capture.sh public/frames
git commit -m "feat(site): web_capture example + scene capture script"
```

---

### Task 4: The teaser page template (uses the **frontend-design** skill)

Author `public/template.html` — hero + four flagship sections + "how it's built" strip + footer — self-contained (inline CSS/JS), terminal-authentic, responsive, motion opt-out, accessible. Frame slots are HTML comment markers filled by Task 5.

> **Use the `frontend-design` skill for the visual execution.** This task pins the structure, exact copy (spec §3), tokens, marker names, and acceptance criteria; frontend-design carries the aesthetic (spacing, rhythm, hierarchy, motion).

**Files:**
- Create: `crates/… n/a` — Create: `public/template.html`

**Interfaces:**
- Consumes: frame markers filled by Task 5.
- Produces: markers `<!--FRAME:hero-->`, `<!--FRAME:economy-->`, `<!--FRAME:palette-->`, `<!--FRAME:summary-->`, `<!--FRAME:detail-->` — each on its own line, to be replaced by the matching `public/frames/*.html` fragment.

- [ ] **Step 1: Scaffold the document with inline token CSS**

Create `public/template.html`. Head + token `:root` (copy the token block verbatim from Global Constraints) + base `.tui` styling:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>zoid — the coding agent that adapts as fast as the ecosystem</title>
<meta name="description" content="zoid — a terminal-native AI coding agent built in Rust. Active context management, importable modes, semantic zoom. Coming soon.">
<style>
:root{
  --bg:#0d1117; --panel:#161b22; --gutter:#0b0e13; --line:#30363d; --line2:#21262d;
  --txt:#c9d1d9; --muted:#8b949e; --dim:#6e7681;
  --acc:#58a6ff; --acc2:#79c0ff; --chip:#0d2a4d;
  --ok:#3fb950; --warn:#d29922; --err:#f85149; --br:#bc8cff; --pink:#f778ba;
  --mono:"JetBrains Mono","SF Mono",Menlo,Consolas,monospace;
}
*{box-sizing:border-box;}
html,body{margin:0;background:var(--bg);color:var(--txt);font-family:var(--mono);
  font-size:15px;line-height:1.6;-webkit-font-smoothing:antialiased;}
body{overflow-x:hidden;}
a{color:var(--acc);}
.wrap{max-width:1120px;margin:0 auto;padding:0 24px;}
/* Terminal frame: horizontal-scroll on narrow screens, never breaks the page. */
.frame{overflow-x:auto;border:1px solid var(--line);border-radius:10px;background:var(--bg);
  box-shadow:0 8px 40px rgba(0,0,0,.4);}
pre.tui{margin:0;padding:14px 16px;font-family:var(--mono);font-size:12px;line-height:1.5;
  color:var(--txt);background:var(--bg);white-space:pre;}
@media (max-width:640px){pre.tui{font-size:10px;}}
</style>
</head>
<body>
<!-- sections inserted in the following steps -->
</body>
</html>
```

- [ ] **Step 2: Hero**

Insert into `<body>` (exact copy from spec §3):

```html
<header class="wrap" role="banner" style="padding-top:96px;padding-bottom:56px;text-align:center;">
  <div style="font-size:34px;font-weight:700;letter-spacing:.02em;">zoid</div>
  <h1 style="font-size:clamp(26px,5vw,46px);line-height:1.2;margin:18px 0 10px;font-weight:600;">
    The coding agent that <span style="color:var(--acc2);">adapts as fast as the ecosystem</span>.
  </h1>
  <p style="color:var(--muted);margin:0 0 8px;">Terminal-native · Built in Rust · One ~6&nbsp;MB binary</p>
  <p style="color:var(--acc);font-weight:600;letter-spacing:.08em;text-transform:uppercase;font-size:13px;margin:0 0 40px;">
    <span class="blink">▌</span> Coming soon
  </p>
  <div class="frame" role="img" aria-label="zoid terminal: a chat session diagnosing a 500 error, with a live context economy rail.">
<!--FRAME:hero-->
  </div>
</header>
```

- [ ] **Step 3: The four flagship sections**

Insert a `<main>` with four `<section>`s. Each: eyebrow, `<h2>` headline, body `<p>`, and a `.frame` with the `role="img"`/`aria-label` and marker. Use the **exact** eyebrow/headline/body wording from spec §3 (§3.1–§3.4). Alternate `frame-left`/`frame-right` on wide viewports via a two-column grid that stacks under 820px. §3 is one frame; §3 "Semantic zoom" holds **two** frames (`summary` + `detail`) side by side. Skeleton for one section (repeat, swapping copy + marker; Modes uses `<!--FRAME:palette-->`, zoom uses both `<!--FRAME:summary-->` and `<!--FRAME:detail-->`):

```html
<main class="wrap" role="main">
  <section style="display:grid;gap:32px;grid-template-columns:1fr 1fr;align-items:center;padding:56px 0;border-top:1px solid var(--line2);">
    <div>
      <div style="color:var(--acc);text-transform:uppercase;letter-spacing:.14em;font-size:12px;">context economy</div>
      <h2 style="font-size:clamp(22px,3vw,30px);margin:10px 0 14px;font-weight:600;">The <em>right</em> context — not just the recent context.</h2>
      <p style="color:var(--muted);">zoid continuously curates the model's working set: it drops what stopped mattering, compacts what's verbose, and <strong style="color:var(--txt);">narrates every move so you can see and undo it.</strong> A coding agent lives or dies on what's in the window — zoid makes that a managed resource, measured in tokens.</p>
    </div>
    <div class="frame" role="img" aria-label="zoid context economy: a token ledger with per-item heat and churn/cache sparklines.">
<!--FRAME:economy-->
    </div>
  </section>
  <!-- §2 Modes (frame-right→left), §3 Semantic zoom (two frames), §4 Rust-native (stat band) … -->
</main>
```

- [ ] **Step 4: "How it's built" strip + footer**

Append a text strip (no frame) and footer:

```html
<section class="wrap" style="padding:48px 0;border-top:1px solid var(--line2);color:var(--muted);text-align:center;">
  <p style="margin:0;">
    <span style="color:var(--br);">⎇</span> event-sourced spine — the conversation is a database &nbsp;·&nbsp;
    modal (vim-like) interaction &nbsp;·&nbsp;
    multi-provider — Ollama local &amp; cloud, Anthropic &nbsp;·&nbsp;
    orchestrated subagents
  </p>
</section>
<footer class="wrap" role="contentinfo" style="padding:56px 0;text-align:center;color:var(--dim);">
  <div style="font-weight:700;color:var(--txt);">zoid</div>
  <p style="margin:8px 0 0;">Coming soon.</p>
  <p style="margin:4px 0 0;font-size:12px;">© 2026</p>
</footer>
```

- [ ] **Step 5: Motion, gated by reduced-motion**

Add to the `<style>` block: a caret blink and a subtle sparkline shimmer, both suppressed under reduced-motion:

```css
@keyframes blink{50%{opacity:0;}}
.blink{animation:blink 1.1s step-end infinite;}
@media (prefers-reduced-motion: reduce){
  .blink{animation:none;}
  *{scroll-behavior:auto;}
}
```

- [ ] **Step 6: Apply the frontend-design skill for polish**

Use **frontend-design** to refine spacing rhythm, section alternation, type hierarchy, and the optional Shift+Tab mode-swap micro-loop on the Modes section — staying within the tokens and the no-external-asset constraint. Keep all five markers intact and on their own lines.

- [ ] **Step 7: Verify structure (markers present, no external assets yet)**

Run: `grep -c 'FRAME:' public/template.html` → Expected: `5`.
Run: `grep -nE 'https?://' public/template.html` → Expected: no matches (exit 1 / empty).

- [ ] **Step 8: Commit**

```bash
git add public/template.html
git commit -m "feat(site): terminal-authentic teaser page template"
```

---

### Task 5: Assemble `index.html` + verify self-contained

Inject the captured frames into the template to produce the shipped self-contained page, then verify.

**Files:**
- Create: `public/build.sh`
- Create (generated): `public/index.html`
- Create: `public/README.md` (how to regenerate + hosting note)

**Interfaces:**
- Consumes: `public/template.html` markers (Task 4), `public/frames/*.html` (Task 3).
- Produces: `public/index.html` (final artifact, zero external references).

- [ ] **Step 1: Write the build script**

Create `public/build.sh` (executable) — replace each `<!--FRAME:x-->` marker with the contents of `public/frames/x.html`:

```sh
#!/bin/sh
# Assemble public/index.html from template.html + captured frames.
# Regenerate frames first with: sh public/capture.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/site"
[ -d frames ] || { echo "run capture.sh first (no frames/)"; exit 1; }
cp template.html index.html
for f in frames/*.html; do
  name=$(basename "$f" .html)
  # Replace the marker line with the fragment file's contents.
  awk -v marker="<!--FRAME:$name-->" -v file="$f" '
    $0 ~ marker { while ((getline line < file) > 0) print line; next }
    { print }
  ' index.html > index.html.tmp && mv index.html.tmp index.html
done
echo "built public/index.html"
```

- [ ] **Step 2: Build the page**

Run: `chmod +x public/build.sh && sh public/build.sh`
Expected: prints `built public/index.html`.

- [ ] **Step 3: Verify self-contained (Global Constraint)**

Run: `grep -nE 'src=|href=|url\(|https?://' public/index.html | grep -vE '#|mailto:'`
Expected: no external asset references (no `http(s)://`, no remote `src`/`href`). Empty result.
Run: `grep -c 'FRAME:' public/index.html` → Expected: `0` (all markers replaced).

- [ ] **Step 4: Visual verification in a browser**

Per the `verify` discipline, open `public/index.html` and confirm by observation: hero frame renders in zoid colors; all five frames present and colored; page has no horizontal body scroll at ~1440px and ~390px (frames scroll within their `.frame` containers); with reduced-motion emulated, the caret does not blink. (Use the chrome-devtools / claude-in-chrome tooling or a local file open.)

- [ ] **Step 5: Write `public/README.md`**

Create `public/README.md`:

```markdown
# zoid teaser site

Self-contained, terminal-authentic teaser page. No build step at serve time —
`index.html` is fully inlined.

## Regenerate
```sh
sh public/capture.sh   # re-render TUI frames from the live renderer → public/frames/
sh public/build.sh     # inject frames into template.html → public/index.html
```

## Hosting
`index.html` is portable — drop it on any static host. For GitHub Pages, publish
from a **public** repo (e.g. `zoid-site`), since the source repo is private.
Do not enable Pages on the private source repo.
```

- [ ] **Step 6: Commit**

```bash
git add public/build.sh public/index.html public/README.md
git commit -m "feat(site): assemble self-contained index.html + docs"
```

---

## Self-Review

**Spec coverage:**
- §1/§3 positioning + hero → Task 4 Steps 1–2. ✓
- §3 four flagship sections (exact copy) → Task 4 Step 3. ✓
- §3 closing strip + footer → Task 4 Step 4. ✓
- §4 site structure (alternating, frames-as-hero) → Task 4 Step 3 + frontend-design (Step 6). ✓
- §5 capture harness (shared scenes, feature-gated converter, honesty rule) → Tasks 1–3 + Global Constraints. ✓
- §6 design system tokens/glyphs/font → Task 4 Step 1 (verbatim tokens) + Global Constraints. ✓
- §7 build & hosting (Approach A, single file, portable) → Task 5 + `public/README.md`. ✓
- §8 responsive / motion / a11y → Task 4 Steps 1,5 (overflow, reduced-motion, `role="img"`/labels) + Task 5 Step 4. ✓
- §9 testing (fidelity, self-contained grep, responsive, reduced-motion) → Task 2 Step 3, Task 5 Steps 3–4. ✓
- §10 out of scope — no backend/pricing/SSG introduced. ✓
- §11 risks — Modes honesty (Global Constraint + Task 3 scenes), wide-glyph alignment (Task 1 `unicode-width` advance), no web-font (Task 4 font stack), hosting (Task 5 README). ✓

**Placeholder scan:** The `/* … exact body … */` markers in Task 2 Step 1 are deliberate *move* instructions (copy verbatim from the read-in `preview.rs`), not invented code — the source is a real file the implementer reads. Task 4 delegates aesthetic polish to frontend-design but pins all copy, structure, tokens, markers, and acceptance tests. No `TBD`/`handle edge cases`/unshown code elsewhere.

**Type consistency:** `buffer_to_html(&Buffer) -> String` (Task 1) is called with `&buf` in Task 3 Step 2. `render_shell_scene(name,w,h) -> Buffer` (Task 2) feeds it. Marker names `hero/economy/palette/summary/detail` are identical across Task 3 (capture filenames), Task 4 (template markers), Task 5 (injection loop). Feature name `web-capture` consistent across Cargo feature, `required-features`, and all run commands.
