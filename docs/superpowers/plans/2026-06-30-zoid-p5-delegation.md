# zoid P5 — Single-Subagent Delegation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Chat hand one discrete, non-trivial unit of work to one isolated subagent — driven by a built-in `AgentProfile`, seeded with a precisely-constructed context (the unit + relevant code, never the session transcript), run in an isolated `git2` worktree, with its result folded back into the conversation as a collapsible card.

**Architecture:** A pure `AgentProfile` (core) parameterizes a reusable subagent executor (bin). The P3 constructed-context assembler is finally wired into dispatch: `context_window → assemble_context` selects the relevant File items, `build_subagent_request` resolves them to a task-focused prompt, and the *generalized* Chat agent loop (`run_agent_turn` behind a `TurnConfig`) runs that prompt on its own `subagent:<id>` branch inside a `git2` worktree. On completion the orchestrator records a `DelegationResult` event on the main branch; `conversation()` folds only main-branch events and renders the result as a `▸ delegated` card (① semantic zoom: collapsed at Normal, expanded at Detail). One subagent at a time; hand-dispatched.

**Tech Stack:** Rust 2021 (workspace: `zoid-core`, `zoid-provider`, `zoid-tui`, `zoid-tools`, `zoid-syntax`, `zoid` bin). `git2` (libgit2, added this plan), `tokio`, `ratatui` + `insta` (`TestBackend`), `FakeProvider` for deterministic offline tests.

**Prerequisites:** Plan 1 (Chat Polish) and Plan 2 (Sessions & DB) are assumed merged. This plan CONSUMES `session_id` on every Event (added in Plan 2) to scope subagent spend to the active session, and renders result cards into the transcript whose message bodies use Plan 1's markdown renderer. This is Plan 3 of 3.

> **Grounding note (read before coding):** the code in this plan is written against the CURRENT tree (as of `b1ccd89`). In the current tree `Event` has **no** `session_id` field and there is **no** markdown renderer — those are the Plan 1 / Plan 2 prerequisites. Where a step would consume them, it is annotated `PLAN-2-SEAM` / `PLAN-1-SEAM` with the grounded fallback that compiles today (session scoping via the per-session DB snapshot; plain-text card body). When Plan 1/Plan 2 land, thread `session_id`/`render_markdown` at those seams; nothing else in this plan changes. Every other snippet compiles against `b1ccd89` as written.

## Global Constraints

- Rust edition 2021; workspace crates `zoid-core`, `zoid-provider`, `zoid-tui`, `zoid-tools`, `zoid-syntax`, `zoid` bin. Size-optimized release profile (unchanged: `opt-level="z"`, `lto`, `strip`, `panic="abort"`).
- **§16 Design tokens:** NO literal glyphs/hex outside `crates/zoid-tui/src/tokens.rs`. The delegated result CARD's glyphs (collapse `▸` / expand `▾` chevrons, `✓`/`⚠` status, `⏎ peek`) and its purple border + `#15101f` background come from tokens (`glyph::COLLAPSED`/`EXPANDED`/`PASS`/`WARNING`/`RETURN`, `color::BRANCH`, new `color::DELEGATE_BG`). The `·` middot separator is plain punctuation (already used verbatim in `chat.rs`/`render.rs`), not a §16 glyph.
- **TDD is the default.** Every code step is preceded by a failing test.
- **Every new/changed TUI screen** (the delegated result card) ships an `insta` snapshot using `format!("{:#?}", terminal.backend().buffer())` (Buffer Debug, captures style) in `crates/zoid-tui/tests/snapshots/`, bound to `docs/ux/chat-mode.html` (which shows the delegation example — the chip `▸ delegated · shared NotFound helper  ✓ done  → peek card`, purple `#bc8cff` border, `#15101f` bg).
- **Superpowers invariant (asserted in a test):** a delegated subagent's constructed context contains ONLY the unit of work + relevant code — NEVER the session transcript/history (core §4.4, chat §5.4/§10).
- **One subagent at a time.** Hand-dispatched, sequential. No parallel fleet, no autonomous scheduler, no per-task review pipeline (all Build concerns).
- Core is clock-free (ts injected via `now: fn() -> i64`). `git2` and process concerns stay OUT of `zoid-core` (bin-only).
- Commit messages END with `Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY`. NEVER a co-author / `Co-Authored-By` trailer (user global instruction).
- Run `cargo test --workspace` and `cargo clippy --all-targets` clean before every commit.

---

## File Structure

Created:
- `crates/zoid-core/src/agent_profile.rs` — pure `AgentProfile` type (mirrors `.claude/agents` schema) + the one built-in profile. **(Work item A)**
- `crates/zoid/src/subagent.rs` — the subagent runtime: `build_subagent_request` (constructed context), `subagent_policy`, `SubagentResult`, `run_subagent`. **(Work items B, C, E)**
- `crates/zoid/src/worktree.rs` — `git2` worktree isolation: `create_worktree` + `WorktreeGuard` (auto-cleanup on drop). **(Work item C)**
- `crates/zoid/tests/worktree_test.rs` — worktree create + isolation + cleanup against a real temp git repo. **(C)**
- `crates/zoid/tests/subagent_integration.rs` — a subagent edits a file inside its worktree; main repo untouched. **(C)**
- `crates/zoid/tests/delegation_integration.rs` — a delegation folds into the main conversation; spend lands in the session ledger. **(D, E)**

