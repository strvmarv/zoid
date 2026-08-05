# TODO — deferred work

## Empty-state guidance for new vs. returning users (DONE)

Implemented in `crates/zoid-tui/src/onboarding.rs` + `crates/zoid/src/main.rs`.
See `docs/superpowers/specs/2026-07-06-empty-state-guidance-design.md`.

## Tool-call approvals (DONE)

Implemented across `crates/zoid-tools/src/approval.rs` (BlacklistGate +
shlex matcher), `crates/zoid/src/agent.rs` (Gate::Prompt arm), and config/CLI
wiring. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`.

## Delete old sessions from the session picker (DONE)

Implemented — session picker supports deleting stale sessions.

## Reduce "thinking" output shown in normal zoom (DONE)

Thinking content is trimmed/collapsed at normal zoom so the main agent's
messages and tool calls stay visible without being drowned out by reasoning.

## Subagents not working correctly (DONE)

Fixed — concurrent subagent pool, per-result delegation wake, DelegationResult
delivery, cancellation paths, and animated drawer spinner all working.
See `docs/superpowers/specs/2026-07-25-concurrent-subagent-execution-design.md`.

## Session startup: parallelize projection passes (DONE)

The 5 independent O(n) projection passes (`conversation`, `context_window`,
`churn_timeline`, `tasks`, `token_ledger`) are now parallelized via
`std::thread::scope` in `ProjectionCache::refresh`
(`crates/zoid/src/main.rs:1458`). Wall-clock cost dropped from
sum(passes) to max(passes).

See `docs/superpowers/specs/2026-07-26-parallelize-projections-design.md`.
Commit `eaa311c`.

## Session startup: lazy-load the body cache (DEFERRED)

`conversation_view` wraps + syntax-highlights every message, but only the
visible viewport is painted. The remaining optimization is to render only
the visible window's messages on the first frame and build the rest on
demand when scrolling. The body cache already supports incremental
rebuilds — extend it to build only a window of lines instead of the full
transcript.

Windowing the event log itself is NOT needed — `Arc<Event>` means bodies
are shared, not copied, and the eviction policy already compacts old
events. The cost is the body cache build, not storage.

## Investigate aggressive context eviction (25-50k floor instead of 200-300k)

Context is being evicted down to 25-50k instead of the normal 200-300k
floor. This causes the model to lose large parts of its conversation history
mid-session — likely the root cause of the state confusion, duplicate
dispatches, and fragmented responses observed during long sessions.

**Root cause identified:** When `ModelInfoFetched` arrives (main.rs:3386),
`app.shell.ctx_ceiling` is set to the live-fetched `info.context_window`.
If the ollama-cloud API reports a smaller context window than the static
`MODEL_CAPS` table (e.g. 32K instead of 1M), the eviction band collapses:

- `EvictionPolicy.capacity` = 32K (from `ctx_ceiling`)
- `derive_band`: `effective_target` = min(300K, 32K - 8K) = 24K
- `low_water` = 24K - 4.8K = 19K
- Eviction fires at 24K, evicts down to 19K — only `recent_n: 4` turns
  protected.

The `context_target` config (300K) stays unchanged because it's `Some`
(line 3393 `unwrap_or_else` is skipped), but `capacity` overrides it in
`derive_band`.

Introduced by `e410be2` (Jul 25): changed the hard-ceiling pass to use
`config.context_window` (live-fetched) instead of the static table. The
eviction policy's `capacity` was always `ctx_ceiling`, but the live fetch
now overrides the 1M static value with whatever the API reports.

**Fix options:**
1. Use `max(ctx_ceiling, model_info(&model).context_window)` as the
   eviction `capacity` — the static table is the floor, the live fetch can
   only raise it.
2. When `ModelInfoFetched` sets `ctx_ceiling` below the static table's
   value, keep the static table's value instead (don't downgrade).
3. Clamp `context_target` down to `ctx_ceiling` when it arrives, so the
   band matches the actual capacity.

## Tool call rendering truncated too short in the UI (DONE)

Fixed — tool-call lines and result previews now use more of the available
column width before truncating, with peek for the full content.

## Add new providers

Add first-class providers for additional hosted LLM backends. Group by
implementation route — most are OpenAI-compat and can share work.

### OpenAI-compat (reuse `openai_compat.rs` — base-url + API-key config)

These speak the OpenAI Chat Completions shape and likely need only a config
preset pointing `openai_compat.rs` at the right base URL, not a new module:

- **OpenRouter** — aggregator exposing many models through one OpenAI-compat
  endpoint.
- **Together AI** — OpenAI-compat endpoint for hosted open models.
- **Perplexity** — OpenAI-compat endpoint; returns citations in the response,
  which could feed the web-search feature.
- **Fireworks AI** — OpenAI-compat, hosted open models including fine-tunes.
- **Hyperbolic** — OpenAI-compat, GPU-efficient open-model inference.
- **Novita AI** — OpenAI-compat, cheap hosted open models.
- **Lepton AI** — OpenAI-compat, fast open-model inference.
- **Predibase / LoRAX** — OpenAI-compat, fine-tuned open models.
- **DeepSeek** — OpenAI-compat endpoint; `reasoning_content` parsing already
  referenced in `lib.rs` comments.
- **Mistral (La Plateforme)** — OpenAI-compat endpoint for Mistral models.
- **Cerebras** — OpenAI-compat, fast inference.
- **Qwen (Alibaba / DashScope)** — Qwen models via DashScope's OpenAI-compat
  endpoint.
- **Kimi (Moonshot)** — Moonshot's OpenAI-compat endpoint. Note: `zai.rs`
  already targets Z.AI (GLM/Kimi's parent) via its own shim; confirm whether
  Kimi/Moonshot is already reachable through `zai` before adding a separate
  provider.
- **Cloudflare Workers AI** — OpenAI-compat, edge-hosted aggregator.

### Self-hosted / local (reuse `openai_compat.rs` — base-url only, no API key)

These run on the user's own hardware and expose an OpenAI-compat server.
Zero new code — just documented config presets — and high-value since users
already on the `ollama-local` path may run these instead:

- **vLLM** — OpenAI-compat server; popular for self-hosted GPU inference.
- **LM Studio** — desktop app exposing an OpenAI-compat local server.
- **llama.cpp server** — OpenAI-compat server mode.
- **llamafile** — single-file OpenAI-compat server.
- **Ollama local** — already supported via `ollama.rs` (`ollama-local` branch),
  but worth a config preset under the OpenAI-compat path too for parity.

### OpenAI-native (wire `openai_responses.rs`)

- **ChatGPT (OpenAI direct)** — the Responses API via `openai_responses.rs`,
  or `openai_compat.rs` for Chat Completions as a fallback.

### Native / separate module (evaluate case-by-case)

Decide per-provider whether a dedicated module is warranted or the
OpenAI-compat config preset suffices. DeepSeek and Qwen both also expose
native APIs with features (reasoning, long context) the compat shim may not
surface — flag here if the compat route turns out to lose capability.

- **Cohere** — Command R+ via Cohere's own (non-OpenAI) API; strong
  retrieval/tool-use would need a dedicated module. Also offers an
  OpenAI-compat endpoint now — evaluate whether the compat route loses the
  retrieval capability before deciding.

### Cloud-provider gateways (evaluate separately)

These are not model hosts — they're gateways/proxies fronting many models
(Anthropic, Meta, Mistral, etc.) behind a provider-specific auth and request
shape. Worth considering for enterprise users but a different problem from
the providers above: each has its own SDK/auth and request format, so a
dedicated adapter per gateway is likely required.

- **AWS Bedrock** — proxies Anthropic, Meta, Mistral, etc. Prefers IAM/SigV4
  auth, but supports API-key (access key + secret, or bearer tokens for some
  models) — the API-key route is closer to the other providers than a full
  SigV4 integration, though not AWS's preferred path. Uses its own
  request/response shape (not OpenAI-compat), so a dedicated adapter is still
  needed; also surface cross-provider model selection.
- **Azure AI Foundry (Azure OpenAI)** — OpenAI-compat with Azure auth
  (`api-version` query param, Entra ID key). Likely a thin `openai_compat`
  variant with Azure-specific auth.
- **Google Vertex AI** — proxies Gemini, Anthropic, and open models behind
  Google auth. Own request shape; dedicated adapter.
- **Vertex AI Model Garden** — hosts many open models behind Google auth.

## UI: move "SELECT" chip away from mode

The "SELECT" chip currently renders as part of the mode display. It should
be moved to a different location in the UI so it doesn't visually crowd the
mode indicator.

## UI: show "YOLO" chip/message when enabled

When YOLO mode (auto-approve all tool calls) is active, there's no visible
indicator in the UI. Add a chip or status message so the user can see at a
glance that approvals are bypassed.

## Agent `model` field not seamed (runtime honors it)

The agents-as-entity design (`docs/superpowers/specs/2026-07-23-agents-as-entity-design.md`
§"Seamed Fields") specifies that `model` is parsed and stored on the
`AgentProfile` but **not honored** at runtime — the subagent should always
inherit the orchestrator's model. The runtime diverged: `subagent.rs:133`
does `let model = profile.model.clone().unwrap_or(default_model);`, which
uses the profile's `model` string as a literal model name sent to the
provider. An agent file with `model: inherit` 404s at the provider
(`model 'inherit' not found`).

**Fix:** either honor the spec (make `model` truly seamed — always use
`default_model`, ignore `profile.model`) or update the spec to say the
field is honored and document the contract. The seamed behavior is safer
(an agent file can't accidentally 400 the session by naming a nonexistent
model); honoring it is more flexible (per-agent model overrides) but
needs validation against the live model list.

## `exit_worktree` intermittently deletes branches with unmerged commits

**Status:** Unreproduced in tests. Diagnostic logging added (commit `f57cb40`).
The bug is real — it happened twice in practice (the `ollama-local-thinking`
and `local-models-phase1` worktrees) — but the code is correct in every test
scenario.

**What happened:** `exit_worktree` returned `"exited worktree"` (no "retained"
warning), meaning `branch_has_unmerged_commits` returned `false` for a branch
that had 4 unmerged commits on top of main HEAD. The branch was deleted by
`remove_worktree(repo_root, name, delete_branch=true)`. Recovery required
cherry-picking from dangling commits found via `git fsck --lost-found`.

**The code path** (`crates/zoid/src/main.rs:6957-6988`):

```
WorktreeAction::Exit → is_worktree_clean(&wt.path)?
  → branch_has_unmerged_commits(repo_root, &wt.name)  ← returned false (wrong)
  → remove_worktree(repo_root, &wt.name, !false = true)  ← deleted the branch
  → Ok((root, None))  ← no "retained" warning
