<!--
  PUBLIC release notes. This file is the customer-facing changelog: cargo-dist
  parses the section matching each release tag and publishes it (verbatim) to
  the public strvmarv/zoid-releases Release. Write for users, as a commercial
  product would — capabilities and fixes, no internal implementation detail
  (crate names, algorithms, file paths). Each version header must be a bare
  `## X.Y.Z` matching the tag (e.g. `## 0.2.0` for tag `v0.2.0`). The detailed
  engineering changelog lives in docs/CHANGELOG.md and is NOT published.
-->

# Release Notes

## 0.2.0

Our largest update yet — smarter memory for long sessions, an open plugin ecosystem, and a faster, more refined terminal experience.

### New

- **Active Context Management** — zoid keeps long conversations coherent by intelligently paging context in and out of the model's window instead of cutting it off. Earlier parts of a session are brought back automatically when they become relevant again.
- **Semantic recall** *(opt-in)* — on-device semantic search over your session history, blending keyword and meaning-based matching so the right context resurfaces even when you don't recall the exact wording. Runs entirely locally — nothing leaves your machine.
- **Connect your own tools (MCP)** — zoid can now use tools from any Model Context Protocol server you configure, right alongside its built-ins. Point it at your existing setup and the tools appear automatically.
- **Modes & skills** — bundle a set of skills into a first-class mode and switch modes instantly with Shift+Tab. Import skills from a folder or a URL.
- **Built-in file toolkit** — first-class read, write, edit, search, glob, and directory-listing tools for working directly in your codebase.
- **In-app feedback** — send feedback or file an issue without leaving zoid.
- **Reasoning controls** — view and tune extended thinking directly in the interface.

### Improved

- **Interruptions that respect you** — press Esc once to gracefully stop the current turn; press it again to force-stop a stuck command immediately.
- **Redesigned settings & model picker** — a guided, full-screen settings experience with a visible provider/model picker, live model discovery, and quick provider switching (Alt+P).
- **Runs safely alongside itself** — multiple zoid instances can run at once without conflict, with a per-project startup picker to resume or start sessions.
- **A cleaner, faster terminal UI** — redesigned command palette, inline question cards, status indicators, first-run onboarding, autocomplete, and a `:compact` command to tidy long sessions.
- **Broader provider support** and an optional local metrics dashboard.

### Fixes

- Wide-ranging stability and correctness improvements across session storage, tools, and terminal rendering.

> **Beta note:** builds are for evaluation and expire ~30 days after release — run `zoid update` periodically to stay current.

## 0.1.2

- Distribution and self-update reliability improvements.

## 0.1.1

- Install instructions and one-command self-update (`zoid update`) refinements.

## 0.1.0

First distributed release.

- Prebuilt binaries for Linux, macOS (Apple Silicon), and Windows.
- One-command install and anonymous, checksum-verified self-update via `zoid update`.
