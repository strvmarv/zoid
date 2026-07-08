# Tool-call approvals — design

> **Status:** design (settled, ready for implementation planning). This document
> supersedes the earlier `docs/APPROVALS.md` design sketch; the TODO entry
> pointing there is resolved by this spec. Implementation should follow this
> document and update it.

## Goal

Add an approval layer that prompts the user before the agent runs a genuinely
dangerous action, while keeping prompts **as rare as possible** — every prompt
must be meaningful, because a prompt that fires on something harmless trains
the user to ⏎-through it, which makes the gate useless precisely when a
`rm -rf` shows up.

> **Guiding principle.** A prompt should only ever block an action that, if
> approved wrongly, **can't be undone from inside zoid** — it reaches outside
> the working directory, is irreversible, or is outward-facing. In-sandbox
> mutations (file writes, edits, `cargo build`, tests) are git-recoverable and
> do not prompt.

## Scope

All five pieces of the settled design are implemented in one pass:

1. `Gate::Prompt` variant + agent-loop integration (reusing the `ask_user` overlay)
2. `BlacklistGate` with builtin dangerous-shell patterns + shlex-based tokenizer
3. Config `[approval]` section (`yolo`, `shell_danger`, `shell_allow`)
4. CLI `--yolo` flag
5. Subagent auto-deny wrapper (headless can't prompt)

These pieces are tightly coupled — the gate-selection logic (`if yolo { AllowAll }
else { BlacklistGate }`) is the natural home for config and CLI wiring, and YOLO
is a one-line branch in that logic. Splitting them would mean implementing the
selection logic twice (once hardcoded, once real).

## Current state (the seam)

The approval machinery already exists as a tested seam:

- **`Gate` enum** (`crates/zoid-tools/src/lib.rs`) — currently `Allow |
  Deny(String)`. The doc comment is explicit about intent: *"this is the
  insertion point where interactive tool approval will later live (an `ask_user`
  prompt gating `Deny`)."*

- **`ToolGate` trait** (same file) — `fn check(&self, call: &ToolCall) -> Gate`.
  Consulted once per pending tool call, immediately before dispatch.

- **`AllowAll`** — the only shipped impl; allows everything.

- **Check site** (`crates/zoid/src/agent.rs`, inside `run_turn_inner`'s per-call
  loop, ~line 682): `gate.check(&tc)` runs before each pending tool call.
  `Deny(reason)` emits an error `ToolResult` with that reason and `continue`s.
  `Allow` falls through to kind-dispatch.

- **Wiring** — production Chat (`main.rs`) and subagents (`subagent.rs`) both
  hardcode `Arc::new(AllowAll)`.

- **Test coverage** (`crates/zoid/tests/agent_loop.rs`):
  `gate_deny_blocks_tool_and_feeds_reason_back` proves the Deny path.

- **Interactive half (built, not yet reused)** — `ask_user`
  (`ToolKind::Interactive`) parks the loop on a `oneshot`, sends
  `AgentUpdate::AskUser { question, choices, reply }` to the UI, and resumes on
  the user's answer. The question overlay is built and snapshotted. `Gate::Prompt`
  reuses this exact mechanism — no new UI.

- **Config** — `Config` (`crates/zoid-core/src/config.rs`) has no approval
  section today. This design adds one.

- **CLI** — hand-rolled parser in `crates/zoid/src/cli.rs`. Three flags
  (`--companion`, `--new`, `--resume`). This design adds `--yolo`.

## Tiering (by danger, not by call frequency)

- **Never prompt** — `read_file`, `search`, `recall`, `show`, `update_tasks`,
  `ask_user`: always allow. No side effects; prompting would be noise (and
  prompting to approve `ask_user` would be recursive/absurd).
- **Allow by default** — `write_file`, `edit_file`: allow; (future: opt-in
  prompt). Git-recoverable, fires constantly — prompting here would train
  ⏎-through.
- **Blacklist-gated** — `shell`: allow **unless** command matches a dangerous
  pattern → prompt. `shell` is the only escape hatch (arbitrary code: `rm -rf`,
  `git push --force`, `curl` to prod, `sudo`…).

In practice: **file writes never prompt; `shell` prompts only on a dangerous
match.** That's the minimal-prompt design.

## Section 1: `Gate::Prompt` + agent-loop integration

### The `Gate` enum change

`zoid-tools/src/lib.rs`:

```rust
pub enum Gate {
    Allow,
    Deny(String),
    Prompt { question: String, choices: Vec<String> },  // new
}
```

`check` stays synchronous — it can only return the *intent* to prompt. The
agent loop handles the actual suspension.

### The agent-loop change

`agent.rs`, at the existing check site (~line 682). Today:

```rust
if let Gate::Deny(reason) = gate.check(&tc) { ... continue; }
```

Becomes a `match` on all three variants:

- `Gate::Allow` — falls through to dispatch (unchanged).
- `Gate::Deny(reason)` — feeds the reason back as an error `ToolResult`,
  `continue` (unchanged).
- `Gate::Prompt { question, choices }` — reuses the exact same `oneshot` +
  `AgentUpdate::AskUser` park-and-await path that `ask_user` already uses:
  1. Emit a `QuestionAsked` event so the prompt renders inline (same as
     `ask_user`).
  2. Send `AgentUpdate::AskUser { question, choices, reply }` on the UI channel.
  3. `rrx.await` — the loop parks here until the user answers.
  4. `Answer::Choice("approve once")` → fall through to normal dispatch.
  5. `Answer::Choice("deny")` → emit an error `ToolResult` with the denial
     reason (same as the `Deny` path), `continue`.
  6. `Err` (Esc / oneshot dropped) → abort the turn, drain remaining pending tool
     calls (same as `ask_user`'s abort path).

### Prompt vs deny semantics

An approval *deny* does NOT abort the turn — the model sees the denial as an
error `ToolResult` and can choose a different approach. Only Esc (the oneshot
`Err`) aborts the turn, matching `ask_user`'s existing behavior.

### What a prompt looks like

The existing question overlay, e.g.:

> **`shell` calls a dangerous action — approve?**
> `rm -rf node_modules/ ..` *(the actual command, shown)*
> [approve once] [deny]

`approve once` (not "always allow this pattern") — keeping it one-shot means
every dangerous call is independently eyeballed, which is the whole point.

The user-facing escape valve for persistent false positives is the `shell_allow`
config field (see Section 3), which requires deliberately editing a config file
— the right amount of friction for silencing a safety check.

### Why no "always allow" in-session button

An in-session "always allow" that auto-writes to `shell_allow` was considered and
rejected:

- It defeats the core design principle — a "stop bothering me" button trains
  the user to click without reading, which makes the gate useless when a
  genuinely dangerous command appears.
- Auto-writing config is a surprising side effect — the user thinks they're
  making a session-local decision but they're silently mutating a persistent
  file that applies to all sessions (and possibly teammates if project-scoped).
- Pattern matching is fuzzy, so "always allow" is fuzzy too — there's no clean
  way to determine what string to write for a given approval. Writing the exact
  command is useless (the next `rm -rf` will be a different path); writing a
  broader pattern widens the safety hole.

The friction of editing config is a feature, not a bug.

## Section 2: `BlacklistGate` — the matcher and patterns

Lives in a new `crates/zoid-tools/src/approval.rs` module.

### Structure

```rust
/// The blacklist gate. Allow unless a `shell` call matches a dangerous pattern.
pub struct BlacklistGate {
    patterns: Vec<Pattern>,     // builtin defaults ⊕ user shell_danger ⊖ user shell_allow
    interactive: bool,           // true = Gate::Prompt on match (Chat); false = Gate::Deny (subagents)
}
```

### The matcher pipeline

A pure function, unit-testable in isolation:

1. **Chain-split:** split the raw command string on shell chain operators.
   Handle `||` before `|` (logical-OR vs pipe), and also `&&` and `;`. Each
   resulting segment is an independent command to check.
2. **shlex-tokenize** each segment. If shlex fails to parse a segment →
   fail-safe: treat it as dangerous (prompt). A cooperating agent won't normally
   produce unparseable commands, and the cost of a false positive here is one
   keystroke.
3. **Pattern match** each segment's token stream against the pattern list.

`shlex` is already in the dependency tree (transitively), so adding it as a
direct dep adds no new transitive bloat.

### Pattern types

Patterns are structured, not raw substrings:

```rust
enum Pattern {
    /// Leading program must be exactly `prog` (e.g. "sudo", "systemctl").
    LeadingProgram { prog: String },
    /// Leading program `prog` with any of `trigger_flags` present
    /// (e.g. curl/wget with -X POST/-d/--data/-X PUT/-X DELETE; git with push+force).
    ProgramWithAnyFlag { prog: String, trigger_flags: Vec<String> },
    /// Leading program `prog` with at least one flag from each of `flag_groups`
    /// present. Used when two independent flag dimensions must both be
    /// satisfied (e.g. rm needs recursive AND force — one alone isn't dangerous).
    ProgramWithAllGroups { prog: String, flag_groups: Vec<Vec<String>> },
    /// Free-form substring match for compound patterns that don't fit the
    /// token model (e.g. "terraform apply", "kubectl delete").
    Substring { pattern: String },
}
```

Matching: for each segment, check the leading token (first token after
tokenization) against the leading-program patterns. `ProgramWithAnyFlag`
matches if any of its trigger flags appear anywhere in the token stream.
`ProgramWithAllGroups` matches if at least one flag from *each* group appears
(e.g. rm matches when a recursive flag and a force flag are both present —
`rm -r` alone is safe, `rm -f` alone is safe, `rm -rf` is dangerous). For
`Substring`, check against the whole segment's raw text. A segment matches
if *any* pattern matches it.

### Builtin default patterns (all 6 categories)

- **Destructive `rm`** — `ProgramWithAllGroups { prog: "rm", flag_groups:
  [["-r", "--recursive"], ["-f", "--force"]] }` — match if *both* a recursive
  flag and a force flag are present. `rm -r` alone is safe; `rm -f` alone is
  safe; `rm -rf`, `rm -r -f`, `rm --recursive --force` all match.
- **Force-push / history rewrite** — `ProgramWithAnyFlag { prog: "git",
  trigger_flags: ["push --force", "push -f", "push --force-with-lease"] }` —
  match if the git subcommand is a push with a force flag. These are tokenized
  as multi-word patterns matched against consecutive tokens. `--force-with-lease`
  is included because it still rewrites public history (irreversible from
  zoid's view).
- **Network/prod writes** — `ProgramWithAnyFlag { prog: "curl", trigger_flags:
  ["-X", "--data", "-d"] }` with an additional check that if `-X` is present, the
  following token is a non-GET method (POST/PUT/DELETE/PATCH); same for `wget`
  with `--post-data`/`--post-file`. Method-aware: GETs are common and read-only,
  so they don't prompt. The `-X` flag + method is checked as two consecutive
  tokens.
- **Privilege escalation** — `LeadingProgram { prog: "sudo" }`,
  `LeadingProgram { prog: "su" }`, `LeadingProgram { prog: "doas" }`.
- **System mutation** — `LeadingProgram { prog: "systemctl" }`,
  `LeadingProgram { prog: "apt" }`, `LeadingProgram { prog: "brew" }`,
  `ProgramWithAllGroups { prog: "pip", flag_groups: [["install"], ["--user"]] }`,
  `Substring { pattern: "chmod -R" }`, `Substring { pattern: "/etc/" }`.
- **Deploy / irrecoverable** — `Substring { pattern: "terraform apply" }`,
  `Substring { pattern: "kubectl delete" }`, `Substring { pattern: "fly deploy" }`,
  `Substring { pattern: "scp" }`, `Substring { pattern: "rsync" }`.

### Config interaction

The effective pattern set is `builtin_defaults ⊕ config.shell_danger ⊖
config.shell_allow`:

- User `shell_danger` entries are added as `Substring` patterns (simplest for
  users to write).
- `shell_allow` entries are matched against the builtin pattern's canonical
  string representation to exempt. If a user puts `"git push --force-with-lease"`
  in `shell_allow`, we check whether any builtin pattern's canonical form
  contains that string and skip it.

### Tiering logic in `check`

```rust
fn check(&self, call: &ToolCall) -> Gate {
    // Never-prompt tier: always allow
    match call.name.as_str() {
        "read_file" | "search" | "recall" | "show" | "update_tasks" | "ask_user"
            => return Gate::Allow,
        _ => {}
    }
    // Allow-by-default tier: write_file, edit_file
    match call.name.as_str() {
        "write_file" | "edit_file" => return Gate::Allow,
        _ => {}
    }
    // Blacklist-gated tier: shell
    if call.name == "shell" {
        let cmd = call.args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(pattern) = self.match_dangerous(cmd) {
            let question = format!("`shell` calls a dangerous action — approve?\n{}", cmd);
            if self.interactive {
                return Gate::Prompt { question, choices: vec!["approve once".into(), "deny".into()] };
            } else {
                return Gate::Deny(format!("blocked by safety blacklist: matched `{}`", pattern));
            }
        }
    }
    Gate::Allow
}
```

### Matching limitations (accepted)

The command is a shell string (`sh -c "<command>"`), so the agent can run
arbitrary pipelines, redirects, variable expansion, `&&` chains, `$(...)`
substitutions. Even with shlex tokenization, obfuscation like
`$(printf rm) -rf` would hide the `rm` inside the substitution. We're not
building a sandbox — we're building a *prompt-before-irreversible-action* speed
bump for a cooperating agent, not defending against a malicious adversary. The
agent isn't trying to bypass the gate; it might just run a dangerous command in
good faith and we want a human eyeball on it. The bar is "best-effort tokenizer +
pattern match, tuned for low false negatives on the common dangerous forms."

**Fail-safe toward prompting** — any unparseable/weird command prompts. False
positives cost one keystroke; false negatives cost your data.

## Section 3: Config `[approval]` section

Adding to `crates/zoid-core/src/config.rs`.

### New config struct

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalConfig {
    pub yolo: bool,
    pub shell_danger: Vec<String>,
    pub shell_allow: Vec<String>,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self { yolo: false, shell_danger: vec![], shell_allow: vec![] }
    }
}
```

### Partial for TOML deserialization

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialApproval {
    pub yolo: Option<bool>,
    pub shell_danger: Option<Vec<String>>,
    pub shell_allow: Option<Vec<String>>,
}
```

### Wiring

- Add `pub approval: ApprovalConfig` to `Config`.
- Add `pub approval: PartialApproval` to `PartialConfig`.
- Merge logic in `merge()`:
  - `yolo`: last-write-wins (same as `reduced_motion`).
  - `shell_danger`: union across layers, dedup (same as `skills.source_dirs`).
  - `shell_allow`: union across layers, dedup.
- Provenance: `yolo` gets a provenance field (scalar, single winner).
  `shell_danger`/`shell_allow` don't need provenance (unioned, no single winner).

### TOML shape

```toml
[approval]
# Disable all approval prompts (deliberate; never the default).
yolo = false

# Add shell-danger patterns beyond the builtin defaults.
shell_danger = ["make deploy", "npm run publish"]

# Exempt builtin patterns that are false positives for you.
shell_allow = ["git push --force-with-lease"]
```

## Section 4: CLI `--yolo` flag

Adding to `crates/zoid/src/cli.rs`.

### `Cli::Run` gets a `yolo: bool` field

```rust
pub enum Cli {
    Run { companion: bool, new: bool, resume: Option<String>, yolo: bool },
    ...
}
```

### Parsing

A `--yolo` flag in the existing `while` loop, same pattern as `--companion`:

```rust
"--yolo" => yolo = true,
```

### Gate selection

In `main.rs`, where the gate is currently hardcoded as `AllowAll`:

```rust
let yolo = config.approval.yolo || cli_yolo;
let gate: Arc<dyn ToolGate> = if yolo {
    Arc::new(AllowAll)
} else {
    Arc::new(BlacklistGate::new(config.approval.shell_danger, config.approval.shell_allow, true))
};
```

`cli_yolo` overrides config — `--yolo` on the CLI forces YOLO even if config says
`yolo = false`. The expression is `config.yolo || cli.yolo` — either source
enables it.

### Help text

```
    zoid --yolo              Disable all approval prompts (dangerous)
```

### YOLO semantics

An escape hatch that disables all approval prompts. Semantically the simplest
possible thing: **every `check` returns `Allow`**, no matter what the blacklist
says. The existing `AllowAll` struct *is* a YOLO gate — it's just not selectable
today.

- **Not the default** — a fresh install runs the blacklist gate. YOLO is
  deliberate opt-in.
- **`ask_user` is unaffected.** YOLO = no *pre-dispatch approval* prompts; the
  model can still ask clarifying questions mid-turn. These are two different
  things and the mental model stays clean.

## Section 5: Subagent wrapper (headless auto-deny)

Subagents can't prompt — `ask_user` is already filtered out of their tool set
(`subagent.rs`). A `Gate::Prompt` would park the loop forever waiting for a reply
that can't come.

### The design

Subagents get the same `BlacklistGate` but with `interactive: false`, which makes
it return `Gate::Deny(reason)` instead of `Gate::Prompt` on a dangerous match.
The denial feeds back to the model as an error `ToolResult` (the existing Deny
path), so the subagent sees "blocked by safety blacklist: matched `rm -rf`" and
can choose another approach or fail.

### Implementation

`subagent.rs`, ~line 167. Today:

```rust
std::sync::Arc::new(zoid_tools::AllowAll),
```

Becomes:

```rust
// Subagents are headless — they can't answer a prompt, so the blacklist
// gate auto-denies dangerous matches instead of prompting.
let gate: Arc<dyn ToolGate> = if yolo {
    Arc::new(AllowAll)
} else {
    Arc::new(BlacklistGate::new(
        config.approval.shell_danger,
        config.approval.shell_allow,
        false,  // interactive = false → Gate::Deny, not Gate::Prompt
    ))
};
```

`run_subagent` gains an `approval: ApprovalConfig` parameter — the slice of
config the subagent needs, not the whole `Config`, keeping the param focused.

### YOLO uniformity

When the whole run is YOLO (`config.approval.yolo || cli --yolo`), subagents get
`AllowAll` too — same as Chat. The `yolo` bool passed in is already resolved
(config || cli), so the subagent just checks that one flag.

### What the model sees on denial

```
blocked by safety blacklist: matched `rm with recursive+force flags`
```

The subagent can then try a safer alternative or report failure.

## Section 6: Testing strategy

### Unit: the matcher (`approval.rs` `#[cfg(test)]`)

The core matcher is a pure function — `fn match_dangerous(cmd: &str, patterns:
&[Pattern]) -> Option<String>`. Test it exhaustively without touching the agent
loop:

- **Positive cases** (should match): `rm -rf /`, `rm -r -f /`,
  `rm --recursive --force ~`, `git push --force`, `git push -f origin main`,
  `git push --force-with-lease`, `sudo apt update`, `curl -X POST localhost`,
  `curl -d 'data' localhost`, `terraform apply`, `chmod -R 777 /`,
  `echo hi && rm -rf /` (chained — second segment matches).
- **Negative cases** (should NOT match): `rm file.txt` (no recursive+force),
  `git push origin main` (no force), `curl localhost` (GET),
  `echo "rm -rf /"` (inside quotes — shlex tokenizes the leading token as
  `echo`, not `rm`), `grep "force" file` (substring false positive guard),
  `git log | grep foo` (both segments safe).
- **Chain splitting**: `echo hi && rm -rf /` → both segments checked, `rm -rf /`
  segment triggers. `git log | grep --force foo` → neither segment triggers.
- **Fail-safe**: unparseable segment (unbalanced quotes) → prompt/deny.

### Unit: `BlacklistGate::check` (`approval.rs` tests)

- Reads never prompt: `check("read_file", ...) == Gate::Allow`.
- File writes allow: `check("write_file", ...) == Gate::Allow`.
- Safe shell allows: `check("shell", { command: "ls -la" }) == Gate::Allow`.
- Dangerous shell with `interactive: true` → `Gate::Prompt { .. }`.
- Dangerous shell with `interactive: false` → `Gate::Deny(reason)`.
- `shell_allow` exempts a builtin pattern.
- `shell_danger` adds a custom pattern.
- Missing/empty command arg → `Gate::Allow` (nothing to match).

### Unit: config (`config.rs` tests)

- `[approval]` section parses and merges.
- `shell_danger` / `shell_allow` union across layers (mirrors
  `skills.source_dirs` test).
- `yolo` last-write-wins (mirrors `reduced_motion` test).
- Default `ApprovalConfig` is `yolo: false`, empty vectors.

### Unit: CLI (`cli.rs` tests)

- `--yolo` parses to `yolo: true`.
- Existing tests updated for the new field.
- `--yolo` + `--companion` combine.

### Integration: `Gate::Prompt` in the agent loop (`tests/agent_loop.rs`)

Follow the existing `gate_deny_blocks_tool_and_feeds_reason_back` pattern, but
with a `PromptGate` test double:

- A gate that returns `Gate::Prompt` for `shell` calls with a dangerous command.
- Scripted provider calls `shell` with `rm -rf /tmp/test`.
- The test's UI drain simulates the user answering "approve once" via the
  oneshot reply.
- Assert: the shell tool actually executed.
- A second test: same setup, but the drain answers "deny" → assert the tool did
  NOT run, and an error `ToolResult` with the denial reason is in the log.

### Integration: subagent auto-deny

- A subagent with a `BlacklistGate { interactive: false }` calling `rm -rf /`.
- Assert: tool did not run, error `ToolResult` with "blocked by safety
  blacklist" in the log, subagent summary reflects the failure.

## Section 7: File-by-file change summary

- **`crates/zoid-tools/src/lib.rs`** — Add `Gate::Prompt { question, choices }`
  variant. Add `mod approval;` and re-export `BlacklistGate`.
- **`crates/zoid-tools/src/approval.rs`** — **New file.** `Pattern` enum,
  `BlacklistGate` struct + impl, builtin defaults, chain-splitter, shlex-based
  segment matcher, `match_dangerous()` pure function, unit tests.
- **`crates/zoid-tools/Cargo.toml`** — Add `shlex` as a direct dependency (already
  transitively present).
- **`crates/zoid-core/src/config.rs`** — `ApprovalConfig` struct,
  `PartialApproval`, wire into `Config`/`PartialConfig`/`merge()`, provenance
  for `yolo`, tests.
- **`crates/zoid/src/agent.rs`** — The check site (~line 682): expand
  `if let Gate::Deny` to a `match` handling `Allow`/`Deny`/`Prompt`. `Prompt` arm
  reuses the `ask_user` oneshot + `AgentUpdate::AskUser` park-and-await.
- **`crates/zoid/src/main.rs`** — Gate selection: replace hardcoded `AllowAll`
  with the `yolo ? AllowAll : BlacklistGate` branch. Thread config + cli yolo
  into the gate.
- **`crates/zoid/src/subagent.rs`** — Replace hardcoded `AllowAll` with `yolo ?
  AllowAll : BlacklistGate { interactive: false }`. Add `approval:
  ApprovalConfig` param to `run_subagent`.
- **`crates/zoid/src/cli.rs`** — Add `yolo: bool` to `Cli::Run`, parse `--yolo`,
  update help text, update existing tests.
- **`crates/zoid/src/spawn_subagent.rs`** — Thread `approval` config through to
  `run_subagent` call.
- **`crates/zoid/tests/agent_loop.rs`** — New integration test: `Gate::Prompt`
  approve + deny paths.

## Summary of decisions

1. **Tiered, not prompt-on-everything** — reads never prompt; file writes
   allow-by-default; `shell` blacklist-gated.
2. **`shell` blacklist-driven prompts** — builtin dangerous-pattern defaults
   (destructive `rm`, force-push incl. `--force-with-lease`, non-GET `curl`,
   `sudo`/`su`/`doas`, system/prod mutation, deploys), user-config additions +
   exemptions.
3. **File writes (`write_file`/`edit_file`) allow by default** — git-recoverable,
   fires constantly.
4. **`Gate::Prompt` mechanism** — sync `check` returns a prompt variant; the
   agent loop reuses the existing `ask_user` overlay; approve-once / deny;
   subagents auto-deny. No "always allow" in-session button (config `shell_allow`
   is the deliberate escape valve).
5. **shlex-based tokenizer** — split on chain operators (`&&`, `;`, `|`, `||`)
   first, then shlex-tokenize each segment, then match the leading program +
   flags against structured patterns (`LeadingProgram`, `ProgramWithAnyFlag`,
   `ProgramWithAllGroups`, `Substring`). Fail-safe toward prompting.
6. **YOLO mode** — `AllowAll` selectable via config or `--yolo`; never the
   default; `ask_user` unaffected. Subagents flow uniformly under YOLO.
7. **All five pieces in one pass** — the pieces are coupled through the
   gate-selection logic; splitting them means implementing it twice.