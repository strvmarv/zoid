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

## 0.6.0

Local models, agents as a first-class entity, peek popups, session management, and active context management that keeps small-context models productive.

### New

- **Local Ollama support** — connect zoid to a local Ollama daemon (`provider = "ollama-local"`). zoid requests an explicit context window from the daemon, so a local model never silently truncates your prompt. Configure the window size directly in your config file — no environment variables required.
- **Agent profiles** — define named agent profiles (e.g. `gilfoyle-tech-reviewer`) as simple `agent.md` files. zoid discovers them from configured source directories, lists them with the new `list_agents` tool, and lets you pick one when dispatching a subagent. Profiles carry a system prompt and tool set without touching code.
- **Peek popups** — click any tool-call line or delegated-chip in the conversation to open a scrollable popup showing the full tool output or delegation summary. Press `Esc` or click away to dismiss.
- **Session delete** — delete sessions right from the startup picker with an inline confirm. Cleanup is transactional — events, FTS index, and embeddings are all removed together.
- **Context budget awareness** — when running on a model with a small context window, zoid tells the assistant how much room it has and nudges it toward efficient tool use: search before reading, page through large files instead of loading them whole, and retrieve compacted content on demand. The assistant self-regulates its context consumption instead of overflowing.
- **Relevance-rescued eviction** — when zoid evicts old turns to make room, it now uses embedding similarity to prefer evicting turns that are least relevant to the current task, keeping the most useful context in the window. A configurable `rescue_weight` controls the balance between relevance and recency.
- **Eviction chips** — the conversation now shows a chip when turns are evicted, so you can see what was dropped. Zoom to Detail for a breakdown of the eviction span.
- **Thinking badge** — the old thinking marker line is replaced with a compact inline `·thinking` badge at Normal zoom, keeping the conversation readable while showing the model is reasoning.
- **Average TPS** — the session widget now shows a rolling per-turn tokens-per-second figure, so you can see how fast the model is generating.
- **Faster test suite** — the release gate now runs through `cargo-nextest`, which parallelizes at the test level and reports a single reliable pass/fail. Targeted build-profile optimization and fixture right-sizing cut execution time by ~30% with no coverage loss.

### Fixes

- **Context overflow protection** — when a single turn accumulates more tool output than the model's context window (e.g. reading several large files), zoid now force-compacts the largest tool results before sending the request, instead of letting the provider reject it. The compaction uses the model's real context window — including live-fetched values from Ollama — not a static fallback.
- **Tool call truncation on thinking models** — models that produce internal reasoning tokens could exhaust the output budget before completing a tool call, producing malformed arguments. zoid now doubles the output budget for thinking-capable models even when thinking is disabled in the request.
- **Smaller default file reads** — the `read` tool now returns 500 lines by default (was 2000), reducing per-call context cost from ~10K to ~2.5K tokens. The assistant can still page through large files with `offset` and `limit`.
- **Width-aware truncation** — tool-call summaries and first-line previews now cap to the available conversation width instead of a fixed 120 columns, so wide terminals show more and narrow ones don't clip.
- **Subagent drawer cleanup** — the subagent ID is no longer shown in the right-rail display; the agent profile name is shown instead when available.

> **Beta note:** builds are for evaluation and expire ~30 days after release — run `zoid update` periodically to stay current.

## 0.5.0

Add-ons open up: browse a community catalog from inside zoid, set up an MCP server in a few keystrokes — always seeing exactly what you're about to trust — and hand the mouse back to your terminal when you just want to copy some text.

### New

- **Plugin catalog** — type `:plugin` to browse a curated catalog of community add-ons without leaving zoid. Nothing is installed until you confirm, and before you do, zoid shows you what you're about to trust: for skill packs, the upstream project, the exact pinned commit, and its license when one is declared. The catalog is public and open to contributions. `:plugin list` opens the same catalog as a read-only listing when you just want to look.
- **Connect MCP servers from the catalog** — set up a Model Context Protocol server straight from the catalog. Before anything is written, zoid shows you the **exact command it will run** and every environment variable the server expects, flagging the ones that aren't set on your machine. Pick whether it lands in your personal setup or just this project. Setup only ever adds — a server you already configured is never overwritten — and zoid never asks for or stores your secrets: credentials stay as references to your own environment variables. The server connects the next time you start zoid.
- **Select mode** — press `Alt+M` (or `:select`) to hand the mouse back to your terminal, so drag-select and your terminal's own copy work anywhere on screen. A SELECT indicator sits in the status bar so you always know which mode you're in.
- **Skill packs** — install an add-on as a full mode or as a plain set of skills with `--mode` / `--skills`. Each pack keeps to its own space, so installing one never disturbs another.

### Fixes

- **Accurate install feedback** — installing a skill pack now reports honestly how many skills landed and tells you a restart is needed to load them, rather than implying they were live immediately.
- **Correct new-session hint** — the empty-session hint now points at `:session resume`, the command that actually exists.

> **Beta note:** builds are for evaluation and expire ~30 days after release — run `zoid update` periodically to stay current.

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
