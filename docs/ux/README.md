# zoid — UX reference (canonical mockups)

The **visual source of truth** for zoid's TUI. The visual-language contract and fidelity pipeline live in the **core-architecture** spec (`docs/superpowers/specs/2026-06-30-zoid-core-architecture.md` §13); per-mode layouts live in the **Chat** and **Build** mode specs (`2026-06-30-zoid-chat-mode-design.md`, `2026-06-30-zoid-build-mode-design.md`). *(These supersede the original combined `2026-06-28-zoid-tui-coding-agent-design.md`.)* The built TUI must match these mockups, enforced via snapshot tests (see "Fidelity pipeline").

> Open any file in a browser — self-contained HTML, no server.

## Canonical screens

| File | Specifies |
|---|---|
| `modes.html` | The two isolated modes (Chat vs Build), mode indicator, switching |
| `chat-mode.html` | **Chat mode** (full scale): conversation + *manual* implementation, light rail (⑤/files/branch/palette), ① semantic zoom, ④ object-first verbs, single-step cadence, Build offramp |
| `build-pipeline.html` | **Build as a stepped pipeline** (superpowers 7-phase): step bar; brainstorm→spec→worktree→plan with the spec/plan approval cards, code-grounded plan, criteria→gates, pre-flight |
| `build-mode.html` | **Build execute step**: 2 panes (Overview · Follow-stream) + rail (④ economy · changed-files tree · steering); per-task review-pipeline status; full width |
| `finalize-and-decisions.html` | The autonomy contract diagram, **blocker escalation**, and **Build's finalize step** (summary + autonomous-decisions log + changed-files/diff + merge/PR/discard) |
| `blocker-notifications.html` | **Blocker** types (ambiguity vs outward-facing consent) + the 4 notification channels + persistent blocked badge |
| `palette.html` | **Command palette (^P)** + command line (`:`); mode-aware grouped actions with keybinds |
| `rust-unlocks.html` | Rust-enabled UX: Ⓡ2 motion, Ⓡ3 tree-sitter rendering, Ⓡ4 live data viz (Ⓡ1 inline graphics = later) |
| `_superseded-*.html` | Earlier iterations kept for history (2×2 Build quad; early Chat scenes) |

## Visual language (authoritative — mirror in the Rust design-tokens module)

**Glyphs:** `●` edit · `✓` pass/done · `◐` running · `☐` pending · `⠿` streaming · `⎇` branch · `⚠` conflict/overlap · `▸`/`▾` collapsed/expanded · `⛔` blocker · `▲` spike · `›` user turn · `▌` caret · `█`/`░` heat bar (Ⓡ4) · `▁▂▃▄▅▆▇█` sparkline ramp (Ⓡ4) · `●` pinned item · `…` collapsed body (Ⓡ3 collapse-to-signatures).

**Mode accent colors:** Chat = blue (`#58a6ff`/`#79c0ff`) · Build = amber (`#e3b341`). (Finalize is Build's last step; it uses a green success accent `#5ddf9c` within Build.)

**Status:** ok `#3fb950` · warn `#d29922` · error/del `#f85149` · branch/accent `#bc8cff` · dim `#6e7681`.

**Heat (⑤a):** hot = ok green `#3fb950` · warm `#d29922` · cold = dim `#6e7681`.

**Syntax (tree-sitter Ⓡ3):** keyword `#ff7b72` · fn `#d2a8ff` · type `#7ee787` · string `#a5d6ff` · number `#79c0ff` · comment `#8b949e`.

## Fidelity pipeline (how these bind to code)

1. **Reference** — these files.
2. **Contract** — spec §6 (layouts, keymaps, min-widths) + the visual-language table above.
3. **Design-tokens module** — one Rust module holds all glyphs/colors/spacing/layout constants; every view renders from it.
4. **Enforcement** — `ratatui::TestBackend` + `insta` snapshot tests per screen; first snapshot built to match the mockup; later drift is a reviewed PR diff.
5. **Acceptance** — each TUI plan task's definition-of-done cites its mockup + snapshot test.

**Limits:** snapshots cover structure/content/layout. Ⓡ2 motion and Ⓡ1 graphics are verified separately (reduced-motion correctness tests + manual/gif; per-terminal visual diff).
