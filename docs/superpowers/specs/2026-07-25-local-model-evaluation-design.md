# Local Model Evaluation for zoid — Design

**Date:** 2026-07-25
**Status:** Approved for planning
**Scope:** Evaluate whether a locally-hosted model can drive zoid's agent loop on this
workstation, and fix the one defect that would invalidate any such measurement.

## Motivation

A new class of open-weight coding models shipped in mid-2026 that vendors advertise as
locally runnable. Two architectural shifts drive this, and they pull in opposite
directions on 12 GB of VRAM:

- **Small-active MoE.** `north-mini-code-1.0` (30B-A3B) and `laguna-xs-2.1` (33B-A3B)
  activate ~3B parameters per token, so generation speed survives CPU offload. But total
  weights still must be resident somewhere: 19–20 GB at Q4.
- **Ternary quantization.** `Ternary-Bonsai-27B` stores weights as {−1, 0, +1} at a true
  1.71 bits/weight — a 27B-class model in 7.17 GB, entirely on-GPU.

This box is below the MoE threshold and comfortably above the ternary one. Which bet wins
here is an empirical question, and the purpose of this work is to answer it with
measurements rather than vendor claims.

## Hardware envelope (measured 2026-07-25)

| Resource | Value | Usable |
|---|---|---|
| GPU | NVIDIA RTX 3060, driver 610.43.03 | **12,288 MiB** VRAM (~11.2 GB practical) |
| CPU | Intel i5-14500, 6 cores / 6 threads | — |
| RAM | 23 GiB total | ~17 GiB free |
| Disk (`/home`) | 508 GB | 210 GB free |
| Ollama | 0.21.1 at `/usr/local/bin/ollama` | `devstral` (14 GB), `qwen2.5-coder:14b` (9 GB) pulled |

Combined VRAM + free RAM is ~28 GB. That is the hard ceiling for any weights + KV cache
combination, and it is why the 19–22 GB MoE candidates are viable only with offload.

## Current state of zoid's local path

The integration already exists and is more complete than assumed:

- `crates/zoid-model/src/lib.rs:88` — `ollama-local` is a first-class registry entry,
  `Status::Available`, `default_base_url: "http://localhost:11434"`, `models: &[]`
  (local tags are free-text, so the picker does not constrain them).
- `crates/zoid/src/main.rs:1046` — special-cased to construct with an empty API key and
  report ready, since localhost needs no auth.
- `crates/zoid-provider/src/ollama.rs` — `list_models` via `/api/tags`, `fetch_model_info`
  via `/api/show`, tool calling wired through native `/api/chat`.

**No new provider is needed.** This effort is measurement plus one defect fix.

## The defect: `num_ctx` is never sent

`ollama::request_body` (`crates/zoid-provider/src/ollama.rs:58-68`) emits only `model`,
`stream`, `messages`, `keep_alive`, `think`, and `tools`. It never sends `options.num_ctx`.

Against Ollama Cloud this is correct — the server owns context sizing. Against a local
daemon it is not: Ollama applies its own server default and then **silently truncates**
an over-long prompt rather than erroring. Three consequences:

1. `is_context_length_error` (`crates/zoid-provider/src/lib.rs:343`) never fires, because
   there is no error to detect.
2. The first content evicted is the system prompt and tool schemas — the model loses its
   instructions and its tools while continuing to emit fluent prose.
3. `fetch_model_info` reads `/api/show` and reports the model's **trained** context as the
   ceiling. Verified against the local daemon: `qwen2.5-coder:14b` reports
   `qwen2.context_length: 32768` with `parameters: None`. That 32768 flows into
   `context_ceiling`, so the economy ⑤ gauge reads confidently wrong.

### Measured fixed overhead

`zoid_tools::registry()` returns **13 tools serializing to 6,112 bytes ≈ 1,700 tokens**
(heaviest: `edit` 778 B, `submit_feedback` 731 B, `web_fetch` 648 B). Plus the ~130-token
`SYSTEM_PROMPT` (`crates/zoid/src/agent.rs:36`), every local turn carries **~1,850 tokens
of fixed overhead** before the user types anything.

Against a 4K default that is ~45% of the window consumed at rest, with the remainder
exhausted by the first `read` of a real source file. Against 32K it is comfortable. The
fix is cheap; it simply has to exist.

## Goals

