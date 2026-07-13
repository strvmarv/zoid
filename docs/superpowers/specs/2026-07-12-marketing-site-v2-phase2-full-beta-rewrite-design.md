# zoid — Marketing Site v2 · Phase 2 · Full Beta Rewrite · Design

**Date:** 2026-07-12
**Status:** Approved design, ready for implementation plan
**Author:** strvmarv (with Claude)
**Parent spec:** `2026-07-12-marketing-site-v2-beta-motion-design.md` (the v2 design;
its §5 Motion System and no-leak/accessibility principles bind this document and
are not repeated here). Phase 1 (real-frame motion pipeline, proven on the
context-economy scene, shipped to `main` @ 7596af3 as the standalone
`public/preview.html`) is complete. **This is Phase 2: the full beta rewrite of
the live `public/index.html`.**

> Reposition the live teaser as a shipping **beta**, integrate the proven
> real-frame animated player into the hero, add the second animated scene
> ("Your tools, your models"), refresh feature coverage to what has actually
> shipped, and correct stale/inaccurate copy — then publish it as the live page.

---

## 1. Decisions (locked)

Settled with the maintainer before planning:

1. **Go-live:** Phase 2 **replaces `public/index.html` directly.** On merge +
   push, `.github/workflows/publish-site.yml` mirrors `public/` →
   `strvmarv/zoid-releases/docs/` → GitHub Pages, so the new page is live
   immediately. No staging page.
2. **Second hero scene** = a **combined 4-frame sequence** (see §4).
3. **Scope** = full rewrite (positioning + both animated scenes + refreshed
   supporting beats + proof band + footer CTA).
4. **Binary size** = **measure the real musl-static release artifact** and use
   the true number; do not ship the false "~6 MB."

---

## 2. Copy audit — corrections this rewrite must make

Every current `index.html` claim was verified against the repo. The rewrite must
apply these:

| Current copy | Verdict | Action |
| --- | --- | --- |
| "~6 MB binary" (×3) | ❌ Inaccurate (real artifact ~11 MB; musl size unmeasured) | Measure the actual musl-static artifact; use the true number. |
| "modal (vim-like) interaction" | ❌ Unsupported (no vim emulation in the TUI) | → "modal (mode-based) interaction". |
| "Coming soon" (×3: meta line 7, hero, footer) | ❌ Stale | Remove; replace with beta positioning + CTA. |
| "Linux · macOS · Windows / 3 OS" | ⚠️ Over-implies arch coverage | State honestly: Linux x86_64 · macOS Apple Silicon · Windows x86_64. |
| "~10 ms cold start" | ✅ Verified (5–11 ms on `--version`) | Keep ("cold-starts in milliseconds"). |
| "checksum-verified self-update" | ✅ Verified | Keep. |
| "0 runtimes to install" | ✅ Verified (static musl) | Keep. |
| "Shift+Tab" mode switch | ✅ Verified (`route.rs` `BackTab => CycleMode`) | Keep. |
| "…/1.0M" context | ✅ Real (glm-5.2:cloud = 1M window) — **undersold** | Promote to real copy, not just mockup chrome. |
| Semantic zoom · event-sourced spine · importable modes | ✅ Shipped | Keep; refresh wording. |

**Feature-coverage guardrails (from parent spec + fact-find):**
- Multi-provider is publicly **only Ollama (local & cloud) + Anthropic**. Do not
  name z.ai / opencode-zen / gemini / openai_compat (internal crate modules only).
- Do **not** claim orchestrated subagents / delegation (no shipped release note).
- Reference **only** `strvmarv/zoid-releases`, never the private source repo.
- macOS is **Apple Silicon only**; do not imply Intel-Mac or Linux-ARM builds.

---

## 3. Positioning & CTA (teaser → beta)

- Remove all three "Coming soon" strings; rewrite the `<meta name="description">`
  to a beta framing (drops "Coming soon").
- **Beta chip** in the hero: **"Now in beta · v0.3.2"**.
- **Install CTA** in the hero (and repeated in the footer):
  - Primary button → `https://github.com/strvmarv/zoid-releases/releases/latest`.
  - A copyable, version-agnostic shell one-liner shown beneath:
    `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/strvmarv/zoid-releases/releases/latest/download/zoid-installer.sh | sh`
    plus a PowerShell note and "or download for your platform."
  - **A build task must confirm this `releases/latest/download/zoid-installer.sh`
    redirect actually resolves against a real release before ship.** If the asset
    name differs, use the real one; if no stable latest-redirect exists, fall
    back to linking the releases page only (no literal one-liner).
- **Honest beta note** near the CTA (visible, not buried):
  *"Evaluation builds expire 30 days after release — run `zoid update` to stay
  current (anonymous, checksum-verified, from the public releases repo)."*

Voice: confident and shipping, benefit-oriented, no internal jargon.

---

## 4. Page arc & the two animated scenes

**Arc:** Hero (animated) → §1 Your tools, your models (animated) → §2 Adapts as
fast as the ecosystem (hand-authored, refreshed) → §3 Semantic zoom (keep) →
proof band → footer (CTA + beta note).

Exactly **two** real-frame animated players; the other terminals stay as the
existing hand-authored CSS mockups with refreshed copy. Both animated players
reuse the Phase-1 client (self-contained ~1 KB vanilla stepper: loops,
IntersectionObserver off-screen pause, `prefers-reduced-motion` → static last
frame, `aria-label`, inert to input).

