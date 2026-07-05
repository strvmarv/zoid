# zoid — Marketing Teaser Site · Design

**Date:** 2026-07-05
**Status:** Approved design, ready for implementation plan
**Author:** strvmarv (with Claude)

> A pre-launch **teaser** site that markets zoid's vision and craft while the
> product is not yet available. Terminal-authentic look, real captured TUI
> frames, single self-contained page, hostable on GitHub Pages. Soft "coming
> soon" tone — no pricing, no waitlist backend. Terminal step after approval is
> `writing-plans`.

---

## 1. Overview

zoid is a cross-platform, terminal-native coding agent built in Rust and shipped
as a single ~6 MB static binary. It is not yet publicly available and is moving
toward a closed-source, trial + subscription model. This site is a **pure
teaser**: it sells the vision and the engineering craft, hints that access is
coming, and captures no data.

The site's job is to make a technical visitor think *"I want that"* and remember
the name. It does this by **showing the real product** — authentic, current TUI
frames rendered by zoid's own renderer — inside a page that speaks zoid's own
visual language (GitHub-dark palette, JetBrains Mono, the glyph vocabulary).

**One-line thesis (hero):** *the coding agent that adapts as fast as the
ecosystem.* This is drawn from the mode-runtime north star — "zoid is a thin,
stable host; the ecosystem moves fast around it" — and is the site's spine.

---

## 2. Goals & Non-Goals

### Goals
- **Communicate the vision and four flagship capabilities** clearly to a
  technical audience, each anchored by a real TUI frame.
- **Look unmistakably like zoid** — reuse the product's design tokens and glyphs
  so the site reads as an extension of the product, not a generic SaaS landing.
- **Zero-backend, trivially hostable** — a single self-contained page that runs
  on GitHub Pages (or any static host) with no build step at serve time.
- **Honest** — no fake screenshots; frames come from the real renderer. Features
  not yet built are represented truthfully.
- **Refreshable frames** — the mechanism that produces TUI frames is reusable so
  the visuals can be regenerated as the TUI evolves.

### Non-Goals
- **No data capture** — no waitlist, no email form, no analytics backend. (A
  soft "coming soon" note only.)
- **No pricing / subscription language** — the monetization model is not stated.
- **No multi-page site, blog, docs, or SSG toolchain.** One page.
- **No claim of general availability** or download links (there is no public
  download to offer yet).
- **Not a replacement for `docs/ux/`** — those remain the internal design source
  of truth; this site is outward-facing marketing.

---

## 3. Positioning & copy

**Tone:** confident, terse, engineering-credible. Short lines. No marketing
fluff, no exclamation marks. The audience is developers who live in a terminal.

**Hero:**
> **zoid**
> The coding agent that adapts as fast as the ecosystem.
> Terminal-native · Built in Rust · One ~6 MB binary
> **Coming soon.**

**Section copy (final wording refined during build, but anchored here):**

1. **Active Context Management** *(primary)*
   - Eyebrow: `context economy`
   - Headline: **The *right* context — not just the recent context.**
   - Body: zoid continuously curates the model's working set: it drops what
     stopped mattering, compacts what's verbose, and **narrates every move so
     you can see and undo it.** A coding agent lives or dies on what's in the
     window — zoid makes that a managed resource, measured in tokens.

2. **Modes** *(primary)*
   - Eyebrow: `extensible by design`
   - Headline: **As the market shifts, so does zoid.**
   - Body: Rich-import **any** skill set and it becomes a first-class **mode** —
     switch between them with **Shift+Tab**. No hardcoded feature surface, no
     waiting on a release: drop in a folder (or paste a URL) and zoid gains a
     capability. The core stays thin; the frontier moves.

3. **Semantic zoom**
   - Eyebrow: `the conversation is a database`
   - Headline: **One conversation, three altitudes.**
   - Body: Because the session is an event log and the UI is a projection over
     it, you can zoom out to scan the shape of a session — or zoom in to read
     every token. Same data, rendered at the altitude you need.