- Determine whether any locally-runnable model can drive zoid's agent loop well enough to
  be useful, and if so which.
- Fix `num_ctx` so that measurements — and later real sessions — are not confounded by
  silent truncation.

## Non-goals

- Replacing the cloud provider. This is evaluation; adoption is a later decision.
- A second local transport (llama.cpp + `openai_compat`). Deferred unless Track A shows
  Ollama-native cannot carry a candidate that otherwise wins.
- Any change to provider selection, the model picker, or config schema beyond `num_ctx`.

## Track A — Out-of-zoid benchmark harness

Lives in the session scratchpad. **Makes no repository changes**, so it cannot collide
with Track B.

### Fidelity principle

The benchmark must measure what zoid would actually send. Rather than hand-writing a test
prompt, generate **one golden request body** from zoid's own code —
`zoid_tools::registry()` → `agent::tool_specs()` → `ollama::request_body()` — dump it to
JSON once, then replay it per-model with only the `model` field and `options.num_ctx`
swapped. This guarantees the 6,112 bytes of tool schemas are present byte-for-byte.

Responses are validated by feeding them back through zoid's own
`ollama::parse_line`, so "did it work" means "would zoid have understood it" rather than
"did it look plausible."

### Step 0 — Confirm the truncation premise

Before any model is benchmarked, verify the defect empirically against the control model:
send the golden body with no `options.num_ctx`, then with an explicit large one, and
compare the reported `prompt_eval_count`. If the unset case reports a materially lower
prompt-token count, truncation is confirmed and Track B is justified by measurement rather
than by reasoning about Ollama's documented behavior. Also record Ollama 0.21.1's actual
default, which is the number the rest of the analysis depends on.

This is cheap, uses a model already pulled, and prevents building both tracks on an
assumption.

### Metrics

| # | Metric | Source | Gate |
|---|---|---|---|
| 1 | Loads; VRAM/RAM split | `ollama ps`, `nvidia-smi` | Must load |
| 2 | Prompt-eval and generation tok/s | `prompt_eval_count`/`_duration`, `eval_count`/`_duration` on the final NDJSON frame | ≥ 10 tok/s generation |
| 3 | **Tool-call correctness** | Response replayed through `ollama::parse_line`; must yield a well-formed `ToolCall` | **Hard gate — pass/fail** |
| 4 | Max `num_ctx` that loads and sustains metric 2's throughput | Bisect `options.num_ctx`, observe the VRAM/RAM split and re-measure tok/s at each step | ≥ 32K |
| 5 | Multi-turn survival | 5-step scripted tool loop | Stays coherent, keeps calling tools |

Metric 3 is the decisive one. A model that scores well on SWE-bench but cannot emit a
tool call zoid's parser accepts is unusable here, and no other metric compensates.

KV-cache footprint is deliberately **measured by bisection rather than computed** — it
depends on layer count, GQA head configuration, and per-model KV quantization
(`laguna-xs-2.1` ships an FP8 KV cache), and an arithmetic estimate would be a guess
dressed as a number.

### Candidates

| Model | Q4 size | Fits 11.2 GB? | Context | Tools declared | Claim (vendor-reported) |
|---|---|---|---|---|---|
| `ornith:9b` (DeepReinforce, MIT) | 5.6 GB | Yes, ~5.6 GB KV headroom | 256K | **Not in Ollama's tools list** — see risk | SWE-bench Verified 69.4% |
| `Ternary-Bonsai-27B` (prism-ml) | 7.17 GB | Yes, ~4 GB KV headroom | 262K | Tool-use bench 74.01 | HumanEval+ 93.9, LiveCodeBench 82.75 |
| `north-mini-code-1.0` (Cohere) | 19 GB | No — offload | 488K | Yes, + interleaved thinking | AA Coding Index 33.4 |
| `laguna-xs-2.1` (poolside) | 20 GB | No — offload | 256K | Yes, + interleaved thinking | SWE-bench Multilingual 63.1% |
| `qwen2.5-coder:14b` **(control)** | 9.0 GB | Yes, KV-starved (32K trained max) | 32K | Yes — *verified locally* | baseline |

Only the control row is verified; every other figure is vendor-reported and is treated as
a hypothesis to be tested, not an input. A 9B model claiming 69.4% on SWE-bench Verified
is an extraordinary claim and should be weighted accordingly.