### 4.1 Hero scene — context economy (signature, reuse Phase 1)
The hero figure becomes the **proven context-economy animated player** — the
signature beat, best foot forward. Reuses the existing
`frames/context-economy/NN.html` capture pipeline and player. The hero headline
and sub-copy reposition to beta; the animated player replaces the current
hand-authored streaming `.term`.

The old standalone §1 context-economy ledger *section* is subsumed into the hero
(no duplicate section), but its explanatory prose ("the right context — not just
the recent context; narrated, undoable compaction; context as a managed resource
measured in tokens") is **preserved as the hero lead/caption**, not dropped — the
animation shows it, the copy names it.

### 4.2 New scene — "Your tools, your models" (combined 4-frame sequence)
A new seeded fixture + `scene_seq` in the Rust renderer, captured through the
exact Phase-1 pipeline into `public/frames/tools-models/NN.html`. Storyboard:

1. **Built-ins.** A chat session; the tool set shows zoid's built-in tools.
2. **Bring your own tools.** An MCP server connects; **its tools appear
   alongside the built-ins** ("configured tools appear automatically").
3. **Your models.** The model/provider is chosen via the palette — Ollama
   local ↔ cloud ↔ Anthropic.
4. **It runs, locally.** A tool call executes (an MCP tool); the answer streams;
   the rail reflects on-device / local semantic recall ("nothing leaves your
   machine").

Fixtures stay deterministic (seeded) so captures are reproducible. The scene must
pass the same in-browser fidelity bar as Phase 1 (scrollbar column straight, no
per-row drift, selectable text, populated rail).

### 4.3 Supporting beats (hand-authored, refreshed)
- **§2 Adapts as fast as the ecosystem** — modes & skills via **Shift+Tab**; the
  **one-command Superpowers install** as the concrete "drop in a capability"
  example. Refresh the existing modes/palette term copy.
- **§3 Semantic zoom** — keep the existing 3-altitude (summary · normal · detail)
  treatment; light copy refresh only.

### 4.4 Proof band
One static binary · cold-starts in milliseconds · **1M-token context (default
model)** · Linux x86_64 · macOS Apple Silicon · Windows x86_64 ·
checksum-verified self-update. Binary size = the **measured** musl figure.

---

## 5. Technical integration (additive, no clobber)

- **Renderer/fixtures:** extend `crates/zoid-tui/examples/scenes/mod.rs` with the
  new `tools-models` seeded fixture + `scene_seq`, following the Phase-1
  `context-economy` pattern (`render_one`, `scene_seq`,
  `render_shell_scene_seq`). Extend the capture CLI/`public/capture-preview.sh`
  to emit `public/frames/tools-models/NN.html`.
- **Assembly:** extend the assemble step to inline **both** players into named
  markers in a hand-authored `index.html` (the way `assemble-preview.sh` inlines
  one player into `preview.html`). The step only replaces content between named
  markers — it never bulk-overwrites authored copy.
- **No clobber:** `public/build.sh` stays disabled; the rewrite edits a
  hand-authored `index.html` source and inlines frames — it does not regenerate
  the page from `template.html`.
- **Binary measurement:** a task builds/inspects the actual musl-static release
  artifact (per `dist-workspace.toml` target `x86_64-unknown-linux-musl`) and
  records the true size for the copy. If the full cargo-dist build is
  infeasible in the environment, the task must say so and the copy falls back to
  "a single static binary" with no MB figure (per the maintainer's "measure"
  intent — a wrong number is worse than none).
- **Publish:** unchanged — `publish-site.yml` mirrors `public/` on push to `main`.

---

## 6. Constraints (inherited, must hold)

- **No leakage:** no internal crate names, algorithms, file paths, or non-public
  provider names in published copy; reference only `strvmarv/zoid-releases`.
- **Accessibility parity with today:** horizontal-scroll frames never break the
  page; `prefers-reduced-motion` fully honored (static last frame); every
  animated frame carries a descriptive `aria-label`; players inert to input.
- **Visual language:** keep and elevate the current system (GitHub-dark palette,
  JetBrains Mono, the `.term` treatment); refine, don't replace.
- **Fidelity:** both animated scenes pass the Phase-1 in-browser bar
  (scrollbar straight, no drift), verified in a browser, not just tests.

---

## 7. Success criteria

- Page presents as **beta** with a working install CTA and honest beta note; no
  "Coming soon" anywhere.
- **Two** real-frame animated scenes (hero context-economy + tools/your-models)
  render pixel-accurate and read as real usage.
- **No inaccurate claims:** binary size is the measured number (or omitted); no
  "vim-like"; platform/arch stated honestly; 1M context surfaced as real.
- **No leakage:** published copy contains no crate names, paths, or non-public
  providers.
- Publishes end-to-end (`index.html` live via Pages), verified in a browser.
- Existing test suite green (`zoid-tui --features web-capture`), new scene's
  fixtures covered by golden/snapshot tests as in Phase 1.

---

## 8. Risks & mitigations

- **New scene reintroduces glyph drift.** Mitigate: reuse the Phase-1
  `buffer_to_html` fidelity fix + golden-frame tests over the new scene's glyph
  set; verify in-browser before ship.
- **Going live directly (no staging).** Mitigate: build on an isolated branch,
  browser-verify the full page before merge; the merge is the only go-live step
  and is reversible via git.
- **`releases/latest/download` asset URL wrong.** Mitigate: a task verifies the
  redirect resolves against a real release; fall back to the releases page link.
- **musl build infeasible in CI/dev env.** Mitigate: fall back to omitting the MB
  figure rather than shipping a guess.