4. **Rust-native & lean**
   - Eyebrow: `no runtime to install`
   - Headline: **One binary. Cold-starts in milliseconds.**
   - Body: A single ~6 MB static binary per platform (Linux, macOS, Windows),
     ~10 ms cold start, checksum-verified self-update. Nothing to install
     around it.

**Closing strip — "how it's built":** one row of terse credibility points:
event-sourced spine (the conversation is a database) · modal (vim-like)
interaction · multi-provider (Ollama local & cloud, Anthropic) · orchestrated
subagents. No frame; text + glyphs only.

**Footer:** wordmark, a quiet **Coming soon** line, © year. No links that
promise a download or a signup.

---

## 4. Site structure

Single vertical-scroll page, self-contained `site/index.html`:

```
┌ hero ─────────────────────────────────────────────┐
│  wordmark · thesis line · subhead · "Coming soon"    │
│  a live-feeling chat TUI frame as the hero visual    │
├ §1 Active Context Management  (frame: economy) ─────┤
├ §2 Modes                      (frame: palette+chip) ─┤
├ §3 Semantic zoom              (frames: summary|detail)┤
├ §4 Rust-native & lean         (stat band + chat frame)┤
├ how it's built  (text strip, glyphs) ──────────────┤
└ footer  (wordmark · coming soon · ©) ──────────────┘
```

- **Alternating rhythm:** feature sections alternate frame-left / frame-right on
  wide viewports; stack frame-over-text on narrow.
- **Frames are the hero of each section** — large, sharp, colored exactly like
  the TUI. Copy is secondary and short.

---

## 5. The capture harness (the only production code)

A reusable example renders zoid scenes to **faithful, self-contained HTML
fragments** using the real renderer, so frames are authentic and refreshable.

- **Location:** `crates/zoid-tui/examples/web_capture.rs` (sibling to the
  existing `preview.rs`, reusing its scene fixtures).
- **Mechanism:** render a scene to a `ratatui::backend::TestBackend` buffer
  (exactly as the snapshot tests do), then walk every cell and emit an HTML
  `<pre class="tui">` where each run of same-styled cells becomes a
  `<span style="color:#rrggbb">` (and `background` when set). Multi-width glyph
  handling mirrors the renderer so alignment holds.
- **Color source:** the cell's own `Style.fg`/`bg` (already exact RGB from the
  design-tokens module) — the fragment needs no separate palette.
