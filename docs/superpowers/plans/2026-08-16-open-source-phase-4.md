# Phase 4 — Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add CODE_OF_CONDUCT.md, create docs/DEVELOPMENT.md, rewrite README.md, update public/index.html to OSS framing, and freeze AGENTS.md as a legacy pointer.

**Architecture:** Documentation and marketing-only changes. No code, no tests to break. The deploy-pages workflow auto-deploys public/index.html changes to GitHub Pages.

**Tech Stack:** Markdown, HTML, Contributor Covenant v2.1.

## Global Constraints

- The spec is at `docs/superpowers/specs/2026-08-16-open-source-phase-4-design.md`. All content must match the spec (which was reviewed by gilfoyle against source code).
- `docs/DEVELOPMENT.md` is now the canonical developer reference; `AGENTS.md` is frozen as a legacy pointer (shrink it to a pointer at the top, keep it for backward compatibility).
- Fix `AGENTS.md` fallback test command to include `--features zoid/local-embed`.
- Do not touch `crates/*/src/`.
- The `deploy-pages.yml` workflow auto-deploys `public/**` pushes to GitHub Pages — no manual deploy needed.

---

### Task 1: Create CODE_OF_CONDUCT.md

**Files:**
- Create: `CODE_OF_CONDUCT.md`

**Interfaces:**
- Produces: root-level code of conduct. README "Get involved" and DEVELOPMENT.md link to it.

- [ ] **Step 1: Download the Contributor Covenant v2.1**

```bash
curl -sL https://www.contributor-covenant.org/version/2/1/code_of_conduct/code_of_conduct.md | head -5
```

If the URL is unreachable, copy the text from https://contributor-covenant.org/version/2/1/code_of_conduct/ — it's the standard Contributor Covenant v2.1, CC BY 4.0 licensed.

- [ ] **Step 2: Create the file with the enforcement contact filled in**

