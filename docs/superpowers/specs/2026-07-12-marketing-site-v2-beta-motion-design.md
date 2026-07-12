# zoid — Marketing Site v2 · Beta Positioning + Real-Frame Motion · Design

**Date:** 2026-07-12
**Status:** Approved design, ready for implementation plan (Phase 1)
**Author:** strvmarv (with Claude)
**Supersedes / evolves:** `2026-07-05-zoid-marketing-site-design.md` (the original
pre-launch teaser). This document does not discard that design — it keeps its
visual language and "show the real product" principle, and changes three things:
positioning (teaser → beta), proof (static hand-authored frames → real *animated*
frames), and feature coverage (the product has shipped features the current page
never mentions).

> Evolve the existing teaser page into a next-level **beta / early-access**
> marketing site that shows zoid *actually working* through real, animated TUI
> frames — produced by zoid's own renderer, not hand-drawn mockups or heavy GIFs.
> Terminal step after approval is `writing-plans` (Phase 1 first).

---

## 1. Context & problem

The live site (`public/index.html`, mirrored to `strvmarv/zoid-releases/docs/`
via `.github/workflows/publish-site.yml`) is framed as a **"Coming soon"
teaser**. But zoid is now a **shipping, self-updating beta** (0.3.2) with
expiring evaluation builds and `zoid update`. Two problems follow:

1. **Positioning drift.** The page says "Coming soon" and offers no download,
   while the product ships today. Decision: reposition as **beta / early-access**
   — honest about expiring eval builds, but with a real install CTA.

2. **Feature drift.** The page tells four stories (context economy, modes,
   semantic zoom, rust-native). `RELEASES.md` (0.2.0–0.3.2) shows shipped
   stories the page omits: **MCP / bring-your-own-tools**, **multi-provider**
   (Ollama local & cloud, Anthropic), **on-device semantic recall**,
   **one-command Superpowers install**, and **orchestrated subagents**.

3. **Motion gap.** The maintainer wants to *show the tool working* (animation).
   A prior attempt used the `web_capture` pipeline to inject real captured
   frames, but it was abandoned for hand-authored static mockups because of two
   concrete defects (see §4). "Animated GIF" is rejected as the default: heavy
   (MBs), blurry at ≥2× DPI, un-selectable text, no theme/reduced-motion
   adaptation — a poor fit for a crisp monospace TUI.

### Why the prior real-frame attempt was abandoned (root cause)

`zoid_tui::web_capture::buffer_to_html` walks the rendered `TestBackend` buffer
and advances the **terminal** grid by each glyph's unicode display width
(`x += sym.width()`), but emits the raw glyph into a `<pre>` and lets the
**browser** lay it out by the font's *natural advance*. For box-drawing that
matches; for **emoji** (📦 🟢 🤖 📊 ⌚) it does not — browsers render color-emoji
with proportional metrics, so a 2-cell terminal glyph rarely occupies exactly
`2ch` in the web font. Every wide glyph then shifts everything to its right on
that row. The scrollbar column (`█`) is the far-right element, so it accumulates
all upstream drift and wobbles row-to-row — the reported "scrollbar was the
worst offender." Second defect: the captured scenes were sparse (empty right-rail
widgets, thin conversation), so they didn't look like real usage.

---

## 2. Goals & Non-Goals

### Goals
- Reposition the site as **beta / early-access** with a real install CTA and an
  honest beta note.
- **Prove the tool works** with real, animated, pixel-accurate TUI frames from
  zoid's own renderer — crisp, selectable, small, theme- and reduced-motion-aware.
- **Fix frame fidelity** so the terminal grid and browser grid are identical
  (scrollbar included), and lock it with tests.
- **Enrich the scenes** to look like genuine usage (populated right-rail widgets,
  a fuller conversation, real numbers).
- **Refresh feature coverage** so the two hero beats reflect the current product.
- Keep zoid's established visual language; refine, don't replace.

### Non-Goals
- No GIF/video/WebM production. No terminal-recorder dependency (VHS/asciinema).
- No ground-up visual redesign; no generic-SaaS look.
- No pricing, waitlist backend, analytics, or data capture.
- No leaking of closed-source internals into the published page (per `AGENTS.md`:
  no crate names, algorithms, or file paths in customer-facing copy).
- Phase 2 (full site rewrite) is documented here but **not** planned/implemented
  until Phase 1 proves the pipeline.

---

## 3. Positioning & voice

**Beta / early-access.** Confident and shipping, not pre-launch. Drop "Coming
soon." The hero carries an **install CTA** (the install one-liner / download from
`strvmarv/zoid-releases`) and a **"Now in beta"** chip. An honest beta note
(evaluation builds expire ~30 days; `zoid update` to stay current) sits near the
CTA — visible, not buried. Copy stays benefit-oriented and free of internal
jargon.

---

## 4. Feature narrative (hero vs. supporting)

Decided with the maintainer. **Hero beats** (front-and-center, animated):

- **§1 Context economy** *(signature)* — the conversation as a database:
  narrated, undoable compaction and a live token ledger. Long sessions stay
  coherent because context is paged in/out, not truncated.
- **§2 Your tools, your models** *(local-first / privacy)* — connect any MCP
  server's tools alongside the built-ins; choose your provider (Ollama local &
  cloud, Anthropic); on-device semantic recall means nothing has to leave your
  machine.

**Supporting beats** (lighter treatment, not hero real estate):

- **§3 Adapts as fast as the ecosystem** — modes & skills via Shift+Tab; the
  one-command Superpowers install as a concrete example of dropping in a
  capability.
- **§4 Semantic zoom** — one conversation, three altitudes (keep the existing
  side-by-side treatment).

**Proof band** — one ~6 MB binary · ms cold start · Linux/macOS/Windows ·
checksum-verified self-update.

