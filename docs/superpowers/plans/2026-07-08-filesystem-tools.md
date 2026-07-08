# Filesystem Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace zoid's four minimal file tools with a six-tool Claude-Code-parity set (`Read`, `Write`, `Edit`, `Grep`, `Glob`, `LS`) so the agent rarely needs `shell` for file work.

**Architecture:** Each tool is a `zoid_tools::Tool` impl (`Local` kind) dispatched through the existing `run_tool` path — no new dispatch/gate machinery. A one-shot rename cutover keeps the whole workspace green, then each tool gains capabilities via TDD. Multi-edit is folded into `Edit`; there is no separate `MultiEdit` tool.

**Tech Stack:** Rust, `serde_json`, `regex` (Grep), `globset` (Glob + Grep filters), `tempfile` (tests).

## Global Constraints

- **Design spec:** `docs/superpowers/specs/2026-07-08-filesystem-tools-design.md` — follow it; update it if reality diverges.
- **No path jailing.** Preserve `resolve(cwd, path)` semantics (`lib.rs:156`) and the documented "no path-jailing" stance (`lib.rs:2-3`). Do not add sandboxing.
- **UTF-8 text only.** Non-UTF-8 reads fail with a clear error. No binary/image support.
- **Hard rename, no aliases.** A site is **functional** (must be renamed) if its string flows through the **registry**, the **approval tiers**, the **`AgentProfile` allowlist**, or a **model-facing prompt** — regardless of which crate it lives in. The functional sites are: the four tool `name()`s; `lib.rs` registries + tests (incl. the `read_tool_resolves_relative_to_cwd` test at `lib.rs:212`); `approval.rs` tiers + tests; `agent_profile.rs` allowlist + prompt + tests; `crates/zoid/tests/agent_loop.rs` and `crates/zoid/tests/subagent_integration.rs` (they dispatch scripted `write_file` calls through the **real** registry and assert the file was written); and the built-in skill prompt in `crates/zoid-core/src/skill.rs` (production model-facing text + its `contains` assertion). A site is **incidental** (intentionally left unchanged) if the string is an arbitrary sample name in a serialization/projection fixture that never reaches the registry: `zoid-provider` parse/request tests, `zoid-core` `context.rs`/`compaction.rs`/`projection.rs`/`zoom.rs`/`economy.rs`/`event.rs` fixtures, `zoid-tui` `overview.rs::sample()` + snapshot fixtures, `obs.rs`, `zoid-testkit`. Verified-safe-to-leave: `agent.rs:1936` (default profile has an empty allowlist → allow-all), `ask_user.rs` (the `read_file` call is drained on abort, never dispatched), and TUI tool-call rendering (name-agnostic: renders generic name+args).
- **Context safety.** Every read/search/list tool has a hard output ceiling and, on hitting it, appends a truncation notice telling the model how to get the rest.
- **Every tool is `ToolKind::Local`** (default `kind()`), dispatched via `run_tool` → `t.run()`.
- **Verify command (whole crate):** `cargo test -p zoid-tools` and, after cross-crate tasks, `cargo build --workspace && cargo test -p zoid-core -p zoid`.

---

### Task 1: Add `regex` and `globset` dependencies

**Files:**
- Modify: `Cargo.toml` (`[workspace.dependencies]`, after line 28)
- Modify: `crates/zoid-tools/Cargo.toml` (`[dependencies]`)

- [ ] **Step 1: Add to workspace dependency table**

In `Cargo.toml`, under `[workspace.dependencies]`, add:

```toml
regex = "1"
globset = "0.4"
```

- [ ] **Step 2: Reference them from zoid-tools**

In `crates/zoid-tools/Cargo.toml`, under `[dependencies]`, add (matching the `workspace = true` convention already used for `serde_json`):

```toml
regex = { workspace = true }
globset = { workspace = true }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p zoid-tools`
Expected: builds clean (deps download + compile; no code uses them yet).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/zoid-tools/Cargo.toml
git commit -m "build(zoid-tools): add regex + globset deps"
```

---

### Task 2: Rename cutover (`Read`/`Write`/`Edit`/`Grep`), behavior preserved

Pure rename of the four existing tools and every **functional** reference, keeping behavior/params identical so the workspace stays green. Capabilities are added in later tasks. This is atomic: a partial rename won't compile.

**Files:**
- Modify: `crates/zoid-tools/src/read.rs` (`ReadFile`→`Read`, name `"read_file"`→`"Read"`)
- Modify: `crates/zoid-tools/src/write.rs` (`WriteFile`→`Write`, `"write_file"`→`"Write"`)
- Modify: `crates/zoid-tools/src/edit.rs` (`EditFile`→`Edit`, `"edit_file"`→`"Edit"`)
- Modify: `crates/zoid-tools/src/search.rs` (`Search`→`Grep`, `"search"`→`"Grep"`)
- Modify: `crates/zoid-tools/src/lib.rs` (registry fns + registry test + `read_tool_resolves_relative_to_cwd` at `:212`)
- Modify: `crates/zoid-tools/src/approval.rs` (tiers at `:329`,`:336` + tests at `:548`,`:556-557`)
- Modify: `crates/zoid-core/src/agent_profile.rs` (allowlist `:40-46`, prompt `:35-39`, tests `:63-64`)
- Modify: `crates/zoid-core/src/skill.rs` (skill prompt body `:130`, `contains` assertion `:181`)
- Modify: `crates/zoid/src/invoke_skill.rs` (name-assertion test `:147-148`)
- Modify: `crates/zoid/tests/agent_loop.rs` (scripted calls + coupled event-name assertions: `:69`,`:132`,`:170`,`:338`)
- Modify: `crates/zoid/tests/subagent_integration.rs` (scripted call `:71`)

**Interfaces:**
- Produces: struct names `read::Read`, `write::Write`, `edit::Edit`, `search::Grep`; model-visible names `"Read"`, `"Write"`, `"Edit"`, `"Grep"`.

- [ ] **Step 1: Rename the four tool structs + `name()` returns**

`read.rs`: `pub struct ReadFile;` → `pub struct Read;`, `impl Tool for Read`, and `fn name(&self) -> &str { "Read" }`. Update the two error strings `read_file(...)` → `Read(...)` and the doc comment. Update its `#[cfg(test)]` uses of `ReadFile` → `Read`.

