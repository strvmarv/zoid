# Open-sourcing zoid — Phase 4: Polish

**Date:** 2026-08-16
**Approach:** Four documentation/marketing deliverables, no code changes.

## Problem

The repo is public and v1.0.0 is released, but the developer docs are
scattered, the README is a minimal stub, the marketing site still has
commercial-era framing, and there's no code of conduct for contributors.
Phase 4 polishes these for first impressions before the Phase 5 community
infra and Phase 6 public launch.

## Solution

Four deliverables:
1. `CODE_OF_CONDUCT.md` — Contributor Covenant v2.1.
2. `docs/DEVELOPMENT.md` — consolidated developer-facing conventions.
3. `README.md` — full OSS rewrite from the minimal Phase 3a stub.
4. `public/index.html` — OSS framing (remove beta, add GitHub CTA).

## Scope

**In scope:**
- Create `CODE_OF_CONDUCT.md` at repo root.
- Create `docs/DEVELOPMENT.md` consolidating build/test/release conventions
  and a crate architecture overview.
- Rewrite `README.md` as a proper project landing page.
- Update `public/index.html` framing: remove "Now in beta" chip and meta
  description, add GitHub stars/link CTA, add "Get involved" section.

**Out of scope:**
- Phase 5 (issue templates, PR template, GitHub Discussions, CI badge —
  the CI badge depends on a test workflow existing, which is a Phase 5
  concern).
- Phase 6 (coordinated public launch).
- Any `crates/*/src/` changes.
- New screenshots — use existing TUI frame art from `public/index.html` if
  imagery is needed; no new renders.

## 1. CODE_OF_CONDUCT.md

Standard Contributor Covenant v2.1 (https://contributor-covenant.org/version/2/1/code_of_conduct/).
Copy verbatim. No project-specific customization needed — the enforcement
contact is `strvmarv@gmail.com` (the maintainer's GitHub email).

## 2. docs/DEVELOPMENT.md

Consolidates developer-facing conventions currently scattered across
`AGENTS.md` (release process, terminal minimum size, dist workflow, memory)
and `docs/RELEASING.md` (release mechanics). This is about consolidating
into one developer-facing doc — not fixing stale framing (already done in
Phase 2).

### Structure

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
| `zoid-tui` | The terminal UI: layout, renderers, snapshot tests |
| `zoid-tools` | Built-in tools (read, write, edit, grep, glob, shell, subagents, etc.) |
| `zoid-mcp` | MCP (Model Context Protocol) server discovery and connection management |
| `zoid-embed` | Local embedding model (candle) for semantic retrieval |
| `zoid-companion` | Companion browser view server (visual side panel) |
| `zoid-plugin` | Plugin manifest types and parsing |
| `zoid-plugin-import` | Filesystem source adapter for plugin import |
| `zoid-syntax` | Syntax highlighting / code parsing utilities |
| `zoid-testkit` | Test fixtures and helpers (intentionally version-pinned, excluded from release) |
| `zoid-web` | Web companion app |

Dependency flow: `zoid` depends on everything; `zoid-core` depends on
`zoid-model` (not `zoid-provider`); `zoid-provider` depends on `zoid-model`;
`zoid-tui` depends on `zoid-core` and `zoid-model`. The provider seam is
intentionally decoupled from core so the provider/plugin surface stays
independent.

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

## 3. README.md — full OSS rewrite

The current README is a 41-line stub from Phase 3a. The rewrite makes it a
proper project landing page for first impressions.

### Structure

```markdown
# zoid

[license badge] [CI badge placeholder]

A terminal-native AI coding agent, built in Rust. Active context management,
bring-your-own-tools via MCP, and your choice of model — Ollama, Anthropic,
OpenAI, Google Gemini, and OpenCode Zen.

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
- **Choose your own model** — Ollama, Anthropic, OpenAI, Google Gemini, and
  OpenCode Zen. Switch providers without restarting your session.
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
- [Contribute](CONTRIBUTING.md) — PRs welcome
- [Code of Conduct](CODE_OF_CONDUCT.md)

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or
[Apache License, Version 2.0](LICENSE-APACHE), at your option.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.
```

### Badge placeholders

The CI badge will be added in Phase 5 once a test workflow exists. For now,
include only the license badge:

```markdown
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)
```

## 4. public/index.html — OSS framing

### Changes

1. **Meta description (line 7):** Remove "Now in beta." from the end.
2. **Hero "Now in beta" chip (line 311):** Remove the
   `<p class="beta fade d3"><span class="chip">Now in beta</span></p>`
   line entirely.
3. **Hero betanote (line 315):** Already neutralized in Phase 3b ("from
   GitHub Releases"). Keep as-is.
4. **Footer "Get involved" section:** Replace the current footer with a
   richer version that includes:
   - The install button (already present)
   - A "Get involved" section with links to GitHub, issues, contributing
   - A GitHub stars link/CTA
   - The neutral betanote (already present from Phase 3b)

### Footer replacement

Current footer (lines 853-857):
```html
<footer class="wrap" role="contentinfo">
  <div class="wordmark">zoid</div>
  <p style="margin:16px 0 0;"><a class="btn" href="https://github.com/strvmarv/zoid/releases/latest">Install zoid</a></p>
  <p style="margin:12px 0 0;font-size:12px;color:var(--dim);">Download the latest release or build from source.</p>
  <p style="margin:14px 0 0;font-size:12px;">© 2026</p>
</footer>
```

New footer:
```html
<footer class="wrap" role="contentinfo">
  <div class="wordmark">zoid</div>
  <p style="margin:16px 0 0;"><a class="btn" href="https://github.com/strvmarv/zoid/releases/latest">Install zoid</a></p>
  <p style="margin:12px 0 0;font-size:12px;color:var(--dim);">Download the latest release or build from source.</p>
  <div class="links" style="margin:16px 0 0;font-size:13px;display:flex;gap:20px;flex-wrap:wrap;">
    <a href="https://github.com/strvmarv/zoid">GitHub</a>
    <a href="https://github.com/strvmarv/zoid/issues">Issues</a>
    <a href="https://github.com/strvmarv/zoid/blob/main/CONTRIBUTING.md">Contributing</a>
    <a href="https://github.com/strvmarv/zoid/blob/main/CHANGELOG.md">Changelog</a>
  </div>
  <p style="margin:14px 0 0;font-size:12px;">&copy; 2026 · MIT OR Apache-2.0</p>
</footer>
```

### What stays

- All TUI frame demos (the product's best selling point)
- The feature sections (tools, modes, zoom, Rust binary)
- The hero headline and subheading
- The install button and curl one-liner
- The color scheme and visual design

## Verification

- `CODE_OF_CONDUCT.md` exists at root and contains the Contributor Covenant.
- `docs/DEVELOPMENT.md` exists and covers build, test, release, architecture.
- `README.md` renders correctly on GitHub (check the preview).
- `public/index.html` has no "Now in beta" references.
- `public/index.html` footer has "Get involved" links.
- `curl -sL https://strvmarv.github.io/zoid/` returns the updated site
  (Pages auto-deploys on `public/**` pushes).
- `grep -rn 'Now in beta' public/index.html` returns nothing.
- No `crates/*/src/` files touched.