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

## 0.4.0

The biggest release yet — more model providers to choose from, one-command add-ons, subagents you can steer and trust, and a clearer view of every edit.

### New

- **More model providers** — beyond the existing options, zoid now speaks to **OpenAI**, **Google Gemini**, and **OpenCode Zen**, with a large built-in catalog of models to pick from. Point zoid at the backend you already use.
- **Plugins** — install curated add-ons in one step with the new `:plugin install` command (also on the command palette). The **Superpowers** engineering skill set ships bundled, so `:plugin install superpowers` sets you up instantly.
- **Scheduled wake-ups** — the assistant can now schedule itself to pick a task back up later and resume on its own, instead of stalling while it waits.
- **Steer your subagents** — cancel a running delegated task at any time (press `Esc` to escalate to a stop), and delegations now enforce sensible idle and overall time limits so a stuck subagent can't hang your session.
- **Trustworthy delegation** — subagent results are now checked for tool-execution integrity: if a delegated task claims work it didn't actually perform, that's flagged instead of silently passing.
- **Inline edit diffs** — when the assistant edits a file you now see `+N −M` line counts and an inline preview of the change, toggleable in settings.

### Fixes

- **Worktree edits land where they should** — commits made while working in a git worktree now reliably go to the worktree's branch, and entering/leaving a worktree keeps tooling responsive.
- **Steadier subagent dispatch** — resolved errors and display corruption that could occur when delegating work, and the main view now wakes promptly when a delegated result arrives.

> **Beta note:** builds are for evaluation and expire ~30 days after release — run `zoid update` periodically to stay current.

## 0.3.2

### New

- **Keyboard-shortcuts help** — a built-in reference of every shortcut and command, so you no longer have to hunt for them. Open it with `?` from the conversation view, the `:help` command, or "Keyboard shortcuts…" in the command palette. It's a scrollable overlay grouped by context — global keys, input, conversation, overlays, commands, and mouse. New sessions now point you to it.

> **Beta note:** builds are for evaluation and expire ~30 days after release — run `zoid update` periodically to stay current.

## 0.3.1

### Fixes

- **Tidier launch output** — fixed a display glitch where the startup progress lines could appear misaligned, each stepping further to the right, on launches that show the session picker.

## 0.3.0

Faster to get started and easier to live with — a one-command way to add a curated engineering skill set, clearer feedback while zoid starts up, and a clean uninstall.

### New

- **One-command Superpowers install** — add a curated skill set for structured software-engineering workflows (test-driven development, systematic debugging, code review, planning, and parallel agents) as a ready-to-use mode, in a single step. It's offered during first-run onboarding, and available any time from the command palette.
- **Startup progress** — zoid now tells you what it's doing while it launches instead of sitting silent — opening your session, preparing skills and modes, and loading its on-device model. The first time it downloads that model, you get a live progress readout so you know it's working, not stuck.
- **Clean uninstall** — `zoid uninstall` removes zoid's data (sessions, configuration, and the model cache) after a typed confirmation, and tells you where the binary lives so you can finish up. Run `zoid uninstall --purge` to remove the binary too.

### Improved

- **Better out-of-the-box defaults** — a larger default working-context size and a smarter automatic-compaction threshold, so long sessions stay coherent for longer before anything is condensed.
- **More reliable multi-line paste** — pasting several lines into the prompt now routes correctly regardless of what's focused on screen.
- **Smoother skill-import wizard** — steadier behavior when reviewing, approving, and rejecting imported skills, plus faster scans of large skill folders.

### Fixes

- Consistency and stability improvements across tools and terminal rendering.

> **Beta note:** builds are for evaluation and expire ~30 days after release — run `zoid update` periodically to stay current.

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
