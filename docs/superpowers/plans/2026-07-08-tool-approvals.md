# Tool-Call Approvals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a dangerous-action approval layer that prompts the user before the agent runs genuinely dangerous shell commands, with a YOLO escape hatch and subagent auto-deny.

**Architecture:** A `BlacklistGate` implementing the existing `ToolGate` trait lives in `zoid-tools/src/approval.rs`. It tokenizes shell commands with `shlex` and matches them against builtin dangerous patterns. The agent loop's existing `Gate::Deny` check site gains a `Gate::Prompt` arm that reuses the `ask_user` oneshot park-and-await overlay. Config `[approval]` and CLI `--yolo` select between `AllowAll` and `BlacklistGate`.

**Tech Stack:** Rust, shlex (already in dep tree), tokio oneshot, serde TOML config

## Global Constraints

- The `Gate` enum is in `crates/zoid-tools/src/lib.rs`; `check` is sync and can only return a `Gate` variant — the agent loop handles suspension.
- The `ask_user` Interactive path (`AgentUpdate::AskUser` + `oneshot`) is already built; `Gate::Prompt` reuses it — no new UI.
- `shlex` is already a transitive dependency — add it as a direct dep to `zoid-tools/Cargo.toml`.
- Config merge follows existing patterns: scalars use last-write-wins, vecs use union-dedup.
- Subagents are headless (`ask_user` filtered from their tool set); they auto-deny instead of prompting.
- Fail-safe toward prompting: unparseable commands prompt; false positives cost one keystroke, false negatives cost data.
- Every existing `Cli::Run { ... }` literal and test must be updated for the new `yolo` field.

---

## File Structure

- **Create:** `crates/zoid-tools/src/approval.rs` — `Pattern` enum, `BlacklistGate`, chain-splitter, shlex matcher, builtin defaults, unit tests.
- **Modify:** `crates/zoid-tools/src/lib.rs` — add `Gate::Prompt` variant, `mod approval;`, re-export `BlacklistGate`.
- **Modify:** `crates/zoid-tools/Cargo.toml` — add `shlex` direct dep.
- **Modify:** `crates/zoid-core/src/config.rs` — `ApprovalConfig`, `PartialApproval`, merge wiring, provenance, tests.
- **Modify:** `crates/zoid/src/agent.rs` — expand the `Gate::Deny` check to a `match` on all three variants; `Gate::Prompt` arm reuses `ask_user` path.
- **Modify:** `crates/zoid/src/cli.rs` — add `yolo: bool` to `Cli::Run`, parse `--yolo`, update help text, update tests.
- **Modify:** `crates/zoid/src/main.rs` — gate selection in `spawn_turn` (replace `AllowAll`); thread `cli_yolo` from CLI parse.
- **Modify:** `crates/zoid/src/subagent.rs` — replace `AllowAll` with `BlacklistGate { interactive: false }`; add `approval` param to `run_subagent`.
- **Modify:** `crates/zoid/src/spawn_subagent.rs` — thread `approval` config to `run_subagent`.
- **Modify:** `crates/zoid/tests/agent_loop.rs` — integration tests for `Gate::Prompt` approve + deny paths.

---

### Task 1: Add `Gate::Prompt` variant to the `Gate` enum

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs`

**Interfaces:**
- Produces: `Gate::Prompt { question: String, choices: Vec<String> }` — a new variant on the existing `Gate` enum. Later tasks match on this in the agent loop.

- [ ] **Step 1: Add the `Prompt` variant to `Gate`**

In `crates/zoid-tools/src/lib.rs`, find the `Gate` enum:

```rust
pub enum Gate {
    Allow,
    /// Block the call; the string is fed back to the model as the tool result.
    Deny(String),
}
```

Replace with:

```rust
pub enum Gate {
    Allow,
    /// Block the call; the string is fed back to the model as the tool result.
    Deny(String),
    /// Request an interactive approval from the user. The agent loop reuses
    /// the existing `ask_user` oneshot + `AgentUpdate::AskUser` park-and-await
    /// path to suspend and resume on the user's answer. `question` is shown in
    /// the question overlay; `choices` are the selectable options.
    Prompt {
        question: String,
        choices: Vec<String>,
    },
}
```

- [ ] **Step 2: Add `mod approval;` and re-export**

In `crates/zoid-tools/src/lib.rs`, near the top after the `pub mod` declarations (around line 3-15), add:

```rust
pub mod approval;
```

And re-export `BlacklistGate` near the `AllowAll` definition:

```rust
pub use approval::BlacklistGate;
```

- [ ] **Step 3: Add `shlex` to `Cargo.toml`**

In `crates/zoid-tools/Cargo.toml`, under `[dependencies]`, add:

```toml
shlex = "2.0"
```

- [ ] **Step 4: Create a stub `approval.rs` so the crate compiles**

Create `crates/zoid-tools/src/approval.rs` with just enough to compile:

```rust
//! Tool-call approval: a blacklist gate that prompts (or denies) on
//! dangerous shell commands. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`.

/// The blacklist gate. Allow unless a `shell` call matches a dangerous pattern.
/// `interactive: true` returns `Gate::Prompt` on a match (Chat);
/// `interactive: false` returns `Gate::Deny` (subagents — headless, can't prompt).
pub struct BlacklistGate {
    interactive: bool,
}

impl BlacklistGate {
    pub fn new(_shell_danger: Vec<String>, _shell_allow: Vec<String>, interactive: bool) -> Self {
        Self { interactive }
    }
}

impl crate::ToolGate for BlacklistGate {
    fn check(&self, _call: &zoid_provider::ToolCall) -> crate::Gate {
        crate::Gate::Allow
    }
}
```

- [ ] **Step 5: Verify the crate compiles**

Run: `cargo check -p zoid-tools`
Expected: compiles with no errors (warnings about unused fields are OK for now)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tools/src/lib.rs crates/zoid-tools/src/approval.rs crates/zoid-tools/Cargo.toml
git commit -m "feat: add Gate::Prompt variant and stub BlacklistGate

Gate::Prompt { question, choices } lets the sync check() request an
interactive approval; the agent loop will reuse the ask_user overlay.
BlacklistGate is a stub that always allows — real matching comes next."
```

---

### Task 2: Implement the chain-splitter and shlex tokenizer