Page arc: Hero (animated) → §1 (animated) → §2 (animated) → §3 → §4 → proof band
→ footer with install CTA + beta note.

---

## 5. Motion system (core new machinery)

Four well-bounded units. Each can be built and tested independently.

### 5.1 Fidelity fix — `web_capture::buffer_to_html`
Stop trusting the browser's font advance for wide glyphs. When a glyph's unicode
display width is ≥ 2, emit it as `<span style="display:inline-block;width:{w}ch;
text-align:center">…</span>` (`w` = its cell count). Normal (width-1) runs
continue to flow naturally at `1ch`/char. Result: the browser grid equals the
terminal grid regardless of font or emoji fallback — scrollbar included. DOM
growth is confined to the handful of wide-glyph cells per frame.
- **Interface unchanged:** still `buffer_to_html(&Buffer) -> String`.
- **Tests:** extend the existing `#[cfg(test)]` unit tests with a wide-glyph
  case asserting the explicit-width span; add golden-frame snapshot(s) of a
  richer scene so drift can't return silently.

### 5.2 Richer scenes — `crates/zoid-tui/examples/scenes/`
Extend the seeded fixtures so a captured frame reads as real usage:
- Right rail populated: repo/branch, session with a real duration and token/cache
  counts, context sparklines with data, an actual task or two.
- A fuller conversation (a few realistic turns, a tool call, a compaction).
- Fixtures stay deterministic (seeded), so captures are reproducible in CI.

### 5.3 Frame *sequences*
Extend the capture example to emit an **ordered set** of buffers per story
(state transitions), not a single still — e.g. hero: `prompt typed → search
running → result compacted → answer streamed`, with the token rail advancing.
Output: `frames/<scene>/NN.html` fragments (zero-padded, ordered).

### 5.4 Web player (client)
A ~1 KB dependency-free JS/CSS stepper embedded in `index.html`:
- Cross-fades / swaps the ordered frame fragments on a fixed cadence, looping.
- `prefers-reduced-motion: reduce` → render the final frame statically, no motion.
- Pauses when off-screen (IntersectionObserver); inert to keyboard/pointer
  (decorative, `aria`-labeled like the existing frames).
- Frames are inlined or referenced as static fragments; no network calls at play
  time. Total payload is KBs, not MBs.

### 5.5 Pipeline (additive, no clobber)
Re-enable capture **additively**. The disabled `build.sh` guard
(`cp template.html index.html`) stays disabled. Add a new capture step that
writes ordered frame fragments into a dedicated dir, then a small, explicit
assembly step **inlines** those fragments into marked slots in the
hand-authored `index.html` (keeping the page self-contained, consistent with the
original design). The assembly step only replaces content between named markers
and never bulk-overwrites authored copy. Publishing is unchanged:
`publish-site.yml` mirrors `public/` → `zoid-releases/docs/` → GitHub Pages.

---

## 6. Visual language

Keep and elevate the current system: GitHub-dark palette, JetBrains Mono, the
terminal-frame treatment and glyph vocabulary. Refine spacing/rhythm and add the
player + CTA. The page must continue to read as an extension of the product, not
a generic landing page. Accessibility parity with today: horizontal-scroll frames
never break the page; reduced-motion fully honored; frames carry descriptive
`aria-label`s.

---

## 7. Phasing

**Phase 1 — Prove the motion (planned & implemented first).**
De-risk the exact seam that failed before, in isolation:
1. §5.1 fidelity fix + tests.
2. §5.2 richer fixtures for **one** hero scene (context economy).
3. §5.3 a frame sequence for that scene.
4. §5.4 the web player, wired to that one scene.
5. §5.5 additive capture path.
**Exit criterion:** that one animated scene renders pixel-perfect (scrollbar
stable), reads as real usage, is crisp/selectable, respects reduced-motion, and
publishes end-to-end. Verified in a browser, not just tests.

**Phase 2 — Full beta-site rewrite (documented, not yet planned).**
Once the pipeline is trusted: beta positioning + CTA (§3), the full narrative
(§4) with the second hero animated scene (Your tools, your models) and the
supporting beats, proof band, footer. Separate spec-review → plan → implement
cycle.

---

## 8. Success criteria

- **Fidelity:** in-browser, the scrollbar column is vertically straight across
  all rows of a richer scene; no per-row drift. Golden-frame tests pass.
- **Authenticity:** a captured frame shows populated widgets + a real
  conversation — not empty rails.
- **Motion quality:** smooth loop; reduced-motion shows a clean static final
  frame; payload in KBs; text remains selectable and theme-consistent.
- **Positioning:** page presents as beta with a working install CTA and honest
  beta note; no "Coming soon."
- **No leakage:** published copy contains no crate names, algorithms, or paths.

---

## 9. Risks & mitigations

- **Fidelity fix insufficient for some glyph.** Mitigate with golden-frame tests
  over the actual scene glyph set; verify in-browser before Phase 2.
- **Frame-sequence authoring drifts from real UI over time.** Mitigate: sequences
  are generated from the real renderer + seeded fixtures, regenerable in CI;
  treat like the existing snapshot discipline.
- **Additive capture reintroducing a clobber foot-gun.** Mitigate: keep the
  `build.sh` guard; the new step only *writes into a frames dir* and never
  overwrites authored `index.html` in place.
- **Payload creep from many frames.** Mitigate: cap frames per scene; small
  grids; reuse the existing compact `<pre>` output.

---

## 10. Open questions (for spec review)

- Exact frame count / cadence for the hero sequence (feel vs. payload) — settle
  during Phase 1 build against a real preview.
- Whether Phase 1's player should ship to the live site immediately or stay on a
  preview until Phase 2 — recommend preview until the surrounding page is ready.
