# Reduce Horizontal Scrollbars — Design

**Date:** 2026-07-13
**Surface:** `public/index.html` (the zoid marketing site, mirrored to `strvmarv/zoid-releases` → GitHub Pages)
**Type:** CSS / markup-attribute change only. No Rust, no capture-pipeline, no JS behavior change.

## Problem

The page currently renders **four** horizontal scrollbars, one per element that is
physically wider than its container:

| # | Element | Native width | Container cap | Cause |
|---|---------|--------------|---------------|-------|
| 1 | context-economy frame (hero, ~line 299) | ~1282px | `.wrap` = 1072px | 160-col `pre.tui` at 13px mono |
| 2 | tools-models frame (~line 480) | ~1282px | `.wrap` = 1072px | same |
| 3 | extensibility frame (~line 659) | ~1282px | `.wrap` = 1072px | same |
| 4 | `.oneliner` bash install command (line 294) | ~850px (118 ch) | `.cta` = **760px** | `width:100%` of a 760px-max flex column |

`.wrap` is `max-width:1120px; padding:0 24px` → 1072px usable. Each animated player's
`pre.tui` is a fixed 160×40 capture (the capture pipeline mandates ≥160×40), which at
13px monospace is ~1282px. Every frame therefore overflows the content column and grows
its own `overflow-x:auto` scrollbar at essentially every viewport size. The one-liner is
a distinct case: it fits inside `.wrap` but is pinned to `width:100%` of the 760px `.cta`.

## Chosen behavior

**Breakout wide, scroll narrow** (user decision, 2026-07-13):

- On viewports wide enough to hold the native render, the figure **breaks out of the
  1072px content column and centers on the viewport** — no scrollbar. Fidelity is
  preserved 1:1 (no scaling, no reflow).
- On viewports narrower than the render, the frame keeps a **single** per-frame
  horizontal scrollbar as the fallback. The **page** never scrolls sideways
  (`body{overflow-x:hidden}` is the backstop).

Net: 4 scrollbars → **0 on desktop** (≥ ~1300px), and at most one confined *inside* each
figure on narrow screens. No scaling engine, no JS, no new frames (YAGNI).

## Components

### Component 1 — Shared `.bleed` wrapper (the three frames)

New reusable class:

```css
.bleed{
  width:100vw;
  margin-left:calc(50% - 50vw);   /* escape .wrap's cap, align to viewport left edge */
  display:flex;
  justify-content:center;          /* center the native-width frame in the viewport */
}
```

Applied as `class="figure bleed"` on the three `.figure` elements. `.frame` is unchanged
— it already carries `width:max-content; max-width:100%; overflow-x:auto`, and `max-width:100%`
now resolves against the 100vw band instead of the 1072px column. On a viewport ≥ the
render width the frame sits fully centered; on a narrower one it scrolls internally.

**Why not restructure the DOM (move figures out of `.wrap`):** rejected — real DOM surgery
on hero + two `.section-full` blocks, higher regression risk near the `<!--FRAMES:*-->`
capture markers, same visual result. The `.bleed` class is the minimal DRY change.

**Why not widen `.wrap`:** rejected — widening the column to ~1320px blows the ~54–64ch
readable measure the entire type system is built around. Scrollbars must not be fixed by
breaking typography.

### Component 2 — One-liner: fit-and-center (the bash command)

The one-liner (~850px) fits inside `.wrap` (1072px); it only scrolls because it is
`width:100%` inside the 760px `.cta`. Change it to size-to-content and center, and let it
exceed the 760px measure (its own `max-width`, since the button/blurb should stay at 760px):

```css
.oneliner{
  display:block;
  width:fit-content;          /* was width:100% */
  max-width:min(100%, 900px); /* show the full ~850px command; still bounded */
  margin-inline:auto;         /* center within the CTA */
  overflow-x:auto;            /* narrow-screen (<~900px) fallback only */
  white-space:pre;            /* unchanged, plus existing bg/border/padding/color */
}
```

`.cta` keeps `max-width:760px` for the button and beta note; the one-liner is allowed to
render wider (up to 900px) and center. No `.bleed` needed — it never exceeds `.wrap`.

### Component 3 — `100vw` offset neutralizer

Add `scrollbar-gutter: stable` to the root element so `100vw` and the visible viewport
width agree when a vertical scrollbar is present, keeping the centered frames from
shifting a few px off-center. `body{overflow-x:hidden}` remains as the page-scroll
backstop (already present).

## Verification

**Automated (regression guard):** this is pure CSS/attribute change, so the frame buffers
do not move. `cargo test -p zoid-tui --features web-capture --example web_capture` must
still pass (confirms nothing in the capture path was disturbed).

**Browser gate (the real acceptance check),** served locally via `python3 -m http.server`
against the assembled `public/index.html`:

- **Wide viewport (≥1300px):** `document.documentElement.scrollWidth === clientWidth`
  (no page horizontal scroll) AND for each `.frame`, `frame.scrollWidth === frame.clientWidth`
  (no per-frame scrollbar). The `.oneliner` is fully visible and centered.
- **Mid viewport (~1000px):** page still does not scroll horizontally; each `.frame`
  scrolls internally (`scrollWidth > clientWidth`); frames remain centered within their band.
- **Narrow viewport (~390px):** page does not scroll horizontally; frames and one-liner
  scroll internally as the single fallback.

Mirrors the `pageHScroll: 0` browser check used in the prior §2 rework.

## Out of scope (YAGNI)

- No `transform: scale` fit-to-width engine (that was the rejected "scale to fit" option).
- No capture-pipeline width change (≥160×40 constraint is fixed).
- No JS, no ResizeObserver, no new scenes or frames.
- No changes to `.term` mini-terminals in the zoom section (CSS-bordered, already fluid).

## Files

- Modify: `public/index.html`
  - CSS block: add `.bleed`; edit `.oneliner`; add `scrollbar-gutter:stable` to root.
  - Markup: add `bleed` to the three `.figure` class attributes (lines ~299, ~480, ~659).
- No other files change. Deploy is the standard push-to-main → `publish-site` → Pages mirror.
