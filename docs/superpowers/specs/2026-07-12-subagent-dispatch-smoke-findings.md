# Subagent Dispatch Smoke-Test Findings

> **Date:** 2026-07-12 (initial), revalidated 2026-07-12 (post-rebuild),
> **RE-VERIFIED 2026-07-12 against source at HEAD `634c114`.**
> **Scope:** Live harness smoke-testing of the subagent-guardrails work.
> **Method:** Direct tool invocation through the zoid harness with `/tmp` file
> artifacts as observable side-effects. ~20 dispatches across 4 test rounds.
>
> ⚠️ **Read this first.** The original findings were authored while the testing
> agent was stuck in a **diagnosis loop** — it kept generating plausible
> mechanisms without falsifying any of them. The re-verification below traced
> each claim to source and **overturned the two highest-severity findings**.
> Every finding paired a *true code observation* with an *unverified causal
> leap* to the symptom; the observations hold, the "therefore this causes X"
> steps mostly did not. **Only one real bug survives (Finding 5's TOCTOU), and
> even its originally-proposed fix is broken.** The struck-through original
> verdicts are kept for provenance — do not act on them.

---

## Re-verification summary (authoritative)

| # | Original claim | Code mechanism | Causal link to symptom | Net verdict |
|---|----------------|----------------|------------------------|-------------|
| 1 | Single-flight is dispatch-only | Real | Holds | ✅ Correct — no fix |
| 2 | "Two gates" | — | — | ❌ Already rejected (one registry) |
| 3 | Channel-drain race blocks dispatch ~35% | Real code, but **serialized** | **Broken** — reap happens-before next dispatch | ❌ **STRIKE — test-harness artifact, not a product bug** |
| 4 | Dropped `JoinHandle` starves the future | Handle *is* dropped | **False** — detaching ≠ cancelling | ⚠️ **Wrong mechanism**; real cause is model hallucination |
| 5 | `hard` TOCTOU deletes `/tmp` artifacts | Override is **intended policy**, not a TOCTOU | **Impossible** — `drop(wt)` can't touch `/tmp` | ❌ **NOT a bug — intended "kill wins" policy; symptom impossible; proposed fix would regress** |
| 6 | Worktree fix correct at its layer | Real | Holds | ✅ Correct — no fix |

**Bottom line:** after tracing to ground, **no finding describes an actionable
product defect.** Findings 1, 2, 6 were already correct/rejected; 3 is a rig
artifact; 4 is upstream model behavior (verification-layer mitigation, not a
code fix); and 5's `hard.is_cancelled()` override is **deliberate design**, not
a bug — see the deep trace in the Finding 5 section.

---

## Finding 1: Single-flight is `dispatch_subagent`-only (VALIDATED ✅)

**Claim:** Only `dispatch_subagent` is gated by single-flight; all other tools
run freely while a subagent is in-flight.

**Test:** Dispatched a subagent with a 15s `sleep`, then immediately exercised
`shell`, `read`, `write`, `edit`, `grep`, and `glob` while it was in-flight.

**Result: ✅ Validated (4 rounds).** All tools executed instantly with no
blocking. Only `dispatch_subagent` itself was correctly refused.

**Code (re-verified at HEAD `634c114`):** `agent.rs:1424-1448` — the `Emitting`
arm for `dispatch_subagent` checks `config.in_flight` (`agent.rs:1426-1427`:
`if let Some(set) = &config.in_flight { if !set.lock().unwrap().is_empty()`).
No other tool consults this registry.

**Verdict:** Correct by design. No fix needed.

---

## Finding 2: "Two separate gates" hypothesis (REJECTED ❌)

**Earlier claim:** There are two independent in-flight trackers — the Rust
`app.in_flight` registry and a separate harness dispatch gate — and they can
disagree.

**Revised finding:** There is **one registry**: `app.in_flight`
(`Arc<Mutex<HashMap<String, SubagentHandle>>>`). Both the agent-turn dispatch
and the harness dispatch go through the same `Emitting` arm in `agent.rs:1424`,
which checks `config.in_flight` (the same `Arc` clone as `app.in_flight`,
wired at `main.rs:5821`: `turn_config.in_flight = Some(app.in_flight.clone())`).
The "a subagent is already running" error string (`agent.rs:1436`) is the only
such message in the codebase.

**Verdict:** The "two gates" framing was a misread of a timing artifact
(Finding 3). One registry, one gate.

---

## Finding 3: Stale in-flight slot after natural completion

### ❌ RE-VERIFIED VERDICT: STRIKE — test-harness artifact, NOT a product bug.

> ~~**Result: ✅ Validated — intermittent.** ~7/20 dispatches were blocked
> (~35%)… Root cause — channel-drain race… Severity: High — blocks sequential
> multi-subagent workflows ~35% of the time.~~ *(Original verdict — WRONG.)*

**What the re-verification found:** The reap and the next-turn dispatch are
**serialized through the single-threaded main loop's `ui_rx` arm**, so the
race the original finding describes cannot occur in normal operation:

- `main.rs:2761-2763` — on receiving the `DelegationResult`, the loop reaps
  synchronously: `app.in_flight.lock().unwrap().remove(subagent_id)`.
- `main.rs:2808-2809` — **in the same synchronous arm, AFTER the reap** —
  `if delegation_arrived && plan_delegation_wake(app) { spawn_turn(app); }`.
- `spawn_turn` builds `turn_config.in_flight` from the already-emptied Arc
  (`main.rs:5821`), so the next turn's gate check (`agent.rs:1427`) reads an
  **empty** set.

Because `remove()` at line 2763 strictly *happens-before* the next turn is even
constructed at line 2809, a real (model-driven) sequential dispatch can never
observe the stale entry.

**Why the smoke-test saw ~35% failures anyway:** the test harness dispatched
out-of-band. The original finding says so directly: *"I receive the
DelegationResult via a separate signal path, so I can call dispatch_subagent
again before the main loop has processed the reap."* That separate path
bypasses the very serialization (the `ui_rx` arm) that prevents the race. The
"delay fixes it" confirmation is consistent with this: the delay simply lets
the main loop drain and reap — something normal operation does unconditionally
before planning the next turn.

**Fix direction:** None required for the product. If anything, the smoke-test
rig should be corrected to dispatch through the normal turn/wake path rather
than out-of-band, so it stops manufacturing a race the product doesn't have.

**Severity:** None (rig artifact).

---

## Finding 4: Silent dispatch no-op (VALIDATED ✅ symptom — ⚠️ wrong mechanism)

**Claim:** `dispatch_subagent` sometimes returns a valid ID, but the
subagent's work never executes — no artifact, no `DelegationResult`.

**Result:** The *symptom* (a write-a-file task intermittently produces no
artifact, ~25%) was observed. The *root cause* attributed to it was wrong.

### Mechanism #1 — "dropped `JoinHandle` starves the future": ❌ FALSE

> ~~The `JoinHandle` is dropped immediately at `spawn_subagent.rs:68`. Under
> runtime pressure, the spawned future may be starved or dropped before
> `run_subagent` starts.~~ *(Original — WRONG.)*

Line 68 (`tokio::spawn(async move {`, re-verified — `spawn_subagent.rs`
unchanged since `a9d303b`) does drop the returned `JoinHandle`. But in Tokio,
**dropping a `JoinHandle` detaches the task; it does not cancel or starve it.**
The spawned future runs to completion regardless. This mechanism cannot produce
the observed no-op.

### Mechanism #2 — model hallucinates tool execution: ✅ the real cause

The finding's own controlled evidence points here: a `shell`-based task
(`echo … > /tmp/…`, which can't be faked with text) landed **100%** of the
time, while `write`-based tasks failed **~25%**. That asymmetry is a
**model-behavior** issue — the subagent's model sometimes emits text claiming
it wrote the file instead of emitting a real `write` tool call — not a defect
in zoid's dispatch/spawn code.

**Fix direction:** This is not a Rust-code bug. Mitigate at the verification
layer: add a **post-turn tool-execution check** that confirms the subagent's
claimed tool calls appear as `ToolCall` + `ToolResult` pairs in the event log,
and surface a not-ok `DelegationResult` when they don't. (The originally-listed
`catch_unwind` / stored-`JoinHandle` / watchdog hardening is reasonable
defense-in-depth for genuine panics/hangs, but it will NOT fix the ~25%
hallucination case — do not expect it to.)

**Severity:** Medium — real lost-work symptom, but the cause is upstream model
behavior, addressed by verification rather than a spawn fix.

---

## Finding 5: Force-cancel destroys completed artifacts

### ❌ RE-VERIFIED VERDICT (deep trace): NOT a bug. The `hard.is_cancelled()` override is intended "kill-wins" policy; the reported symptom is impossible; and the originally-proposed fix would REGRESS the design.

**The code (CONFIRMED in source):** `spawn_subagent.rs:113` checks
`hard.is_cancelled()` **unconditionally**, forcing the failure branch:

```rust
let res = if hard.is_cancelled() {
    let reason = *abort_reason.lock().unwrap();
    Err(anyhow::anyhow!(abort_summary(reason)))
} else {
    res
};
```

At first glance this looks like a TOCTOU ("a completed `Ok` gets flipped to
`Err` and its worktree dropped"). **Tracing the control flow to ground shows it
is not:**

1. **The override only matters for a *mid-run* kill.** `subagent.rs:214` is
   `run_agent_turn_cancellable(...).await?` — a `hard`-induced turn error
   propagates through the `?`, so `run_subagent` only returns `Ok` when the
   turn genuinely drained. The override at line 113 therefore only changes the
   outcome when a kill was requested **during** the run but the turn drained to
   `Ok` anyway. Discarding that partial work is the **documented intent**
   (`spawn_subagent.rs:107-108`: "force the failure branch regardless of what
   the drained turn returned").
2. **A genuinely-completed subagent cannot be flipped.** There is **no `.await`
   between `run_subagent` returning (line 102) and the `hard` check (line
   113)**, and `done.cancel()` (line 105) stops the `WakeTimer`. So for a run
   that finished on its own, `hard` can only be set at line 113 inside the
   **~250ms ceiling-boundary race the code comment already documents and
   accepts** (lines 107-112) — not via any late external cancel.
3. **A late `cancel_subagent` has no effect on a completed subagent.** The
   `DelegationResult` is emitted at line 165 and the handle is reaped from the
   registry (`main.rs:2763`), so `fire_subagent_kill` finds nothing to fire.
   Tokens are **per-dispatch** (`agent.rs:1512` `sub_hard =
   CancellationToken::new()`), so there is no cross-subagent aliasing either.

**Why the originally-proposed fix would REGRESS:** making the override
conditional so a completed `Ok` survives would *also* make a **mid-run kill
that drains to `Ok`** keep its partial work — the exact opposite of the
intended policy. (And the specific `done`-token gate proposed is inert anyway;
see below.)

### ❌ The reported `/tmp` symptom cannot have this cause.

> ~~After the cancel, the file [`/tmp/zoid-smoke-verify.txt`] was gone.~~

The test dispatched with `worktree: false` (default), so `wt` is `None` —
`drop(wt)` at line 147 is a **no-op**. Even with a worktree, teardown removes a
*git worktree directory*; it can never delete a file at an **absolute `/tmp`
path**. The reported "cancel deletes my `/tmp` file" observation is physically
impossible through this code path — almost certainly a misobservation, or the
file was never actually written (see Finding 4's hallucination case). The
`cancelled: 0` return the test noted is itself the tell: nothing was tracked to
kill, so nothing in this path ran.

### ❌ And the originally-proposed fix is broken.

> ~~Check `done.is_cancelled()` before `hard.is_cancelled()`:~~
> ~~`let res = if !done.is_cancelled() && hard.is_cancelled() { … }`~~

`done` (`spawn_subagent.rs:71`) is a **local supervisor-stop token**, cancelled
**unconditionally at line 105** (`done.cancel();`) on *every* return path —
normal completion or abort alike — to stop the `WakeTimer`. By the time line
113 runs, `done.is_cancelled()` is **always `true`**, so
`!done.is_cancelled() && hard.is_cancelled()` is **always `false`**. That fix
would silently **disable the idle/ceiling-timeout abort entirely**. The
original finding misread `done` as a "normal-completion signal"; it is not.

### ✅ Correct action: none.

There is no actionable defect. The `hard.is_cancelled()` override is
deliberate "kill-wins-over-graceful-drain" policy, the reported symptom is
impossible via this path, and the only residual imperfection is the sub-second
ceiling-boundary race the author already accepted in a comment. "Fixing" it
would invert the intended mid-run-kill semantics for a vanishing window with no
reproducible failure — a net regression. Leave the code as-is.

**Severity:** None (intended behavior). Originally reported as Medium data
loss; that symptom does not exist.

---

## Finding 6: Worktree tooling fix is correct at its layer (VALIDATED ✅)

The earlier "broken" finding was retracted. The worktree fix (`bf63407` merge)
is implemented and verified:
- 8/8 `compute_worktree_switch` unit tests pass
- 1/1 WT-1 regression test (`worktree_wt1_loop.rs`) passes
- The fix targets the agent-turn `cwd_for_exec` and `spawn_turn`'s per-turn
  cwd override — correct for its intended layer

The live harness test showed no relocation because direct `enter_worktree` /
`shell` calls don't flow through `run_agent_turn`. This is expected.

---

## Summary (post re-verification)

| # | Finding | Status | Real bug? | Fix priority |
|---|---------|--------|-----------|-------------|
| 1 | Single-flight is dispatch-only | ✅ Correct by design | No | — |
| 2 | "Two gates" hypothesis | ❌ Rejected — one registry | No | — |
| 3 | Stale in-flight slot after completion | ❌ **Struck — rig artifact** (reap is serialized before next dispatch) | No | — |
| 4 | Silent dispatch no-op | ⚠️ Symptom real; mechanism = **model hallucination**, not spawn-drop | Not a code bug | Verification layer (Medium) |
| 5 | Force-cancel destroys artifacts | ❌ **NOT a bug** — intended kill-wins policy; symptom impossible; proposed fix would regress | No | **None** |
| 6 | Worktree fix correct at its layer | ✅ Validated | No | — |

## Recommended action

**No product code changes are warranted by any finding.** Specifically:

1. **Finding 5:** leave `spawn_subagent.rs:113` as-is. The override is
   deliberate policy, not a defect; the reported symptom is impossible; and the
   proposed fix would keep partial work from mid-run kills (a regression).
2. **Finding 4:** the only *possible* follow-up is a verification-layer guard
   (post-turn check that claimed tool calls appear as `ToolCall`+`ToolResult`
   pairs) to catch model-hallucinated no-ops. This is a product *enhancement*,
   not a bug fix, and is optional.
3. **Finding 3:** no product change. Optionally fix the smoke-test rig to
   dispatch through the normal wake path so it stops manufacturing the race.

## Diagnosis-loop lessons (why the originals were wrong)

- Every overturned finding coupled a **true code observation** (the handle is
  dropped; `hard.is_cancelled()` is unconditional; `ui.send` is fire-and-forget)
  with an **unverified causal leap** to the symptom. The observations were
  accurate; the leaps were never falsified — the signature of a stuck agent.
- **Two of three "bugs" were measurement artifacts of the test rig**, not the
  product: Finding 3's race requires bypassing main-loop serialization, and
  Finding 5's symptom is impossible under `worktree: false`.
- Line-number drift mattered least: `spawn_subagent.rs` was untouched since
  `a9d303b` (Findings 4/5 anchors exact); `agent.rs`/`main.rs` shifted with the
  scheduled-wakeups merge, but the substantive errors were in the *reasoning*.

## Code references (re-verified at HEAD `634c114`)

| File | Lines | Role |
|------|-------|------|
| `crates/zoid/src/agent.rs` | 1424-1448 | Single-flight gate (`dispatch_subagent` checks `config.in_flight`) |
| `crates/zoid/src/spawn_subagent.rs` | 68 | `tokio::spawn` (detached — `JoinHandle` dropped; task NOT starved) |
| `crates/zoid/src/spawn_subagent.rs` | 71, 105 | `done` local supervisor-stop token; cancelled unconditionally on every path |
| `crates/zoid/src/spawn_subagent.rs` | 113-118 | `hard.is_cancelled()` override → **real TOCTOU** (Finding 5) |
| `crates/zoid/src/spawn_subagent.rs` | 147 | `drop(wt)` cleanup (no-op when `worktree: false` → `wt = None`) |
| `crates/zoid/src/spawn_subagent.rs` | 164-165 | `DelegationResult` appended + sent to `ui` (`let _ =` fire-and-forget) |
| `crates/zoid/src/main.rs` | 2761-2763 | `DelegationResult` reap (`app.in_flight.remove`) — synchronous |
| `crates/zoid/src/main.rs` | 2808-2809 | `plan_delegation_wake` → `spawn_turn` — **after** the reap, same arm |
| `crates/zoid/src/main.rs` | 5821 | `turn_config.in_flight = Some(app.in_flight.clone())` — shared Arc |
