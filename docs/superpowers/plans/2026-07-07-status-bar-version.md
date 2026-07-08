# Top Status Bar Binary Version — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the running binary's crate version (`v0.1.2`) flush-left in the top status bar, keeping the "zoid" wordmark centered and the palette hint flush-right.

**Architecture:** Extract the top bar's line construction from `render_title` into a pure `title_line(w: usize) -> Line<'static>` helper, then have that helper emit a third, left-flush version span that overlays the existing left padding. The wordmark-centering and hint-right-alignment arithmetic is unchanged, so the version is purely additive and degrades to the original bar on very narrow terminals. The pure helper is unit-tested directly at multiple widths.

**Tech Stack:** Rust, ratatui 0.30 (`Line`/`Span`/`Paragraph`/`Style`), `unicode_width::UnicodeWidthStr`, `insta` snapshot tests, `env!`/`concat!` compile-time macros.

## Global Constraints

- Crate version comes from `env!("CARGO_PKG_VERSION")`; `zoid-tui` inherits `version.workspace = true`, so this equals `zoid --version` (workspace `version = "0.1.2"`). Copied verbatim: the displayed string is `concat!("v", env!("CARGO_PKG_VERSION"))`.
- NO new `build.rs`, NO git SHA / build date / dirty flag. Bare semver only.
- Version style MUST match existing bar chrome: `Style::new().fg(color::DIM)` (`DIM = Rgb(0x6e,0x76,0x81)`).
- The wordmark's centered column and the hint's flush-right position MUST render byte-for-byte identically to the pre-change bar; the version only occupies previously-empty left padding.
- Graceful degradation: when the left pad cannot hold the version plus a ≥1-column gap (`pad < ver_w + 1`), render the original two-element bar with no version — never overflow, never shift the wordmark.
- Commit messages MUST NOT include any `Co-Authored-By` / co-author trailer.

---

### Task 1: Version chip in the top status bar

**Files:**
- Modify: `crates/zoid-tui/src/render.rs` — add module const `VERSION`, extract `title_line`, slim `render_title` (current `render_title` is lines 220-241; inline test module is at line 1434).
- Test: `crates/zoid-tui/src/render.rs` — new unit tests in the existing `#[cfg(test)] mod tests` (line 1434).
- Regenerate (do not hand-edit): `crates/zoid-tui/tests/snapshots/*` fixtures whose frames include the title row (driven by `tasks_snapshot.rs`, `session_snapshot.rs`, and any other `render_shell` snapshot test).

**Interfaces:**
- Consumes: `color::DIM` (`crate::tokens`), `unicode_width::UnicodeWidthStr` (already imported at `render.rs:25`), ratatui `Line`/`Span`/`Style`/`Paragraph` (already imported), `ShellState`, `Rect`, `Frame`.
- Produces:
  - `const VERSION: &str` = `concat!("v", env!("CARGO_PKG_VERSION"))` (module-private).
  - `fn title_line(w: usize) -> Line<'static>` (module-private, pure).
  - `fn render_title(frame: &mut Frame, _state: &ShellState, area: Rect)` — signature UNCHANGED (still called as `render_title(frame, state, layout.title)` at `render.rs:168`).

- [ ] **Step 1: Write the failing unit tests**

Add to the existing `#[cfg(test)] mod tests { ... }` block in `crates/zoid-tui/src/render.rs` (the block starting at line 1434, which already has `use super::*;`). Append these two tests plus the small text helper:

