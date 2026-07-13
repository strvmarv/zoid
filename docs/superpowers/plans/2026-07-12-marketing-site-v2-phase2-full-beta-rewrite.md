# Marketing Site v2 — Phase 2 Full Beta Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reposition the live `public/index.html` from teaser to beta, integrate
two real-frame animated scenes (hero context-economy + new "Your tools, your
models"), refresh feature coverage, and correct stale/inaccurate copy — then
publish it as the live page.

**Architecture:** Extend the Phase-1 real-frame pipeline (`web_capture` +
`examples/scenes` + capture/assemble scripts). Add one new deterministic scene
sequence built *only* from existing renderable `ShellState` (no new TUI code).
Capture both scenes to `public/frames/<scene>/NN.html`, then an idempotent
assemble step inlines both frame sets into marker slots in a hand-authored
`index.html`. Publishing is unchanged (`publish-site.yml` mirrors `public/`).

**Tech Stack:** Rust (`zoid-tui` example + `web-capture` feature), POSIX `sh`
capture/assemble scripts, a self-contained HTML/CSS/JS page (no build step, no
dependencies).

## Global Constraints

Every task's requirements implicitly include this section. Values are exact.

- **No leakage** (AGENTS.md): published copy and captured frames contain **no**
  internal crate names, algorithms, file paths, or non-public provider names.
- **Providers, public:** name **only Ollama (local & cloud) and Anthropic**.
  Never render z.ai / opencode-zen / gemini / openai_compat. In the scene, seed
  `switch_providers` **by hand** — do **not** call
  `config_view::provider_options()` (it enumerates the whole registry, including
  internal providers).
- **Do not claim** orchestrated subagents / delegation (no shipped release note).
- **Releases repo only:** reference `strvmarv/zoid-releases`, never the private
  `strvmarv/zoid` source repo.
- **Platform/arch honesty:** Linux **x86_64**, macOS **Apple Silicon**, Windows
  **x86_64**. Do not imply Intel-Mac or Linux-ARM builds.
- **Binary size:** use the **measured** musl-static artifact size (Task 5); if the
  build is infeasible, omit the MB figure entirely. Never ship "~6 MB".
- **No "vim-like":** interaction is "modal (mode-based)".
- **No "Coming soon"** anywhere (meta, hero, footer).
- **Version:** 0.3.2. **Eval-build expiry:** 30 days; `zoid update` self-updates
  (anonymous, checksum-verified, from the public releases repo).
- **Accessibility parity:** `prefers-reduced-motion` honored (static last frame);
  every animated player carries a descriptive `aria-label`; players inert to
  input; horizontal-scroll frames never break the page (no page-level h-scroll).
- **No clobber:** `public/build.sh` stays disabled/untouched; the rewrite edits a
  hand-authored `index.html` and inlines frames between markers — it never
  regenerates the page from `template.html`.
- **Captures ≥ 160×40** (renderer's `MIN_WIDTH`/`MIN_HEIGHT`), else "too small".
- **Commits:** no `Co-Authored-By`/co-author trailer (per `~/CLAUDE.md`).

## File Structure

- `crates/zoid-tui/examples/scenes/mod.rs` — **modify**: add the `tools-models`
  fixtures (`seeded_tools_models()`, `seeded_mcp_status()`,
  `seeded_switch_providers()`, `seeded_switch_models()`) and a `scene_seq`
  `"tools-models"` arm (4 frames). One responsibility: shared scene fixtures.
- `crates/zoid-tui/examples/web_capture.rs` — **no change** (already generalizes
  over scene name via `--count`/`--frame`).
- `public/capture-preview.sh` — **modify**: loop over both scene names, capturing
  each to `public/frames/<scene>/NN.html`.
- `public/frames/tools-models/NN.html` — **generated** (new scene fragments).
- `public/index.html` — **rewrite in place**: beta positioning + CTA, corrected
  copy, two animated-player slots with embedded player CSS/JS and `BEGIN/END`
  marker pairs. Hand-authored; the single source page.
- `public/assemble-site.sh` — **new**: idempotent — replace content between each
  scene's `BEGIN/END` markers in `index.html` with that scene's current frames.
- `docs/superpowers/specs/2026-07-12-marketing-site-v2-phase2-full-beta-rewrite-design.md`
  — the spec (already written); the fixed contract.

Reference (read, do not modify): `public/preview.template.html` (the proven
player CSS + ~1 KB JS to port), `public/assemble-preview.sh` (the awk pattern),
`crates/zoid-tui/src/web_capture.rs` (fidelity fix, unchanged).

---

### Task 1: New `tools-models` scene fixtures + 4-frame sequence

Build the second animated scene entirely from existing renderable `ShellState`.
Faithful staging (verified against the renderer): F0 MCP-servers overlay, F1
provider/model quick-switch, F2 chosen model + question, F3 tool call running.

**Files:**
- Modify: `crates/zoid-tui/examples/scenes/mod.rs`
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes (existing, from `zoid_tui::state`): `Overlay::{Mcp,ProviderSwitch,None}`,
  `McpStatusRow{name:String,state:String,tool_count:usize}`, `SwitchPane::Model`,
  `config_view::PickOption{id:String,label:String,detail:String,selectable:bool,is_current:bool}`,
  and `ShellState` fields `overlay`, `mcp_status`, `switch_providers`,
  `switch_models`, `switch_pane`, `switch_model_sel`, `switch_provider_sel`,
  `model`, `provider`, `busy`, `active_tool`; `ChatMsg`/`ToolCallRef` from
  `zoid_core::projection`.
- Produces: a `scene_seq("tools-models")` returning 4
  `(ShellState, Vec<ChatMsg>, EconomyView)` frames, consumed unchanged by the
  existing `render_shell_scene_seq` and the `web_capture` example.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/zoid-tui/examples/scenes/mod.rs`:

```rust
#[test]
fn tools_models_sequence_stages_tools_then_models_then_run() {
    let seq = scene_seq("tools-models");
    assert_eq!(seq.len(), 4, "expected a 4-frame tools-models sequence");

    // F0: the MCP servers overlay (your tools).
    assert_eq!(seq[0].0.overlay, Overlay::Mcp, "frame 0 shows MCP overlay");
    assert!(!seq[0].0.mcp_status.is_empty(), "frame 0 has MCP servers");

    // F1: the provider/model quick-switch (your models).
    assert_eq!(
        seq[1].0.overlay,
        Overlay::ProviderSwitch,
        "frame 1 shows the quick-switch picker"
    );
    assert!(!seq[1].0.switch_providers.is_empty(), "providers seeded");
    assert!(!seq[1].0.switch_models.is_empty(), "models seeded");
    // Leak guard: only Ollama + Anthropic may appear as providers.
    for p in &seq[1].0.switch_providers {
        assert!(
            p.id == "ollama" || p.id == "anthropic",
            "public providers only; got leaked provider id {:?}",
            p.id
        );
    }

    // F2/F3: overlays closed; F3 shows a tool running.
    assert_eq!(seq[2].0.overlay, Overlay::None, "frame 2 overlay closed");
    assert_eq!(seq[3].0.overlay, Overlay::None, "frame 3 overlay closed");
    assert!(seq[3].0.busy, "frame 3 is busy (a tool is running)");
    assert!(
        seq[3].0.active_tool.is_some(),
        "frame 3 names the running tool"
    );

    // Renders at the required min size.
    let frames = render_shell_scene_seq("tools-models", 160, 40);
    assert_eq!(frames.len(), 4);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p zoid-tui --features web-capture --example web_capture tools_models_sequence_stages_tools_then_models_then_run`
Expected: FAIL — `scene_seq("tools-models")` currently returns `vec![scene("tools-models")]` (len 1), so the length assertion fails.

- [ ] **Step 3: Add the fixtures**

Add these helpers to `crates/zoid-tui/examples/scenes/mod.rs` (near the other
`seeded_*` fns). Use **real** public model ids from the `zoid-model` registry;
the values below are the intended ones (glm-5.2:cloud is the real default). Keep
providers to Ollama + Anthropic only.

```rust
use zoid_tui::state::{McpStatusRow, SwitchPane};
use zoid_tui::config_view::PickOption;

/// Connected MCP servers for the "your tools" frame (server list + tool counts —
/// the only MCP state the renderer shows; it does not list individual tools).
fn seeded_mcp_status() -> Vec<McpStatusRow> {
    vec![
        McpStatusRow { name: "filesystem".into(), state: "ready".into(), tool_count: 8 },
        McpStatusRow { name: "github".into(),     state: "ready".into(), tool_count: 12 },
        McpStatusRow { name: "postgres".into(),   state: "ready".into(), tool_count: 6 },
    ]
}

/// Provider options for the quick-switch — HAND-SEEDED to public providers only.
/// Do NOT use config_view::provider_options(): it enumerates the whole registry,
/// including internal/planned providers, and would leak them into the frame.
fn seeded_switch_providers() -> Vec<PickOption> {
    vec![
        PickOption { id: "ollama".into(),    label: "Ollama".into(),    detail: "local & cloud".into(), selectable: true, is_current: true },
        PickOption { id: "anthropic".into(), label: "Anthropic".into(), detail: "cloud".into(),         selectable: true, is_current: false },
    ]
}

/// Models shown for the highlighted (Ollama) provider. Real registry ids; the
/// default `glm-5.2:cloud` (a 1M-context model) is current.
fn seeded_switch_models() -> Vec<PickOption> {
    vec![
        PickOption { id: "glm-5.2:cloud".into(), label: "glm-5.2:cloud".into(), detail: "1M context".into(), selectable: true, is_current: true },
        PickOption { id: "glm-5.2".into(),       label: "glm-5.2".into(),       detail: "local".into(),      selectable: true, is_current: false },
    ]
}

/// A short, realistic turn that calls an MCP-provided tool (dotted name signals
/// it comes from the `github` server — reinforcing "your tools").
fn seeded_tools_models_turn() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User { text: "any open issues about the login flow?".into(), ts: 0 },
        ChatMsg::Assistant {
            thinking: None,
            text: "checking the github MCP server".into(),
            tool_calls: vec![ToolCallRef {
                id: "t1".into(),
                name: "github.search_issues".into(),
                args: r#"{"q":"login flow"}"#.into(),
            }],
            ts: 0,
        },
        ChatMsg::ToolResult {
            id: "t1".into(),
            name: "github.search_issues".into(),
            output: "#412 login redirect loop\n#419 2FA prompt after logout".into(),
            is_error: false,
            compacted: false,
            ts: 0,
        },
    ]
}

/// Tasks for the tools-models scene's Tasks drawer — coherent with the story
/// (a server connected, a model chosen). Two tasks, matching the base state's
/// `tasks_len = 2`, so the drawer shows real rows, not empty reserved space.
fn seeded_tools_models_tasks() -> Vec<zoid_core::tasks::TaskItem> {
    use zoid_core::tasks::{TaskItem, TaskStatus};
    vec![
        TaskItem { text: "connect the github MCP server".into(), status: TaskStatus::Done },
        TaskItem { text: "switch to glm-5.2:cloud".into(),       status: TaskStatus::Active },
    ]
}
```

Then extend `scene_tasks` so the new scene's drawer is populated (mirrors how
`"economy" | "context-economy"` already map to `seeded_tasks()`):

```rust
fn scene_tasks(name: &str) -> Vec<zoid_core::tasks::TaskItem> {
    match name {
        "economy" | "context-economy" => seeded_tasks(),
        "tools-models" => seeded_tools_models_tasks(),
        _ => vec![],
    }
}
```

- [ ] **Step 4: Add the `tools-models` sequence arm**

Extend `scene_seq` in the same file. Build a base enriched state (reuse the
`economy` session rail so the frame reads as real usage), then vary
`overlay`/`model`/`busy`/`active_tool` per frame:

```rust
        "tools-models" => {
            // Enriched right-rail (repo/session), reused across frames.
            let base = || {
                let (s, _m, _e) = scene("economy");
                s
            };
            let turn = seeded_tools_models_turn();

            // F0 — your tools: the MCP servers overlay.
            let mut f0 = base();
            f0.overlay = Overlay::Mcp;
            f0.mcp_status = seeded_mcp_status();

            // F1 — your models: the provider/model quick-switch, Model pane.
            let mut f1 = base();
            f1.overlay = Overlay::ProviderSwitch;
            f1.switch_providers = seeded_switch_providers();
            f1.switch_models = seeded_switch_models();
            f1.switch_pane = SwitchPane::Model;
            f1.switch_provider_sel = 0; // Ollama
            f1.switch_model_sel = 0;    // glm-5.2:cloud (current)

            // F2 — chosen: overlay closed, session drawer shows model·provider,
            // the user asks a question.
            let mut f2 = base();
            f2.model = "glm-5.2:cloud".into();
            f2.provider = "ollama".into();

            // F3 — it runs, locally: a tool is executing.
            let mut f3 = base();
            f3.model = "glm-5.2:cloud".into();
            f3.provider = "ollama".into();
            f3.busy = true;
            f3.active_tool = Some("github.search_issues".into());

            // F3 shows the tool genuinely in flight: the assistant's tool CALL
            // is visible (turn[..2]) but its result is NOT yet on screen, so the
            // "running" status indicator is coherent (not paired with a returned
            // result). The ToolResult (turn[2]) intentionally stays unrevealed.
            vec![
                (f0, turn[..1].to_vec(), empty_economy()),
                (f1, turn[..1].to_vec(), empty_economy()),
                (f2, turn[..1].to_vec(), seeded_economy()),
                (f3, turn[..2].to_vec(), seeded_economy()),
            ]
        }
```

(Insert this arm inside the existing `match name { … }` in `scene_seq`, before
the `_ => vec![scene(name)]` fallback.)

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p zoid-tui --features web-capture --example web_capture tools_models_sequence_stages_tools_then_models_then_run`
Expected: PASS.

- [ ] **Step 6: Run the scene's full test set + fmt the file**

Run: `cargo test -p zoid-tui --features web-capture --example web_capture`
Expected: all scene tests PASS (existing `economy`/`context-economy` tests still green).
Then format ONLY the changed file (never crate-wide fmt): `rustfmt --edition 2021 crates/zoid-tui/examples/scenes/mod.rs`

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/examples/scenes/mod.rs
git commit -m "feat(site): tools-models animated scene fixtures (4-frame sequence)"
```

---

### Task 2: Capture both scenes via a generalized script

`web_capture.rs` already generalizes over scene name (`--count <scene>`,
`--frame <i> <scene> [w] [h]`) — no Rust change. Generalize the capture script to
loop over both scenes.

**Files:**
- Modify: `public/capture-preview.sh`

- [ ] **Step 1: Rewrite the capture script to loop scenes**

Replace `public/capture-preview.sh` with:

```sh
#!/bin/sh
# Render each scene's frame sequence into public/frames/<scene>/.
# Run from repo root: sh public/capture-preview.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUN="cargo run -q -p zoid-tui --features web-capture --example web_capture --"
SCENES="context-economy tools-models"
for scene in $SCENES; do
  OUT="$ROOT/public/frames/$scene"
  mkdir -p "$OUT"
  rm -f "$OUT"/*.html
  N=$($RUN --count "$scene")
  i=0
  while [ "$i" -lt "$N" ]; do
    f=$(printf "%02d" "$i")
    $RUN --frame "$i" "$scene" 160 40 > "$OUT/$f.html"
    i=$((i + 1))
  done
  echo "captured $N frames → $OUT"
done
```

- [ ] **Step 2: Run it and verify both scenes captured**

Run: `sh public/capture-preview.sh`
Expected output includes `captured 4 frames → …/frames/context-economy` and
`captured 4 frames → …/frames/tools-models`.
Verify: `ls public/frames/tools-models/` shows `00.html 01.html 02.html 03.html`.

- [ ] **Step 3: Spot-check fidelity + leak-safety of the new frames**

Run: `grep -l -iE 'zai|opencode|gemini|openai_compat|strvmarv/zoid[^-]' public/frames/tools-models/*.html || echo "clean"`
Expected: `clean` (no internal provider names, no private-repo path). Also
confirm wide/symbol glyphs are boxed: `grep -c 'display:inline-block' public/frames/tools-models/03.html` returns a non-zero count (the fidelity fix is active).

- [ ] **Step 4: Commit**

```bash
git add public/capture-preview.sh public/frames/tools-models
git commit -m "feat(site): capture both scenes (context-economy + tools-models)"
```

---

### Task 3: Rewrite `index.html` — beta positioning, CTA, corrected copy, two player slots

Rewrite the hand-authored page in place. This task does the copy + structure +
embedded player CSS/JS + marker slots. Frames are inlined by Task 4; for now the
slots contain a single static placeholder frame so the page is viewable.

**Files:**
- Modify: `public/index.html`

**Copy corrections (apply every one — Global Constraints):**

| Location | From | To |
| --- | --- | --- |
| `<meta name="description">` (line 7) | "…Coming soon." | "…a terminal-native AI coding agent built in Rust. Active context management, bring-your-own-tools (MCP), your choice of model. Now in beta." |
| hero `.soon` (line 238) | "Coming soon" | remove; replace with the beta chip + CTA block below |
| built band (line 484) | "modal (vim-like) interaction" | "modal (mode-based) interaction" |
| footer (line 491) | "Coming soon." | remove; replace with the footer CTA + beta note |
| binary size, **all 3 occurrences**: hero sub (line ~237, `One ~6&nbsp;MB binary`), §4 lead (line ~469), stat tile (line ~471) | "~6 MB" / "6&nbsp;MB" (×3) | the **measured** size from Task 5 (placeholder `~11 MB` until Task 5 confirms) |
| §4 / stats | (no context stat) | add a "1M" context stat (the default model's real window) |
| proof/stat platforms | "Linux · macOS · Windows" | "Linux x86_64 · macOS Apple Silicon · Windows x86_64" |

- [ ] **Step 1: Add the beta chip + hero install CTA**

In the hero (replacing the `<p class="soon …">Coming soon</p>` line), insert a
beta chip, an install button, a copyable one-liner, and the beta note. Add
matching CSS in the `<style>` block (follow the existing token vocabulary
`--acc`, `--chip`, `--ok`, `--muted`, `--line`). HTML:

```html
  <p class="beta fade d3"><span class="chip">Now in beta · v0.3.2</span></p>
  <div class="cta fade d3">
    <a class="btn" href="https://github.com/strvmarv/zoid-releases/releases/latest">Install zoid</a>
    <code class="oneliner">curl --proto '=https' --tlsv1.2 -LsSf https://github.com/strvmarv/zoid-releases/releases/latest/download/zoid-installer.sh | sh</code>
    <p class="betanote">Evaluation builds expire 30 days after release — run <code>zoid update</code> to stay current (anonymous, checksum-verified, from the public releases repo). PowerShell installer &amp; per-platform archives on the releases page.</p>
  </div>
```

CSS (add to `<style>`):

```css
.beta{margin:0 0 20px;}
.chip{display:inline-block;background:var(--chip);color:var(--acc);font-weight:600;
  letter-spacing:.08em;text-transform:uppercase;font-size:11px;padding:4px 11px;border-radius:20px;}
.cta{display:flex;flex-direction:column;align-items:center;gap:12px;margin:0 auto 44px;max-width:760px;}
.btn{display:inline-block;background:var(--acc);color:#0d1117;font-weight:700;
  text-decoration:none;padding:11px 26px;border-radius:8px;letter-spacing:.01em;}
.btn:hover{background:var(--acc2);}
.oneliner{display:block;width:100%;overflow-x:auto;white-space:pre;background:var(--panel);
  border:1px solid var(--line);border-radius:8px;padding:10px 14px;color:var(--muted);font-size:12px;}
.betanote{color:var(--dim);font-size:12px;margin:0;max-width:60ch;line-height:1.7;}
.betanote code{color:var(--muted);}
```

- [ ] **Step 2: Port the animated-player CSS**

Copy the player CSS from `public/preview.template.html` (the `.frame`, `pre.tui`,
`.player`, `.tui-frame`, and the `@media (prefers-reduced-motion)` block,
including its PHASE-2 TRAP comment) into `index.html`'s `<style>`. Both scenes
have 4 frames, so the trap does not apply, but keep the comment. Scope note: the
existing page already defines `.term`/`.figure`; the player's `.frame`/`pre.tui`
are additive and coexist.

- [ ] **Step 3: Replace the hero terminal with the context-economy player slot**

Replace the hero `<div class="figure"> … </div>` (the hand-authored
diagnosing-500 `.term`, lines ~241-292) with the animated player, and add the
context-economy lead copy as the hero caption (preserving the story that moves
up from the old §1):

```html
  <div class="figure">
    <div class="frame">
      <div class="player nojs" role="img"
           aria-label="zoid diagnosing a 500: the turn streams in while the context-economy rail fills — context as a managed, measured resource.">
<!--FRAMES:context-economy:BEGIN-->
        <div class="tui-frame"><pre class="tui"># context economy — captured frames inline here</pre></div>
<!--FRAMES:context-economy:END-->
      </div>
    </div>
    <p class="figcap">The <em>right</em> context — not just the recent context. zoid curates the working set, compacts what's verbose, and narrates every move so you can see and undo it.</p>
  </div>
```

(Add a `.figcap{color:var(--muted);max-width:64ch;margin:16px auto 0;text-align:center;font-size:13px;}` rule.)

- [ ] **Step 4: Rewrite old §1 into "Your tools, your models" with the second player**

Replace the old §1 context-economy `<section class="section"> … </section>`
(lines ~298-358) with the new hero-beat section carrying the tools-models player:

```html
  <!-- §1 Your tools, your models -->
  <section class="section rev">
    <div>
      <p class="eyebrow c2">your tools, your models</p>
      <h2>Bring your own <em>tools</em>. Choose your own <em>model</em>.</h2>
      <p class="lead">Connect any <strong>MCP server's tools</strong> alongside zoid's built-ins — configured tools appear automatically. Point zoid at the provider you want: <strong>Ollama</strong> (local &amp; cloud) or <strong>Anthropic</strong>. And <strong>on-device semantic recall</strong> means nothing has to leave your machine.</p>
    </div>
    <div class="figure">
      <div class="frame">
        <div class="player nojs" role="img"
             aria-label="zoid: MCP servers connected and contributing tools, the provider/model quick-switch, then a tool call running on the chosen model.">
<!--FRAMES:tools-models:BEGIN-->
          <div class="tui-frame"><pre class="tui"># your tools, your models — captured frames inline here</pre></div>
<!--FRAMES:tools-models:END-->
        </div>
      </div>
    </div>
  </section>
```

- [ ] **Step 5: Refresh §2 (modes/Superpowers), proof band, built band, footer**

- §2 "extensible by design" / "As the market shifts": keep the modes term; refresh
  the lead to name the **one-command Superpowers install** as the concrete example
  ("add a curated skill set — TDD, systematic debugging, code review, planning — as
  a ready-to-use mode in a single step"). Keep Shift+Tab (verified).
- §4 "rust-native" → **proof band**: update stat tiles to `[measured] MB · ~10 ms
  cold start · 1M context · 3 OS`, and set the platform sub-labels to
  "Linux x86_64 · macOS Apple Silicon · Windows x86_64". Keep checksum-verified
  self-update copy.
- built band (line 484): `modal (mode-based) interaction`; keep
  `multi-provider (Ollama local & cloud, Anthropic)` (accurate).
- footer: remove "Coming soon."; add a compact repeat of the install button +
  a one-line beta note.

- [ ] **Step 6: Add the player JS**

Copy the ~1 KB player `<script>` from `public/preview.template.html` verbatim to
just before `</body>` in `index.html` (it selects every `.player`, removes
`nojs`, honors reduced-motion, pauses off-screen via IntersectionObserver, loops
at 1400 ms). It operates on both players generically — no per-scene code.

- [ ] **Step 7: Verify structure (no captures yet)**

Run: `grep -c 'FRAMES:context-economy:BEGIN\|FRAMES:tools-models:BEGIN' public/index.html`
Expected: `2`.
Run: `grep -c 'Coming soon' public/index.html` → `0`.
Run: `grep -c 'vim-like' public/index.html` → `0`.
Open `public/index.html` in a browser; confirm the page renders (placeholder
frame text visible in both player slots), CTA button + one-liner present, no
"Coming soon".

- [ ] **Step 8: Commit**

```bash
git add public/index.html
git commit -m "feat(site): beta rewrite of index.html — CTA, corrected copy, two player slots"
```

---

### Task 4: `assemble-site.sh` — idempotent inlining of both scenes into `index.html`

Replace the content between each scene's `BEGIN/END` markers with that scene's
current frames, in place, re-runnably.

**Files:**
- Create: `public/assemble-site.sh`

**Interfaces:**
- Consumes: `public/frames/<scene>/NN.html` (Task 2 output), the marker pairs in
  `index.html` (Task 3).
- Produces: `index.html` with both player slots populated; byte-identical on
  re-run from the same frames (idempotent).

- [ ] **Step 1: Write the assemble script**

```sh
#!/bin/sh
# Inline captured frames into public/index.html between per-scene markers.
# Idempotent: re-running with the same frames yields a byte-identical file.
# Run from repo root: sh public/assemble-site.sh  (after capture-preview.sh)
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
HTML="$ROOT/public/index.html"
SCENES="context-economy tools-models"

for scene in $SCENES; do
  DIR="$ROOT/public/frames/$scene"
  [ -d "$DIR" ] || { echo "missing frames for $scene (run capture-preview.sh)"; exit 1; }

  # Build the frames block (each fragment wrapped in a .tui-frame div).
  BLOCK=$(mktemp)
  for f in "$DIR"/*.html; do
    printf '        <div class="tui-frame">' >> "$BLOCK"
    cat "$f" >> "$BLOCK"
    printf '</div>\n' >> "$BLOCK"
  done

  # Replace everything between the BEGIN/END markers (exclusive) with the block.
  TMP=$(mktemp)
  awk -v b="FRAMES:$scene:BEGIN" -v e="FRAMES:$scene:END" -v ff="$BLOCK" '
    $0 ~ b { print; while ((getline line < ff) > 0) print line; skip=1; next }
    $0 ~ e { skip=0 }
    skip==1 { next }
    { print }
  ' "$HTML" > "$TMP"
  mv "$TMP" "$HTML"
  rm -f "$BLOCK"
  echo "inlined $scene → index.html"
done
```

- [ ] **Step 2: Run capture + assemble**

Run: `sh public/capture-preview.sh && sh public/assemble-site.sh`
Expected: `inlined context-economy → index.html` and `inlined tools-models → index.html`.

- [ ] **Step 3: Verify idempotency (byte-identical re-run)**

Run:
```bash
cp public/index.html /tmp/site-a.html
sh public/assemble-site.sh
diff -q /tmp/site-a.html public/index.html && echo "IDEMPOTENT"
```
Expected: `IDEMPOTENT` (assemble on already-assembled frames changes nothing).

- [ ] **Step 4: Verify the placeholders are gone and real frames are in**

Run: `grep -c 'captured frames inline here' public/index.html` → `0`.
Run: `grep -c 'pre class="tui"' public/index.html` → 8 (4 frames × 2 scenes).

- [ ] **Step 5: Commit**

```bash
git add public/assemble-site.sh public/index.html
git commit -m "feat(site): idempotent assemble step inlines both scenes into index.html"
```

---

### Task 5: Measure the real musl binary size + verify the install one-liner URL

Replace the placeholder size with the measured musl-static figure, and confirm
the version-agnostic install URL resolves.

**Files:**
- Modify: `public/index.html` (the size figures + fallback)

- [ ] **Step 1: Measure the musl-static release artifact**

Attempt the real target build (may be slow):
`cargo build -q --release -p zoid --target x86_64-unknown-linux-musl`
Then: `ls -la target/x86_64-unknown-linux-musl/release/zoid` and
`du -h target/x86_64-unknown-linux-musl/release/zoid`.
Record the real size (round to a clean marketing figure, e.g. "~11 MB").
**If the musl target is unavailable** (no musl toolchain / sandbox denial): report
that, and per Global Constraints **omit the MB figure** — change the copy to "a
single static binary — nothing to install" (no number). Do not ship a guess.

- [ ] **Step 2: Verify the install one-liner URL resolves**

Run: `curl -sSI -o /dev/null -w '%{http_code} %{redirect_url}\n' https://github.com/strvmarv/zoid-releases/releases/latest/download/zoid-installer.sh`
Expected: a 302 redirect to a real release asset (or 200). If it does **not**
resolve (404), drop the literal one-liner and keep only the "Install zoid" button
linking to `…/releases/latest`, plus "download for your platform." Record the
outcome in the report.

- [ ] **Step 3: Apply the measured size + verified CTA to `index.html`**

Update **all three** size references to the measured figure (or the no-number
copy): the hero sub-line (~237, `One ~6&nbsp;MB binary`), the §4 lead (~469), and
the stat tile (~471). If Step 2 failed, remove the `<code class="oneliner">` line.

- [ ] **Step 4: Verify**

Run: `grep -c '~6 MB\|6&nbsp;MB' public/index.html` → `0` (the false claim is gone).
Confirm the size shown matches the measured value (or that no MB figure remains).

- [ ] **Step 5: Commit**

```bash
git add public/index.html
git commit -m "fix(site): measured musl binary size + verified install one-liner"
```

---

### Task 6: Full-page browser verification gate

Not a code task — the exit gate. Verify the assembled live page in a browser
before the branch is finished/merged (merge is the single go-live step).

- [ ] **Step 1: Serve and open the page**

Run (background): a static server on the repo, e.g. `python3 -m http.server 8099`
from `public/`. Open `http://localhost:8099/index.html`.

- [ ] **Step 2: Verify the fidelity + motion criteria (both players)**

Confirm in-browser:
- Both animated players loop; each frame's **scrollbar column is vertically
  straight** (no per-row drift), rails populated, text selectable.
- The tools-models player visibly shows: MCP servers overlay → provider/model
  picker → chosen model in the session drawer → a tool running.
- No horizontal **page** scroll at desktop widths; frames themselves may scroll
  inside their `.frame` on very narrow viewports only.
- Reduced motion: with `prefers-reduced-motion: reduce`, each player shows a
  single static final frame (no motion), and no blank/stacked frames.
- CTA button links to `…/releases/latest`; one-liner present (or omitted per Task
  5); beta chip + beta note visible; **no "Coming soon"** anywhere.

- [ ] **Step 3: Leak + accuracy scan**

Run: `grep -inE 'coming soon|vim-like|zai|opencode|gemini|openai_compat|~6 ?MB|6&nbsp;MB' public/index.html || echo "clean"`
Expected: `clean`. (The `6&nbsp;MB` alternative is essential — the page stores the
size as an HTML entity, so a bare `~6 ?MB` would silently pass on the real encoding.)
Run: `grep -n 'strvmarv/zoid[^-]' public/index.html || echo "no private-repo refs"`
Expected: `no private-repo refs` (only `strvmarv/zoid-releases`).

- [ ] **Step 4: Deterministic regen check**

Run: `sh public/capture-preview.sh && sh public/assemble-site.sh` and confirm
`git status --short public/` shows no diff (captures reproduce byte-identically).

- [ ] **Step 5: Full test suite green**

Run: `cargo test -p zoid-tui --features web-capture`
Expected: all pass (web_capture fidelity tests + both scene sequences).

- [ ] **Step 6: Record verification in the ledger** (no commit — verification only).
```
