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

**Load-bearing detail (not merely "unchanged"):** the narrow-screen fallback works *only*
because `.frame` has `overflow-x:auto`. As a flex item of `.bleed`, that `overflow` resets
the frame's automatic flex minimum to 0, letting it shrink below its ~1282px `max-content`
down to `max-width:100%` and scroll internally. If `overflow-x` is ever stripped from
`.frame`, the flex minimum reverts to ~1282px, the frame stops shrinking and overhangs the
`100vw` band (clipped, unreachable, no scrollbar). The implementation adds a short comment
on `.bleed` recording this dependency.

**Note:** the two-column `.section{display:grid}` / `.section.rev .figure{order:-1}` rules
(≈ lines 64–76) are **dead code** — `class="section"` appears zero times in the document;
every player lives in a `.section-full` block or the hero. So `.bleed`'s `display:flex`
cannot fight any grid.

**Why not restructure the DOM (move figures out of `.wrap`):** rejected — real DOM surgery
on hero + two `.section-full` blocks, higher regression risk near the `<!--FRAMES:*-->`
capture markers, same visual result. The `.bleed` class is the minimal DRY change.

**Why not widen `.wrap`:** rejected — widening the column to ~1320px blows the ~54–64ch
readable measure the entire type system is built around. Scrollbars must not be fixed by
breaking typography.

### Component 2 — One-liner: fit-and-center (the bash command)

The one-liner (~850px) fits inside `.wrap` (1072px); it only scrolls because it is
`width:100%` inside the 760px `.cta`. **`.cta` is a `display:flex; flex-direction:column;
align-items:center; max-width:760px` column with no horizontal padding** — so its content
box is exactly 760px. That is the load-bearing constraint: any `%`- or `fit-content`-based
width resolves *against* that 760px containing block and clamps to 760, which cannot show
the ~850px command. Only an **intrinsic** width (`max-content`) is allowed to overflow the
containing block, and only flex centering (`align-items:center`) centers an *overflowing*
item — `margin-inline:auto` collapses to 0 under negative free space and jams the box to
the left edge. So:

```css
.oneliner{
  display:block;
  width:max-content;                 /* was width:100% — intrinsic, may exceed the 760px .cta */
  max-width:min(100vw - 48px, 900px);/* cap below the viewport (minus .wrap's 48px padding); ~900 ceiling on desktop */
  overflow-x:auto;                   /* narrow-screen (<~900px) fallback only */
  white-space:pre;                   /* unchanged, plus existing bg/border/padding/color */
}
```

Do **not** add `margin-inline:auto` — `.cta`'s existing `align-items:center` centers the
(now overflowing) one-liner correctly, including when it is wider than the 760px column.
`.cta` keeps `max-width:760px` for the button and beta note; the one-liner deliberately
renders wider (up to ~850px, capped at 900px) and centers. No `.bleed` needed — at ~850px
it stays inside `.wrap`'s 1072px, so the page never scrolls.

### Component 3 — `scrollbar-gutter:stable` (layout-stability polish)

Add `scrollbar-gutter: stable` to the root element. **Correction from review:** this does
*not* make `100vw == visible width` — `100vw` always includes the scrollbar gutter. The
frames center correctly regardless, because `margin-left:calc(50% - 50vw)` uses `50%` of
the *actual* `.wrap` content width, which places the bleed box's center on the visible
center at every viewport. What `scrollbar-gutter:stable` actually buys us is **preventing
horizontal layout shift / scrollbar-flash** when a vertical scrollbar appears or disappears,
and a symmetric gutter across pages. It is polish, not a correctness dependency.

The real page-scroll backstop is **`body{overflow-x:hidden}`** (already present): it clips
the ~15px that `.bleed{width:100vw}` overhangs the content area, so no page-level horizontal
scrollbar ever appears. Do not remove it.

## Verification

**Automated (regression guard):** this is pure CSS/attribute change, so the frame buffers
do not move. `cargo test -p zoid-tui --features web-capture --example web_capture` must
still pass (confirms nothing in the capture path was disturbed).

**Browser gate (the real acceptance check),** served locally via `python3 -m http.server`
against `public/index.html`. Note `pageHScroll === 0` alone is *near-tautological* because
`body{overflow-x:hidden}` clips overflow so `scrollWidth` under-reports — it proves "no page
scrollbar," not "nothing clipped off-screen." So each width also asserts a real **breakout
check**: for every `.bleed`, its rendered right edge does not exceed the viewport
(`getBoundingClientRect().right ≤ clientWidth + 1`) AND its box is centered
(`|(left+right)/2 − clientWidth/2| < 4`).

- **Wide viewport (≥1300px):** page does not scroll horizontally; every `.frame` has
  `scrollWidth === clientWidth` (no per-frame scrollbar); every `.bleed` passes the breakout
  + centering check; the `.oneliner` is fully visible (`scrollWidth === clientWidth`) and
  centered (`|center − viewportCenter| < 24`).
- **Narrow viewport (~600px):** page does not scroll horizontally; each `.frame` **and** the
  `.oneliner` scroll internally (`scrollWidth > clientWidth`) as the single fallback; `.bleed`
  boxes still pass the breakout check (right edge within the viewport).

600px is chosen as the single narrow width because it exercises the fallback scrollbar on
*both* the frames (<~1300px) and the one-liner (<~900px) at once. Mirrors the `pageHScroll: 0`
browser check used in the prior §2 rework, hardened with the breakout assertion.

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
