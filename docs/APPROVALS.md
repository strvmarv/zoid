# Tool-call approvals — design

> **Status:** implemented. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`
> for the full spec and `docs/superpowers/plans/2026-07-08-tool-approvals.md` for the
> implementation plan. This document is retained as the original design sketch.

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

## Current state (the seam)

The approval machinery already exists as a tested seam:

- **`ToolGate`** (`crates/zoid-tools/src/lib.rs`) — consulted once per pending
  tool call, immediately before dispatch:
  ```rust
  pub enum Gate { Allow, Deny(String) }
  pub trait ToolGate: Send + Sync { fn check(&self, call: &ToolCall) -> Gate; }
  pub struct AllowAll;   // the ONLY shipped impl — allows everything
  ```
  The doc comment is explicit about intent: *"this is the insertion point
  where interactive tool approval will later live (an `ask_user` prompt gating
  `Deny`)."*

- **Check site** (`crates/zoid/src/agent.rs`, inside `run_turn_inner`'s per-call
  loop): `gate.check(&tc)` runs before each pending tool call. `Deny(reason)`
  emits an error `ToolResult` with that reason and `continue`s (the loop keeps
  going; the model sees the denial). `Allow` falls through to kind-dispatch.

- **Wiring** — production Chat (`main.rs`) and subagents (`subagent.rs`) both
  hardcode `Arc::new(AllowAll)`. Nothing else is wired.

- **Test coverage** (`crates/zoid/tests/agent_loop.rs`):
  `gate_deny_blocks_tool_and_feeds_reason_back` proves the Deny path.

- **Interactive half (designed, not built)** — `ask_user`
  (`ToolKind::Interactive`) already parks the loop on a `oneshot`, sends
  `AgentUpdate::AskUser { question, choices, reply }` to the UI, and resumes on
  the user's answer. The question overlay is built and snapshotted. The design
  intent is that *"approve/deny is just an `ask_user` with two choices
  consulted by `ToolGate`."* But `check` is **synchronous** — it can only
  `Allow`/`Deny`; it cannot itself suspend and ask the user. That needs a new
  `Gate::Prompt` variant (see Mechanism below).

- **Conceptual ancestor** — the deferred Build-mode spec
  (`docs/superpowers/archive/2026-07-02/specs/2026-06-30-zoid-build-mode-design.md`
  §2.1) already names a dangerous-actions list (force-push to main, prod/network
  writes, deleting external data, spending money), framed as Build "blockers by
  definition — never auto-done regardless of settings." This approvals design
  brings that concept into Chat via the `ToolGate` seam.

- **Config** — `Config` (`crates/zoid-core/src/config.rs`) has no approval
  section today. This design adds one.

## Tiering (by danger, not by call frequency)

| Tier | Tools | Disposition | Why |
|------|-------|------------|-----|
| **Never prompt** | `read_file`, `search`, `recall`, `show`, `update_tasks`, `ask_user` | always allow | no side effects; prompting would be noise (and prompting to approve `ask_user` would be recursive/absurd) |
| **Allow by default** | `write_file`, `edit_file` | allow; (future: opt-in prompt) | git-recoverable, fires constantly — prompting here would train ⏎-through |
| **Blacklist-gated** | `shell` | allow **unless** command matches a dangerous pattern → prompt | `shell` is the only escape hatch (arbitrary code: `rm -rf`, `git push --force`, `curl` to prod, `sudo`…) |

In practice: **file writes never prompt; `shell` prompts only on a dangerous
match.** That's the minimal-prompt design.

## The `shell` blacklist

This is where "fewest prompts, every one meaningful" lives or dies.

### What counts as dangerous

The blacklist is actions that **escape the working directory or are
irreversible**, grouped:

- **Destructive to files outside cwd** — `rm -rf /`, `rm -rf ~`, `rm -rf $HOME`,
  `rm -rf *` (from cwd root), `rm -rf ..` …
- **Force-push / history rewrite** — `git push --force`, `git push -f`,
  `git push --force-with-lease` (still rewrites public history), `git rebase`
  onto a shared branch, `git commit --amend` on a pushed branch…
- **Network/prod writes** — `curl`/`wget` with non-GET methods (POST/PUT/DELETE,
  `-d`/`--data`, `-X POST`…), `scp`/`rsync` to a remote, anything hitting a
  prod URL…
- **Privilege escalation** — `sudo`, `su`, `doas`…
- **System mutation outside repo** — package install (`apt`, `brew`, `pip install
  --user`…), `systemctl`, editing `/etc/`, `chmod -R`…
- **Spending / irrecoverable** — cloud-deploy (`terraform apply`, `kubectl
  delete`, `aws … delete`, `fly deploy`…).

This is inherently fuzzy and the list will grow.

### Where the blacklist lives

**Builtin defaults + user config override** (recommended):

- zoid ships a curated default blacklist (the patterns above).
- `[approval]` in config lets you **add** patterns (`shell_danger`) and
  **exempt** builtin ones (`shell_allow`) that are false positives for you.
- Sane out-of-the-box, tunable. Mirrors how the economy config already works.

Rationale: pure-builtin isn't tunable (can't add project-specific dangers like
`make deploy-prod` or silence a false positive without a code change); pure-config
leaves a fresh install with no protection.

### How matching works (the trap)

- **The command is a shell string** (`sh -c "<command>"`), so the agent can run
  arbitrary pipelines, redirects, variable expansion, `&&` chains, `$(...)`
  substitutions. Naive substring matching (`command.contains("rm -rf")`) is
  **trivially bypassable** (`rm -r -f`, `rm --recursive --force`, `$(echo rm)
  -rf`) and **false-positive-prone** (matching `rm -rf` inside a `grep` pattern
  or a string literal).
- A robust matcher **tokenizes the command** (splits on shell metacharacters)
  and matches command/flags, with awareness of `&&`/`;`/`|` so that
  `echo hi && rm -rf /` still trips on the `rm` segment.
- Even tokenization is imperfect against obfuscation (`$(printf rm)`), but
  **we're not building a sandbox** — we're building a *prompt-before-
  irreversible-action* speed bump for a cooperating agent, not defending against
  a malicious adversary. The agent isn't trying to bypass the gate; it might
  just run a dangerous command in good faith and we want a human eyeball on it.
  So the bar is "best-effort tokenizer + pattern match, tuned for low false
  negatives on the common dangerous forms," not a hardened parser.
- **Fail-safe toward prompting** — any unparseable/weird command prompts.
  False positives cost one keystroke; false negatives cost your data. This is
  what makes a fuzzy matcher acceptable.

Matching targets:
- the **leading program** of each `&&`/`;`/`|`-separated segment (so chained
  commands are all checked);
- plus a small set of **flag/argument patterns** (e.g. `rm` with
  `-r`/`-f`/`--recursive`/`--force` present; `git push` with `--force`/`-f`;
  `sudo` as a leading token; `curl`/`wget` with a non-GET method or `-d`);

### Open matching decisions (settled)

- **`--force-with-lease`** — blacklist it (still rewrites public history;
  irreversible from zoid's view).
- **Network reads vs writes** — method-aware: blacklist only non-GET
  `curl`/`wget` (POST/PUT/DELETE, `-d`/`--data`, `-X POST`), since GETs are
  common and read-only.

## Mechanism: `Gate::Prompt`

```rust
pub enum Gate {
    Allow,
    Deny(String),
    Prompt { question: String, choices: Vec<String> },   // new
}
```

`check` stays sync. The **agent loop**, on seeing `Gate::Prompt`, reuses the
*exact same* `oneshot` + `AgentUpdate::AskUser` park-and-await path that
`ask_user` uses today (`agent.rs`). Approve → dispatch; Deny/Esc → feed a
reason back as an error `ToolResult` (the existing Deny path). **No new UI** —
the question overlay is already built and snapshotted.

### What a prompt looks like

The existing question overlay, e.g.:

> **`shell` calls a dangerous action — approve?**
> `rm -rf node_modules/ ..` *(the actual command, shown)*
> [approve once] [deny]

`approve once` (not "always allow this pattern") — keeping it one-shot means
every dangerous call is independently eyeballed, which is the whole point.

## Subagents (headless)

Subagents are headless — `ask_user` is already filtered out of their tool set
(`subagent.rs`). They literally cannot answer a prompt. So a prompt-based gate
**cannot apply to subagents**. Resolution: the policy is **mode-aware** —
- **Chat** prompts on dangerous matches (`Gate::Prompt`);
- **Subagents** get the same blacklist as **auto-deny** (feed the reason back,
  let the subagent find another way or fail).

The dangerous-action blacklist stays load-bearing in both contexts, just with
different dispositions. A YOLO run (below) makes subagents flow uniformly.

## YOLO mode

An escape hatch that disables all approval prompts. Semantically the simplest
possible thing: **every `check` returns `Allow`**, no matter what the blacklist
says. The existing `AllowAll` struct *is* a YOLO gate — it's just not selectable
today. So YOLO mode is "make `AllowAll` the active gate instead of the blacklist
gate."

- **Not the default** — a fresh install runs the blacklist gate. YOLO is
  deliberate opt-in.
- **Opt-in via:**
  - config: `approval.yolo = true` (writing it is itself deliberate); and/or
  - a `--yolo` CLI flag (useful for CI/scripted/headless runs where there's no
    human to answer prompts anyway).
- **If there's an in-app toggle**, it should confirm ("this disables all safety
  approval prompts — sure?") — "disable all safety prompts" is itself a
  dangerous action and shouldn't be a typo away.
- **`ask_user` is unaffected.** YOLO = no *pre-dispatch approval* prompts; the
  model can still ask clarifying questions mid-turn. These are two different
  things and the mental model should stay clean.

### Gate selection

```
if config.approval.yolo || cli --yolo:
    gate = AllowAll            # everything flows
