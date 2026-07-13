# Reduce Horizontal Scrollbars Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the four horizontal scrollbars on the zoid marketing page on desktop, confining any remaining scroll to inside a single figure on narrow screens.

**Architecture:** Pure CSS + class-attribute change in one file (`public/index.html`). Three fixed-width TUI player figures break out of the 1072px content column and center on the viewport via a shared `.bleed` class; the bash one-liner sizes to its content and centers within the CTA; a root `scrollbar-gutter:stable` keeps the `100vw` breakout perfectly centered. No Rust, no JS, no capture-pipeline change.

**Tech Stack:** Static HTML/CSS. Regression guard: `cargo test -p zoid-tui --features web-capture`. Acceptance: browser measurements against a locally served copy.

## Global Constraints

- Change **only** `public/index.html` (plus this repo's docs). No Rust, no JS, no new frames, no capture-pipeline edits.
- Native render fidelity is preserved 1:1 — **no `transform: scale`, no font-size change, no reflow** of the `pre.tui` frames.
- Behavior contract: **breakout wide, scroll narrow.** Desktop (viewport ≥ ~1300px) shows every figure fully centered with no scrollbar; narrower viewports keep at most one horizontal scrollbar *inside* each figure. The **page** (`document.documentElement`) must never scroll horizontally at any width.
- `body{overflow-x:hidden}` (line 19) stays as the page-scroll backstop — do not remove it.
- Do not widen `.wrap` and do not restructure the DOM around the `<!--FRAMES:*-->` capture markers.
- Deploy is out of scope for this plan (standard push-to-main → `publish-site` → Pages happens at finish time, not per-task).

**Reference — current exact state (verify before editing; line numbers may drift):**
- `public/index.html:17` — `html,body{margin:0;background:var(--bg);...}`
- `public/index.html:19` — `body{overflow-x:hidden;}`
- `public/index.html:22` — `.wrap{max-width:1120px;margin:0 auto;padding:0 24px;}`
- `public/index.html:44` — `.oneliner{display:block;width:100%;overflow-x:auto;white-space:pre;background:var(--panel);`
- `public/index.html:299`, `:480`, `:659` — `<div class="figure">` (the three animated players: context-economy hero, tools-models, extensibility)

---

### Task 1: Full-bleed breakout for the three frame figures

Break the three fixed-width player figures out of the 1072px `.wrap` column so they center on the viewport at native size (no scrollbar) on wide screens, while each frame's existing `overflow-x:auto` remains the narrow-screen fallback. Add `scrollbar-gutter:stable` so the `100vw` breakout stays centered when a vertical scrollbar is present.

**Files:**
- Modify: `public/index.html` (CSS block near line 22; markup at lines ~299, ~480, ~659)

**Interfaces:**
- Consumes: existing `.frame{width:max-content;max-width:100%;overflow-x:auto;...}` (unchanged) and `body{overflow-x:hidden}` (unchanged).
- Produces: a reusable `.bleed` class (`width:100vw;margin-left:calc(50% - 50vw);display:flex;justify-content:center;`) applied to `.figure` elements. Task 2 does not depend on this.

- [ ] **Step 1: Add the `.bleed` class and root scrollbar-gutter**

In the CSS, immediately after the `.wrap` rule (currently `public/index.html:22`), add:

```css
/* Full-bleed: let a fixed-width figure escape .wrap's column and center on the
   viewport. On viewports wider than the figure it shows fully with no scrollbar;
   narrower, the figure's own overflow-x:auto is the single fallback. body's
   overflow-x:hidden keeps the page itself from ever scrolling sideways.
   LOAD-BEARING: the narrow-screen fallback works only because .frame has
   overflow-x:auto — that resets its flex min-size to 0 so it can shrink below its
   ~1282px content width inside this 100vw band. Do not strip overflow-x off .frame. */
.bleed{width:100vw;margin-left:calc(50% - 50vw);display:flex;justify-content:center;}
```

Then, immediately after the `body{overflow-x:hidden;}` rule (currently `public/index.html:19`), add:

```css
/* Reserve the vertical-scrollbar gutter to prevent horizontal layout shift /
   scrollbar-flash when a scrollbar appears/disappears. NOTE: this does NOT shrink
   100vw (100vw always includes the gutter); centering is correct regardless because
   .bleed's 50% tracks the actual content width. body{overflow-x:hidden} is what
   clips the ~15px 100vw overhang and prevents any page-level horizontal scroll. */
html{scrollbar-gutter:stable;}
```

- [ ] **Step 2: Apply `bleed` to the three figures**

Change each of the three player wrappers from `<div class="figure">` to `<div class="figure bleed">`:
- context-economy hero (currently `public/index.html:299`)
- tools-models (currently `public/index.html:480`)
- extensibility (currently `public/index.html:659`)

Use an exact-match replace on `    <div class="figure">` — note there are exactly three `class="figure"` occurrences and all three are player wrappers; do not touch any `class="figure bleed"` you have already changed. Verify count before and after:

```bash
grep -c 'class="figure"' public/index.html   # expect 0 after all three are changed
grep -c 'class="figure bleed"' public/index.html   # expect 3 after
```

- [ ] **Step 3: Run the capture-path regression guard**

Confirm the CSS/markup change did not disturb anything in the render/capture path.

Run: `cargo test -p zoid-tui --features web-capture --example web_capture`
Expected: PASS. **Honest scope:** this asserts against the `pre.tui` *buffer text*, not page
layout, so it does not directly guard this CSS change — it only confirms no Rust/capture file
was touched by accident. It is a cheap sanity check, not the acceptance test; the browser
measurement in Step 4 is the real gate.

- [ ] **Step 4: Serve the page and measure the breakout in a browser**

Serve the working copy (no assembly needed — the frames are already inlined in `public/index.html`):

```bash
cd public && python3 -m http.server 8099 >/dev/null 2>&1 &
```

Load `http://localhost:8099/index.html` in the browser tool. At a **wide** viewport (≥1300px, e.g. 1440×900) run this in the page console and record the result:

```js
(() => { const vw = document.documentElement.clientWidth;
  return JSON.stringify({
    pageHScroll: document.documentElement.scrollWidth - vw,
    frames: [...document.querySelectorAll('.frame')].map(f => f.scrollWidth - f.clientWidth),
    bleed: [...document.querySelectorAll('.bleed')].map(b => { const r = b.getBoundingClientRect();
      return { overflowR: Math.round(r.right - vw), centered: Math.abs((r.left + r.right)/2 - vw/2) < 4 }; })
  }); })()
```

Expected at ≥1300px: `pageHScroll` is `0`; every entry in `frames` is `0` (no per-frame
scrollbar); every `bleed` entry has `overflowR <= 1` (right edge within the viewport) and
`centered: true` (the frame is actually centered — the breakout's whole point).

> Why the extra `bleed` check: `pageHScroll === 0` is near-tautological because
> `body{overflow-x:hidden}` clips overflow, so `scrollWidth` under-reports. The
> `getBoundingClientRect` breakout + centering assertion is the real acceptance signal.

Then resize to **~600px** wide and re-run the same snippet.
Expected at ~600px: `pageHScroll` is `0`; every entry in `frames` is `> 0` (each frame
scrolls internally — the intended fallback); every `bleed` entry still has `overflowR <= 1`
(no frame overhangs the viewport).

Stop the server when done:

```bash
pkill -f "http.server 8099" || true
```

- [ ] **Step 5: Commit**

```bash
git add public/index.html
git commit -m "fix(site): full-bleed breakout removes frame scrollbars on desktop"
```

---

### Task 2: Fit-and-center the bash one-liner

The `.oneliner` install command (~850px, 118 chars) fits inside `.wrap` (1072px) but scrolls only because it is pinned to `width:100%` of the 760px-max `.cta` column. **`.cta` is `display:flex; flex-direction:column; align-items:center; max-width:760px` with no horizontal padding** — so `%`- and `fit-content`-based widths resolve against 760px and clamp there, which cannot show an ~850px command. Give it an **intrinsic** `max-content` width (allowed to exceed the 760px parent), cap it below the viewport, and let `.cta`'s existing `align-items:center` center the overflowing box — so the full command shows with no scrollbar down to ~900px viewport, then scrolls as the narrow fallback.

**Files:**
- Modify: `public/index.html` (`.oneliner` rule, currently line 44)

**Interfaces:**
- Consumes: nothing from Task 1 (independent).
- Produces: nothing later tasks depend on.

- [ ] **Step 1: Change `.oneliner` from stretch-to-fill to fit-and-center**

Replace the width declaration in the `.oneliner` rule (currently `public/index.html:44`). Current:

```css
.oneliner{display:block;width:100%;overflow-x:auto;white-space:pre;background:var(--panel);
```

New (only `width:100%` becomes two declarations — `width:max-content` plus a `max-width`; everything else on the line and the wrapped line below stays byte-for-byte identical). **Do NOT add `margin-inline:auto`** — under negative free space it collapses to 0 and jams the box to the left edge, defeating centering; `.cta`'s `align-items:center` is what centers the overflowing one-liner:

```css
.oneliner{display:block;width:max-content;max-width:min(100vw - 48px,900px);overflow-x:auto;white-space:pre;background:var(--panel);
```

- `width:max-content` → the command's intrinsic ~850px, permitted to exceed the 760px `.cta`.
- `max-width:min(100vw - 48px, 900px)` → caps below the viewport (minus `.wrap`'s 48px padding) so it shrinks and `overflow-x:auto` becomes the fallback on narrow screens; ~900px ceiling on desktop. `%`/`fit-content` are deliberately avoided because they resolve against the 760px `.cta`.

- [ ] **Step 2: Serve the page and measure the one-liner**

Serve (if not already running from Task 1):

```bash
cd public && python3 -m http.server 8099 >/dev/null 2>&1 &
```

At a **wide** viewport (≥1300px), run in the page console:

```js
(() => { const o = document.querySelector('.oneliner');
  const r = o.getBoundingClientRect(), vw = document.documentElement.clientWidth;
  return JSON.stringify({
    onelinerHScroll: o.scrollWidth - o.clientWidth,          // 0 = fully visible, no scrollbar
    widthPx: Math.round(r.width),                            // expect ~850 (full command), not ~760
    centered: Math.abs((r.left + r.right)/2 - vw/2) < 24,    // true = centered within ~24px
    pageHScroll: document.documentElement.scrollWidth - vw   // must stay 0
  }); })()
```

Expected at ≥1300px: `onelinerHScroll` is `0` (full command visible, no scrollbar),
`widthPx` is ~850 (**not** clamped to ~760 — that clamp is the SEV-1 failure mode),
`centered` is `true`, and `pageHScroll` is `0`.

Then resize to **~600px** wide and re-run.
Expected at ~600px: `onelinerHScroll` is `> 0` (command scrolls internally — the intended fallback) AND `pageHScroll` is still `0`.

Stop the server:

```bash
pkill -f "http.server 8099" || true
```

- [ ] **Step 3: Commit**

```bash
git add public/index.html
git commit -m "fix(site): fit-and-center install one-liner, drop its default scrollbar"
```

---

## Notes for the implementer

- There is no unit-test-first cycle here because the change is pure presentational CSS with no JS logic to assert against. The honest test is the **browser measurement** in each task (page/frame/one-liner `scrollWidth − clientWidth`), plus the `cargo` capture-path regression guard in Task 1. Treat a non-zero `pageHScroll` at any tested width, or a non-zero frame/one-liner scroll at the wide viewport, as a **failing** check — fix before committing.
- The `100vw` breakout depends on `body{overflow-x:hidden}` staying in place; if a page-level scrollbar ever appears at a wide viewport, the first suspect is that rule being removed or a `.bleed` child adding intrinsic width beyond `100vw`.
- If the browser tool is unavailable, the fallback acceptance is a manual resize check by the human at ~1440px and ~600px against the same expected values.
- The two-column `.section{display:grid}` / `.section.rev .figure{order:-1}` rules (≈ lines 64–76) are **dead code** — `class="section"` appears zero times; all players are in `.section-full` or the hero. So `.bleed`'s `display:flex` cannot collide with a grid. Do not touch those rules in this plan (out of scope), but don't be alarmed by them either.
