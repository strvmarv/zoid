# zoid spike — Claude Code as a streaming inference endpoint (B')

Decision experiment: can `claude -p --output-format stream-json` serve as a
subscription-backed inference call replacing zoid's `AnthropicProvider` —
keeping zoid's agent loop, tools, ACM, and multi-provider story intact —
while billing to the flat-rate Claude Code subscription instead of the
metered API?

Dev box: `claude` 2.1.201 (Claude Code), Arch Linux, OAuth subscription
(active, in 7-day overage at time of spike).

## Falsification targets vs. results

| # | Claim | Result | Verdict |
|---|---|---|---|
| 1 | `claude -p --output-format stream-json --verbose` emits usable NDJSON streaming events | Yes — one JSON object per line, events arrive as they happen (`system`, `assistant`, `result`, `rate_limit_event`, `hook_*`). | ✅ confirmed |
| 2 | `--allowedTools ""` / `--tools ""` forces infer-only — no tool execution, returns `tool_use` blocks for zoid to run | **No.** `--tools ""` clears the *model's* tool set (the model says "I can't do that, no Bash tool"), but Claude Code **never returns a `tool_use` block to the caller for execution**. It either executes the tool itself (and emits the `tool_result` inline in the stream) or refuses. There is no "infer-only, hand me the tool_use" mode. | ❌ **falsified** |
| 3 | The stream emits token-level `text` deltas that map onto zoid's `ProviderEvent::TextDelta` | **No.** `stream-json` streams at the *event* level (thinking deltas, then one final `text` block), NOT at the token level. The entire assistant text arrives in a single `{"type":"text","text":"..."}` block at the end of the turn. No incremental `TextDelta` chunks. zoid's streaming TUI rendering would degrade to "wait for the final blob." | ❌ **falsified** |
| 4 | The stream carries `usage` (input/output/cache) zoid's economy drawer needs | Yes — both the `assistant` `message.usage` blocks and the final `result.usage` carry `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`. Maps cleanly onto zoid's `Usage`. | ✅ confirmed |
| 5 | TTFT + startup latency is tolerable (sub-second, not multi-second) | **No.** `time claude -p … "pong"`: real **8.8s** total, `ttft_ms: 7180`, `ttft_stream_ms: 1781`. A longer prompt ran 11.5s and 20.5s. Process startup + Claude Code's own session init (hooks, plugin sync, MCP connect) dominates. zoid's `AnthropicProvider` TTFT is typically <1s. | ❌ falsified (by a lot) |
| 6 | It works against the subscription (OAuth), not just an API key | Yes — `apiKeySource: "none"` across all non-`--bare` runs. The subscription is used. `--bare` (which would skip the heavy init) forces `ANTHROPIC_API_KEY` and refuses OAuth, so the two optimizations are mutually exclusive. | ✅ confirmed (but see #5 + the `--bare` tension) |
| 7 | A pure stdin/stdout contract works — no hidden TTY/state requirements | Mostly yes — stdin prompt + stream-json on stdout works headlessly. But: cwd becomes the session's `cwd` (CLAUDE.md auto-discovery fires), `SessionStart` hooks fire (the total-recall plugin's `session_start` ran and stored a 50KB memory dump into the stream as a `tool_result`), and MCP servers auto-connect. The "clean inference call" is not clean. | ⚠ partial |

## The load-bearing finding — B' is structurally impossible, not just slow

The spike falsified the core premise of B' on **claim 2**. zoid's agent loop
is built on the contract that the *provider* returns `ProviderEvent::ToolCall`
and the *agent* executes the tool (see `zoid-provider/src/lib.rs:135` —
`ProviderEvent::ToolCall(ToolCall)`). The whole point of B' was "zoid stays
its own agent; Claude Code is just the inference call."

But `claude -p` is an **agent**, not an inference endpoint. Its semantics are:

- If a tool is available and the model decides to call it → Claude Code
  **executes the tool itself** and emits the `tool_result` inline. The caller
  never sees a `tool_use` to act on.
- If the model's tool set is cleared (`--tools ""`) → the model refuses with
  text ("I can't do that, no Bash tool"). It does **not** return a `tool_use`
  for the caller to run. There is no "return tool_use, don't execute" mode.
- MCP/plugin tools leak through `--tools ""` (the total-recall MCP tools were
  still callable) and are executed inline by Claude Code.