```rust
    /// Flatten a `Line`'s spans back into the visible string.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn title_shows_version_flush_left_and_keeps_wordmark_centered() {
        let line = title_line(100);
        let text = line_text(&line);
        // Version is the leftmost visible content.
        assert!(
            text.starts_with(VERSION),
            "version should be flush-left: {text:?}"
        );
        assert!(text.contains("zoid"), "wordmark present: {text:?}");
        assert!(
            text.trim_end().ends_with("palette"),
            "hint stays flush-right: {text:?}"
        );
        // Wordmark start column is unchanged by the version: still (w - 4) / 2.
        assert_eq!(
            text.find("zoid").unwrap(),
            (100 - 4) / 2,
            "wordmark must remain centered: {text:?}"
        );
    }

    #[test]
    fn title_drops_version_when_left_pad_too_narrow() {
        // width 16 -> pad = (16 - 4) / 2 = 6, which is < ver_w(6) + 1, so the
        // version is dropped and the wordmark stays centered.
        let line = title_line(16);
        let text = line_text(&line);
        assert!(
            !text.contains(VERSION),
            "version dropped when it cannot fit the left pad: {text:?}"
        );
        assert!(text.contains("zoid"), "wordmark still present: {text:?}");
        assert_eq!(
            text.find("zoid").unwrap(),
            (16 - 4) / 2,
            "wordmark still centered in the fallback: {text:?}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `cargo test -p zoid-tui --lib title_ 2>&1 | tail -20`
Expected: compile error — `cannot find function 'title_line' in this scope` and `cannot find value 'VERSION' in this scope`. (This is the "failing" state for a not-yet-written function.)

- [ ] **Step 3: Add the `VERSION` const and the pure `title_line` helper, and slim `render_title`**

Replace the entire current `render_title` function (`crates/zoid-tui/src/render.rs:220-241`) with the following const + two functions:

```rust
/// The running crate version, e.g. `v0.1.2`. Resolved at compile time from the
/// workspace `version` (`zoid-tui` inherits `version.workspace = true`), so it
/// always matches `zoid --version`.
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

/// Build the one-row top status bar for inner width `w`.
///
/// Three zones on a single line: the crate `VERSION` flush-left, the `zoid`
/// wordmark centered, and the palette hint flush-right. The wordmark-centering
/// and hint-right-alignment math is identical to the pre-version bar — the
/// version merely overlays the left padding that used to be blank spaces. When
/// the left pad cannot hold the version plus a one-column gap (`pad < ver_w + 1`,
/// i.e. a very narrow terminal) the version is dropped and the original
/// two-element bar renders unchanged.
fn title_line(w: usize) -> Line<'static> {
    let wordmark = "zoid";
    let palette_hint = "Esc interrupt · : command · ^P palette";
    let wm_w = wordmark.width();
    let pad = w.saturating_sub(wm_w) / 2;
    let ver_w = VERSION.width();

    let mut spans = Vec::new();
    if pad >= ver_w + 1 {
        spans.push(Span::styled(VERSION, Style::new().fg(color::DIM)));
        spans.push(Span::styled(" ".repeat(pad - ver_w), Style::new()));
    } else {
        spans.push(Span::styled(" ".repeat(pad), Style::new()));
    }
    spans.push(Span::styled(wordmark.to_string(), Style::new().fg(color::DIM)));

    let used = pad + wm_w;
    let right_pad = w.saturating_sub(used).saturating_sub(palette_hint.width());
    if right_pad > 0 {
        spans.push(Span::styled(" ".repeat(right_pad), Style::new()));
    }
    spans.push(Span::styled(
        palette_hint.to_string(),
        Style::new().fg(color::DIM),
    ));
    Line::from(spans)
}

fn render_title(frame: &mut Frame, _state: &ShellState, area: Rect) {
    frame.render_widget(Paragraph::new(title_line(area.width as usize)), area);
}
```

Notes for the implementer:
- `VERSION` is `&'static str`, so `Span::styled(VERSION, ...)` borrows a static and keeps `Line<'static>` valid; the space runs and `wordmark.to_string()` are owned `String`s (also `'static`).
- `.width()` is `UnicodeWidthStr::width`, already in scope via `use unicode_width::UnicodeWidthStr;` at `render.rs:25`.
- Do NOT change `render_title`'s signature or its call site at `render.rs:168` (`render_title(frame, state, layout.title)`); `_state` stays intentionally unused.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p zoid-tui --lib title_ 2>&1 | tail -20`
Expected: PASS — `title_shows_version_flush_left_and_keeps_wordmark_centered` and `title_drops_version_when_left_pad_too_narrow` both green.