Pull cost is ~52 GB against 210 GB free.

`Ternary-Bonsai-27B` is not in the Ollama library and requires an `hf.co/` pull or a
Modelfile. Note also that `unsloth` publishes dynamic quants (e.g. `UD-IQ2_XXS` at
11.5 GB) for several MoE candidates; if metric 4 fails at Q4, these are the fallback rung
before declaring a model out of reach.

### Excluded candidates

**`Qwen-AgentWorld-35B-A3B`** — excluded despite matching the small-active-MoE shape and
carrying an Apache 2.0 license. It is a *world model*, not an agent: it simulates agentic
environments across seven domains, predicting the next environment state given an action
and interaction history, and its card states it does not support tool calling in the
conventional sense. zoid needs a model that **emits** tool calls; AgentWorld is built to
emit the tool **results**. It is also 22 GB at Q4, the largest candidate considered.

## Track B — The `num_ctx` fix

Runs in parallel in a git worktree, TDD, dispatched to an agent.

1. **Emit `options.num_ctx`** in `ollama::request_body`.
2. **Source it from config/env** — `ZOID_NUM_CTX`, following the existing
   `ZOID_CONTEXT_CEILING` / `ZOID_HTTP_IDLE_SECS` idiom in `lib.rs:44` (positive integer,
   else default). Explicit and defaulted, never left to the server.
3. **Make `fetch_model_info` honest for `ollama-local`** — the usable ceiling is what we
   requested, not what the weights support. `/api/show`'s trained context must not be
   reported as the economy denominator.

### Cloud safety

`num_ctx` is meaningful for a local daemon and may be ignored or rejected by Ollama Cloud.
The emission must therefore be **conditional on the provider variant**. The registry
already distinguishes `ollama-local` from `ollama-cloud`
(`crates/zoid-model/src/lib.rs:88,99`), both in the `ollama` family, so the discriminator
exists and no new concept is required. `ollama-cloud` request bodies must remain
byte-identical to today's — this is a regression test, not a comment.

### Testing

- `request_body` emits `options.num_ctx` for local and omits it for cloud.
- `ZOID_NUM_CTX` parses like its sibling env vars; invalid and zero values fall back.
- Cloud body byte-identical to the pre-change body (guards the seam).
- `fetch_model_info` does not report a ceiling above the requested `num_ctx` for local.

## Decision rule

Track A produces a table, and the outcome is one of:

- **A candidate passes metrics 1–5** → local coding is viable here; proceed to a real zoid
  session against it and decide adoption on felt quality.
- **Candidates pass 1, 2, 4 but fail 3 (tool calling)** → the blocker is packaging, not
  capability. Revisit the deferred llama.cpp + `openai_compat` transport, which sidesteps
  Ollama chat-template gaps.
- **Only the control passes** → this hardware is below the bar for current agentic coding
  models. Record the numbers and revisit when the next quantization or MoE generation
  lands. Track B's fix is retained regardless, since it is a correctness bug.

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| `ornith` Ollama tag lacks a tools template | Best-fitting candidate unusable via Ollama | Confirmed absent from Ollama's `c=tools` listing while present unfiltered. Model itself supports OpenAI-compatible tool calling (works with OpenHands, OpenCode, llama.cpp), so the gap is packaging. Fallback: custom Modelfile, or llama.cpp transport. |
| MoE offload too slow to be usable | Two candidates eliminated | Accept it — that is a valid finding, recorded as tok/s. Try unsloth dynamic quants before declaring failure. |
| Ternary Bonsai weak in multi-turn agentic loops | Strong benchmarks, poor real behavior | Its card admits agentic coding is not a primary focus. Metric 5 exists precisely to catch this. |
| Vendor benchmark numbers unreproducible | Wasted pull time | Expected. Metrics are measured locally; vendor claims only prioritize pull order. |
| Track A and Track B conflict | Lost work | Track A is scratchpad-only and touches no repository file. Track B runs in a worktree on its own branch. |

## Open questions

- Should `ZOID_NUM_CTX` have a config-file counterpart alongside the env var, matching how
  `base_url` is surfaced in `zoid-core/src/config.rs:37`? Deferred to the implementation
  plan.
- If a local model wins, does it become a *mode* (per-mode provider) rather than a global
  provider switch? Out of scope here; noted because the MODES seam work is already queued.