Copy the full Contributor Covenant v2.1 text into `CODE_OF_CONDUCT.md`. In the "Enforcement" section, fill in the enforcement contact with `strvmarv@gmail.com` (the maintainer's GitHub email). This is the only project-specific customization.

- [ ] **Step 3: Verify the file**

```bash
test -f CODE_OF_CONDUCT.md && head -1 CODE_OF_CONDUCT.md
grep -c 'strvmarv@gmail.com' CODE_OF_CONDUCT.md
```

Expected: file exists, first line is `# Code of Conduct`, and the email appears at least once (in the enforcement section).

- [ ] **Step 4: Commit**

```bash
git add CODE_OF_CONDUCT.md
git commit -m "docs: add Contributor Covenant v2.1 code of conduct"
```

---

### Task 2: Create docs/DEVELOPMENT.md

**Files:**
- Create: `docs/DEVELOPMENT.md`
- Modify: `AGENTS.md` — shrink to a legacy pointer + fix the fallback test command

**Interfaces:**
- Consumes: the architecture table from the spec, the release process from `docs/RELEASING.md`.
- Produces: the canonical developer reference. README links to it. AGENTS.md points to it.

- [ ] **Step 1: Create docs/DEVELOPMENT.md**

Copy the full content from the spec's §2 "docs/DEVELOPMENT.md" section (the architecture table, build/test commands, release summary, terminal minimum size, plugin catalog, memory note). The exact content is:

```markdown
# Developing zoid

Developer-facing conventions for building, testing, and releasing zoid.

## Crate architecture

zoid is a Cargo workspace of 14 crates:

| Crate | Responsibility |
|---|---|
| `zoid` | The binary — TUI app, agent loop, session management, CLI entry point |
| `zoid-core` | Core domain: sessions, events, config, secrets, skills, modes, agents, economy, eviction |
| `zoid-model` | Dependency-free model/provider catalog (static registry, ModelInfo, ProviderEntry) |
| `zoid-provider` | The LLM provider seam: streaming interface + per-provider implementations |
| `zoid-tui` | The terminal UI: layout, renderers, snapshot tests. Depends on `zoid-core`, `zoid-model`, and `zoid-syntax` |
| `zoid-tools` | Built-in tools (read, write, edit, grep, glob, shell, subagents, web_fetch, web_search, etc.) |
| `zoid-mcp` | MCP (Model Context Protocol) server discovery and connection management |
| `zoid-embed` | Local embedding model (candle) for semantic retrieval |
| `zoid-companion` | Optional localhost companion server: pushes a single HTML card over SSE to a side panel. std threads, no tokio |
| `zoid-plugin` | Plugin manifest types and parsing (local `plugins/*.toml` resolution) |
| `zoid-plugin-import` | GitHub-tree-fetching CLI for importing external plugin repos (repo/bulk subcommands) |
| `zoid-syntax` | Tree-sitter highlight/symbols/folds utilities |
| `zoid-testkit` | Test fixtures and helpers (excluded from the dist release; rides the workspace version) |
| `zoid-web` | DuckDuckGo search + readability fetch leaf — powers `web_fetch`/`web_search` via `zoid-tools` |

Dependency flow: `zoid` depends on all first-party leaf crates it
orchestrates (core, provider, tools, tui, companion, mcp, embed);
`zoid-core` depends on `zoid-model` (not `zoid-provider`); `zoid-provider`
depends on `zoid-model`; `zoid-tui` depends on `zoid-core`, `zoid-model`, and
`zoid-syntax`. The provider seam is intentionally decoupled from core so the
provider/plugin surface stays independent.

## Building

```bash
cargo build --workspace --release
```

For the release build (matches what cargo-dist produces):

```bash
cargo build --workspace --release --features zoid/local-embed
```

The `local-embed` feature bakes in the candle embedder for semantic
retrieval. Release builds always include it; dev builds can skip it for
faster compilation.

## Testing

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

`cargo test --workspace --features zoid/local-embed --no-fail-fast` works
as a fallback if nextest is not installed. Use `--no-fail-fast` so one
failing test doesn't hide others.

### TUI snapshot tests

The TUI uses insta snapshots. If a UI change modifies rendered output:

```bash
cargo insta test --accept -p zoid-tui
```

Confirm `git diff` is intentional before committing — snapshot changes
should be reviewed, not blindly accepted.

## Releasing

Full runbook: `docs/RELEASING.md`. Summary:

1. Bump `[workspace.package].version` in `Cargo.toml`.
2. Add a `## X.Y.Z` section to `CHANGELOG.md`.
3. Regenerate TUI snapshots (version appears in the status bar).
4. Verify: `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast`.
5. Commit, then `git tag vX.Y.Z && git push origin main --tags`.

The tag push fires `release.yml` (generated by `dist generate` from
`dist-workspace.toml`). Do not hand-edit `release.yml` — re-run
`dist generate` after any `dist-workspace.toml` change.

## Terminal minimum size

The TUI enforces a hard minimum of 160×40 (`layout::MIN_WIDTH` /
`MIN_HEIGHT`). Below this, a "too small" overlay renders instead of the
normal shell. Renderers can assume at least 160 columns — no narrow-terminal
fallback or progressive-collapse logic is needed in render code.

## Plugin catalog

The `plugins/` directory holds one TOML manifest per plugin plus a
generated `index.json`. The `catalog-index.yml` workflow regenerates
`index.json` when `plugins/*.toml` changes. To add a plugin, create a
`plugins/<id>.toml` and push — CI handles the rest.

## Memory

The maintainer (strvmarv) uses the **total-recall** MCP as the system of
record for decisions/corrections/preferences. Persist durable facts there;
do not hand-write into host memory files.
```

- [ ] **Step 2: Shrink AGENTS.md to a legacy pointer**

Replace the entire content of `AGENTS.md` with:

```markdown
# AGENTS.md

This file is frozen as a legacy reference. For current developer conventions,
see **[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)** — it consolidates build,
test, release, and architecture conventions.

The release process summary and terminal minimum size notes that previously
lived here now live in `docs/DEVELOPMENT.md`. The full release runbook
remains at `docs/RELEASING.md`.
```

This preserves backward compatibility (external links to `AGENTS.md` still
resolve) while making `DEVELOPMENT.md` the canonical source.

- [ ] **Step 3: Commit**

```bash
git add docs/DEVELOPMENT.md AGENTS.md
git commit -m "docs: add DEVELOPMENT.md as canonical developer reference, freeze AGENTS.md as legacy pointer"
```

---

### Task 3: Rewrite README.md

**Files:**
- Modify: `README.md` (full rewrite)

**Interfaces:**
- Consumes: `docs/DEVELOPMENT.md` (Task 2), `CODE_OF_CONDUCT.md` (Task 1), `CONTRIBUTING.md`, `SECURITY.md`, `CHANGELOG.md`, `LICENSE-MIT`, `LICENSE-APACHE`.
- Produces: the project landing page.

- [ ] **Step 1: Replace README.md with the full OSS rewrite**

```markdown
# zoid

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)

A terminal-native AI coding agent, built in Rust. Active context management,
bring-your-own-tools via MCP, and your choice of model — Ollama, Anthropic,
OpenAI, and OpenCode Zen.

zoid manages context like a database (event-sourced conversation history with
automatic compaction and eviction), not a growing text buffer. It runs as a
single ~16 MB binary, cold-starts in milliseconds, and lets you swap models
and tools without changing your workflow.

## Install

**Linux / macOS:**
```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.sh | sh
```

**Windows (PowerShell):**
```pwsh
irm https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.ps1 | iex
```

Once installed, `zoid update` keeps your install current (anonymous,
checksum-verified).

## Quickstart

```bash
zoid
```

On first launch, zoid walks you through choosing a provider, entering your
API key, and picking a model. After that you land straight in the chat.

## Features

- **Active context management** — event-sourced conversation history with
  automatic compaction, eviction, and a token economy view. The context
  window is managed, not just filled.
- **Bring your own tools** — MCP (Model Context Protocol) support for
  connecting external tools (GitHub, databases, APIs). Built-in tools
  (read, write, edit, grep, glob, shell, subagents) included.
- **Choose your own model** — Ollama, Anthropic, OpenAI, and OpenCode Zen.
  Switch providers without restarting your session. (Google Gemini support
  is implemented but not yet user-selectable.)
- **Modes and skills** — importable skill sets (like
  [Superpowers](https://github.com/obra/superpowers)) become first-class
  modes with their own system prompts and behaviors.
- **Steerable subagents** — dispatch isolated subagents for parallel work,
  with approval gates and timeout guardrails.
- **One binary** — ~16 MB, cold-starts in milliseconds. No runtime, no
  Electron, no language server daemon.

## Building from source

```bash
cargo build --workspace --release --features zoid/local-embed
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the full development
setup, including how to run the test suite, the crate architecture, and
release conventions.

## Get involved

- [Report a bug](https://github.com/strvmarv/zoid/issues)
- [Request a feature](https://github.com/strvmarv/zoid/issues)
- [Report a vulnerability](SECURITY.md)
- [Contribute](CONTRIBUTING.md) — PRs welcome
- [Code of Conduct](CODE_OF_CONDUCT.md)

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or
[Apache License, Version 2.0](LICENSE-APACHE), at your option.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.
```

- [ ] **Step 2: Verify all links resolve**

```bash
test -f CODE_OF_CONDUCT.md && echo "CoC ✓" || echo "CoC MISSING"
test -f docs/DEVELOPMENT.md && echo "DEVELOPMENT ✓" || echo "DEVELOPMENT MISSING"
test -f CONTRIBUTING.md && echo "CONTRIBUTING ✓" || echo "CONTRIBUTING MISSING"
test -f SECURITY.md && echo "SECURITY ✓" || echo "SECURITY MISSING"
test -f CHANGELOG.md && echo "CHANGELOG ✓" || echo "CHANGELOG MISSING"
test -f LICENSE-MIT && echo "LICENSE-MIT ✓" || echo "LICENSE-MIT MISSING"
test -f LICENSE-APACHE && echo "LICENSE-APACHE ✓" || echo "LICENSE-APACHE MISSING"
```

Expected: all ✓.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: full README rewrite for v1.0.0 public launch"
```

---

### Task 4: Update public/index.html to OSS framing

**Files:**
- Modify: `public/index.html` — remove "Now in beta", rewrite hero betanote, replace footer

**Interfaces:**
- Consumes: nothing from Tasks 1-3.
- Produces: OSS-framed marketing site. `deploy-pages.yml` auto-deploys on push.

- [ ] **Step 1: Fix the meta description (line 7)**

Remove "Now in beta." from the end of the meta description. Change:

```html
<meta name="description" content="zoid — a terminal-native AI coding agent built in Rust. Active context management, bring-your-own-tools (MCP), and your choice of model — Ollama, Anthropic, OpenAI, Google Gemini, OpenCode Zen. Now in beta.">
```

to:

```html
<meta name="description" content="zoid — a terminal-native AI coding agent built in Rust. Active context management, bring-your-own-tools (MCP), and your choice of model — Ollama, Anthropic, OpenAI, OpenCode Zen.">
```

- [ ] **Step 2: Remove the "Now in beta" chip (line 311)**

Delete the entire line:

```html
  <p class="beta fade d3"><span class="chip">Now in beta</span></p>
```

- [ ] **Step 3: Rewrite the hero betanote (line 315)**

Change:

```html
    <p class="betanote">Evaluation builds expire 30 days after release — run <code>zoid update</code> to stay current (anonymous, checksum-verified, from GitHub Releases). PowerShell installer &amp; per-platform archives on the releases page.</p>
```

to:

```html
    <p class="betanote">Run <code>zoid update</code> to stay current (anonymous, checksum-verified, from GitHub Releases). PowerShell installer &amp; per-platform archives on the releases page.</p>
```

- [ ] **Step 4: Replace the footer (lines 853-857)**

Replace:

```html
<footer class="wrap" role="contentinfo">
  <div class="wordmark">zoid</div>
  <p style="margin:16px 0 0;"><a class="btn" href="https://github.com/strvmarv/zoid/releases/latest">Install zoid</a></p>
  <p style="margin:12px 0 0;font-size:12px;color:var(--dim);">Download the latest release or build from source.</p>
  <p style="margin:14px 0 0;font-size:12px;">© 2026</p>
</footer>
```

with:

```html
<footer class="wrap" role="contentinfo">
  <div class="wordmark">zoid</div>
  <p style="margin:16px 0 0;"><a class="btn" href="https://github.com/strvmarv/zoid/releases/latest">Install zoid</a></p>
  <p style="margin:12px 0 0;font-size:12px;color:var(--dim);">Download the latest release or build from source.</p>
  <div class="links" style="margin:16px 0 0;font-size:13px;display:flex;gap:20px;flex-wrap:wrap;">
    <a href="https://github.com/strvmarv/zoid">⭐ GitHub</a>
    <a href="https://github.com/strvmarv/zoid/issues">Issues</a>
    <a href="https://github.com/strvmarv/zoid/blob/main/CONTRIBUTING.md">Contributing</a>
    <a href="https://github.com/strvmarv/zoid/blob/main/CHANGELOG.md">Changelog</a>
  </div>
  <p style="margin:14px 0 0;font-size:12px;">&copy; 2026 · MIT OR Apache-2.0</p>
</footer>
```

- [ ] **Step 5: Verify no commercial framing remains**

```bash
grep -in 'Now in beta\|Evaluation builds\|evaluation builds expire' public/index.html
```

Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add public/index.html
git commit -m "docs(site): remove beta framing, add GitHub CTA and get-involved footer links"
```

- [ ] **Step 7: Verify Pages auto-deploys**

After pushing, the `deploy-pages.yml` workflow fires on the `public/**`
path change. Verify:

```bash
sleep 10 && gh run list --workflow=deploy-pages.yml --limit 1 --json status,conclusion
```

The workflow should be `in_progress` or `completed`. Once complete, verify
the site:

```bash
curl -sL https://strvmarv.github.io/zoid/ | grep -c 'Now in beta'
```

Expected: `0` (no matches).