- [ ] **Step 5: Regenerate the frame snapshots that include the title row**

The version now appears on the top row of every full-frame snapshot. Run the crate's test suite to surface the pending snapshot diffs:

Run: `cargo test -p zoid-tui 2>&1 | tail -30`
Expected: snapshot assertion failures in title-bearing frames. Full blast radius (~46 fixtures across three files — all driven by `render_shell`, which renders the title row):
- `crates/zoid-tui/tests/snapshots/shell_snapshot__*` — **~35 fixtures, the dominant file** (driven by `tests/shell_snapshot.rs`). Expect most of your diffs here.
- `crates/zoid-tui/tests/snapshots/tasks_snapshot__*` — ~8 fixtures.
- `crates/zoid-tui/tests/snapshots/session_snapshot__*` — ~3 fixtures.
- UNAFFECTED (do NOT expect diffs): `chat_snapshot__*` and `syntax_snapshot__*` — they drive `render_chat`/`conversation_view` directly, never `render_shell`, so they have no title row.

Review each pending snapshot and confirm the ONLY change is the added left-zone version (`v0.1.2` replacing leading blanks on row 1) — the wordmark and every other row must be unchanged:

Run: `cargo insta review`
- Accept a snapshot only if its diff is confined to the top row's left zone.
- If any diff touches the wordmark column or any body/status row, STOP — the arithmetic regressed; re-check Step 3.
- **Expect some full-frame overlay snapshots to show NO diff at all** and this is correct: `render_config` and `render_provider_switch` call `frame.render_widget(Clear, frame.area())` (`render.rs:1034`, `render.rs:1281`), overpainting row 0 — the version is hidden behind the overlay, so those ~5 fixtures are unchanged. Nothing to accept for them.
- Before committing, run `git status` and confirm no stray `*.snap.new` (rejected) files remain — `git add .../snapshots` in Step 7 would otherwise stage them.

(Non-interactive alternative once diffs are confirmed: `cargo insta accept`.)

- [ ] **Step 6: Run the full workspace test suite**

Run: `cargo test 2>&1 | tail -20`
Expected: all workspace tests pass (green), including the regenerated `zoid-tui` snapshots. (`crates/zoid/tests/version_embed.rs` only asserts `CARGO_PKG_VERSION` is valid semver — this change does not touch it, and it stays green.)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/render.rs crates/zoid-tui/tests/snapshots
git commit -m "feat(tui): show binary version flush-left in top status bar"
```

---

## Self-Review

**Spec coverage** (against `docs/superpowers/specs/2026-07-07-status-bar-version-design.md`):
- Left-zone placement, wordmark centered, hint flush-right → `title_line` zone construction (Step 3) + centering assertion (Step 1).
- Bare semver `v0.1.2`, no build.rs → `VERSION` const via `concat!/env!` (Step 3); Global Constraints forbid build machinery.
- `color::DIM` styling → applied to the version span (Step 3).
- Graceful degradation (`pad < ver_w + 1`) → fallback branch (Step 3) + narrow-width test (Step 1/Step 4).
- Compile-time static, no hot-path alloc → `const VERSION: &str` (Step 3).
- Snapshot regeneration expected → Step 5; full suite green → Step 6.

**Placeholder scan:** none — every code and command step is concrete.

**Type consistency:** `VERSION: &str`, `title_line(w: usize) -> Line<'static>`, and `render_title(&mut Frame, &ShellState, Rect)` are used identically in the tests (Step 1) and the implementation (Step 3). `line_text` takes `&Line` and reads `span.content` (a `Cow<str>` → `.as_ref()`), matching ratatui's `Span` API.