`write.rs`: `WriteFile` → `Write`, `name()` → `"Write"`, error `write_file(...)` → `Write(...)`, tests `WriteFile` → `Write`.

`edit.rs`: `EditFile` → `Edit`, `name()` → `"Edit"`, error prefixes `edit_file(...)` → `Edit(...)`, tests `EditFile` → `Edit`.

`search.rs`: `Search` → `Grep`, `name()` → `"Grep"`, tests `Search` → `Grep`. Leave `query`/literal behavior as-is (Task 4 reshapes it).

- [ ] **Step 2: Update the registry (both constructors) + registry test**

In `crates/zoid-tools/src/lib.rs`, both `registry()` and `registry_with_kill()`:

```rust
        Box::new(read::Read),
        Box::new(write::Write),
        Box::new(edit::Edit),
        Box::new(search::Grep),
```

And the `registry_has_unique_named_tools` test assertions:

```rust
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"Write"));
        assert!(names.contains(&"Edit"));
        assert!(names.contains(&"Grep"));
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"update_tasks"));
        assert!(names.contains(&"ask_user"));
```

- [ ] **Step 3: Update the approval tiers + their tests**

In `crates/zoid-tools/src/approval.rs`, the never-prompt arm (`:329`) and allow arm (`:336`):

```rust
            "Read" | "Grep" | "recall" | "show" | "update_tasks" | "ask_user" => {
                return crate::Gate::Allow;
            }
```
```rust
            "Write" | "Edit" => return crate::Gate::Allow,
```

And the tests (`:548`, `:556-557`):

```rust
        for name in ["Read", "Grep", "recall", "show", "update_tasks", "ask_user"] {
```
```rust
        assert_eq!(g.check(&tool_call("Write")), crate::Gate::Allow);
        assert_eq!(g.check(&tool_call("Edit")), crate::Gate::Allow);
```

- [ ] **Step 4: Update the AgentProfile allowlist + prompt + tests**

In `crates/zoid-core/src/agent_profile.rs`, `builtin()`:

```rust
            system_prompt: "You are a zoid subagent. You are given ONE discrete task and the \
                relevant code. Complete the task end to end using the tools (Read, Write, Edit, \
                Grep, shell). Work autonomously — do not ask questions. When done, give \
                a one-paragraph summary of what you changed."
                .into(),
            tools: vec![
                "Read".into(),
                "Write".into(),
                "Edit".into(),
                "Grep".into(),
                "shell".into(),
            ],
```

(`Glob`/`LS` are added to both the tool list **and** the prompt in Tasks 5 and 6, so the prompt never advertises a tool the allowlist would deny.) Update tests (`:63-64`):

```rust
        assert!(p.allows("Write"));
        assert!(p.allows("Edit"));
```

- [ ] **Step 5: Update the invoke_skill name-assertion test**

In `crates/zoid/src/invoke_skill.rs` (`:147-148`):

```rust
        assert!(names.contains(&"Write"));
        assert!(names.contains(&"Read"));
```

- [ ] **Step 6: Fix the `lib.rs` cwd-resolve test (compile break)**

`crates/zoid-tools/src/lib.rs:212` calls the renamed struct; after `ReadFile → Read` the old symbol is gone. Update line 212:

```rust
        let out = crate::read::Read.run(&serde_json::json!({ "path": "note.txt" }), dir.path());
```

Leave its `assert_eq!(out.text, "in cwd")` **as-is for now** — Task 3 changes `Read`'s output format and updates this assertion in the same task. (Between Task 2 and Task 3 this test still passes because Task 2 keeps `Read`'s body unchanged.)

- [ ] **Step 7: Update the built-in skill prompt (production model-facing text)**

`crates/zoid-core/src/skill.rs:130` instructs the model to "Use the write_file tool." Rename it, and flip the assertion at `:181` that currently hides the change:

```rust
                    Use the Write tool. Then confirm in one sentence that you wrote it."
```
```rust
        assert!(imp.body.contains("Write"));
```

