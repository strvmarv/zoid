# zoid — UX reference (canonical mockups)

The **visual source of truth** for zoid's TUI. The visual-language contract and fidelity pipeline live in the **core-architecture** spec (`docs/superpowers/specs/2026-06-30-zoid-core-architecture.md` §13); the Chat-mode layout lives in `2026-06-30-zoid-chat-mode-design.md`. The built TUI must match these mockups, enforced via snapshot tests (see "Fidelity pipeline").

> **Build mode is deferred and being redesigned.** Its spec and mockups (build-pipeline, build-mode, finalize-and-decisions, blocker-notifications) plus the earlier superseded scenes were archived on 2026-07-02 to `docs/superpowers/archive/2026-07-02/`. The screens below are the live Chat-mode set.

> Open any file in a browser — self-contained HTML, no server.

## Canonical screens

| File | Specifies |
|---|---|
| `modes.html` | The two isolated modes (Chat vs Build), mode indicator, switching |
| `chat-mode.html` | **Chat mode** (full scale): conversation + *manual* implementation, flush-left stream + measure-cap slack, light rail (**repo / session / context ⑤**, no drawer keybinds; files via palette), ① semantic zoom, ④ object-first verbs, markdown + highlighted-code message rendering, single-step cadence, single-subagent delegation |
| `palette.html` | **Command palette (^P)** + command line (`:`); mode-aware grouped actions with keybinds |
| `rust-unlocks.html` | Rust-enabled UX: Ⓡ2 motion, Ⓡ3 tree-sitter rendering, Ⓡ4 live data viz (Ⓡ1 inline graphics = later) |

*Build-mode screens (`build-pipeline`, `build-mode`, `finalize-and-decisions`, `blocker-notifications`) and the `_superseded-*` scenes are archived under `docs/superpowers/archive/2026-07-02/ux/`.*

## Visual language (authoritative — mirror in the Rust design-tokens module)

**Glyphs:** `●` edit · `✓` pass/done · `◐` running · `☐` pending · `⠿` streaming · `⎇` branch · `⚠` conflict/overlap · `▸`/`▾` collapsed/expanded · `⛔` blocker · `▲` spike · `›` user turn · `▌` caret · `█`/`░` heat bar (Ⓡ4) · `▁▂▃▄▅▆▇█` sparkline ramp (Ⓡ4) · `●` pinned item · `…` collapsed body (Ⓡ3 collapse-to-signatures) · `⊟` compacted tool-result (ACM-1).

**Mode accent colors:** Chat = blue (`#58a6ff`/`#79c0ff`) · Build = amber (`#e3b341`). (Finalize is Build's last step; it uses a green success accent `#5ddf9c` within Build.)

**Status:** ok `#3fb950` · warn `#d29922` · error/del `#f85149` · branch/accent `#bc8cff` · dim `#6e7681`.

**Heat (⑤a):** hot = ok green `#3fb950` · warm `#d29922` · cold = dim `#6e7681`.

**Syntax (tree-sitter Ⓡ3):** keyword `#ff7b72` · fn `#d2a8ff` · type `#7ee787` · string `#a5d6ff` · number `#79c0ff` · comment `#8b949e`.

## Fidelity pipeline (how these bind to code)

1. **Reference** — these files.
2. **Contract** — the Chat-mode spec (layouts, keymaps, min-widths, rail drawer sets) + the core spec §13 + the visual-language table above.
3. **Design-tokens module** — one Rust module holds all glyphs/colors/spacing/layout constants; every view renders from it.
4. **Enforcement** — `ratatui::TestBackend` + `insta` snapshot tests per screen; first snapshot built to match the mockup; later drift is a reviewed PR diff.
5. **Acceptance** — each TUI plan task's definition-of-done cites its mockup + snapshot test.

**Limits:** snapshots cover structure/content/layout. Ⓡ2 motion and Ⓡ1 graphics are verified separately (reduced-motion correctness tests + manual/gif; per-terminal visual diff).