There is no invocation contract where `claude -p` acts as a pure Messages-API
stand-in: take a prompt, return `tool_use` blocks, let the caller execute.
That is the layer B' required, and it does not exist. What exists is
Architecture A (Claude Code is the agent) wearing a stream-json costume.

This is not a limitation we can engineer around in zoid. The CLI's contract is
"agent or nothing," and that contract is set by Anthropic.

## Secondary findings (would matter even if #2 held)

- **No token streaming.** `stream-json` is event-streamed, not token-streamed.
  zoid's whole TUI is built around incremental `TextDelta` events feeding the
  spinner + live render. B' would force a "render the whole turn when it
  lands" UX, which is a real degradation, not a cosmetic one.
- **~8s TTFT for "pong."** Claude Code's session init (hooks, plugin sync, MCP
  connect, CLAUDE.md discovery) runs on every invocation. `--bare` skips it
  but forces API-key auth, killing the subscription benefit. So you can have
  fast startup *or* subscription billing, not both.
- **18,726 cache_creation tokens of overhead** on a 10-token "pong" prompt.
  Claude Code injects its own system prompt + tool catalog + plugins into
  every call. zoid's economy drawer would show ~99.95% of tokens as
  Claude-Code-imposed overhead, not the user's prompt. The drawer becomes
  meaningless.
- **`--bare` vs subscription are mutually exclusive.** `--bare` (the only
  flag that would tame the init overhead and skip the MCP/plugin leakage)
  hard-requires `ANTHROPIC_API_KEY` — OAuth is refused. So the two
  optimizations that would make B' viable can't coexist with the one reason
  to choose B' (flat-rate sub).
- **ToS risk is unmeasurable but real.** Using `claude -p` as a
  metered-free inference endpoint to replace a paid API is the kind of thing
  Anthropic can shut off without notice. The spike can't falsify this; it's
  a product/legal risk that survives any technical result.
- **Hooks + MCP leak into the stream.** The total-recall `SessionStart` hook
  fired on every `claude -p` call and wrote a 50KB memory dump into the
  stream as a `tool_result`. A "clean inference call" would need
  `--no-hooks`-style flags that don't exist as a stable contract.

## Verdict

**B' is falsified. The premise does not hold.**

The core finding (claim 2) is structural, not a performance tuning problem:
`claude -p` is an agent that executes its own tools, not an inference
endpoint that returns `tool_use` blocks for the caller to run. There is no
flag combination that makes it "infer only, hand me the tool calls." That
collapses B' back into Architecture A (Claude Code is the agent), which is a
different design with a different thesis — and not the one the user wanted.

Even setting aside the structural falsification, the secondary findings (no
token streaming, 8s TTFT, 18k-token overhead, `--bare` vs subscription
mutually exclusive, hooks/MCP leakage) each independently make B' a worse
experience than zoid's existing hand-rolled `AnthropicProvider`. B' would
trade a working streaming agent for a non-streaming, slow, leaky one — to
save API cost. The cost saving is real (claim 6 held), but the price is
zoid's identity as its own agent plus its streaming UX.

## What survives for the spec path

- **Path 4 (Architecture A — Claude Code is the agent) is technically
  viable.** stream-json works, usage is exposed, subscription auth holds, the
  stream is parseable. If zoid ever wants a "drive Claude Code" mode, the
  plumbing is proven. But that's a different brainstorm and a different
  thesis (zoid becomes a TUI for Claude Code, not its own agent).
- **Path 3 (Rust Anthropic crate inside the `Provider` seam) remains the
  natural next step for zoid-as-agent.** It closes the real tool-use gap
  (the "capability lie" at `zoid-model/src/lib.rs:113`) without redefining
  what zoid is. The spike didn't touch Path 3; it's still the contained,
  high-value next step.
- **The flat-rate-subscription goal is not reachable via B'.** If it's a
  product priority, the path is Architecture A (drive Claude Code, accept
  it's the agent) or wait for Anthropic to offer a subscription-billed API
  tier (out of zoid's control).

**Recommendation: shelve B'. Return to Path 3 (Anthropic Rust crate /
typed internal submodule inside the `Provider` seam) as the next spec, or
open a fresh Architecture-A brainstorm if "zoid as a Claude Code TUI"
becomes the thesis. Do not spec B'.**