- [ ] **Step 8: Rename the scripted tool calls in the agent-loop tests**

These dispatch through the real registry. In `crates/zoid/tests/agent_loop.rs`, change `"write_file"` → `"Write"` at the scripted calls **and** the coupled event-name assertion:
- `:69` `zoid_testkit::tool_call("Write", …)`
- `:132` `… if name == "Write"` (the recorded event name mirrors the call name — this assertion breaks if left)
- `:170` `name: "Write".into(),`
- `:338` `zoid_testkit::tool_call("Write", …)`

(Only the `:69` test hard-fails after rename — it runs under `AllowAll` and asserts the write happened. The `:170` `DenyAll` and `:338` cancellation tests would pass vacuously, but rename them to preserve intent.)

- [ ] **Step 9: Rename the scripted call in the subagent integration test**

`crates/zoid/tests/subagent_integration.rs:71` — `name: "Write".into(),`. (This one breaks doubly: after Task 2 the built-in profile allowlist contains `"Write"`, not `"write_file"`, so the old name is now denied *and* unknown.)

- [ ] **Step 10: Verify the workspace builds and tests pass**

Run: `cargo build --workspace && cargo test -p zoid-tools -p zoid-core -p zoid`
Expected: PASS. (Incidental fixtures using `"read_file"`/`"search"` in provider/projection tests are untouched and still pass — they test serialization, not the registry.)

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor(tools): rename file tools to Read/Write/Edit/Grep (behavior unchanged)"
```

---

### Task 3: `Read` — offset/limit paging, line numbers, output cap

**Files:**
- Modify: `crates/zoid-tools/src/read.rs`

**Interfaces:**
- Produces: `Read` accepts `{ path, offset?, limit? }`; output is `cat -n`-style (`"{n}\t{line}\n"`), 1-indexed from `offset`.

- [ ] **Step 1: Write the failing tests**

Add to `read.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn reads_with_line_numbers() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "alpha\nbeta\ngamma").unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, "1\talpha\n2\tbeta\n3\tgamma\n");
    }

    #[test]
    fn offset_and_limit_page_the_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "l1\nl2\nl3\nl4\nl5").unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap(), "offset": 2, "limit": 2 }),
            std::path::Path::new("."),
        );
        // `end (3) < total (5)`, so a "there's more" notice follows the two
        // requested lines — assert the prefix, not exact equality.
        assert!(out.text.starts_with("2\tl2\n3\tl3\n"), "got: {}", out.text);
        assert!(out.text.contains("offset=4"));
    }

    #[test]
    fn over_long_line_is_truncated() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{}", "x".repeat(5000)).unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("(line truncated)"));
        assert!(out.text.len() < 4000, "a 5000-char line must not pass through whole");
    }

    #[test]
    fn non_utf8_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bin");
        std::fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        let out = Read.run(
            &json!({ "path": p.to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
    }

    #[test]
    fn over_cap_appends_truncation_notice() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let body: String = (1..=2100).map(|n| format!("line{n}\n")).collect();
        write!(f, "{body}").unwrap();
        let out = Read.run(
            &json!({ "path": f.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.starts_with("1\tline1\n"));
        assert!(out.text.contains("truncated"));
        assert!(out.text.contains("offset=2001"));
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tools read::tests -- --include-ignored`
Expected: FAIL — `reads_with_line_numbers` expects `"1\talpha\n…"` but current `Read` returns raw contents.

- [ ] **Step 3: Implement paging + line numbers + cap**

Replace `read.rs`'s `spec()` params and `run()`:

```rust
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Read a UTF-8 text file. Output is line-numbered. Use offset/limit to page through large files.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "File path relative to the working directory." },
                    "offset": { "type": "integer", "description": "1-indexed line to start from (default 1)." },
                    "limit":  { "type": "integer", "description": "Max lines to return (default 2000)." }
                },
                "required": ["path"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        const DEFAULT_LIMIT: usize = 2000;
        const MAX_LINE: usize = 2000; // per-line char cap (CC parity) — stops a
                                      // single giant line from blowing context.
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let contents = match std::fs::read_to_string(crate::resolve(cwd, &path)) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("Read({path}): {e}")),
        };
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|n| n.max(1) as usize)
            .unwrap_or(1);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT);
        let lines: Vec<&str> = contents.lines().collect();
        let total = lines.len();
        let start = offset.saturating_sub(1).min(total);
        let end = start.saturating_add(limit).min(total);
        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let shown = if line.chars().count() > MAX_LINE {
                let head: String = line.chars().take(MAX_LINE).collect();
                format!("{head}… (line truncated)")
            } else {
                (*line).to_string()
            };
            out.push_str(&format!("{}\t{}\n", offset + i, shown));
        }
        if end < total {
            out.push_str(&format!(
                "… truncated; {} more lines, continue with offset={}\n",
                total - end,
                end + 1
            ));
        }
        ToolOutput::ok(out)
    }
```

- [ ] **Step 4: Update the two assertions that the new output format breaks**

The line-numbered output changes two existing exact-match assertions:

In `read.rs`'s `reads_existing_file`:

```rust
        assert_eq!(out.text, "1\thello tools\n");