Modified:
- `crates/zoid-core/src/lib.rs` — `pub mod agent_profile;`.
- `crates/zoid-core/src/context.rs` — add `file_contents(events)` (resolve File keys → content). **(B)**
- `crates/zoid-core/src/event.rs` — add `EventKind::DelegationResult { branch, summary, ok }`. **(D)**
- `crates/zoid-core/src/projection.rs` — add `ChatMsg::Delegated { summary, ok }`; branch-filter + fold. **(D)**
- `crates/zoid-core/src/zoom.rs` — `ChatMsg::Delegated` digest arm. **(D)**
- `crates/zoid-tools/src/lib.rs` — thread `cwd: &Path` through `Tool::run` + `run_tool`; add `resolve`. **(B)**
- `crates/zoid-tools/src/{read,write,edit,search,shell}.rs` — `run` takes `cwd`; resolve paths against it. **(B)**
- `crates/zoid/src/agent.rs` — `TurnConfig`; `build_request` gains `system`; `run_agent_turn` returns `Vec<Event>`; thread branch/cwd; `map_msg` `Delegated` arm. **(B, D)**
- `crates/zoid/src/lib.rs` — `pub mod subagent;`, `pub mod worktree;`.
- `crates/zoid/src/main.rs` — Chat caller builds `chat_turn_config()`; `delegating` guard; `start_delegation`; `VerbPick` rewire; clear guard on result. **(D)**
- `crates/zoid-tui/src/tokens.rs` — add `color::DELEGATE_BG` (#15101f). **(D)**
- `crates/zoid-tui/src/chat.rs` — render `ChatMsg::Delegated` (collapsed/expanded by altitude). **(D)**
- `crates/zoid-tui/src/command.rs` — `Command::Delegate(String)`. **(D)**
- `crates/zoid-tui/tests/shell_snapshot.rs` — delegated-card snapshots @100/@140. **(D)**

Suggested execution order: **A → B → C → D → E** (a work item's tasks are numbered `A1`, `B1`, …). Each task is one reviewable, committed unit.

---

## A. `AgentProfile` (core §4.4 / §7)

Spec: workers are parameterized by an `AgentProfile` — "system prompt + skill overlays + tool allow-list + model, shaped to mirror the adopted `.claude/agents` file schema (name, description, tools, model + system-prompt body)". v1 ships ONE built-in profile used by Chat's delegation; **no file loader yet** (POST-V1, per §7 "loaders built on demand").

### Task A1: `AgentProfile` type + one built-in profile

**Files:** Create `crates/zoid-core/src/agent_profile.rs`; Modify `crates/zoid-core/src/lib.rs`. Test: inline `mod tests`.

**Interfaces:**
- Consumes: nothing (pure; no external deps — `zoid-core` stays `git2`/provider-free).
- Produces:
  - `pub struct AgentProfile { pub name: String, pub description: String, pub system_prompt: String, pub tools: Vec<String>, pub model: Option<String> }` — `tools` is the tool-name allow-list (empty = all); `model: None` = inherit the orchestrator's model.
  - `pub fn allows(&self, tool: &str) -> bool`.
  - `pub fn builtin() -> AgentProfile` — the single v1 delegation profile.

- [ ] **Step 1: Write the failing test** — in `crates/zoid-core/src/agent_profile.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profile_exposes_allow_list_and_prompt() {
        let p = AgentProfile::builtin();
        assert!(!p.name.is_empty());
        assert!(!p.description.is_empty());
        assert!(!p.system_prompt.is_empty());
        // The built-in profile may edit files and run the shell.
        assert!(p.allows("write_file"));
        assert!(p.allows("edit_file"));
        assert!(p.allows("shell"));
        // A tool NOT on the allow-list is denied.
        assert!(!p.allows("launch_missiles"));
    }

    #[test]
    fn empty_allow_list_permits_everything() {
        let p = AgentProfile {
            name: "open".into(),
            description: "anything".into(),
            system_prompt: "sys".into(),
            tools: vec![],
            model: None,
        };
        assert!(p.allows("anything_at_all"));
    }
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p zoid-core agent_profile`
  Expected: compile error — module/type undefined.

- [ ] **Step 3: Implement** — `crates/zoid-core/src/agent_profile.rs` (above the test module):

```rust
//! `AgentProfile` (core §4.4/§7): the parameterization of a subagent worker —
//! shaped to mirror the `.claude/agents` file schema (name, description, tools,
//! model, system-prompt body). v1 ships ONE built-in profile used by Chat's
//! delegation; the file loader and named registry are POST-V1 (loaders built on
//! demand — §7). Pure; `zoid-core` takes no provider/`git2`/process deps.

/// A subagent worker's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    /// Stable profile name (`.claude/agents` `name`).
    pub name: String,
    /// One-line description of what this worker is for.
    pub description: String,
    /// The worker's system prompt (the `.claude/agents` markdown body).
    pub system_prompt: String,
    /// Tool-name allow-list. Empty = every tool is permitted.
    pub tools: Vec<String>,
    /// Model override; `None` inherits the orchestrator's model.
    pub model: Option<String>,
}

impl AgentProfile {
    /// Whether this profile permits calling `tool`. An empty allow-list permits
    /// all tools (the profile does not constrain the tool set).
    pub fn allows(&self, tool: &str) -> bool {
        self.tools.is_empty() || self.tools.iter().any(|t| t == tool)
    }

    /// The single built-in delegation profile (v1). Full curated tool set;
    /// inherits the orchestrator's model.
    pub fn builtin() -> AgentProfile {
        AgentProfile {
            name: "delegate".into(),
            description: "Complete one discrete unit of work autonomously.".into(),
            system_prompt: "You are a zoid subagent. You are given ONE discrete task and the \
                relevant code. Complete the task end to end using the tools (read, write, edit, \
                search, shell). Work autonomously — do not ask questions. When done, give a \
                one-paragraph summary of what you changed."
                .into(),
            tools: vec![
                "read_file".into(),
                "write_file".into(),
                "edit_file".into(),
                "search".into(),
                "shell".into(),
            ],
            model: None,
        }
    }
}
```

In `crates/zoid-core/src/lib.rs`, add `pub mod agent_profile;` (alongside the other `pub mod` lines).

- [ ] **Step 4: Run to confirm pass** — `cargo test -p zoid-core agent_profile`
  Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/agent_profile.rs crates/zoid-core/src/lib.rs
git commit -m "feat(core): AgentProfile — .claude/agents-shaped worker config + built-in profile

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

## B. Subagent executor + constructed-context wiring (core §4.4)

The reusable executor runs ONE agent turn in isolation from a **constructed context** + an `AgentProfile`, NEVER the session history. It reuses the `agent.rs` loop shape (generalized, not forked) against a provider (FakeProvider in tests). Token spend lands in the same session log (→ work item E). B1 gives tools a working directory (needed so a subagent runs somewhere other than the process cwd — the worktree, work item C). B2/B3 build the constructed context. B4 generalizes the loop and adds `run_subagent`.

### Task B1: Thread `cwd: &Path` through the tool trait and all five tools

**Files:** Modify `crates/zoid-tools/src/lib.rs` and `crates/zoid-tools/src/{read,write,edit,search,shell}.rs`. Test: inline (existing tests adopt the arg + 2 new).

**Interfaces:**
- Produces:
  - `trait Tool { fn run(&self, args: &Value, cwd: &Path) -> ToolOutput; … }`.
  - `pub(crate) fn resolve(cwd: &Path, path: &str) -> PathBuf` — `cwd.join(path)` for relative, passthrough for absolute.
  - `pub fn run_tool(tools: &[Box<dyn Tool>], name: &str, args: &Value, cwd: &Path) -> ToolOutput`.

> One atomic task: the trait change forces every impl + `run_tool` to update together or the crate won't compile. Resolving against `cwd` is for subagent relocation, **not** a security jail (spec §9: no path-jailing — do not add path-escape checks).

- [ ] **Step 1: Write the failing tests** — in `crates/zoid-tools/src/lib.rs` `mod tests`:

```rust
#[test]
fn resolve_joins_relative_and_passes_absolute() {
    use std::path::Path;
    assert_eq!(resolve(Path::new("/work"), "src/a.rs"), Path::new("/work/src/a.rs"));
    assert_eq!(resolve(Path::new("/work"), "/etc/hosts"), Path::new("/etc/hosts"));
}

#[test]
fn read_tool_resolves_relative_to_cwd() {
    use std::path::Path;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "in cwd").unwrap();
    let out = crate::read::ReadFile.run(&serde_json::json!({ "path": "note.txt" }), dir.path());
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(out.text, "in cwd");
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p zoid-tools resolve`
  Expected: compile error — `resolve` undefined / `run` takes 1 argument.

- [ ] **Step 3: Trait + `resolve` + `run_tool` in `lib.rs`** — add `use std::path::{Path, PathBuf};` next to `use serde_json::Value;`. Change the trait method to `fn run(&self, args: &Value, cwd: &Path) -> ToolOutput;`. Add after `str_arg`:

```rust
/// Resolve a tool's path argument against the run's working directory.
/// Relative paths join `cwd`; absolute paths pass through. For subagent
/// relocation, NOT a security jail (spec §9: no path-jailing).
pub(crate) fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
}
```

Change `run_tool`:

```rust
pub fn run_tool(tools: &[Box<dyn Tool>], name: &str, args: &Value, cwd: &Path) -> ToolOutput {
    match tools.iter().find(|t| t.name() == name) {
        Some(t) => t.run(args, cwd),
        None => ToolOutput::err(format!("unknown tool: {name}")),
    }
}
```

Fix the existing `unknown_tool_is_error_not_panic` call to `run_tool(&reg, "nope", &json!({}), std::path::Path::new("."))`.

- [ ] **Step 4: `read.rs`** — add `use std::path::Path;`; change `run` to resolve:

```rust
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") { Ok(p) => p, Err(e) => return e };
        match std::fs::read_to_string(crate::resolve(cwd, &path)) {
            Ok(contents) => ToolOutput::ok(contents),
            Err(e) => ToolOutput::err(format!("read_file({path}): {e}")),
        }
    }
```

In `read.rs` `mod tests`, append `, std::path::Path::new(".")` to each `ReadFile.run(...)` call (existing tests pass absolute tempfile paths or `/no/such/...`, so `cwd="."` is behavior-preserving).

- [ ] **Step 5: `write.rs`** — add `use std::path::Path;`; wrap the write path in `crate::resolve(cwd, &path)`; append `, std::path::Path::new(".")` to each `WriteFile.run(...)` test call.

- [ ] **Step 6: `edit.rs`** — add `use std::path::Path;`; resolve once (`let full = crate::resolve(cwd, &path);`) and use `&full` for both `read_to_string` and `write`; append `, std::path::Path::new(".")` to each `EditFile.run(...)` test call.

- [ ] **Step 7: `search.rs`** — change `run(&self, args: &Value, cwd: &Path)`; root the walk at `crate::resolve(cwd, args.get("path").and_then(|v| v.as_str()).unwrap_or("."))`; append `, std::path::Path::new(".")` to each `Search.run(...)` test call (add `use std::path::Path;` to its test module).

- [ ] **Step 8: `shell.rs`** — add `use std::path::Path;`; change `run(&self, args: &Value, cwd: &Path)` and set the command cwd:

```rust
        let output = if cfg!(windows) {
            Command::new("cmd").arg("/C").arg(&command).current_dir(cwd).output()
        } else {
            Command::new("sh").arg("-c").arg(&command).current_dir(cwd).output()
        };
```

Keep the rest of the body unchanged. Append `, std::path::Path::new(".")` to each `Shell.run(...)` test call (shell tests are cwd-independent — `echo`/`exit`).

- [ ] **Step 9: Run the crate suite** — `cargo test -p zoid-tools && cargo clippy -p zoid-tools --all-targets`
  Expected: PASS (existing + 2 new), zero warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-tools/src/
git commit -m "feat(tools): thread cwd through Tool::run + run_tool; resolve relative paths against it

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task B2: `file_contents()` — resolve File item keys to content (core)

**Files:** Modify `crates/zoid-core/src/context.rs`. Test: inline `mod tests`.

**Interfaces:**
- Consumes: `Event`, `EventKind`, `economy::tool_path` (already imported at top of `context.rs`).
- Produces: `pub fn file_contents(events: &[Event]) -> std::collections::HashMap<String, String>` — `"file:{path}"` → the latest **non-error** tool-result output for that path (latest wins). Keys match `context_window`'s File keys exactly, so a `ContextItem.key` resolves here.

- [ ] **Step 1: Write the failing test** — in `crates/zoid-core/src/context.rs` `mod tests` (reuse the existing `u`/`call`/`result` helpers there):

```rust
#[test]
fn file_contents_resolves_latest_output_by_path_key() {
    let evs = vec![
        u("go"),
        call("c1", "read_file", "src/a.rs"),
        result("c1", "read_file", "fn one() {}"),
        call("c2", "read_file", "src/a.rs"),       // re-read → latest wins
        result("c2", "read_file", "fn two() {}"),
        call("c3", "read_file", "src/b.rs"),
        result("c3", "read_file", "// b"),
        // a non-file tool result must NOT be keyed as a file
        ev(EventKind::ToolCall { id: "c4".into(), name: "shell".into(), args: r#"{"command":"ls"}"#.into() }),
        ev(EventKind::ToolResult { id: "c4".into(), name: "shell".into(), output: "out".into(), is_error: false }),
    ];
    let map = file_contents(&evs);
    assert_eq!(map.get("file:src/a.rs").map(String::as_str), Some("fn two() {}"));
    assert_eq!(map.get("file:src/b.rs").map(String::as_str), Some("// b"));
    assert!(!map.keys().any(|k| k.starts_with("tool:")));
}

#[test]
fn file_contents_skips_errored_results() {
    let evs = vec![
        u("go"),
        call("c1", "read_file", "x.rs"),
        ev(EventKind::ToolResult { id: "c1".into(), name: "read_file".into(), output: "boom".into(), is_error: true }),
    ];
    assert!(file_contents(&evs).get("file:x.rs").is_none());
}
```

(The `ev`/`u`/`call`/`result` helpers exist in `context.rs` `mod tests`; `call` produces a `read_file` ToolCall with `{"path":...}` args, `result` a non-error ToolResult.)

- [ ] **Step 2: Run to confirm failure** — `cargo test -p zoid-core context::tests::file_contents`
  Expected: FAIL — `file_contents` undefined.

- [ ] **Step 3: Implement** — in `crates/zoid-core/src/context.rs`, after `context_window`:

```rust
/// Resolve each File context item to its content: `"file:{path}"` → the latest
/// non-error tool-result output for that path. Mirrors `context_window`'s File
/// keying so a `ContextItem.key` looks up here. Used by the subagent context
/// builder (P5) to fetch relevant code WITHOUT the chat transcript.
pub fn file_contents(events: &[Event]) -> HashMap<String, String> {
    let mut call_path: HashMap<String, String> = HashMap::new(); // tool id → path
    let mut out: HashMap<String, String> = HashMap::new();
    for e in events {
        match &e.kind {
            EventKind::ToolCall { id, args, .. } => {
                if let Some(p) = tool_path(args) {
                    call_path.insert(id.clone(), p);
                }
            }
            EventKind::ToolResult { id, output, is_error, .. } => {
                if !is_error {
                    if let Some(p) = call_path.get(id) {
                        out.insert(format!("file:{p}"), output.clone()); // latest wins
                    }
                }
            }
            _ => {}
        }
    }
    out
}
```

(`HashMap` and `tool_path` are already imported at the top of `context.rs`.)

- [ ] **Step 4: Run to confirm pass** — `cargo test -p zoid-core context::tests::file_contents`
  Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/context.rs
git commit -m "feat(core): file_contents — resolve File item keys to latest content (subagent ctx)

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task B3: `build_subagent_request` + `subagent_policy` — assembler → constructed context (the superpowers invariant)

**Files:** Create `crates/zoid/src/subagent.rs`; Modify `crates/zoid/src/lib.rs`. Test: inline `mod tests`.

**Interfaces:**
- Consumes: `zoid_core::context::{context_window, file_contents, ItemKind}`, `zoid_core::assembler::{assemble_context, ContextPolicy}`, `zoid_core::agent_profile::AgentProfile`, `zoid_provider::{CompletionRequest, Message}`, `crate::agent::tool_specs` (already `pub`), `zoid_tools::Tool`.
- Produces:
  - `pub fn subagent_policy() -> ContextPolicy` — cold-evicting + a token ceiling (bounded context).
  - `pub fn build_subagent_request(task: &str, events: &[Event], policy: &ContextPolicy, profile: &AgentProfile, model: &str, tools: &[Box<dyn Tool>]) -> CompletionRequest`.

> Reuse the P3 assembler (DRY): selection of *what* is relevant is `assemble_context(window, policy)` — do not reimplement pin/evict/cold/ceiling. `build_subagent_request` only resolves + formats the already-selected **File** items. Session messages/tool transcripts are deliberately excluded — this is the superpowers invariant.

- [ ] **Step 1: Write the failing test** — `crates/zoid/src/subagent.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::agent_profile::AgentProfile;
    use zoid_core::assembler::ContextPolicy;
    use zoid_core::event::{Event, EventKind};
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event { Event::new(Ulid::new(), None, 0, kind) }
    fn call(id: &str, path: &str) -> Event {
        ev(EventKind::ToolCall { id: id.into(), name: "read_file".into(), args: format!(r#"{{"path":"{path}"}}"#) })
    }
    fn result(id: &str, out: &str) -> Event {
        ev(EventKind::ToolResult { id: id.into(), name: "read_file".into(), output: out.into(), is_error: false })
    }

    #[test]
    fn request_carries_task_and_relevant_file_never_history() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "secret chat history".into() }),
            call("c1", "src/ast.rs"),
            result("c1", "fn parse() {}"),
        ];
        let profile = AgentProfile::builtin();
        let tools = zoid_tools::registry();
        let req = build_subagent_request("refactor parse()", &evs, &subagent_policy(), &profile, "glm", &tools);

        assert_eq!(req.model, "glm");
        assert_eq!(req.system.as_deref(), Some(profile.system_prompt.as_str()));
        assert_eq!(req.messages.len(), 1, "subagent gets ONE constructed user message");
        let body = &req.messages[0].content;
        assert!(body.contains("refactor parse()"), "task present");
        assert!(body.contains("fn parse() {}"), "relevant file content present");
        assert!(body.contains("src/ast.rs"), "file labeled by path");
        // THE SUPERPOWERS INVARIANT: never the session transcript.
        assert!(!body.contains("secret chat history"), "session history excluded (spec §4.4/§5.4)");
        assert!(!req.tools.is_empty(), "tools advertised");
    }

    #[test]
    fn request_without_files_is_just_the_task() {
        let req = build_subagent_request(
            "do a thing", &[], &subagent_policy(), &AgentProfile::builtin(), "glm", &zoid_tools::registry());
        assert!(req.messages[0].content.contains("do a thing"));
    }

    #[test]
    fn subagent_policy_is_bounded_and_evicts_cold() {
        let p = subagent_policy();
        assert!(p.auto_evict_cold, "cold items dropped from a subagent's context");
        assert!(p.token_ceiling.is_some(), "subagent context is token-bounded");
    }
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p zoid subagent`
  Expected: compile error — module/fns undefined.

- [ ] **Step 3: Implement** — `crates/zoid/src/subagent.rs` (above the test module):

```rust
//! The subagent runtime (spec §4.4/§7). Builds a subagent's constructed context
//! (task + relevant code, NEVER session history) and runs it in isolation. The
//! orchestrator (the Chat loop) dispatches one at a time.

use zoid_core::agent_profile::AgentProfile;
use zoid_core::assembler::{assemble_context, ContextPolicy};
use zoid_core::context::{context_window, file_contents, ItemKind};
use zoid_core::event::Event;
use zoid_provider::{CompletionRequest, Message};
use zoid_tools::Tool;

use crate::agent::tool_specs;

/// Per-subagent max output tokens (mirrors the Chat loop's budget).
const SUBAGENT_MAX_TOKENS: u32 = 4096;

/// Token ceiling for a subagent's constructed context (≈ half a 64k window,
/// leaving room for the task, tool round-trips, and output).
const SUBAGENT_CONTEXT_CEILING: u64 = 32_000;

/// Default context budget for a dispatched subagent: drop cold items and cap the
/// constructed context so it stays a *precise* slice, not a dump.
pub fn subagent_policy() -> ContextPolicy {
    ContextPolicy {
        token_ceiling: Some(SUBAGENT_CONTEXT_CEILING),
        auto_evict_cold: true,
        compact_threshold: None,
    }
}

/// Build a subagent `CompletionRequest`: the P3 assembler selects the relevant
/// context items from `events`; we keep the included **File** items, resolve
/// their content, and compose a task-focused prompt. Session messages/tool
/// transcripts are intentionally excluded (spec §4.4/§5.4: never session history).
pub fn build_subagent_request(
    task: &str,
    events: &[Event],
    policy: &ContextPolicy,
    profile: &AgentProfile,
    model: &str,
    tools: &[Box<dyn Tool>],
) -> CompletionRequest {
    let window = context_window(events);
    let selection = assemble_context(&window, policy);
    let contents = file_contents(events);

    let mut ctx = String::new();
    for item in selection.included.iter().filter(|i| i.kind == ItemKind::File) {
        if let Some(c) = contents.get(&item.key) {
            ctx.push_str(&format!("\n// {}\n{}\n", item.label, c));
        }
    }

    let user = if ctx.is_empty() {
        format!("Task:\n{task}")
    } else {
        format!("Task:\n{task}\n\nRelevant files:\n{ctx}")
    };

    CompletionRequest {
        model: model.to_string(),
        system: Some(profile.system_prompt.clone()),
        messages: vec![Message::user(user)],
        max_tokens: SUBAGENT_MAX_TOKENS,
        tools: tool_specs(tools),
    }
}
```

In `crates/zoid/src/lib.rs`, add `pub mod subagent;`.

- [ ] **Step 4: Run to confirm pass** — `cargo test -p zoid subagent`
  Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/subagent.rs crates/zoid/src/lib.rs
git commit -m "feat(zoid): build_subagent_request — assembler → constructed context (task + code, never history)

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task B4: Generalize the agent loop (`TurnConfig`) + `run_subagent`

**Files:** Modify `crates/zoid/src/agent.rs`, `crates/zoid/src/main.rs`, `crates/zoid/tests/{agent_loop,economy_integration}.rs`; Modify `crates/zoid/src/subagent.rs`. Test: inline (agent + subagent) + existing tests adopt the new signature.

**Interfaces:**
- Produces (agent.rs):
  - `pub struct TurnConfig { pub system: String, pub cwd: std::path::PathBuf, pub branch: zoid_core::event::BranchId }`.
  - `pub fn chat_turn_config() -> TurnConfig` — `{ SYSTEM_PROMPT, ".", BranchId::default() }`.
  - `pub fn build_request(events, model, tools, system: &str) -> CompletionRequest` (gains `system`).
  - `pub async fn run_agent_turn(config: TurnConfig, provider, tools, session, events, model, ui, now) -> Result<Vec<Event>>` (returns accumulated events).
- Produces (subagent.rs):
  - `pub struct SubagentResult { pub branch: String, pub summary: String, pub ok: bool }`.
  - `pub async fn run_subagent(task, context_events, profile, provider, cwd, default_model, session, ui, now) -> Result<SubagentResult>`.

> Generalize, don't fork (DRY): ONE agent loop serves both Chat (main branch, cwd `"."`) and subagents (own branch, own cwd). A subagent is that loop seeded with the B3 constructed prompt as a single `UserMessage` on `BranchId("subagent:<id>")`.

- [ ] **Step 1: Write the failing agent test** — in `crates/zoid/src/agent.rs`, add a `mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::event::BranchId;

    #[test]
    fn chat_turn_config_is_main_branch_cwd_dot() {
        let c = chat_turn_config();
        assert_eq!(c.branch, BranchId::default());
        assert_eq!(c.cwd, std::path::PathBuf::from("."));
        assert_eq!(c.system, SYSTEM_PROMPT);
    }

    #[test]
    fn build_request_uses_the_given_system_prompt() {
        let req = build_request(&[], "m", &zoid_tools::registry(), "CUSTOM SYS");
        assert_eq!(req.system.as_deref(), Some("CUSTOM SYS"));
    }
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p zoid agent::tests`
  Expected: FAIL — `TurnConfig`/`chat_turn_config` undefined; `build_request` arity.

- [ ] **Step 3: Generalize `agent.rs`** — add near `SYSTEM_PROMPT`:

```rust
use std::path::PathBuf;
use zoid_core::event::BranchId;

/// How one agent turn is run: its system prompt, working directory, and the
/// event branch its output is recorded on. Chat uses the main branch + process
/// cwd; a subagent uses its own branch + (optionally) a worktree.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub system: String,
    pub cwd: PathBuf,
    pub branch: BranchId,
}

/// The orchestrator (Chat) turn config: main branch, process cwd, Chat prompt.
pub fn chat_turn_config() -> TurnConfig {
    TurnConfig { system: SYSTEM_PROMPT.to_string(), cwd: PathBuf::from("."), branch: BranchId::default() }
}
```

Change `build_request` to accept the system prompt:

```rust
pub fn build_request(events: &[Event], model: &str, tools: &[Box<dyn Tool>], system: &str) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        system: Some(system.to_string()),
        messages: conversation(events).into_iter().map(map_msg).collect(),
        max_tokens: 4096,
        tools: tool_specs(tools),
    }
}
```

Change `run_agent_turn` + `run_turn_inner` to take `config: TurnConfig` / `&TurnConfig` (as the FIRST param) and return `Result<Vec<Event>>`:

```rust
pub async fn run_agent_turn(
    config: TurnConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    session: SessionHandle,
    events: Vec<Event>,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
) -> Result<Vec<Event>> {
    let result = run_turn_inner(&config, provider, tools, session, events, model, &ui, now).await;
    let _ = ui.send(AgentUpdate::TurnComplete).await;
    result
}
```

In `run_turn_inner` (now `config: &TurnConfig`, `mut events`, return `Result<Vec<Event>>`):
- `let req = build_request(&events, &model, &tools, &config.system);`
- before the `spawn_blocking`, capture `let cwd_for_exec = config.cwd.clone();` and call `zoid_tools::run_tool(&tools_for_exec, &name, &args, &cwd_for_exec)`.
- pass `&config.branch` to every `emit(...)` / `emit_with_tokens(...)` call.
- replace the final `Ok(())` with `Ok(events)` (every `break 'turn` path falls through to it).

Thread the branch through `emit_with_tokens` (and `emit` forwards it):

```rust
async fn emit_with_tokens(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    tokens: Option<zoid_core::event::TokenStat>,
    now: fn() -> i64,
) -> Result<()> {
    let mut ev = Event::new(Ulid::new(), None, now(), kind);
    ev.branch = branch.clone();
    ev.tokens = tokens;
    session.append(ev.clone()).await?;
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(ev)).await;
    Ok(())
}

async fn emit(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    now: fn() -> i64,
) -> Result<()> {
    emit_with_tokens(session, events, ui, branch, kind, None, now).await
}
```

> PLAN-2-SEAM: when Plan 2 adds `session_id` to `Event`, set it here too (`ev.session_id = …`) so subagent events are scoped to the active session. Grounded today: `Event::new` takes `(id, parent, ts, kind)`; the session DB is per-session, so events are already session-scoped by the store.

- [ ] **Step 4: Update the Chat caller (`main.rs`)** — `spawn_turn` passes the config and ignores the returned events:

```rust
tokio::spawn(async move {
    let _ = run_agent_turn(zoid::agent::chat_turn_config(), provider, tools, session, seed, model, ui, now_ms).await;
});
```

- [ ] **Step 5: Update existing bin tests** — in `crates/zoid/tests/agent_loop.rs` and `crates/zoid/tests/economy_integration.rs`, each `run_agent_turn(provider, tools, session, seed, model, tx, now)` call gains `zoid::agent::chat_turn_config()` as the first argument; the returned value is now `Result<Vec<Event>>` (bind or `let _ = … .await.unwrap();`). Assertions are unchanged (Chat still uses main branch + cwd `"."`).

- [ ] **Step 6: Write the failing `run_subagent` test** — in `crates/zoid/src/subagent.rs` `mod tests`:

```rust
#[tokio::test]
async fn subagent_runs_constructed_task_and_returns_summary() {
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use zoid_core::session::SessionHandle;
    use zoid_provider::{FakeProvider, ProviderEvent, Usage};

    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("Refactored parse() into two functions.".into()),
        ProviderEvent::Usage(Usage { input_tokens: 200, output_tokens: 30 }),
        ProviderEvent::Done,
    ]));
    let session = SessionHandle::spawn(":memory:").unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let res = run_subagent(
        "refactor parse()",
        &[],
        &AgentProfile::builtin(),
        provider,
        std::path::PathBuf::from("."),
        "glm".into(),
        session.clone(),
        tx,
        || 0,
    )
    .await
    .unwrap();

    assert!(res.ok, "no error emitted → ok");
    assert!(res.summary.contains("Refactored parse()"), "summary = subagent's final text");
    assert!(res.branch.starts_with("subagent:"));
    // The subagent's work is persisted on ITS OWN branch.
    let snap = session.snapshot().await.unwrap();
    assert!(snap.iter().any(|e| e.branch.0 == res.branch));
}
```

- [ ] **Step 7: Run to confirm failure** — `cargo test -p zoid subagent::tests::subagent_runs`
  Expected: FAIL — `run_subagent`/`SubagentResult` undefined.

- [ ] **Step 8: Implement `run_subagent`** — add imports + code to `crates/zoid/src/subagent.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use ulid::Ulid;

use zoid_core::event::{BranchId, Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::Provider;

use crate::agent::{run_agent_turn, AgentUpdate, TurnConfig};

/// ⚠ marks an agent-loop error message (mirrors `agent::WARN_GLYPH`); a summary
/// starting with it means the subagent failed.
const WARN_GLYPH: char = '⚠';

/// The outcome of a dispatched subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentResult {
    pub branch: String,
    pub summary: String,
    pub ok: bool,
}

/// Run `task` as an isolated subagent: build its constructed context (B3), seed
/// it as the first user message on a fresh `subagent:<id>` branch, run the
/// generalized agent loop in `cwd` under `profile`, and distill a
/// `SubagentResult`. Sequential — the caller dispatches one at a time.
#[allow(clippy::too_many_arguments)]
pub async fn run_subagent(
    task: &str,
    context_events: &[Event],
    profile: &AgentProfile,
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
    default_model: String,
    session: SessionHandle,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
) -> Result<SubagentResult> {
    let branch = BranchId(format!("subagent:{}", Ulid::new()));
    let model = profile.model.clone().unwrap_or(default_model);

    // Only the tools this profile allows (fresh registry, filtered by allow-list).
    let tools: Arc<Vec<Box<dyn Tool>>> =
        Arc::new(zoid_tools::registry().into_iter().filter(|t| profile.allows(t.name())).collect());

    // The constructed prompt (task + relevant code) becomes the seed user turn.
    let req = build_subagent_request(task, context_events, &subagent_policy(), profile, &model, &tools);
    let prompt = req.messages[0].content.clone();
    let mut seed = Event::new(Ulid::new(), None, now(), EventKind::UserMessage { text: prompt });
    seed.branch = branch.clone();
    session.append(seed.clone()).await?;

    let config = TurnConfig { system: profile.system_prompt.clone(), cwd, branch: branch.clone() };
    let produced = run_agent_turn(config, provider, tools, session, vec![seed], model, ui, now).await?;

    // Distill: last non-empty assistant text = summary; an emitted ⚠ = not ok.
    let summary = conversation(&produced)
        .iter()
        .rev()
        .find_map(|m| match m {
            ChatMsg::Assistant { text, .. } if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let ok = !summary.starts_with(WARN_GLYPH);

    Ok(SubagentResult { branch: branch.0, summary, ok })
}
```

- [ ] **Step 9: Run to confirm pass** — `cargo test -p zoid && cargo clippy -p zoid --all-targets`
  Expected: PASS (agent + subagent + existing tests), zero warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs crates/zoid/src/subagent.rs crates/zoid/tests/
git commit -m "feat(zoid): generalize agent loop (TurnConfig); run_subagent → isolated branch/cwd + SubagentResult

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

## C. `git2` worktree isolation (core §3, §4.4)

Spec: a Chat-delegated non-trivial unit "runs in an isolated git worktree" (chat §6); `git2` is "justified from Chat's P5 delegation onward" (core §3). Create a temporary worktree for the unit; the subagent's tools execute THERE; clean up on drop; a subagent failure is isolated (never corrupts main) and recorded as an event (the `DelegationResult { ok:false }` — work item D). Integration tests use real temp git repos + worktrees (`git2`), per core §8.

### Task C1: `git2` worktree module (create + auto-cleanup)

**Files:** Modify `Cargo.toml` + `crates/zoid/Cargo.toml`; Create `crates/zoid/src/worktree.rs` + `crates/zoid/tests/worktree_test.rs`; Modify `crates/zoid/src/lib.rs`.

**Interfaces:**
- Consumes: `git2`.
- Produces:
  - `pub struct WorktreeGuard` with `pub fn path(&self) -> &Path`; removed on `Drop`.
  - `pub fn create_worktree(repo_root: &Path, name: &str) -> anyhow::Result<WorktreeGuard>` — worktree at `repo_root/.zoid/worktrees/<name>`, branched from HEAD.

- [ ] **Step 1: Add the dependency** — top-level `Cargo.toml` `[workspace.dependencies]`: `git2 = "0.19"`. `crates/zoid/Cargo.toml` `[dependencies]`: `git2 = { workspace = true }`.

> If the build can't find a system libgit2, use `git2 = { workspace = true, features = ["vendored-libgit2"] }` in the bin manifest (heavier build, self-contained). `git2` stays a `zoid`-bin dep only — never `zoid-core` (verify with grep in Final Verification).

- [ ] **Step 2: Write the failing test** — `crates/zoid/tests/worktree_test.rs`:

```rust
use std::path::Path;
use zoid::worktree::create_worktree;

/// Init a git repo at `dir` with one committed file (worktrees need a HEAD).
fn init_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).unwrap();
    std::fs::write(dir.join("a.txt"), "hi").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("zoid", "zoid@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
}

#[test]
fn worktree_is_a_working_copy_and_cleans_up_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    let path;
    {
        let wt = create_worktree(tmp.path(), "sub-ax3").unwrap();
        path = wt.path().to_path_buf();
        assert!(path.exists(), "worktree dir should exist");
        assert!(path.join("a.txt").exists(), "HEAD content should be checked out");
    } // WorktreeGuard dropped here

    assert!(!path.exists(), "worktree dir removed on drop");
}
```

- [ ] **Step 3: Run to confirm failure** — `cargo test -p zoid --test worktree_test`
  Expected: compile error — `zoid::worktree` does not exist.

- [ ] **Step 4: Implement** — `crates/zoid/src/worktree.rs`:

```rust
//! Isolated git worktrees for subagent execution (spec §3/§4.4). A dispatched
//! Chat subagent runs in its own worktree so its file edits are isolated from
//! the main working copy until judged. A `WorktreeGuard` removes its worktree on
//! drop, so a panicking or abandoned subagent never leaks a registration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{Repository, WorktreeAddOptions, WorktreePruneOptions};

/// An isolated worktree. Dropping it removes the working directory and prunes the
/// git registration (best-effort).
pub struct WorktreeGuard {
    name: String,
    path: PathBuf,
    repo_root: PathBuf,
}

impl WorktreeGuard {
    /// The worktree's checked-out directory — a subagent's `cwd`.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Create a worktree named `name` for the repo at `repo_root`, checked out at
/// `repo_root/.zoid/worktrees/<name>` and branched from HEAD.
pub fn create_worktree(repo_root: &Path, name: &str) -> Result<WorktreeGuard> {
    let repo = Repository::open(repo_root).context("open repo for worktree")?;
    let path = repo_root.join(".zoid").join("worktrees").join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create worktree parent dir")?;
    }
    let opts = WorktreeAddOptions::new();
    repo.worktree(name, &path, Some(&opts)).with_context(|| format!("add worktree {name}"))?;
    Ok(WorktreeGuard { name: name.to_string(), path, repo_root: repo_root.to_path_buf() })
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        // Remove the working dir first, then prune the registration. Best-effort:
        // Drop can't surface errors, and a leaked worktree is recoverable.
        let _ = std::fs::remove_dir_all(&self.path);
        if let Ok(repo) = Repository::open(&self.repo_root) {
            if let Ok(wt) = repo.find_worktree(&self.name) {
                let mut po = WorktreePruneOptions::new();
                po.valid(true).working_tree(true);
                let _ = wt.prune(Some(&mut po));
            }
        }
    }
}
```

In `crates/zoid/src/lib.rs`, add `pub mod worktree;`.

> If a `git2` API name differs in the resolved version (e.g. `WorktreePruneOptions::working_tree` ↔ `locked`), run `cargo doc -p git2 --open` and adapt — the shape (open repo → `worktree(name, path, opts)` → `find_worktree`/`prune`) is stable across recent versions.

- [ ] **Step 5: Run to confirm pass** — `cargo test -p zoid --test worktree_test`
  Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/zoid/Cargo.toml crates/zoid/src/worktree.rs crates/zoid/src/lib.rs crates/zoid/tests/worktree_test.rs
git commit -m "feat(zoid): git2 worktree module — create + auto-cleanup for subagent isolation

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task C2: Subagent runs in its worktree — isolation + cleanup integration test

**Files:** Create `crates/zoid/tests/subagent_integration.rs`.

**Interfaces:** Consumes `create_worktree`, `run_subagent`, `AgentProfile::builtin`, `FakeProvider` (scripted tool call).

> Proves the B + C chain: a subagent receives a constructed task, calls a tool **inside its worktree** (B1 cwd seam + C1 worktree), the main working copy is untouched, and the worktree is cleaned up.

- [ ] **Step 1: Write the failing integration test** — `crates/zoid/tests/subagent_integration.rs`:

```rust
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use zoid::subagent::run_subagent;
use zoid::worktree::create_worktree;
use zoid_core::agent_profile::AgentProfile;
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent, ToolCall};

fn init_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).unwrap();
    std::fs::write(dir.join("seed.txt"), "seed").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("seed.txt")).unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("zoid", "zoid@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
}

#[tokio::test]
async fn subagent_writes_inside_its_worktree_not_the_main_copy() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());

    // Scripted: turn 1 writes a file; turn 2 (after the tool result) summarizes.
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::ToolCall(ToolCall {
            id: "w1".into(),
            name: "write_file".into(),
            args: serde_json::json!({ "path": "out.txt", "content": "made by subagent" }),
        }),
        ProviderEvent::Done,
        ProviderEvent::TextDelta("Wrote out.txt.".into()),
        ProviderEvent::Done,
    ]));

    let session = SessionHandle::spawn(":memory:").unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let wt_path;
    {
        let wt = create_worktree(tmp.path(), "sub-int").unwrap();
        wt_path = wt.path().to_path_buf();
        let res = run_subagent(
            "create out.txt",
            &[],
            &AgentProfile::builtin(),
            provider,
            wt.path().to_path_buf(),   // subagent cwd = the worktree (B1 seam)
            "glm".into(),
            session,
            tx,
            || 0,
        )
        .await
        .unwrap();
        assert!(res.ok);
        // The write landed INSIDE the worktree.
        assert_eq!(std::fs::read_to_string(wt.path().join("out.txt")).unwrap(), "made by subagent");
    } // worktree dropped → cleaned up

    // Isolation: the main working copy never saw the subagent's file.
    assert!(!tmp.path().join("out.txt").exists(), "main copy untouched");
    // Cleanup: the worktree directory is gone.
    assert!(!wt_path.exists(), "worktree removed on drop");
}
```

> If `FakeProvider` does not replay batched events across successive `stream()` calls, adapt the script to its contract (the P3 `economy_integration.rs` test is a working reference). The load-bearing assertions are: the file lands in the worktree, NOT the main copy, and the worktree is removed on drop.

- [ ] **Step 2: Run to confirm pass** — `cargo test -p zoid --test subagent_integration`
  Expected: PASS (adjust the fake script to its replay contract if needed).

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/tests/subagent_integration.rs
git commit -m "test(zoid): subagent writes in its worktree; main copy isolated + cleaned up

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

## D. Wire dispatch from Chat + collapsible result card (§6)

Turn the queued object-verb / explicit delegate path (`main.rs` ~344-358, currently just seeds the input with "queued · runs as a subagent in P5") into a real dispatch: construct context (B) → run the subagent in a worktree (C) → fold its result back as a **collapsible result card** (① semantic zoom), inline with the requesting turn. **Trivial edits stay inline** (the ordinary chat turn path is unchanged — no worktree, no card; only the explicit `:delegate`/verb path delegates).

### Task D1: `DelegationResult` event + branch-folding `conversation()`

**Files:** Modify `crates/zoid-core/src/event.rs`, `crates/zoid-core/src/projection.rs`, `crates/zoid/src/agent.rs` (`map_msg` arm). Test: inline.

**Interfaces:**
- Produces:
  - `EventKind::DelegationResult { branch: String, summary: String, ok: bool }` (recorded on the MAIN branch when a subagent finishes).
  - `ChatMsg::Delegated { summary: String, ok: bool }`.
  - `conversation()` skips non-main-branch events and folds `DelegationResult` → `ChatMsg::Delegated`.

- [ ] **Step 1: Write the failing tests** — in `crates/zoid-core/src/projection.rs` `mod tests`:

```rust
#[test]
fn conversation_skips_subagent_branch_and_folds_result() {
    use crate::event::BranchId;
    let mut work = Event::new(Ulid::from(10u128), None, 0, EventKind::ModelDelta { text: "subagent thinking".into() });
    work.branch = BranchId("subagent:ax3".into());
    let result = Event::new(Ulid::from(11u128), None, 0, EventKind::DelegationResult {
        branch: "subagent:ax3".into(), summary: "Refactored parse()".into(), ok: true,
    });
    let evs = vec![user(1, "delegate this"), work, result];
    let conv = conversation(&evs);
    assert_eq!(conv, vec![
        ChatMsg::User { text: "delegate this".into(), ts: 0 },
        ChatMsg::Delegated { summary: "Refactored parse()".into(), ok: true },
    ]);
}
```

In `crates/zoid-core/src/event.rs` `mod tests`:

```rust
#[test]
fn delegation_result_round_trips() {
    let ev = Event::new(Ulid::new(), None, 0, EventKind::DelegationResult {
        branch: "subagent:zz".into(), summary: "did it".into(), ok: false,
    });
    let json = serde_json::to_string(&ev).unwrap();
    assert_eq!(ev, serde_json::from_str::<Event>(&json).unwrap());
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p zoid-core`
  Expected: compile error — `DelegationResult`/`Delegated` undefined.

- [ ] **Step 3: Add the variants** — in `crates/zoid-core/src/event.rs`, add to `EventKind` (before the closing brace):

```rust
    /// A finished subagent's outcome, recorded on the MAIN branch. `branch`
    /// names the subagent's sub-branch; `summary` is its closing report.
    DelegationResult { branch: String, summary: String, ok: bool },
```

In `crates/zoid-core/src/projection.rs`, add to `enum ChatMsg`:

```rust
    /// A folded subagent delegation — rendered as a collapsible card (① zoom).
    Delegated { summary: String, ok: bool },
```

- [ ] **Step 4: Branch-filter + fold in `conversation()`** — in `crates/zoid-core/src/projection.rs`, at the very top of the `for e in events` loop, skip non-main work (but NOT `DelegationResult`, which is itself on main):

```rust
    for e in events {
        // Subagent work lives on its own branch and never appears in the main
        // conversation; only its folded DelegationResult (on main) surfaces.
        if e.branch != crate::event::BranchId::default() {
            continue;
        }
        match &e.kind {
            // ... existing arms unchanged ...
            EventKind::DelegationResult { summary, ok, .. } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                out.push(ChatMsg::Delegated { summary: summary.clone(), ok: *ok });
            }
            EventKind::Usage | EventKind::ContextMutation { .. } => {
                // Economy bookkeeping; not part of the conversation projection.
            }
        }
    }
```

- [ ] **Step 5: Add the `map_msg` arm in the bin** — in `crates/zoid/src/agent.rs`, `map_msg` matches `ChatMsg`. Add:

```rust
        ChatMsg::Delegated { summary, .. } => Message {
            role: zoid_provider::MsgRole::Assistant,
            content: format!("[delegated subagent] {summary}"),
            tool_calls: vec![],
            tool_name: None,
        },
```

(So a later Chat turn sees the delegation outcome in context.)

- [ ] **Step 6: Run to confirm pass** — `cargo test -p zoid-core && cargo build -p zoid`
  Expected: `zoid-core` PASS; `zoid` compiles once the remaining `ChatMsg` arms land in D2 (this step may still flag non-exhaustive matches in `zoid-tui` — expected, fixed in D2).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/projection.rs crates/zoid/src/agent.rs
git commit -m "feat(core): DelegationResult event; conversation() folds Delegated, skips sub-branches

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task D2: Render the delegated card (respecting altitude) + tokens + snapshots

**Files:** Modify `crates/zoid-tui/src/tokens.rs`, `crates/zoid-tui/src/chat.rs`, `crates/zoid-core/src/zoom.rs`, `crates/zoid-tui/tests/shell_snapshot.rs`. Test: token test + snapshots @100/@140.

**Interfaces:**
- Consumes: `ChatMsg::Delegated`.
- Produces: a `▸ delegated · {summary}  {✓|⚠}  ⏎ peek` card line (collapsed at Normal/Summary), an expanded `▾ delegated` body at Detail; new `color::DELEGATE_BG`; exhaustive `ChatMsg` matches updated.

- [ ] **Step 1: Add the card background token (failing test)** — in `crates/zoid-tui/src/tokens.rs` `mod tests`:

```rust
#[test]
fn p5_delegate_token_present() {
    use ratatui::style::Color;
    // Card background from docs/ux/chat-mode.html `.chip` (#15101f).
    assert_eq!(color::DELEGATE_BG, Color::Rgb(0x15, 0x10, 0x1f));
}
```

Run `cargo test -p zoid-tui tokens::` → FAIL (undefined). Then add to `mod color`:

```rust
    pub const DELEGATE_BG: Color = Color::Rgb(0x15, 0x10, 0x1f); // ▸ delegated card bg (chat-mode.html .chip)
```

(The card BORDER reuses `color::BRANCH` = `#bc8cff`, matching the mock's `--br`; status reuses `color::OK`/`color::ERROR`; chevrons `glyph::COLLAPSED`/`EXPANDED`; peek `glyph::RETURN`.)

- [ ] **Step 2: Write the failing render test** — in `crates/zoid-tui/src/chat.rs` `mod tests`:

```rust
#[test]
fn delegated_card_renders_chevron_status_and_bg() {
    use crate::tokens::{color, glyph};
    let msgs = vec![ChatMsg::Delegated { summary: "Added shared NotFound helper.".into(), ok: true }];
    let lines = conversation_lines(&msgs, false, true, 0);
    let joined: String = lines.iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string())).collect();
    assert!(joined.contains(glyph::COLLAPSED), "collapsed chevron ▸ present");
    assert!(joined.contains("delegated"));
    assert!(joined.contains(glyph::PASS), "done status ✓ present");
    // The card label carries the delegate background (proves §16 token use).
    assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.style.bg == Some(color::DELEGATE_BG))));
}
```

Run `cargo test -p zoid-tui chat::tests::delegated_card` → FAIL (non-exhaustive match / assertions).

- [ ] **Step 3: Render the card in `conversation_lines`** — add a `ChatMsg::Delegated` arm to the `match m` in `conversation_lines`:

```rust
            ChatMsg::Delegated { summary, ok } => {
                let (mark, mark_color) = if *ok { (glyph::PASS, color::OK) } else { (glyph::WARNING, color::ERROR) };
                lines.push(Line::from(vec![
                    // Purple label with the card background = the collapsed chip.
                    Span::styled(
                        format!("{} delegated · {}", glyph::COLLAPSED, first_line(summary)),
                        Style::new().fg(color::BRANCH).bg(color::DELEGATE_BG),
                    ),
                    Span::styled(format!("  {mark} "), Style::new().fg(mark_color)),
                    Span::styled(format!("{} peek", glyph::RETURN), Style::new().fg(color::DIM)),
                ]));
            }
```

Then handle the **Detail** (expanded) altitude in `detail_lines`: its `other =>` arm already routes non-tool-result messages through `conversation_lines`, so `Delegated` renders there too — but to realize ① "expand to inspect", add an explicit arm BEFORE the `other =>` that shows the expanded chevron + full body:

```rust
            ChatMsg::Delegated { summary, ok } => {
                let (mark, mark_color) = if *ok { (glyph::PASS, color::OK) } else { (glyph::WARNING, color::ERROR) };
                out.push(Line::from(vec![
                    Span::styled(format!("{} delegated ", glyph::EXPANDED), Style::new().fg(color::BRANCH).bg(color::DELEGATE_BG)),
                    Span::styled(format!("{mark}"), Style::new().fg(mark_color)),
                ]));
                // PLAN-1-SEAM: route `summary` through Plan 1's markdown renderer
                // (`crate::markdown::render_markdown(summary)`, added in Plan 1) here,
                // pushing its `Vec<Line>` instead of the loop below. Grounded fallback: plain body lines.
                for line in summary.lines() {
                    out.push(Line::from(Span::styled(format!("    {line}"), Style::new().fg(color::TXT))));
                }
            }
```

(This gives the collapsed `▸` chip at Normal/Summary and the expanded `▾` + body at Detail — the ① altitude behavior the constraint requires.)

- [ ] **Step 4: Update the remaining exhaustive `ChatMsg` matches (compile wall)** — `cargo build --workspace` lists each non-exhaustive match. Add:
  - `crates/zoid-core/src/zoom.rs` `digests`: a folded delegation belongs to the current turn, no extra tool/file counts:
    ```rust
                ChatMsg::Delegated { .. } => {
                    // A folded delegation belongs to the current turn; no extra counts.
                    let _ = cur.get_or_insert_with(|| TurnDigest {
                        headline: String::new(), tools: 0, files: 0, has_error: false,
                    });
                }
    ```
  - `crates/zoid-tui/src/objects.rs` `selectable_objects` matches only `Assistant`/`ToolResult` via `if let` — a `Delegated` yields no object; **no change** (confirm it's `if let`, not an exhaustive `match`).

- [ ] **Step 5: Write the snapshot tests** — in `crates/zoid-tui/tests/shell_snapshot.rs` (reuse `empty_economy`/`normal_view` already defined there):

```rust
fn seeded_delegated() -> Vec<ChatMsg> {
    vec![
        ChatMsg::User { text: "extract NotFound handling into a shared helper".into(), ts: 0 },
        ChatMsg::Delegated { summary: "Added shared NotFound helper; get_user reuses it.".into(), ok: true },
    ]
}

fn draw_delegated(w: u16, h: u16) -> String {
    let s = ShellState::new();
    let input = TextArea::default();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_shell(f, &s, &empty_economy(), &seeded_delegated(), &input, false, &normal_view()))
        .unwrap();
    // Buffer Debug per §8 snapshot standard — captures the DELEGATE_BG style.
    format!("{:#?}", terminal.backend().buffer())
}

#[test] fn delegated_card_frame() { insta::assert_snapshot!(draw_delegated(100, 24)); }
#[test] fn delegated_card_wide_frame() { insta::assert_snapshot!(draw_delegated(140, 24)); }
```

- [ ] **Step 6: Accept snapshots + verify** — `cargo build --workspace` (resolve any remaining arms), then `INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot`. Read the two new `.snap` files: a `▸ delegated · Added shared NotFound helper…  ✓  ⏎ peek` line appears, bound to `docs/ux/chat-mode.html`'s chip. Re-run without the env var: `cargo test -p zoid-tui --test shell_snapshot` → PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs crates/zoid-tui/src/chat.rs crates/zoid-core/src/zoom.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(tui): delegated result card (collapsed/expanded by altitude) + DELEGATE_BG token + snapshots

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task D3: Orchestrator — `:delegate` command, worktree dispatch, busy guard, verb rewire

**Files:** Modify `crates/zoid-tui/src/command.rs`, `crates/zoid/src/main.rs`. Test: inline command test + manual smoke.

**Interfaces:**
- Consumes: `run_subagent`/`SubagentResult` (B4), `create_worktree` (C1), `AgentProfile::builtin` (A1), `parse_command`.
- Produces: `Command::Delegate(String)`; a `delegating: bool` guard on `App`; `fn start_delegation(app, task)`.

- [ ] **Step 1: Write the failing command test** — in `crates/zoid-tui/src/command.rs` `mod tests`:

```rust
#[test]
fn parses_delegate_with_task() {
    assert_eq!(parse_command(":delegate add a test for parse()"), Command::Delegate("add a test for parse()".into()));
    assert_eq!(parse_command(":delegate"), Command::Delegate(String::new()));
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p zoid-tui command::tests::parses_delegate`
  Expected: FAIL — `Command::Delegate` undefined.

- [ ] **Step 3: Add the command** — in `crates/zoid-tui/src/command.rs`, add `Delegate(String),` to `enum Command`. In `parse_command`, replace the `other =>` arm with a guard before it:

```rust
        rest if rest == "delegate" || rest.starts_with("delegate ") => {
            Command::Delegate(rest.strip_prefix("delegate").unwrap().trim().to_string())
        }
        other => Command::Unknown(other.to_string()),
```

- [ ] **Step 4: Add the `delegating` guard + dispatch (`main.rs`)** — add `delegating: bool,` to `struct App` (init `false` in `main`). Add:

```rust
/// Dispatch `task` to a single subagent (spec §6). One at a time. Non-trivial:
/// runs in an isolated git worktree (falls back to cwd if not a repo); its
/// DelegationResult folds back as a card. (Trivial edits use the normal inline
/// chat path — this is the explicit delegate path only.)
fn start_delegation(app: &mut App, task: String) {
    if app.streaming || app.delegating {
        app.shell.status_hint = Some("busy · one subagent at a time".into());
        return;
    }
    if task.trim().is_empty() {
        app.shell.status_hint = Some("usage: :delegate <task>".into());
        return;
    }
    app.delegating = true;
    app.shell.status_hint = Some(format!("{} delegating…", zoid_tui::tokens::glyph::RUNNING));

    let provider = app.provider.clone();
    let session = app.session.clone();
    let seed = app.events.clone();       // context for construction (B3)
    let model = app.model.clone();
    let ui = app.ui_tx.clone();
    tokio::spawn(async move {
        // Isolated worktree for the unit (spec §3/§4.4); fall back to cwd if the
        // process is not inside a git repo (e.g. offline smoke).
        let wt = zoid::worktree::create_worktree(
            std::path::Path::new("."),
            &format!("sub-{}", Ulid::new()),
        )
        .ok();
        let cwd = wt.as_ref().map(|w| w.path().to_path_buf()).unwrap_or_else(|| PathBuf::from("."));

        let res = zoid::subagent::run_subagent(
            &task,
            &seed,
            &zoid_core::agent_profile::AgentProfile::builtin(),
            provider,
            cwd,
            model,
            session.clone(),
            ui.clone(),
            now_ms,
        )
        .await;
        // WorktreeGuard `wt` drops here → worktree cleaned up (isolation preserved
        // even on failure — the main copy never saw the subagent's edits).

        let (branch, summary, ok) = match res {
            Ok(r) => (r.branch, r.summary, r.ok),
            Err(e) => (String::new(), format!("delegation failed: {e}"), false),
        };
        // Record the outcome on the MAIN branch so conversation() folds it.
        let ev = Event::new(
            Ulid::new(),
            None,
            now_ms(),
            EventKind::DelegationResult { branch, summary, ok },
        );
        let _ = session.append(ev.clone()).await;
        let _ = ui.send(AgentUpdate::Appended(ev)).await;
    });
}
```

(`PathBuf` is already imported in `main.rs`; ensure `use std::path::PathBuf;` is present — it is, via `use std::path::{Path, PathBuf};`.)

- [ ] **Step 5: Wire the command, clear the guard, rewire the verb pick** — in `exec_command`, add:

```rust
        Command::Delegate(task) => { start_delegation(app, task); Ok(false) }
```

In the main loop's `AgentUpdate::Appended` handler, clear the guard + hint when the result lands:

```rust
                    AgentUpdate::Appended(ev) => {
                        if matches!(ev.kind, EventKind::DelegationResult { .. }) {
                            app.delegating = false;
                            app.shell.status_hint = None;
                        }
                        app.events.push(ev);
                    }
```

Rewire `Action::VerbPick` (currently seeds the input + "queued · runs as a subagent in P5") to dispatch:

```rust
        Action::VerbPick => {
            let objs = zoid_tui::objects::selectable_objects(&conversation(&app.events));
            let osel = zoid_tui::palette::nav(app.shell.objects.obj_selected, 0, objs.len());
            if let Some(obj) = objs.get(osel) {
                let verbs = zoid_tui::objects::verbs_for(obj.kind);
                let vsel = zoid_tui::palette::nav(app.shell.objects.verb_selected, 0, verbs.len());
                if let Some(verb) = verbs.get(vsel) {
                    let task = zoid_tui::objects::verb_prompt(verb, obj);
                    app.shell.close_overlay();
                    start_delegation(app, task); // now dispatches (P5) — closes P4d's "queued"
                    return Ok(false);
                }
            }
            app.shell.close_overlay();
        }
```

- [ ] **Step 6: Build + test** — `cargo test --workspace && cargo clippy --all-targets`
  Expected: PASS, zero warnings.
  Manual smoke (inside a git repo, with `OLLAMA_API_KEY`/`ANTHROPIC_API_KEY` set): `cargo run -p zoid` → `:delegate add a hello function to src/lib.rs` dispatches a subagent; a second `:delegate` while it runs shows "busy · one subagent at a time"; on completion a `▸ delegated ✓ …` card appears.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): :delegate + verb dispatch → one subagent at a time in a worktree; fold result card

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

### Task D4: Integration — delegate dispatches, result folds into the main conversation

**Files:** Create `crates/zoid/tests/delegation_integration.rs`.

**Interfaces:** Consumes `run_subagent`, `conversation`, `FakeProvider`.

> End-to-end at the projection level: a subagent runs on its sub-branch, a `DelegationResult` is recorded on main, and `conversation()` of the full log shows exactly the user turn + the folded `Delegated` card (subagent work hidden).

- [ ] **Step 1: Write the failing test** — `crates/zoid/tests/delegation_integration.rs`:

```rust
use std::sync::Arc;
use tokio::sync::mpsc;
use ulid::Ulid;
use zoid::subagent::run_subagent;
use zoid_core::agent_profile::AgentProfile;
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent};

#[tokio::test]
async fn delegated_result_folds_into_main_conversation() {
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("Added the function.".into()),
        ProviderEvent::Done,
    ]));
    let session = SessionHandle::spawn(":memory:").unwrap();

    // Seed the user turn on main (the request that triggered delegation).
    session.append(Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "delegate: add fn".into() }))
        .await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let seed = session.snapshot().await.unwrap();
    let res = run_subagent("add fn", &seed, &AgentProfile::builtin(), provider,
        std::path::PathBuf::from("."), "glm".into(), session.clone(), tx, || 0).await.unwrap();

    // Orchestrator records the result on main.
    session.append(Event::new(Ulid::new(), None, 0, EventKind::DelegationResult {
        branch: res.branch, summary: res.summary, ok: res.ok,
    })).await.unwrap();

    let conv = conversation(&session.snapshot().await.unwrap());
    assert_eq!(conv.first(), Some(&ChatMsg::User { text: "delegate: add fn".into(), ts: 0 }));
    assert!(matches!(conv.last(), Some(ChatMsg::Delegated { ok: true, .. })));
    // Subagent work events exist in the log but are NOT in the main conversation.
    assert!(!conv.iter().any(|m| matches!(m, ChatMsg::Assistant { text, .. } if text == "Added the function.")));
}
```

- [ ] **Step 2: Run to confirm pass** — `cargo test -p zoid --test delegation_integration`
  Expected: PASS (adapt the `FakeProvider` script to its replay contract if needed).

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/tests/delegation_integration.rs
git commit -m "test(zoid): delegated DelegationResult folds into the main conversation; sub-branch hidden

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

## E. Ledger + session integration

Spec §6: "a Chat delegation's tokens land in the same `TokenLedger` as everything else, within the current session." Because a subagent runs through the generalized loop, its `EventKind::Usage` events are appended to the **same session** as the orchestrator, so `token_ledger(session.snapshot())` includes the subagent's spend.

### Task E1: Delegation spend appears in the session ledger

**Files:** Add to `crates/zoid/tests/delegation_integration.rs` (or a sibling test). Test: the test itself.

**Interfaces:** Consumes `run_subagent`, `zoid_core::economy::token_ledger`, `FakeProvider` emitting `Usage`.

- [ ] **Step 1: Write the failing test** — append to `crates/zoid/tests/delegation_integration.rs`:

```rust
#[tokio::test]
async fn delegation_spend_lands_in_the_session_ledger() {
    use zoid_core::economy::token_ledger;
    use zoid_provider::Usage;

    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("done".into()),
        ProviderEvent::Usage(Usage { input_tokens: 320, output_tokens: 45 }),
        ProviderEvent::Done,
    ]));
    let session = SessionHandle::spawn(":memory:").unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let _res = run_subagent("do the unit", &[], &AgentProfile::builtin(), provider,
        std::path::PathBuf::from("."), "glm".into(), session.clone(), tx, || 0).await.unwrap();

    // The subagent's Usage is in the SAME session log → the ledger reflects it.
    let ledger = token_ledger(&session.snapshot().await.unwrap());
    assert_eq!(ledger.input, 320);
    assert_eq!(ledger.output, 45);
    assert_eq!(ledger.total, 365);
}
```

> PLAN-2-SEAM: with Plan 2's `session_id` on `Event`, scope the ledger to the active session (`token_ledger` filtered by `session_id`). Grounded today: one session per DB, so the whole snapshot IS the active session — `token_ledger(&snapshot)` is already session-scoped.

- [ ] **Step 2: Run to confirm pass** — `cargo test -p zoid --test delegation_integration`
  Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/tests/delegation_integration.rs
git commit -m "test(zoid): a delegation's token spend lands in the active session's ledger

Claude-Session: https://claude.ai/code/session_01JdLxmJhT6KA3k4tCuVs3DY"
```

---

## Final verification (before the whole-branch review)

- [ ] `cargo test --workspace` green; `cargo clippy --all-targets` zero warnings.
- [ ] **AgentProfile:** `AgentProfile::builtin()` constructs; `allows()` reflects the allow-list; empty allow-list permits all. (A1)
- [ ] **Superpowers invariant:** `build_subagent_request` output contains the task + relevant file content and **excludes** session messages/tool transcripts (grep the `request_carries_task_and_relevant_file_never_history` assertion). (B3)
- [ ] **One agent loop:** `run_agent_turn(TurnConfig, …) -> Result<Vec<Event>>` serves both Chat and subagents (no duplicated loop); Chat events stay on `"main"`, subagent events on `subagent:<id>`. (B4)
- [ ] **git2 worktree:** `git2` is a `zoid`-bin dep only (`grep -R "git2" crates/zoid-core` returns nothing); a subagent writes inside its worktree, the main copy is untouched, and the worktree is removed on drop. (C1/C2)
- [ ] **Dispatch + card:** `:delegate <task>` and an object-verb both dispatch one subagent; a second dispatch while busy is refused with a hint; `conversation()` shows the user turn + a `▸ delegated` card; subagent work never appears. (D)
- [ ] **Altitude:** the card is collapsed (`▸`) at Normal/Summary and expanded (`▾` + body) at Detail (① semantic zoom). (D2)
- [ ] **Snapshots:** delegated-card `insta` snapshots exist at 100 and 140 (Buffer Debug), bound to `docs/ux/chat-mode.html`. (D2)
- [ ] **Tokens (§16):** no literal glyphs/hex added outside `tokens.rs` (the card uses `glyph::COLLAPSED/EXPANDED/PASS/WARNING/RETURN`, `color::BRANCH`, new `color::DELEGATE_BG`). (D2)
- [ ] **Ledger/session:** a delegation's `Usage` spend appears in `token_ledger(session.snapshot())`. (E1)
- [ ] No `Co-Authored-By` trailer on any commit; every commit ends with the `Claude-Session:` line.

## Out of scope / next

- **Mode seam** (open `ModeId` + `trait Mode` + `ModeRegistry`, refactoring Chat behind it) is a SEPARATE follow-on landing after P5 and before Build — not in this plan.
- **Build's automated loop** (many units in sequence, a scheduler, the TDD→spec-review→quality-review→fix pipeline, differentiated implementer→reviewer→fix profiles) reuses this identical runtime — Build spec, not here.
- **Parallel fleet / async background delegates** (③, ⑧c) — deferred.
- **`.claude/agents` file loader + named profile registry** — POST-V1 (this plan ships ONE built-in `AgentProfile`).
- **`DelegationStarted` in-flight card** — this plan shows the in-flight state as a status-bar hint; a folded running card is a nicety for later.

## Self-review notes (author)

- **Spec coverage:** §5.4 constructed-context assembler → B2/B3 (`file_contents` + `build_subagent_request`, reusing P3 `assemble_context`); §6 delegation UX → D (worktree dispatch + collapsible card + one-at-a-time guard + trivial-edits-inline); §10 delegation tests → B3 invariant test, C2 isolation, D4 fold, E1 ledger; core §4.4 shared runtime → A (`AgentProfile`) + B4 (generalized loop + `run_subagent`); core §3 git2 → C; core §7 `.claude/agents` schema → A.
- **Type/name consistency:** `AgentProfile { name, description, system_prompt, tools, model }` (A) is consumed by `build_subagent_request(…, profile, …)` (B3) and `run_subagent(…, profile, …)` (B4). `TurnConfig { system, cwd, branch }` (B4) threads into `build_request`/`run_tool`/`emit`. `SubagentResult { branch, summary, ok }` (B4) ↔ `EventKind::DelegationResult { branch, summary, ok }` (D1) ↔ `ChatMsg::Delegated { summary, ok }` (D1). `ContextSelection`/`ContextPolicy` used identically to P3 (B3 reuses `assemble_context`, no reimplementation). `create_worktree → WorktreeGuard::path()` (C1) is the `cwd` passed to `run_subagent` (D3).
- **Placeholder scan:** no `todo!`/`unimplemented!`/`...` in code; the two acknowledged prerequisite seams (`PLAN-1-SEAM` markdown body, `PLAN-2-SEAM` session_id) each ship a compiling grounded fallback so the branch is green on the current tree.