**Files:**
- Modify: `crates/zoid-tools/src/approval.rs`
- Test: `crates/zoid-tools/src/approval.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `fn split_segments(cmd: &str) -> Vec<String>` — splits a command string on `&&`, `||`, `;`, `|` into independent command segments. `||` is handled before `|` (logical-OR vs pipe).
- Produces: `enum Pattern` — the structured pattern types for matching.
- Produces: `fn builtin_defaults() -> Vec<Pattern>` — the curated builtin dangerous patterns.

- [ ] **Step 1: Write the failing test for `split_segments`**

Add to `crates/zoid-tools/src/approval.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_segments_basic() {
        assert_eq!(split_segments("echo hi"), vec!["echo hi".to_string()]);
        assert_eq!(split_segments("echo hi && rm -rf /"), vec!["echo hi ".to_string(), " rm -rf /".to_string()]);
    }

    #[test]
    fn split_segments_pipe_vs_or() {
        // || must be handled before | (logical-OR vs pipe)
        let segs = split_segments("false || echo ok");
        assert_eq!(segs, vec!["false ".to_string(), " echo ok".to_string()]);
        // single pipe is a pipe, not logical-OR
        let segs = split_segments("git log | grep foo");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], "git log ".to_string());
        assert_eq!(segs[1], " grep foo".to_string());
    }

    #[test]
    fn split_segments_semicolon() {
        assert_eq!(split_segments("cd /tmp; ls"), vec!["cd /tmp".to_string(), " ls".to_string()]);
    }

    #[test]
    fn split_segments_multiple() {
        let segs = split_segments("a && b || c ; d | e");
        assert_eq!(segs.len(), 5);
    }

    #[test]
    fn split_segments_no_separator() {
        assert_eq!(split_segments("ls -la"), vec!["ls -la".to_string()]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tools approval::tests::split_segments -- --nocapture`
Expected: FAIL — `split_segments` not defined

- [ ] **Step 3: Implement `split_segments`**

Add to `crates/zoid-tools/src/approval.rs` (above the test module):

```rust
/// Split a command string on shell chain operators (`&&`, `||`, `;`, `|`) into
/// independent command segments. `||` is handled before `|` (logical-OR vs
/// pipe). Each segment is checked independently by the matcher.
fn split_segments(cmd: &str) -> Vec<String> {
    // Walk the string, splitting on `&&`, `||`, `;`, `|` (in that scan order
    // within each position: `&&` and `||` are two-char, checked before the
    // single-char `|`).
    let mut segments = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '&' && chars[i + 1] == '&' {
            segments.push(current.clone());
            current.clear();
            i += 2;
        } else if i + 1 < chars.len() && chars[i] == '|' && chars[i + 1] == '|' {
            segments.push(current.clone());
            current.clear();
            i += 2;
        } else if chars[i] == ';' {
            segments.push(current.clone());
            current.clear();
            i += 1;
        } else if chars[i] == '|' {
            segments.push(current.clone());
            current.clear();
            i += 1;
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }
    segments.push(current);
    segments
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tools approval::tests::split_segments`
Expected: PASS

- [ ] **Step 5: Add the `Pattern` enum and `builtin_defaults`**

Add to `crates/zoid-tools/src/approval.rs` (above `split_segments`):

```rust
/// A structured dangerous-command pattern. Patterns are matched against each
/// segment's token stream (or raw text for `Substring`).
#[derive(Debug, Clone)]
enum Pattern {
    /// Leading program must be exactly `prog` (e.g. "sudo", "systemctl").
    LeadingProgram { prog: String },
    /// Leading program `prog` with any of `trigger_flags` present in the
    /// token stream (e.g. curl with -X POST, -d, --data).
    ProgramWithAnyFlag { prog: String, trigger_flags: Vec<String> },
    /// Leading program `prog` with at least one flag from each of `flag_groups`
    /// present. Used when two independent flag dimensions must both be
    /// satisfied (e.g. rm needs recursive AND force).
    ProgramWithAllGroups { prog: String, flag_groups: Vec<Vec<String>> },
    /// Free-form substring match against the segment's raw text
    /// (e.g. "terraform apply", "kubectl delete").
    Substring { pattern: String },
}

impl Pattern {
    /// A human-readable label for the matched pattern (used in denial messages).
    fn label(&self) -> String {
        match self {
            Pattern::LeadingProgram { prog } => prog.clone(),
            Pattern::ProgramWithAnyFlag { prog, .. } => prog.clone(),
            Pattern::ProgramWithAllGroups { prog, .. } => prog.clone(),
            Pattern::Substring { pattern } => pattern.clone(),
        }
    }
}

/// The curated builtin dangerous patterns (all 6 categories from the design).
/// User `shell_danger` additions are appended; user `shell_allow` exemptions
/// remove matching builtin patterns.
fn builtin_defaults() -> Vec<Pattern> {
    vec![
        // Destructive rm: recursive AND force
        Pattern::ProgramWithAllGroups {
            prog: "rm".into(),
            flag_groups: vec![
                vec!["-r".into(), "--recursive".into()],
                vec!["-f".into(), "--force".into()],
            ],
        },
        // Force-push / history rewrite — match only when `git push` has a force
        // flag. Using ProgramWithAllGroups avoids false positives on `git commit -f`
        // (fixup shorthand) and `git fetch -f` (neither has `push` as a token).
        Pattern::ProgramWithAllGroups {
            prog: "git".into(),
            flag_groups: vec![
                vec!["push".into()],
                vec!["--force".into(), "-f".into(), "--force-with-lease".into()],
            ],
        },
        // Network/prod writes (curl with non-GET method or data)
        Pattern::ProgramWithAnyFlag {
            prog: "curl".into(),
            trigger_flags: vec!["-X".into(), "--data".into(), "-d".into()],
        },
        Pattern::ProgramWithAnyFlag {
            prog: "wget".into(),
            trigger_flags: vec!["--post-data".into(), "--post-file".into()],
        },
        // Privilege escalation
        Pattern::LeadingProgram { prog: "sudo".into() },
        Pattern::LeadingProgram { prog: "su".into() },
        Pattern::LeadingProgram { prog: "doas".into() },
        // System mutation
        Pattern::LeadingProgram { prog: "systemctl".into() },
        Pattern::LeadingProgram { prog: "apt".into() },
        Pattern::LeadingProgram { prog: "brew".into() },
        Pattern::ProgramWithAllGroups {
            prog: "pip".into(),
            flag_groups: vec![vec!["install".into()], vec!["--user".into()]],
        },
        Pattern::Substring { pattern: "chmod -R".into() },
        Pattern::Substring { pattern: "/etc/".into() },
        // Deploy / irrecoverable
        Pattern::Substring { pattern: "terraform apply".into() },
        Pattern::Substring { pattern: "kubectl delete".into() },
        Pattern::Substring { pattern: "fly deploy".into() },
        Pattern::Substring { pattern: "scp".into() },
        Pattern::Substring { pattern: "rsync".into() },
    ]
}
```

Note: the `git push --force` pattern uses `ProgramWithAllGroups` to require both `push` and a force flag — this avoids false positives on `git commit -f` (fixup shorthand) and `git fetch -f`. The curl `-X` flag needs special handling in the matcher (Task 3): `-X` alone isn't dangerous (it could be `-X GET`), so the matcher checks that the token after `-X` is a non-GET method, handling both `-X POST` (space-separated) and `-XPOST` (joined) forms.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p zoid-tools`
Expected: compiles (Pattern and builtin_defaults are unused for now, but that's OK)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tools/src/approval.rs
git commit -m "feat: add chain-splitter, Pattern enum, and builtin dangerous patterns

split_segments splits on &&/||/;/| (|| before |). Pattern enum covers
LeadingProgram, ProgramWithAnyFlag, ProgramWithAllGroups, and Substring.
builtin_defaults returns the 6-category curated pattern list."
```

---

### Task 3: Implement the segment matcher

**Files:**
- Modify: `crates/zoid-tools/src/approval.rs`
- Test: `crates/zoid-tools/src/approval.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `fn match_dangerous(cmd: &str, patterns: &[Pattern]) -> Option<String>` — pure function returning the label of the first matched pattern, or `None`.

- [ ] **Step 1: Write failing tests for `match_dangerous`**

Add to the `tests` module in `crates/zoid-tools/src/approval.rs`:

```rust
    #[test]
    fn match_dangerous_rm_rf() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("rm -rf /", &patterns).is_some());
        assert!(match_dangerous("rm -r -f /", &patterns).is_some());
        assert!(match_dangerous("rm --recursive --force ~", &patterns).is_some());
        // rm without both recursive AND force is safe
        assert!(match_dangerous("rm file.txt", &patterns).is_none());
        assert!(match_dangerous("rm -r dist/", &patterns).is_none());
        assert!(match_dangerous("rm -f tempfile", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_git_force_push() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("git push --force", &patterns).is_some());
        assert!(match_dangerous("git push -f origin main", &patterns).is_some());
        assert!(match_dangerous("git push --force-with-lease", &patterns).is_some());
        // git push without force is safe
        assert!(match_dangerous("git push origin main", &patterns).is_none());
        // git log is safe
        assert!(match_dangerous("git log --oneline", &patterns).is_none());
        // git commit -f (fixup shorthand) is NOT a force push — must not match
        assert!(match_dangerous("git commit -f 123abc", &patterns).is_none());
        // git fetch -f is not a force push
        assert!(match_dangerous("git fetch -f", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_curl_post() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("curl -X POST localhost", &patterns).is_some());
        assert!(match_dangerous("curl -d 'data' localhost", &patterns).is_some());
        // curl -XPOST (no space, joined) must also match
        assert!(match_dangerous("curl -XPOST localhost", &patterns).is_some());
        assert!(match_dangerous("curl -XPUT localhost", &patterns).is_some());
        // curl -XGET (GET method) is safe
        assert!(match_dangerous("curl -XGET localhost", &patterns).is_none());
        assert!(match_dangerous("curl -X GET localhost", &patterns).is_none());
        // curl GET (no -X) is safe
        assert!(match_dangerous("curl localhost", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_sudo() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("sudo apt update", &patterns).is_some());
        assert!(match_dangerous("su root", &patterns).is_some());
    }

    #[test]
    fn match_dangerous_deploy() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("terraform apply", &patterns).is_some());
        assert!(match_dangerous("kubectl delete pod foo", &patterns).is_some());
        assert!(match_dangerous("fly deploy", &patterns).is_some());
    }

    #[test]
    fn match_dangerous_chained() {
        let patterns = builtin_defaults();
        // echo hi && rm -rf / — second segment should match
        assert!(match_dangerous("echo hi && rm -rf /", &patterns).is_some());
        // both segments safe
        assert!(match_dangerous("git log | grep foo", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_quoted_rm_is_safe() {
        let patterns = builtin_defaults();
        // echo "rm -rf /" — shlex tokenizes leading program as echo, not rm
        assert!(match_dangerous("echo \"rm -rf /\"", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_unparseable_prompts() {
        let patterns = builtin_defaults();
        // Unbalanced quotes → shlex fails → fail-safe: treat as dangerous
        assert!(match_dangerous("echo 'unterminated", &patterns).is_some());
    }

    #[test]
    fn match_dangerous_safe_commands() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("ls -la", &patterns).is_none());
        assert!(match_dangerous("cargo build", &patterns).is_none());
        assert!(match_dangerous("echo hello", &patterns).is_none());
        assert!(match_dangerous("grep 'force' file", &patterns).is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tools approval::tests::match_dangerous`
Expected: FAIL — `match_dangerous` not defined

- [ ] **Step 3: Implement `match_dangerous`**

Add to `crates/zoid-tools/src/approval.rs` (above the test module):

```rust
/// Check whether a command string matches any dangerous pattern. Splits on
/// chain operators, tokenizes each segment with shlex, then matches against
/// the pattern list. Returns the label of the first matched pattern, or `None`.
/// Fail-safe: an unparseable segment is treated as dangerous (prompt).
fn match_dangerous(cmd: &str, patterns: &[Pattern]) -> Option<String> {
    for segment in split_segments(cmd) {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(label) = match_segment(trimmed, patterns) {
            return Some(label);
        }
    }
    None
}

/// Match a single command segment against the pattern list. Returns the label
/// of the first matching pattern, or `None`.
fn match_segment(segment: &str, patterns: &[Pattern]) -> Option<String> {
    // shlex-tokenize the segment. On failure → fail-safe: treat as dangerous.
    let tokens = match shlex::split(segment) {
        Some(t) if !t.is_empty() => t,
        _ => return Some(format!("unparseable: {}", segment)),
    };
    let leading = &tokens[0];

    for pattern in patterns {
        if pattern_matches(pattern, leading, &tokens, segment) {
            return Some(pattern.label());
        }
    }
    None
}

/// Check whether a single pattern matches. `leading` is tokens[0]; `segment`
/// is the raw segment text (for Substring matching).
fn pattern_matches(pattern: &Pattern, leading: &str, tokens: &[String], segment: &str) -> bool {
    match pattern {
        Pattern::LeadingProgram { prog } => leading == prog,

        Pattern::ProgramWithAnyFlag { prog, trigger_flags } => {
            if leading != prog {
                return false;
            }
            for flag in trigger_flags {
                if flag == "-X" {
                    // -X is dangerous only if followed by a non-GET method.
                    // Handle both `curl -X POST` (space-separated) and
                    // `curl -XPOST` (joined) forms.
                    for (i, tok) in tokens.iter().enumerate() {
                        // Space-separated: `-X` then next token is the method.
                        if tok == "-X" && i + 1 < tokens.len() {
                            let method = tokens[i + 1].to_uppercase();
                            if method != "GET" {
                                return true;
                            }
                        }
                        // Joined: `-XPOST` — the method is everything after `-X`.
                        if tok.starts_with("-X") && tok.len() > 2 {
                            let method = tok[2..].to_uppercase();
                            if method != "GET" {
                                return true;
                            }
                        }
                    }
                } else if tokens.contains(flag) {
                    return true;
                }
            }
            false
        }

        Pattern::ProgramWithAllGroups { prog, flag_groups } => {
            if leading != prog {
                return false;
            }
            flag_groups.iter().all(|group| {
                group.iter().any(|flag| tokens.contains(flag))
            })
        }

        Pattern::Substring { pattern } => segment.contains(pattern),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tools approval::tests::match_dangerous`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/approval.rs
git commit -m "feat: implement match_dangerous with shlex tokenizer + pattern matcher

Splits on chain operators, shlex-tokenizes each segment, matches the
leading program + flags against structured patterns. -X for curl checks
the following token is a non-GET method. Unparseable segments fail-safe
to dangerous. Substring patterns match against the raw segment text."
```

---

### Task 4: Implement `BlacklistGate::check` with tiering and config interaction

**Files:**
- Modify: `crates/zoid-tools/src/approval.rs`
- Test: `crates/zoid-tools/src/approval.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `BlacklistGate::new(shell_danger, shell_allow, interactive)` — constructs the gate with builtin defaults + user additions - user exemptions. `check()` implements the tiering logic.

- [ ] **Step 1: Write failing tests for `BlacklistGate::check`**

Add to the `tests` module:

```rust
    fn shell_call(cmd: &str) -> zoid_provider::ToolCall {
        zoid_provider::ToolCall {
            id: String::new(),
            name: "shell".into(),
            args: serde_json::json!({ "command": cmd }),
        }
    }

    fn tool_call(name: &str) -> zoid_provider::ToolCall {
        zoid_provider::ToolCall {
            id: String::new(),
            name: name.into(),
            args: serde_json::json!({}),
        }
    }

    #[test]
    fn gate_never_prompt_tier_always_allows() {
        let g = BlacklistGate::new(vec![], vec![], true);
        for name in ["read_file", "search", "recall", "show", "update_tasks", "ask_user"] {
            assert_eq!(g.check(&tool_call(name)), crate::Gate::Allow, "{} must allow", name);
        }
    }

    #[test]
    fn gate_file_writes_allow_by_default() {
        let g = BlacklistGate::new(vec![], vec![], true);
        assert_eq!(g.check(&tool_call("write_file")), crate::Gate::Allow);
        assert_eq!(g.check(&tool_call("edit_file")), crate::Gate::Allow);
    }

    #[test]
    fn gate_safe_shell_allows() {
        let g = BlacklistGate::new(vec![], vec![], true);
        assert_eq!(g.check(&shell_call("ls -la")), crate::Gate::Allow);
    }

    #[test]
    fn gate_dangerous_shell_prompts_when_interactive() {
        let g = BlacklistGate::new(vec![], vec![], true);
        let result = g.check(&shell_call("rm -rf /"));
        assert!(matches!(result, crate::Gate::Prompt { .. }));
    }

    #[test]
    fn gate_dangerous_shell_denies_when_not_interactive() {
        let g = BlacklistGate::new(vec![], vec![], false);
        let result = g.check(&shell_call("rm -rf /"));
        assert!(matches!(result, crate::Gate::Deny(ref r) if r.contains("blocked by safety blacklist")));
    }

    #[test]
    fn gate_shell_danger_adds_custom_pattern() {
        let g = BlacklistGate::new(vec!["make deploy".into()], vec![], true);
        let result = g.check(&shell_call("make deploy"));
        assert!(matches!(result, crate::Gate::Prompt { .. }));
    }

    #[test]
    fn gate_shell_allow_exempts_builtin() {
        // Exempt the force-push pattern: "--force-with-lease" is in the
        // pattern's canonical form, so adding it to shell_allow removes
        // the entire force-push pattern.
        let g = BlacklistGate::new(vec![], vec!["--force-with-lease".into()], true);
        let result = g.check(&shell_call("git push --force-with-lease"));
        assert_eq!(result, crate::Gate::Allow);
        // But --force (without --force-with-lease) still prompts — the whole
        // pattern was exempted, not just the one flag.
        let result2 = g.check(&shell_call("git push --force"));
        assert_eq!(result2, crate::Gate::Allow);
    }

    #[test]
    fn gate_missing_command_arg_allows() {
        let g = BlacklistGate::new(vec![], vec![], true);
        let call = zoid_provider::ToolCall {
            id: String::new(),
            name: "shell".into(),
            args: serde_json::json!({}),
        };
        assert_eq!(g.check(&call), crate::Gate::Allow);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tools approval::tests::gate_`
Expected: FAIL — `BlacklistGate::check` is the stub that always returns Allow

- [ ] **Step 3: Implement `BlacklistGate` with real `check`**

Replace the stub `BlacklistGate` in `crates/zoid-tools/src/approval.rs`:

```rust
/// The blacklist gate. Allow unless a `shell` call matches a dangerous pattern.
/// `interactive: true` returns `Gate::Prompt` on a match (Chat);
/// `interactive: false` returns `Gate::Deny` (subagents — headless, can't prompt).
pub struct BlacklistGate {
    patterns: Vec<Pattern>,
    interactive: bool,
}

impl BlacklistGate {
    /// Construct the gate: builtin defaults ⊕ user `shell_danger` ⊖ user
    /// `shell_allow`. User `shell_danger` entries are added as `Substring`
    /// patterns. User `shell_allow` entries exempt builtin patterns whose
    /// canonical form contains the entry string.
    pub fn new(shell_danger: Vec<String>, shell_allow: Vec<String>, interactive: bool) -> Self {
        let mut patterns: Vec<Pattern> = builtin_defaults()
            .into_iter()
            .filter(|p| !is_exempted(p, &shell_allow))
            .collect();
        // User additions are substring patterns (simplest for users to write).
        for d in shell_danger {
            patterns.push(Pattern::Substring { pattern: d });
        }
        Self { patterns, interactive }
    }
}

/// Check whether a builtin pattern is exempted by any `shell_allow` entry.
/// An entry exempts a pattern if the pattern's canonical form contains the
/// entry string (so `"--force-with-lease"` exempts the force-push pattern
/// whose trigger_flags include that string).
fn is_exempted(pattern: &Pattern, shell_allow: &[String]) -> bool {
    let canonical = pattern_canonical(pattern);
    shell_allow.iter().any(|allow| canonical.contains(allow))
}

/// A canonical string representation of a pattern, for `shell_allow` matching.
fn pattern_canonical(pattern: &Pattern) -> String {
    match pattern {
        Pattern::LeadingProgram { prog } => prog.clone(),
        Pattern::ProgramWithAnyFlag { prog, trigger_flags } => {
            format!("{} {}", prog, trigger_flags.join(" "))
        }
        Pattern::ProgramWithAllGroups { prog, flag_groups } => {
            format!("{} {}", prog, flag_groups.iter().flatten().cloned().collect::<Vec<_>>().join(" "))
        }
        Pattern::Substring { pattern } => pattern.clone(),
    }
}

impl crate::ToolGate for BlacklistGate {
    fn check(&self, call: &zoid_provider::ToolCall) -> crate::Gate {
        // Never-prompt tier: always allow
        match call.name.as_str() {
            "read_file" | "search" | "recall" | "show" | "update_tasks" | "ask_user" => {
                return crate::Gate::Allow;
            }
            _ => {}
        }
        // Allow-by-default tier: write_file, edit_file
        match call.name.as_str() {
            "write_file" | "edit_file" => return crate::Gate::Allow,
            _ => {}
        }
        // Blacklist-gated tier: shell
        if call.name == "shell" {
            let cmd = call
                .args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(label) = match_dangerous(cmd, &self.patterns) {
                if self.interactive {
                    let question = format!(
                        "`shell` calls a dangerous action — approve?\n{}",
                        cmd
                    );
                    return crate::Gate::Prompt {
                        question,
                        choices: vec!["approve once".into(), "deny".into()],
                    };
                } else {
                    return crate::Gate::Deny(format!(
                        "blocked by safety blacklist: matched `{}`",
                        label
                    ));
                }
            }
        }
        crate::Gate::Allow
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tools approval`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/approval.rs
git commit -m "feat: implement BlacklistGate::check with tiering and config interaction

Never-prompt tier (reads/ask_user) always allows. File writes allow by
default. shell is blacklist-gated: interactive=true prompts, false denies.
shell_danger adds Substring patterns; shell_allow exempts builtin patterns
by canonical-form containment matching."
```

---

### Task 5: Add `ApprovalConfig` to the config system

**Files:**
- Modify: `crates/zoid-core/src/config.rs`

**Interfaces:**
- Produces: `pub struct ApprovalConfig { yolo, shell_danger, shell_allow }` — the config section consumed by gate selection.
- Produces: `pub struct PartialApproval` — the TOML deserialization partial.
- Produces: `ApprovalConfig` field on `Config` and `PartialApproval` field on `PartialConfig`.

- [ ] **Step 1: Write failing tests for the approval config section**

Add a new test module at the end of `crates/zoid-core/src/config.rs`:

```rust
#[cfg(test)]
mod approval_config_tests {
    use super::*;

    #[test]
    fn approval_section_parses_and_merges() {
        let (p, _) = parse_toml(
            "[approval]\nyolo = true\nshell_danger = [\"make deploy\"]\nshell_allow = [\"git push --force-with-lease\"]"
        ).unwrap();
        assert_eq!(p.approval.yolo, Some(true));
        assert_eq!(p.approval.shell_danger, Some(vec!["make deploy".to_string()]));
        assert_eq!(p.approval.shell_allow, Some(vec!["git push --force-with-lease".to_string()]));
        let (cfg, _) = merge(&[(Source::UserGlobal, p)]);
        assert!(cfg.approval.yolo);
        assert_eq!(cfg.approval.shell_danger, vec!["make deploy".to_string()]);
        assert_eq!(cfg.approval.shell_allow, vec!["git push --force-with-lease".to_string()]);
    }

    #[test]
    fn approval_defaults_to_safe() {
        let (cfg, _) = merge(&[]);
        assert!(!cfg.approval.yolo);
        assert!(cfg.approval.shell_danger.is_empty());
        assert!(cfg.approval.shell_allow.is_empty());
    }

    #[test]
    fn approval_shell_danger_unions_across_layers() {
        let (user, _) = parse_toml("[approval]\nshell_danger = [\"a\", \"b\"]").unwrap();
        let (proj, _) = parse_toml("[approval]\nshell_danger = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(cfg.approval.shell_danger, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn approval_shell_allow_unions_across_layers() {
        let (user, _) = parse_toml("[approval]\nshell_allow = [\"x\"]").unwrap();
        let (proj, _) = parse_toml("[approval]\nshell_allow = [\"y\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(cfg.approval.shell_allow, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn approval_yolo_last_write_wins() {
        let (user, _) = parse_toml("[approval]\nyolo = true").unwrap();
        let (proj, _) = parse_toml("[approval]\nyolo = false").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert!(!cfg.approval.yolo, "project layer overrides user-global");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-core approval_config_tests`
Expected: FAIL — no `approval` field on `PartialConfig`/`Config`

- [ ] **Step 3: Add `ApprovalConfig` and `PartialApproval`**

In `crates/zoid-core/src/config.rs`, add the structs (near `CompanionConfig`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalConfig {
    pub yolo: bool,
    pub shell_danger: Vec<String>,
    pub shell_allow: Vec<String>,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            yolo: false,
            shell_danger: vec![],
            shell_allow: vec![],
        }
    }
}
```

Add the partial:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialApproval {
    pub yolo: Option<bool>,
    pub shell_danger: Option<Vec<String>>,
    pub shell_allow: Option<Vec<String>>,
}
```

- [ ] **Step 4: Wire into `Config` and `PartialConfig`**

Add `pub approval: ApprovalConfig` to the `Config` struct. Add `pub approval: PartialApproval` to `PartialConfig`.

Update `Config::default()`:

```rust
approval: ApprovalConfig::default(),
```

Add `pub approval: Source` to `Provenance`.

- [ ] **Step 5: Wire into `merge()`**

In the `merge` function's loop body, add:

```rust
        if let Some(v) = p.approval.yolo {
            cfg.approval.yolo = v;
            prov.approval = *src;
        }
        if let Some(dirs) = &p.approval.shell_danger {
            for d in dirs {
                if !cfg.approval.shell_danger.contains(d) {
                    cfg.approval.shell_danger.push(d.clone());
                }
            }
        }
        if let Some(dirs) = &p.approval.shell_allow {
            for d in dirs {
                if !cfg.approval.shell_allow.contains(d) {
                    cfg.approval.shell_allow.push(d.clone());
                }
            }
        }
```

Also update the `Provenance` initialization in `merge()` to include `approval: Source::Default`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p zoid-core approval_config_tests`
Expected: PASS

- [ ] **Step 7: Fix any other code that constructs `Config` or `Provenance` literally**

Search for `Config {` and `Provenance {` in the codebase and add the new `approval` field. Key locations:
- `main.rs` `test_app()` constructs `Config::default()` (no change needed — uses Default)
- `main.rs` `test_app()` constructs `Provenance { ... }` literally — add `approval: Source::Default`

Run: `cargo check -p zoid`
Expected: compile errors at any literal `Provenance { ... }` — add the field. Repeat until clean.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-core/src/config.rs crates/zoid/src/main.rs
git commit -m "feat: add [approval] config section (yolo, shell_danger, shell_allow)

ApprovalConfig + PartialApproval with merge semantics: yolo is
last-write-wins, shell_danger/shell_allow are union-dedup across layers.
Provenance tracks the yolo source."
```

---

### Task 6: Add `--yolo` CLI flag

**Files:**
- Modify: `crates/zoid/src/cli.rs`

**Interfaces:**
- Produces: `Cli::Run { ..., yolo: bool }` — the parsed CLI now carries a yolo flag.

- [ ] **Step 1: Write failing tests for `--yolo`**

Add to the `tests` module in `crates/zoid/src/cli.rs`:

```rust
    #[test]
    fn parses_yolo_flag() {
        assert_eq!(
            super::parse_args(vec!["--yolo".to_string()]),
            super::Cli::Run { companion: false, new: false, resume: None, yolo: true }
        );
    }

    #[test]
    fn yolo_combines_with_companion() {
        assert_eq!(
            super::parse_args(vec!["--companion".to_string(), "--yolo".to_string()]),
            super::Cli::Run { companion: true, new: false, resume: None, yolo: true }
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid cli::tests::parses_yolo`
Expected: FAIL — no `yolo` field on `Cli::Run`

- [ ] **Step 3: Add `yolo` to `Cli::Run` and parse `--yolo`**

Update the `Cli::Run` variant:

```rust
    Run {
        companion: bool,
        new: bool,
        resume: Option<String>,
        yolo: bool,
    },
```

Add parsing in `parse_args` (add `let mut yolo = false;` and the match arm `"--yolo" => yolo = true,`). Update the return to include `yolo`.

- [ ] **Step 4: Update help text**

Add to `help_text()`:

```
    zoid --yolo              Disable all approval prompts (dangerous)
```

- [ ] **Step 5: Update all existing `Cli::Run { ... }` literals and tests**

Every existing `Cli::Run { companion, new, resume }` in tests must become `Cli::Run { companion, new, resume, yolo: false }`. Search for `Cli::Run {` in `cli.rs` tests and update each one.

- [ ] **Step 6: Update `main.rs` to extract `yolo` from the parsed CLI**

In `main.rs`, the `Cli::Run` match arm currently destructures `{ companion, new, resume }`. Add `yolo`:

```rust
        zoid::cli::Cli::Run { companion, new, resume, yolo } => {
```

Store `yolo` so `spawn_turn` can use it. Add a field to `App`:

```rust
    /// Whether YOLO mode is active (no approval prompts). Resolved from
    /// config + CLI: `config.approval.yolo || cli --yolo`.
    yolo: bool,
```

Set it when constructing `App`:

```rust
yolo: config.approval.yolo || yolo,
```

Note: `config` is loaded after the CLI parse, so the `yolo` from CLI is in scope. The App field resolves the OR.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p zoid cli`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/cli.rs crates/zoid/src/main.rs
git commit -m "feat: add --yolo CLI flag

Disables all approval prompts. Resolved as config.approval.yolo || cli.yolo.
Stored on App so spawn_turn can select AllowAll vs BlacklistGate."
```

---

### Task 7: Wire gate selection in `spawn_turn` (Chat)

**Files:**
- Modify: `crates/zoid/src/main.rs`

**Interfaces:**
- Consumes: `BlacklistGate::new(shell_danger, shell_allow, interactive)` from Task 4, `ApprovalConfig` from Task 5, `App.yolo` from Task 6.

- [ ] **Step 1: Replace `AllowAll` in `spawn_turn` with gate selection**

In `crates/zoid/src/main.rs`, `spawn_turn` function, find:

```rust
            std::sync::Arc::new(zoid_tools::AllowAll),
```

Replace with:

```rust
            if app.yolo {
                std::sync::Arc::new(zoid_tools::AllowAll) as std::sync::Arc<dyn zoid_tools::ToolGate>
            } else {
                std::sync::Arc::new(zoid_tools::BlacklistGate::new(
                    app.config.approval.shell_danger.clone(),
                    app.config.approval.shell_allow.clone(),
                    true, // interactive — Chat prompts
                )) as std::sync::Arc<dyn zoid_tools::ToolGate>
            },
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p zoid`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat: wire BlacklistGate into Chat spawn_turn

YOLO → AllowAll; otherwise BlacklistGate with interactive=true (prompts
on dangerous shell matches). Config shell_danger/shell_allow thread in."
```

---

### Task 8: Implement the `Gate::Prompt` arm in the agent loop

**Files:**
- Modify: `crates/zoid/src/agent.rs`

**Interfaces:**
- Consumes: `Gate::Prompt { question, choices }` from Task 1, the existing `AgentUpdate::AskUser` + `oneshot` path, the existing `QuestionAsked`/`QuestionAnswered` events.

- [ ] **Step 1: Write failing integration test for `Gate::Prompt` approve path**

Add to `crates/zoid/tests/agent_loop.rs`:

```rust
/// A gate that returns Gate::Prompt for shell calls with a dangerous command,
/// Allow otherwise.
struct PromptGate;
impl zoid_tools::ToolGate for PromptGate {
    fn check(&self, c: &zoid_provider::ToolCall) -> zoid_tools::Gate {
        if c.name == "shell" {
            let cmd = c.args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd.contains("rm -rf") {
                return zoid_tools::Gate::Prompt {
                    question: format!("approve? {}", cmd),
                    choices: vec!["approve once".into(), "deny".into()],
                };
            }
        }
        zoid_tools::Gate::Allow
    }
}

#[tokio::test]
async fn gate_prompt_approve_runs_tool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proof.txt");
    let path_str = path.to_str().unwrap().to_string();
    // The "dangerous" command writes a file (safe for testing).
    let cmd = format!("rm -rf /tmp/nonexistent && echo hi > {}", path_str);

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "shell".into(),
                    args: json!({ "command": cmd }),
                }),
                ProviderEvent::Done,
            ],
            vec![ProviderEvent::TextDelta("done".into()), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    // Drain UI updates, auto-approving any AskUser prompt.
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            match u {
                AgentUpdate::AskUser { reply, .. } => {
                    let _ = reply.send(zoid::agent::Answer::Choice("approve once".into()));
                }
                AgentUpdate::TurnComplete => complete = true,
                _ => {}
            }
        }
        complete
    });

    run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(PromptGate),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();

    let complete = drain.await.unwrap();
    assert!(complete, "loop must emit TurnComplete");
    // The shell tool ran (the file was created via the approved command).
    assert!(path.exists(), "approved command must have executed");
}

#[tokio::test]
async fn gate_prompt_deny_blocks_tool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("must_not_exist.txt");
    let path_str = path.to_str().unwrap().to_string();
    let cmd = format!("rm -rf /tmp/nonexistent && echo hi > {}", path_str);

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "shell".into(),
                    args: json!({ "command": cmd }),
                }),
                ProviderEvent::Done,
            ],
            vec![ProviderEvent::TextDelta("ok".into()), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            match u {
                AgentUpdate::AskUser { reply, .. } => {
                    let _ = reply.send(zoid::agent::Answer::Choice("deny".into()));
                }
                AgentUpdate::TurnComplete => complete = true,
                _ => {}
            }
        }
        complete
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(PromptGate),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();

    let _ = drain.await;
    assert!(!path.exists(), "denied command must not execute");
    // A ToolResult with an error should be in the log (the denial reason).
    assert!(events.iter().any(|e| {
        matches!(&e.kind, EventKind::ToolResult { is_error, .. } if *is_error)
    }), "denied prompt must produce an error ToolResult");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid gate_prompt_approve_runs_tool`
Expected: FAIL — the `Gate::Prompt` arm doesn't exist yet; the `Gate::Deny` check site only handles `Deny`

- [ ] **Step 3: Implement the `Gate::Prompt` arm in `agent.rs`**

In `crates/zoid/src/agent.rs`, find the gate check site (around line 682):

```rust
            if let Gate::Deny(reason) = gate.check(&tc) {
                let reason_msg = reason.clone();
                emit(
                    &session,
                    &mut events,
                    ui,
                    &config.branch,
                    EventKind::ToolResult {
                        id: tc.id,
                        name: tc.name,
                        output: reason,
                        is_error: true,
                    },
                    session_id,
                    now,
                )
                .await?;
                tracing::info!(
                    kind = "tool",
                    name = tool_name.as_str(),
                    ms = tool_start.elapsed().as_millis() as u64,
                    ok = false,
                    "tool executed"
                );
                let ctx = format!("tool {tool_name}");
                tracing::warn!(
                    ctx = ctx.as_str(),
                    message = reason_msg.as_str(),
                    "tool failed"
                );
                continue;
            }
```

Replace with a `match` on all three `Gate` variants. The `Gate::Allow` arm is empty (falls through to dispatch). The `Gate::Deny` arm is the existing code. The `Gate::Prompt` arm reuses the `ask_user` oneshot path:

```rust
            match gate.check(&tc) {
                Gate::Allow => { /* fall through to dispatch */ }
                Gate::Deny(reason) => {
                    let reason_msg = reason.clone();
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: reason,
                            is_error: true,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = tool_name.as_str(),
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = false,
                        "tool executed"
                    );
                    let ctx = format!("tool {tool_name}");
                    tracing::warn!(
                        ctx = ctx.as_str(),
                        message = reason_msg.as_str(),
                        "tool failed"
                    );
                    continue;
                }
                Gate::Prompt { question, choices } => {
                    // Reuse the ask_user park-and-await path: emit a
                    // QuestionAsked event, send AgentUpdate::AskUser, await
                    // the reply. Approve → fall through to dispatch. Deny →
                    // error ToolResult + continue. Esc → abort the turn.
                    // QuestionKind::Ask is intentional — the TUI already
                    // handles it as a plain question card (same as ask_user).
                    // No new QuestionKind variant is needed.
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAsked {
                            id: tc.id.clone(),
                            kind: zoid_core::event::QuestionKind::Ask,
                            question: question.clone(),
                            choices: choices.clone(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    let (rtx, rrx) = oneshot::channel::<Answer>();
                    let sent = ui
                        .send(AgentUpdate::AskUser {
                            question,
                            choices,
                            reply: rtx,
                        })
                        .await;
                    if sent.is_err() {
                        // UI channel closed — treat as abort.
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::ToolResult {
                                id: tc.id,
                                name: tc.name,
                                output: "[user aborted]".to_string(),
                                is_error: true,
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        outcome = "aborted";
                        break 'turn;
                    }
                    let ans = rrx.await;
                    let (output, is_error, approved) = match ans {
                        Ok(Answer::Choice(s)) => {
                            let approved = s == "approve once";
                            if approved {
                                (s, false, true)
                            } else {
                                (s, true, false)
                            }
                        }
                        Ok(Answer::FreeText(s)) => (s, false, true),
                        Ok(Answer::LetYouDecide) => ("[let you decide]".to_string(), false, true),
                        Err(_) => ("[user aborted]".to_string(), true, false),
                    };
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAnswered {
                            id: tc.id.clone(),
                            answer: output.clone(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    if !approved {
                        // Deny or abort: feed an error ToolResult back.
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::ToolResult {
                                id: tc.id,
                                name: tc.name,
                                output,
                                is_error,
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        if is_error && output == "[user aborted]" {
                            // Esc: drain remaining tool calls and abort the turn
                            // (same as the ask_user abort path).
                            for rest in pending_iter.by_ref() {
                                emit(
                                    &session,
                                    &mut events,
                                    ui,
                                    &config.branch,
                                    EventKind::ToolResult {
                                        id: rest.id,
                                        name: rest.name,
                                        output: "[skipped: turn aborted]".to_string(),
                                        is_error: false,
                                    },
                                    session_id,
                                    now,
                                )
                                .await?;
                            }
                            outcome = "aborted";
                            break 'turn;
                        }
                        continue;
                    }
                    // Approved: fall through to normal dispatch below.
                    tracing::info!(
                        kind = "tool",
                        name = tool_name.as_str(),
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "tool approved"
                    );
                }
            }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid gate_prompt_`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/tests/agent_loop.rs
git commit -m "feat: implement Gate::Prompt arm in the agent loop

Reuses the ask_user oneshot + AgentUpdate::AskUser park-and-await path.
Approve → fall through to dispatch. Deny → error ToolResult + continue
(model can try a different approach). Esc → abort the turn (drain
remaining tool calls). Integration tests cover approve and deny paths."
```

---

### Task 9: Wire subagent auto-deny

**Files:**
- Modify: `crates/zoid/src/subagent.rs`
- Modify: `crates/zoid/src/spawn_subagent.rs`
- Modify: `crates/zoid/src/agent.rs` (the `dispatch_subagent` arm that calls `spawn_subagent`)

**Interfaces:**
- Consumes: `BlacklistGate::new(shell_danger, shell_allow, false)` from Task 4, `ApprovalConfig` from Task 5.

- [ ] **Step 1: Add `approval` param to `run_subagent`**

In `crates/zoid/src/subagent.rs`, add `approval: zoid_core::config::ApprovalConfig` to the `run_subagent` signature. Update the doc comment to note it carries the approval config for gate selection.

- [ ] **Step 2: Replace `AllowAll` in `run_subagent` with gate selection**

Find in `run_subagent`:

```rust
        std::sync::Arc::new(zoid_tools::AllowAll),
```

Replace with:

```rust
        let gate: std::sync::Arc<dyn zoid_tools::ToolGate> = if approval.yolo {
            std::sync::Arc::new(zoid_tools::AllowAll)
        } else {
            std::sync::Arc::new(zoid_tools::BlacklistGate::new(
                approval.shell_danger.clone(),
                approval.shell_allow.clone(),
                false, // interactive = false → Gate::Deny, not Gate::Prompt
            ))
        };
```

Then pass `gate` instead of `std::sync::Arc::new(zoid_tools::AllowAll)` to `run_agent_turn`.

- [ ] **Step 3: Thread `approval` through `spawn_subagent`**

In `crates/zoid/src/spawn_subagent.rs`, add `approval: zoid_core::config::ApprovalConfig` to `spawn_subagent`'s params. Pass it to `run_subagent`.

- [ ] **Step 4: Thread `approval` through the `dispatch_subagent` call site**

In `crates/zoid/src/agent.rs`, find the `dispatch_subagent` arm (around line 941) that calls `crate::spawn_subagent::spawn_subagent`. Add `app` config thread — but this is in the agent loop which doesn't have direct access to `App`. The `run_subagent` call is in `spawn_subagent::spawn_subagent` which is called from `run_turn_inner`. The approval config needs to be passed through `TurnConfig` or as an explicit parameter.

The cleanest approach: add `approval: zoid_core::config::ApprovalConfig` to `TurnConfig` so it's available in `run_turn_inner` where `dispatch_subagent` is handled.

In `crates/zoid/src/agent.rs`, add to `TurnConfig`:

```rust
    /// Approval config for gate selection. Subagents use the blacklist with
    /// interactive=false (auto-deny) unless yolo.
    pub approval: zoid_core::config::ApprovalConfig,
```

Update `chat_turn_config_with` and `chat_turn_config` to set `approval: zoid_core::config::ApprovalConfig::default()` (Chat uses the real gate from `spawn_turn`, not from TurnConfig — the TurnConfig value is only consumed by the `dispatch_subagent` path for subagents).

Wait — actually, the Chat gate is selected in `spawn_turn` (main.rs), not from TurnConfig. The `dispatch_subagent` arm inside `run_turn_inner` needs the approval config to pass to `spawn_subagent`. Since `TurnConfig` is already passed to `run_turn_inner`, adding `approval` there is the right path.

In the `dispatch_subagent` arm, pass `config.approval.clone()` to `spawn_subagent`.

- [ ] **Step 5: Update `spawn_turn` in `main.rs` to set `TurnConfig.approval`**

In `spawn_turn`, add:

```rust
    turn_config.approval = app.config.approval.clone();
```

- [ ] **Step 6: Update all `chat_turn_config()` / `chat_turn_config_with()` callers in tests**

Every test that calls `chat_turn_config()` or constructs a `TurnConfig` literally needs the new `approval` field. Since `chat_turn_config()` sets it to `ApprovalConfig::default()`, most tests don't need changes. But any literal `TurnConfig { ... }` construction needs the field added.

Search for `TurnConfig {` in the codebase and add `approval: zoid_core::config::ApprovalConfig::default()` to each.

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p zoid`
Expected: compiles (fix any literal TurnConfig constructions)

- [ ] **Step 8: Run all tests**

Run: `cargo test -p zoid`
Expected: PASS (existing tests should be unaffected — they use `AllowAll` as the gate directly, not via TurnConfig)

- [ ] **Step 9: Run full workspace tests**

Run: `cargo test --workspace`
Expected: PASS — verifies no cross-crate integration issues (zoid-tools + zoid-core + zoid all compile and test together).

- [ ] **Step 10: Commit**

```bash
git add crates/zoid/src/subagent.rs crates/zoid/src/spawn_subagent.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat: wire subagent auto-deny with BlacklistGate

Subagents get BlacklistGate with interactive=false (auto-deny on
dangerous matches) unless yolo. ApprovalConfig threads through TurnConfig
to the dispatch_subagent arm. Chat's gate is set in spawn_turn (Task 7)."
```

---

### Task 10: Update the TODO and APPROVALS docs

**Files:**
- Modify: `docs/TODO.md`
- Modify: `docs/APPROVALS.md`

- [ ] **Step 1: Update `docs/TODO.md`**

Change the approvals entry from a pointer to a "DONE" entry (matching the empty-state guidance entry's format):

```markdown
## Tool-call approvals (DONE)

Implemented across `crates/zoid-tools/src/approval.rs` (BlacklistGate +
shlex matcher), `crates/zoid/src/agent.rs` (Gate::Prompt arm), and config/CLI
wiring. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`.
```

- [ ] **Step 2: Update `docs/APPROVALS.md` status line**

Change the status to "implemented" with a pointer to the spec and plan.

- [ ] **Step 3: Commit**

```bash
git add docs/TODO.md docs/APPROVALS.md
git commit -m "docs: mark approvals as implemented"
```

---

## Self-Review

**Spec coverage:**
1. ✅ `Gate::Prompt` variant + agent-loop integration — Task 1 (enum), Task 8 (agent loop)
2. ✅ `BlacklistGate` with builtin patterns + shlex tokenizer — Tasks 2-4
3. ✅ Config `[approval]` section — Task 5
4. ✅ CLI `--yolo` flag — Task 6
5. ✅ Subagent auto-deny wrapper — Task 9
6. ✅ Gate selection (YOLO vs BlacklistGate) — Task 7 (Chat), Task 9 (subagent)

**Placeholder scan:** No TBDs, no "add appropriate error handling", no "similar to Task N". All code blocks contain actual implementation.

**Type consistency:** `BlacklistGate::new(shell_danger: Vec<String>, shell_allow: Vec<String>, interactive: bool)` — consistent across Tasks 4, 7, 9. `Gate::Prompt { question: String, choices: Vec<String> }` — consistent across Tasks 1, 8. `ApprovalConfig { yolo, shell_danger, shell_allow }` — consistent across Tasks 5, 7, 9.