```

In `crates/zoid-tools/src/lib.rs`'s `read_tool_resolves_relative_to_cwd` (`:214`, whose call site was renamed to `crate::read::Read` in Task 2 Step 6):

```rust
        assert_eq!(out.text, "1\tin cwd\n");
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p zoid-tools read:: && cargo test -p zoid-tools read_tool_resolves_relative_to_cwd`
Expected: PASS (all new Read tests + both updated assertions).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tools/src/read.rs crates/zoid-tools/src/lib.rs
git commit -m "feat(tools): Read gains offset/limit paging + line numbers + per-line cap"
```

---

### Task 4: `Grep` — regex, context lines, glob filter, output modes

**Files:**
- Modify: `crates/zoid-tools/src/search.rs`

**Interfaces:**
- Consumes: `regex::Regex`, `globset::Glob`.
- Produces: `Grep` accepts `{ pattern, path?, glob?, "-i"?, output_mode? , "-A"?, "-B"?, "-C"? }`; `output_mode` ∈ `files_with_matches` (default) | `content` | `count`.

- [ ] **Step 1: Write the failing tests**

Replace `search.rs`'s test module with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello\nWORLD\n").unwrap();
        dir
    }

    #[test]
    fn regex_content_mode_returns_numbered_hits() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": r"fn \w+", "path": dir.path().to_str().unwrap(), "output_mode": "content" }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.rs:1:"));
        assert!(out.text.contains("a.rs:2:"));
    }

    #[test]
    fn files_with_matches_is_default() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "fn", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("a.rs"));
        assert!(!out.text.contains("b.txt"));
        assert!(!out.text.contains(":1:"), "default mode lists files, not lines");
    }

    #[test]
    fn glob_filter_restricts_file_set() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": ".", "path": dir.path().to_str().unwrap(), "glob": "*.txt" }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("b.txt"));
        assert!(!out.text.contains("a.rs"));
    }

    #[test]
    fn case_insensitive_flag() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "world", "-i": true, "path": dir.path().to_str().unwrap(), "output_mode": "content" }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("b.txt:2:"));
    }

    #[test]
    fn count_mode_reports_totals() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "fn", "path": dir.path().to_str().unwrap(), "output_mode": "count" }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("a.rs:2"));
    }

    #[test]
    fn invalid_regex_is_error() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "(", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
    }

    #[test]
    fn no_match_reports_cleanly() {
        let dir = seed();
        let out = Grep.run(
            &json!({ "pattern": "zzzznomatch", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error);
        assert!(out.text.contains("no matches"));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tools search::tests`
Expected: FAIL — `Grep` still does literal `query` matching and ignores `pattern`/`output_mode`.

- [ ] **Step 3: Implement the regex Grep**

Replace **everything above the `#[cfg(test)]` module** in `search.rs` — the imports, the old `const MAX_RESULTS`, the `Grep` impl, **and delete the existing module-level `skip`/`walk` functions** (the block below re-declares both; leaving the originals is a duplicate-definition compile error). The new file content above the test module is:

```rust
use crate::{str_arg, Tool, ToolOutput};
use globset::Glob;
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 200;

/// Recursive regex search over text files under a root directory (default `.`).
/// Skips hidden entries and common build dirs; never follows symlinks.
pub struct Grep;

impl Tool for Grep {
    fn name(&self) -> &str {
        "Grep"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Search file contents with a regular expression.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern":     { "type": "string", "description": "Regular expression to search for." },
                    "path":        { "type": "string", "description": "Root directory to search (default '.')." },
                    "glob":        { "type": "string", "description": "Only search files matching this glob (e.g. '*.rs')." },
                    "-i":          { "type": "boolean", "description": "Case-insensitive match." },
                    "output_mode": { "type": "string", "enum": ["files_with_matches", "content", "count"], "description": "Default 'files_with_matches'." }
                },
                "required": ["pattern"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let pattern = match str_arg(args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let case_insensitive = args.get("-i").and_then(|v| v.as_bool()).unwrap_or(false);
        let re = match RegexBuilder::new(&pattern)
            .case_insensitive(case_insensitive)
            .build()
        {
            Ok(re) => re,
            Err(e) => return ToolOutput::err(format!("Grep: invalid regex: {e}")),
        };
        let glob = match args.get("glob").and_then(|v| v.as_str()) {
            Some(g) => match Glob::new(g) {
                Ok(g) => Some(g.compile_matcher()),
                Err(e) => return ToolOutput::err(format!("Grep: invalid glob: {e}")),
            },
            None => None,
        };
        let mode = args
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches");
        let root = crate::resolve(
            cwd,
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
        );

        // (relpath, line_no, line_text) hits, capped at MAX_RESULTS.
        let mut hits: Vec<(String, usize, String)> = Vec::new();
        walk(&root, &root, &re, glob.as_ref(), &mut hits);

        if hits.is_empty() {
            return ToolOutput::ok(format!("no matches for {pattern:?}"));
        }
        let truncated = hits.len() >= MAX_RESULTS;
        let mut text = match mode {
            "content" => hits
                .iter()
                .map(|(rel, n, line)| format!("{rel}:{n}: {}", line.trim_end()))
                .collect::<Vec<_>>()
                .join("\n"),
            "count" => {
                let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
                for (rel, _, _) in &hits {
                    *counts.entry(rel.as_str()).or_default() += 1;
                }
                counts
                    .iter()
                    .map(|(rel, c)| format!("{rel}:{c}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => {
                // files_with_matches: unique paths in first-seen order.
                let mut seen: Vec<&str> = Vec::new();
                for (rel, _, _) in &hits {
                    if !seen.contains(&rel.as_str()) {
                        seen.push(rel);
                    }
                }
                seen.join("\n")
            }
        };
        if truncated {
            text.push_str(&format!("\n… (truncated at {MAX_RESULTS} matches; narrow the pattern or path)"));
        }
        ToolOutput::ok(text)
    }
}

fn skip(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn walk(
    root: &Path,
    dir: &Path,
    re: &regex::Regex,
    glob: Option<&globset::GlobMatcher>,
    hits: &mut Vec<(String, usize, String)>,
) {
    if hits.len() >= MAX_RESULTS {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if hits.len() >= MAX_RESULTS {
            return;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if skip(name) {
            continue;
        }
        if path.is_symlink() {
            continue;
        } else if path.is_dir() {
            walk(root, &path, re, glob, hits);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if let Some(g) = glob {
                if !g.is_match(&rel) {
                    continue;
                }
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                for (i, line) in contents.lines().enumerate() {
                    if re.is_match(line) {
                        hits.push((rel.clone(), i + 1, line.to_string()));
                        if hits.len() >= MAX_RESULTS {
                            return;
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zoid-tools search::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/search.rs
git commit -m "feat(tools): Grep does regex search with glob filter + output modes"
```

---

### Task 5: `Glob` — filename pattern matching

**Files:**
- Create: `crates/zoid-tools/src/glob.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (module decl + both registries)
- Modify: `crates/zoid-core/src/agent_profile.rs` (append `"Glob"` to allowlist)

**Interfaces:**
- Produces: `glob::GlobTool` with `name()` `"Glob"`, accepts `{ pattern, path? }`, returns newline-joined relative paths sorted by mtime (newest first).

- [ ] **Step 1: Write the failing test in a new file**

Create `crates/zoid-tools/src/glob.rs`:

```rust
use crate::{str_arg, Tool, ToolOutput};
use globset::Glob;
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 200;

/// Match files by name/glob pattern (e.g. `**/*.rs`) under a root, newest first.
pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Find files by glob pattern (e.g. '**/*.rs'), sorted by modification time."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern, e.g. '**/*.rs'." },
                    "path":    { "type": "string", "description": "Root directory to search (default '.')." }
                },
                "required": ["pattern"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let pattern = match str_arg(args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let matcher = match Glob::new(&pattern) {
            Ok(g) => g.compile_matcher(),
            Err(e) => return ToolOutput::err(format!("Glob: invalid pattern: {e}")),
        };
        let root = crate::resolve(
            cwd,
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
        );
        let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
        walk(&root, &root, &matcher, &mut found);
        if found.is_empty() {
            return ToolOutput::ok(format!("no files match {pattern:?}"));
        }
        // Newest first.
        found.sort_by(|a, b| b.0.cmp(&a.0));
        let truncated = found.len() > MAX_RESULTS;
        found.truncate(MAX_RESULTS);
        let mut text = found
            .into_iter()
            .map(|(_, rel)| rel)
            .collect::<Vec<_>>()
            .join("\n");
        if truncated {
            text.push_str(&format!("\n… (truncated at {MAX_RESULTS} files)"));
        }
        ToolOutput::ok(text)
    }
}

fn skip(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn walk(
    root: &Path,
    dir: &Path,
    matcher: &globset::GlobMatcher,
    found: &mut Vec<(std::time::SystemTime, String)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if skip(name) {
            continue;
        }
        if path.is_symlink() {
            continue;
        } else if path.is_dir() {
            walk(root, &path, matcher, found);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if matcher.is_match(&rel) {
                let mtime = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                found.push((mtime, rel));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_by_extension_recursively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.rs"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();
        let out = GlobTool.run(
            &json!({ "pattern": "**/*.rs", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.rs"));
        assert!(out.text.contains("b.rs") || out.text.contains("sub/b.rs") || out.text.contains("sub\\b.rs"));
        assert!(!out.text.contains("c.txt"));
    }

    #[test]
    fn no_match_reports_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let out = GlobTool.run(
            &json!({ "pattern": "*.rs", "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("no files match"));
    }
}
```

- [ ] **Step 2: Register the module + tool**

In `crates/zoid-tools/src/lib.rs`: add `pub mod glob;` (keep the module list alphabetical-ish, next to `edit`). Add `Box::new(glob::GlobTool),` to both `registry()` and `registry_with_kill()` (after the `search::Grep` line). Add `assert!(names.contains(&"Glob"));` to `registry_has_unique_named_tools`.

- [ ] **Step 3: Add `Glob` to the subagent allowlist + prompt**

In `crates/zoid-core/src/agent_profile.rs` `builtin()`: add `"Glob".into(),` to the `tools` vec after `"Grep".into(),`, and update the `system_prompt` tool list to `(Read, Write, Edit, Grep, Glob, shell)` so the prompt and allowlist stay in sync.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zoid-tools glob::tests && cargo test -p zoid-tools registry`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/glob.rs crates/zoid-tools/src/lib.rs crates/zoid-core/src/agent_profile.rs
git commit -m "feat(tools): add Glob file-pattern tool"
```

---

### Task 6: `LS` — directory listing

**Files:**
- Create: `crates/zoid-tools/src/ls.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (module decl + both registries + test)
- Modify: `crates/zoid-core/src/agent_profile.rs` (append `"LS"`)

**Interfaces:**
- Produces: `ls::Ls` with `name()` `"LS"`, accepts `{ path, ignore? }`, returns one entry per line: `"{type}\t{size}\t{name}"` where type ∈ `dir`/`file`/`link`.

- [ ] **Step 1: Write the tool + failing test**

Create `crates/zoid-tools/src/ls.rs`:

```rust
use crate::{str_arg, Tool, ToolOutput};
use globset::Glob;
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

const MAX_RESULTS: usize = 500;

/// List the entries of a directory (non-recursive): type, size, name.
pub struct Ls;

impl Tool for Ls {
    fn name(&self) -> &str {
        "LS"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "List a directory's entries (type, size, name).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "Directory to list." },
                    "ignore": { "type": "array", "items": { "type": "string" }, "description": "Glob patterns to omit." }
                },
                "required": ["path"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let ignores: Vec<globset::GlobMatcher> = args
            .get("ignore")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| Glob::new(s).ok())
                    .map(|g| g.compile_matcher())
                    .collect()
            })
            .unwrap_or_default();
        let dir = crate::resolve(cwd, &path);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => return ToolOutput::err(format!("LS({path}): {e}")),
        };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        let mut rows: Vec<String> = Vec::new();
        for p in paths {
            if rows.len() >= MAX_RESULTS {
                rows.push(format!("… (truncated at {MAX_RESULTS} entries)"));
                break;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if name.starts_with('.') || matches!(name.as_str(), "target" | "node_modules") {
                continue;
            }
            if ignores.iter().any(|g| g.is_match(&name)) {
                continue;
            }
            let (kind, size) = if p.is_symlink() {
                ("link", 0)
            } else if p.is_dir() {
                ("dir", 0)
            } else {
                ("file", p.metadata().map(|m| m.len()).unwrap_or(0))
            };
            rows.push(format!("{kind}\t{size}\t{name}"));
        }
        if rows.is_empty() {
            return ToolOutput::ok("(empty)".to_string());
        }
        ToolOutput::ok(rows.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lists_entries_with_types() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "abc").unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let out = Ls.run(
            &json!({ "path": dir.path().to_str().unwrap() }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("file\t3\tf.txt"));
        assert!(out.text.contains("dir\t0\td"));
    }

    #[test]
    fn ignore_globs_and_skiplist_are_applied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.rs"), "").unwrap();
        std::fs::write(dir.path().join("skip.log"), "").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        let out = Ls.run(
            &json!({ "path": dir.path().to_str().unwrap(), "ignore": ["*.log"] }),
            std::path::Path::new("."),
        );
        assert!(out.text.contains("keep.rs"));
        assert!(!out.text.contains("skip.log"));
        assert!(!out.text.contains("target"));
    }

    #[test]
    fn missing_dir_is_error() {
        let out = Ls.run(
            &json!({ "path": "/no/such/zoid/dir" }),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
    }
}
```

- [ ] **Step 2: Register + allowlist + prompt**

In `crates/zoid-tools/src/lib.rs`: add `pub mod ls;`, add `Box::new(ls::Ls),` to both registries, add `assert!(names.contains(&"LS"));` to the registry test. In `agent_profile.rs`: append `"LS".into(),` after `"Glob".into(),`, and update the `system_prompt` tool list to its final form `(Read, Write, Edit, Grep, Glob, LS, shell)`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p zoid-tools ls::tests && cargo test -p zoid-tools registry`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tools/src/ls.rs crates/zoid-tools/src/lib.rs crates/zoid-core/src/agent_profile.rs
git commit -m "feat(tools): add LS directory-listing tool"
```

---

### Task 7: `Edit` — `replace_all` + atomic multi-edit

**Files:**
- Modify: `crates/zoid-tools/src/edit.rs`

**Interfaces:**
- Produces: `Edit` accepts single `{ path, old_string, new_string, replace_all? }` **or** `{ path, edits: [{old_string,new_string,replace_all?}] }`. Multi-edit is all-or-nothing.

- [ ] **Step 1: Write the failing tests**

Add to `edit.rs`'s test module (keep the existing `seed`):

```rust
    #[test]
    fn replace_all_replaces_every_occurrence() {
        let (_d, path) = seed("x x x");
        let out = Edit.run(
            &json!({ "path": path, "old_string": "x", "new_string": "y", "replace_all": true }),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "y y y");
    }

    #[test]
    fn multi_edit_applies_all_atomically() {
        let (_d, path) = seed("alpha beta gamma");
        let out = Edit.run(
            &json!({ "path": path, "edits": [
                { "old_string": "alpha", "new_string": "A" },
                { "old_string": "gamma", "new_string": "G" }
            ]}),
            std::path::Path::new("."),
        );
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A beta G");
    }

    #[test]
    fn multi_edit_failure_leaves_file_untouched() {
        let (_d, path) = seed("alpha beta");
        let out = Edit.run(
            &json!({ "path": path, "edits": [
                { "old_string": "alpha", "new_string": "A" },
                { "old_string": "zzz", "new_string": "Z" }
            ]}),
            std::path::Path::new("."),
        );
        assert!(out.is_error);
        // First edit must NOT have been written.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha beta");
    }
```

Also update the two existing tests to the new param names: `"old"`→`"old_string"`, `"new"`→`"new_string"`.

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tools edit::tests`
Expected: FAIL — current `Edit` uses `old`/`new` and has no `edits`/`replace_all`.

- [ ] **Step 3: Implement single + multi + replace_all**

Replace `edit.rs`'s `spec()` and `run()` (and add a helper):

```rust
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: "Edit a file: replace an exact unique string, or apply a batch of edits atomically.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":        { "type": "string" },
                    "old_string":  { "type": "string", "description": "Exact text to find (must occur once unless replace_all)." },
                    "new_string":  { "type": "string", "description": "Replacement text." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)." },
                    "edits":       { "type": "array", "description": "Batch of {old_string,new_string,replace_all?} applied atomically.",
                        "items": { "type": "object", "properties": {
                            "old_string": { "type": "string" }, "new_string": { "type": "string" }, "replace_all": { "type": "boolean" }
                        }, "required": ["old_string", "new_string"] } }
                },
                "required": ["path"]
            }),
        }
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let path = match str_arg(args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        // Normalize to a list of edits: either `edits: [...]` or a single triple.
        let edits: Vec<(String, String, bool)> = if let Some(arr) = args.get("edits").and_then(|v| v.as_array()) {
            let mut v = Vec::new();
            for (i, e) in arr.iter().enumerate() {
                let old = match e.get("old_string").and_then(|x| x.as_str()) {
                    Some(s) => s.to_string(),
                    None => return ToolOutput::err(format!("Edit({path}): edits[{i}] missing old_string")),
                };
                let new = match e.get("new_string").and_then(|x| x.as_str()) {
                    Some(s) => s.to_string(),
                    None => return ToolOutput::err(format!("Edit({path}): edits[{i}] missing new_string")),
                };
                let all = e.get("replace_all").and_then(|x| x.as_bool()).unwrap_or(false);
                v.push((old, new, all));
            }
            v
        } else {
            let old = match str_arg(args, "old_string") {
                Ok(o) => o,
                Err(e) => return e,
            };
            let new = match str_arg(args, "new_string") {
                Ok(n) => n,
                Err(e) => return e,
            };
            let all = args.get("replace_all").and_then(|x| x.as_bool()).unwrap_or(false);
            vec![(old, new, all)]
        };

        let full = crate::resolve(cwd, &path);
        let mut contents = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("Edit({path}): {e}")),
        };
        // Apply all edits in memory; bail (writing nothing) on the first failure.
        for (i, (old, new, replace_all)) in edits.iter().enumerate() {
            match apply_one(&contents, old, new, *replace_all) {
                Ok(updated) => contents = updated,
                Err(msg) => return ToolOutput::err(format!("Edit({path}) edit #{}: {msg}", i + 1)),
            }
        }
        match std::fs::write(&full, contents.as_bytes()) {
            Ok(()) => ToolOutput::ok(format!("edited {path} ({} change(s))", edits.len())),
            Err(e) => ToolOutput::err(format!("Edit({path}): {e}")),
        }
    }
}

/// Apply one edit to `contents`, enforcing the unambiguous-match rule unless
/// `replace_all`. Returns the updated string or an error message.
fn apply_one(contents: &str, old: &str, new: &str, replace_all: bool) -> Result<String, String> {
    let count = contents.matches(old).count();
    if count == 0 {
        return Err("`old_string` not found".into());
    }
    if count > 1 && !replace_all {
        return Err(format!("`old_string` is ambiguous ({count} matches)"));
    }
    if replace_all {
        Ok(contents.replace(old, new))
    } else {
        Ok(contents.replacen(old, new, 1))
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p zoid-tools edit::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/edit.rs
git commit -m "feat(tools): Edit gains replace_all + atomic multi-edit"
```

---

### Task 8: Registry advertisement test + final sweep

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs` (add a spec-advertisement test)

**Interfaces:**
- Consumes: `registry()`, each tool's `spec()`.

- [ ] **Step 1: Write the advertisement test**

Add to `lib.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn fs_tools_advertise_valid_object_schemas() {
        let reg = registry();
        for want in ["Read", "Write", "Edit", "Grep", "Glob", "LS"] {
            let t = reg
                .iter()
                .find(|t| t.name() == want)
                .unwrap_or_else(|| panic!("{want} must be registered"));
            let spec = t.spec();
            assert_eq!(spec.name, want);
            assert_eq!(
                spec.parameters["type"], "object",
                "{want} params must be a JSON object schema"
            );
            assert!(
                spec.parameters["properties"].is_object(),
                "{want} must declare properties"
            );
        }
    }
```

- [ ] **Step 2: Run test**

Run: `cargo test -p zoid-tools fs_tools_advertise_valid_object_schemas`
Expected: PASS.

- [ ] **Step 3: Sweep for stale references — struct symbols (workspace-wide)**

The renamed struct symbols must not survive anywhere:

Run: `rg -n 'ReadFile|WriteFile|EditFile|search::Search|\bstruct Search\b' crates/`
Expected: **no hits.**

- [ ] **Step 4: Sweep the functional string surfaces**

Old tool-name strings must be gone from every functional surface (registry, approval tiers, profile, model-facing prompts, and the tests that dispatch through the real registry). Do **not** touch incidental serialization/projection fixtures (see Global Constraints):

Run: `rg -n '"read_file"|"write_file"|"edit_file"|"search"' crates/zoid-tools/src crates/zoid-core/src/agent_profile.rs crates/zoid-core/src/skill.rs crates/zoid/src/invoke_skill.rs crates/zoid/tests/agent_loop.rs crates/zoid/tests/subagent_integration.rs`
Expected: **no hits.** (Any remaining hits elsewhere — `zoid-provider`, `context.rs`/`compaction.rs`/`projection.rs`/`zoom.rs`/`economy.rs`/`event.rs`, `zoid-tui`, `obs.rs`, `zoid-testkit`, `agent.rs:1936`, `ask_user.rs` — are the verified-incidental fixtures and stay.)

- [ ] **Step 5: Full workspace verification**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace`
Expected: PASS, no new clippy warnings in touched files.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tools/src/lib.rs
git commit -m "test(tools): assert the six FS tools advertise valid schemas"
```

---

## Self-Review

**1. Spec coverage.**
- Read offset/limit/line-numbers/cap → Task 3 ✓
- Write parity rename → Task 2 ✓
- Edit replace_all + atomic multi-edit → Task 7 ✓
- Grep regex/context/glob/output-modes/cap → Task 4 ✓ (note: `-A/-B/-C` context lines are specced but **deferred** — see gap below)
- Glob → Task 5 ✓
- LS → Task 6 ✓
- Approval-tier rename → Task 2 ✓
- AgentProfile allowlist (incl. Glob/LS) → Tasks 2/5/6 ✓
- Registry advertisement test → Task 8 ✓
- Dependencies (regex, globset) → Task 1 ✓

**Gap found & resolved:** the spec lists Grep context lines (`-A/-B/-C`). To keep Task 4 reviewable and because context-line windowing adds meaningful complexity, v1 implements `pattern`/`glob`/`-i`/`output_mode` and **defers context lines** to a fast-follow. Recorded here so it is a conscious cut, not an omission; the spec's Grep section should be annotated "context lines: fast-follow." `multiline` and `type` filters are likewise deferred (glob covers the common file-filtering need).

**2. Placeholder scan.** No TBD/TODO; every code step shows complete code. ✓

**3. Type consistency.** Struct names (`Read`, `Write`, `Edit`, `Grep`, `glob::GlobTool`, `ls::Ls`), model names (`"Read"`/`"Write"`/`"Edit"`/`"Grep"`/`"Glob"`/`"LS"`), and `apply_one` helper are used consistently across tasks. Edit param names (`old_string`/`new_string`) are introduced in Task 7 and the existing tests updated in the same task. ✓

## Revisions from technical review (2026-07-08)

Incorporated after a blocking-level review:

- **Migration completeness (was the root defect):** reclassified functional-vs-incidental by **data-flow** (registry / approval / allowlist / model-prompt), not by crate. Added the four missed functional sites to Task 2 — `lib.rs:212` (compile break), `crates/zoid/tests/agent_loop.rs` + `subagent_integration.rs` (scripted `write_file` calls dispatched through the real registry), and `crates/zoid-core/src/skill.rs` (production skill prompt whose `contains` assertion was *hiding* the break). Widened Task 8's sweep to the workspace with an explicit incidental allow-list.
- **Task 3 test bug:** the `offset/limit` test used `assert_eq!` but the implementation appends a truncation notice when more lines remain → switched to `starts_with`.
- **Read byte ceiling (spec §Read) was silently dropped:** added a per-line `MAX_LINE` char cap (CC parity) + a truncation test, so a single giant line can't blow context.
- **Task 4 ambiguity:** made the `search.rs` replacement explicit about deleting the old module-level `skip`/`walk` (would otherwise be a duplicate-definition compile error).
- **Prompt/allowlist ordering:** Task 2's subagent prompt no longer advertises `Glob`/`LS`; Tasks 5/6 add them to prompt **and** allowlist together.
- **Confirmed non-issue:** globset's default (`literal_separator=false`) makes `*.rs` match `sub/b.rs` and `**/*.rs` match top-level `a.rs`, so the Grep/Glob/LS glob tests pass as written; `Glob::compile_matcher()`/`GlobMatcher::is_match(&str)` is the correct API.