- **Scenes captured (reusing `preview.rs`'s set):** `chat` (hero), `economy`
  (§1), `palette` (§2), `summary` + `detail` (§3). Widths chosen per section
  (hero/economy/palette at 140×24; zoom pair narrower to sit side-by-side).
- **Output:** the example prints one fragment to stdout; a tiny wrapper
  (Makefile target or shell script `site/capture.sh`) runs it per scene and
  writes `site/frames/<scene>.html`, which the build step inlines.
- **Honesty rule for Modes (§2):** the Shift+Tab mode registry is Slice 3
  (designed, not fully built). Capture the **current** palette frame (the
  "Switch mode ▸" group exists in the design; if not yet in code at build time,
  capture the closest current palette state and present the switch as a
  near-term capability in copy — never fabricate a frame of unbuilt UI).

> The example is **headless and network-free** (TestBackend), so frames build in
> CI or locally with no terminal and no provider/API key.

---

## 6. Design system

Reuse zoid's tokens verbatim (from `docs/ux` / the design-tokens module):

- **Surfaces:** bg `#0d1117` · panel `#161b22` · gutter `#0b0e13` · line
  `#30363d` · line2 `#21262d`
- **Text:** primary `#c9d1d9` · muted `#8b949e` · dim `#6e7681`
- **Accent (Chat blue):** `#58a6ff` / `#79c0ff` · chip bg `#0d2a4d`
- **Status:** ok `#3fb950` · warn `#d29922` · error/del `#f85149` · branch
  `#bc8cff` · pink `#f778ba`
- **Type:** `"JetBrains Mono","SF Mono",Menlo,Consolas,monospace`. Ship the
  wordmark and body in this mono stack for total cohesion. (System-font fallback
  only; **no external font fetch** — keeps the page self-contained and CSP-safe.)
- **Glyphs:** `●✓◐☐⠿⎇▸▾█░▁▂▃▄▅▆▇` used as inline UI accents (bullets, section
  markers, sparkline motifs).

---

## 7. Build & hosting

- **Approach A — single self-contained page.** The deliverable is
  `site/index.html` with **all CSS, JS, and TUI frame fragments inlined**. A
  small build script (`site/build.sh`) concatenates the captured
  `site/frames/*.html` into the page template so authoring stays modular while
  the shipped artifact is one file. (Matches the project's own
  self-contained-HTML convention in `docs/ux/`.)
- **No serve-time build / no runtime deps.** Open `index.html` in a browser or
  drop it on any static host.
- **Hosting (decide at publish time, not blocking the build):** the private
  source repo must **not** serve Pages. Recommended: a dedicated **public
  `zoid-site` repo** (or the existing public *releases* repo) with GitHub Pages
  enabled on `main` (root or `/docs`). The build targets a portable
  `site/index.html` so the eventual host is a copy step. A `.github/workflows`
  Pages deploy can be added when the destination repo is chosen.

---

## 8. Responsive, motion & accessibility

- **Responsive:** desktop-first (the TUI is a wide medium), gracefully degrading.
  Frames live in `overflow-x:auto` containers so they scroll horizontally on
  narrow screens rather than breaking the page; font-size steps down at
  breakpoints. Body never scrolls horizontally.
- **Motion (subtle, opt-out):** streaming-caret blink on the hero, a gentle
  sparkline shimmer on the economy frame, and a short **Shift+Tab mode-swap**
  micro-loop (chip + menu highlight). All motion gated behind
  `@media (prefers-reduced-motion: reduce)` → static.
- **Accessibility:** semantic landmarks (`header`/`main`/`section`/`footer`),
  real headings, sufficient contrast (the dark palette already clears AA for
  body text on `#0d1117`), and TUI frames given `role="img"` + an `aria-label`
  summarizing what the frame shows (screen readers get the meaning, not the ANSI
  grid). Respect keyboard focus order; no focus traps.

---

## 9. Testing / verification

- **Frame fidelity:** the `web_capture` output for a scene must match its
  `preview.rs` text content (same renderer, same fixtures) — a quick diff guards
  against a broken walker.
- **Self-contained check:** grep the built `site/index.html` for external
  `http(s)://` asset references (script/link/img/font) — there must be none.
- **Responsive smoke:** render the page at ~1440px and ~390px widths and confirm
  no horizontal body scroll and that frames scroll within their containers.
- **Reduced-motion:** with `prefers-reduced-motion` emulated, confirm animations
  are suppressed.
- **Verification is visual** — drive the page in a browser and observe, per the
  `verify` discipline, not just a file existence check.

---

## 10. Out of scope

- Any backend, form, waitlist, analytics, or email capture.
- Pricing, plans, or subscription copy.
- Download links, install one-liners, or a real "get it now" CTA.
- Multi-page site, blog, changelog, or documentation hosting.
- A static-site generator or framework (Astro/11ty/React).
- Inline raster graphics / video; frames are text (HTML) only.
- Building the Modes Shift+Tab runtime — the site *markets* it; the feature is
  owned by its own spec (`2026-07-05-mode-promotion-quickswitch-design.md`).

---

## 11. Risks

1. **Modes frame authenticity (§5).** The switch UI is not fully built, so its
   frame risks looking fabricated. Mitigation: capture the closest current
   palette state and let copy carry the "near-term capability" framing; never
   invent a frame.
2. **Multi-width glyph alignment.** Emoji/CJK in the rail (📦🟢🤖⌚📊) occupy two
   cells; a naive cell walker misaligns them. Mitigation: mirror the renderer's
   width handling (the snapshots already annotate these positions) and diff
   against `preview.rs`.
3. **Font cohesion without external fetch.** JetBrains Mono may not be installed
   locally. Mitigation: mono fallback stack; the design tolerates any monospace,
   and we do **not** fetch a web font (self-contained + CSP-safe).
4. **Hosting repo undecided.** Not blocking — the artifact is portable; publish
   is a copy step once the public repo is chosen.