```

**`branch_has_unmerged_commits`** (`crates/zoid/src/worktree.rs:133-156`):
opens the repo at `repo_root`, finds the branch by name, gets the branch tip
OID, resolves main HEAD via `commondir()`, and returns
`graph_descendant_of(branch_oid, head_oid)`. If any step fails, it returns
`false` (conservative — don't block cleanup).

**`repo_root` in production:** `handle_worktree_request` passes
`Path::new(".")` (main.rs:7004). The process cwd never changes (`set_current_dir`
is never called), so `.` is the main checkout. Tests confirm this path works.

**Tests added** (`crates/zoid/tests/worktree_test.rs`):
- `exit_worktree_retains_branch_with_unmerged_commits` — full enter→commit→exit
  round-trip, asserts branch retained. **Passes.**
- `remove_worktree_from_inside_worktree_path` — `remove_worktree` called with
  the worktree path as `repo_root`. **Passes.**
- `branch_has_unmerged_commits_detects_from_inside_worktree` (pre-existing) —
  tests the `commondir()` fix. **Passes.**

**Hypotheses (untested):**

1. **Race condition:** `handle_worktree_request` runs in the UI event loop.
  Subagent commits may not be fully flushed to git's object store by the time
  `graph_descendant_of` runs. The subagent's `git commit` and the UI loop's
  `branch_has_unmerged_commits` could race on the same `.git` directory.

2. **`git2` branch visibility:** `find_branch(name, Local)` on the main repo
  might not see a branch created by `git2::Repository::worktree()` in a prior
  event-loop iteration if git2 caches the ref list. The `find_branch` returning
  `Err` causes `branch_has_unmerged_commits` to return `false` (line 137-139).

3. **`main_head_oid` returns wrong OID:** `commondir()` resolution might return
  a stale or worktree HEAD in some edge case, making `graph_descendant_of` a
  self-comparison (branch vs itself) which returns `false`.

**Diagnostic logging added** (main.rs:6995-7001): when `has_unmerged` is false
and the branch is deleted, the return message now includes `branch_oid` and
`head_oid` — e.g. `"exited worktree (branch 'foo' deleted — no unmerged commits
detected; branch_oid=Some(...) head_oid=Some(...))"`. This is visible in the
tool result (not just `tracing::warn`), so no `ZOID_LOG` env var is needed.
Next time this happens, the agent sees the OIDs directly in the `exit_worktree`
tool result and can determine which hypothesis is correct.

**Mitigation until fixed:** when exiting a worktree with unmerged work, check
the tool result for the "retained" warning. If absent, the branch was deleted —
recover with `git fsck --lost-found` and cherry-pick the dangling commits.