else:
    gate = BlacklistGate {     # the new gate
        shell_patterns: builtin_defaults ⊕ config.shell_danger ⊖ config.shell_allow,
        interactive: true,     # Chat: Gate::Prompt on match
    }
```

Subagents wrap the blacklist gate in a "prompt → auto-deny" adapter (since they
can't prompt), *unless* the whole run is YOLO, in which case `AllowAll` applies
uniformly.

## Proposed config shape

```toml
[approval]
# Disable all approval prompts (deliberate; never the default).
yolo = false

# Add shell-danger patterns beyond the builtin defaults.
shell_danger = ["make deploy", "npm run publish"]

# Exempt builtin patterns that are false positives for you.
shell_allow  = ["git push --force-with-lease"]
```

## Summary of decisions

1. **Tiered, not prompt-on-everything** — reads never prompt; file writes
   allow-by-default; `shell` blacklist-gated.
2. **`shell` blacklist-driven prompts** — builtin dangerous-pattern defaults
   (destructive `rm`, force-push incl. `--force-with-lease`, non-GET `curl`,
   `sudo`, system/prod mutation, deploys), user-config additions + exemptions.
3. **File writes (`write_file`/`edit_file`) allow by default** — git-recoverable,
   fires constantly.
4. **`Gate::Prompt` mechanism** — sync `check` returns a prompt variant; the
   agent loop reuses the existing `ask_user` overlay; approve-once / deny;
   subagents auto-deny.
5. **YOLO mode** — `AllowAll` selectable via config or `--yolo`; never the
   default; `ask_user` unaffected.