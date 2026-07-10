# Web search/fetch tooling — DuckDuckGo search + readability-fetch with char paging

Date: 2026-07-09
Status: Design (approved in brainstorm; awaiting spec review → writing-plans)
Extends: `2026-07-08-tool-approvals-design.md` (ToolGate/Gate seam), the curated tool registry (`zoid-tools/src/lib.rs`)

## 1. Problem

zoid's agent can read files, run shell commands, and search the repo, but it has no web access — it cannot look up a fact, read a docs page, or ground an answer in a current source. This slice adds the full research loop: **search** the web for results, then **fetch** a result's page and read its content. The model drives the loop (search → pick a result → fetch → page through it), exactly as a developer would.

The two operations are outward-facing (they make network requests to URLs/queries the model chooses), so this slice also resolves how the agent loop executes async, non-MCP tools — today the only async tool path is `ToolKind::Mcp`, tightly coupled to MCP servers.

### Key decisions (brainstorm-settled)

| Decision | Choice | Rationale |
|---|---|---|
| Scope | Full research loop (search + fetch) | Most capable; the model drives search → fetch → page. |
| Execution seam | New `ToolKind::Network` (async) + agent-loop arm | Faithful to the existing `ToolKind` pattern; no change to `Local`/`Emitting`/`Interactive`/`Mcp`. |
| Search backend | DuckDuckGo HTML scrape (`html.duckduckgo.com/html`), zero config, no API key | Credential-free; no new secret in the store. Brittleness accepted. |
| Fetch extraction | Readability extraction + HTML→markdown | Best signal-to-noise for the model. |
| Fetch paging | Char offset/limit (like the `read` tool) + heading outline on the first fetch | Deterministic paging; the model can jump to the right section instead of blindly paging. |
| Approval gate | Auto-allow by default (no prompt) | Read-only GETs are a lower risk class than the destructive shell commands the approvals spec targets. Deliberate, documented departure from the approvals spec's "outward-facing should prompt" principle (see §5). |
| Module shape | Separate `zoid-web` leaf crate; tools are thin shells in `zoid-tools` | Cleanest isolation of the brittleness (DDG scrape, HTML parsers). Matches the `zoid-model` leaf-crate precedent. |

## 2. Goals / Non-goals

### In scope (this spec → one plan)
- A new `ToolKind::Network` + an async trait method on `Tool` + a new agent-loop dispatch arm.
- A new `zoid-web` leaf crate: shared HTTP client, DuckDuckGo HTML search, readability extraction, HTML→markdown, heading-outline builder, char-offset paging.
- Two thin-shell tools in `zoid-tools`: `web_search` (query → numbered result list) and `web_fetch` (url + offset/limit → paged markdown + outline).
- Registry wiring (`registry()`, `registry_with_kill()`); the `registry_has_unique_named_tools` test gains the two names.
- URL-scheme validation (http/https only — no `file://`/`data://` exfiltration), offset-past-end error, empty-content error.
- Offline tests: fixture-HTML parsing, `TcpListener` stub fetch round-trips, pure extraction unit tests, agent-loop hard-cancel of a `Network` tool. No live-endpoint CI.
- An `#[ignore]` live smoke test (gated, never run in CI) for manual DDG/fetch verification.

### Out of scope (separate future specs)
- **Fetch caching across calls in a turn.** Re-GETting the same URL to page is cheap but wasteful; a per-turn URL→content cache is a follow-up.
- **Search backend abstraction.** v1 is DuckDuckGo-only. A `SearchBackend` trait + pluggable backends (SearXNG, Tavily) is a follow-up if DDG brittleness bites.
- **Rate-limit backoff.** A 429 from DDG surfaces as an error; automatic retry/backoff is a follow-up.
- **Web mutation (POST/PUT).** Fetch is GET-only by design. Any web mutation is out of scope and would belong behind the approval gate.
- **Approval prompts on web tools.** Auto-allow is the v1 default (see §5). Wiring web tools into `BlacklistGate`/`Gate::Prompt` is a follow-up if the risk profile changes.
- **New `ProviderEvent` variants.** Tool results flow back as `ToolResult` events exactly like Local tools — no event-surface changes.

## 3. Current state (what exists)

- `crates/zoid-tools/src/lib.rs` — `Tool` trait (`name`/`spec`/`run`/`kind`), `ToolKind` (`Local`/`Emitting`/`Interactive`/`Mcp`), `ToolOutput` (`ok`/`err`), `ToolGate`/`Gate` (`Allow`/`Deny`/`Prompt`), `run_tool()` (sync dispatch by name), `registry()`/`registry_with_kill(kill)` (curated 10-tool set), `str_arg`/`resolve`/`skip_entry`/`walk_files` helpers.
- `crates/zoid-tools/src/*.rs` — one module per tool (`read`, `write`, `edit`, `search`/`grep`, `glob`, `ls`, `shell`, `tasks`/`update_tasks`, `ask`/`ask_user`, `feedback`/`submit_feedback`).
- `crates/zoid/src/agent.rs` — `run_turn_inner`'s per-call loop dispatches on `ToolKind`: `Local` → `spawn_blocking` + `run_tool`; `Emitting` → UI event arms (`update_tasks`/`recall`/`show`/`dispatch_subagent`); `Interactive` → `ask_user` park/resume; `Mcp` → async `McpManager` call with `hard.cancelled()` tokio::select. `gate.check(&tc)` runs before each call.
- `crates/zoid-tools/src/approval.rs` — `BlacklistGate` (dangerous-shell patterns; `curl -X`/`-d`, `wget --post-*` flagged). The approvals spec's guiding principle: prompt only for actions that reach outside the cwd / are irreversible / are outward-facing.
- `crates/zoid/src/github_fetch.rs`, `update.rs` — existing `reqwest` HTTP code in the bin crate (not reusable as a tool dep; informs the client shape).
- Workspace: `zoid-tools` depends on `zoid-provider` (for `ToolSpec`/`ToolCall`); a new `zoid-web` leaf has no such coupling.

## 4. Design

### 4.1 The async `ToolKind::Network` seam

**`zoid-tools/src/lib.rs`** — add the kind and an async trait method:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Local,
    Emitting,
    Interactive,
    Mcp,
    /// Async HTTP (web_search, web_fetch). run_async(), not run().
    Network,
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput;
    fn kind(&self) -> ToolKind { ToolKind::Local }
    /// Async execution for `ToolKind::Network` tools. The agent loop only calls
    /// this in the Network arm; the default panics so a sync tool that wrongly
    /// returns Network fails loudly instead of silently doing nothing.
    fn run_async(&self, _args: &Value, _cwd: &Path)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolOutput> + Send + '_>>
    {
        Box::pin(async { panic!("run_async called on non-Network tool {}", self.name()) })
    }
}
```

The boxed-future signature (`Pin<Box<dyn Future<Output = ToolOutput> + Send + '_>>`) is the standard stable-Rust pattern for an optional async trait method without forcing all impls through `async-trait`. It touches none of the 10 existing sync tools (their `kind()` returns `Local`; `run_async` is never called on them). The `run()` method on `Network` tools is unreachable in practice (the agent loop never calls `run` for `kind() != Local`); it panics with a clear message for safety.

**`crates/zoid/src/agent.rs`** — a new dispatch arm alongside `Mcp`/`Local`, inside the per-call loop after `gate.check`:

```rust
Some(zoid_tools::ToolKind::Network) => {
    // async, in-task — no spawn_blocking. Reuses the hard-cancel tokio::select
    // pattern from the Mcp arm so a stuck fetch is abandonable on Esc.
    let tool_for_async = tools.clone();
    let name = tc.name.clone();
    let args = tc.args.clone();
    let cwd = cwd_for_exec.clone();
    let out = tokio::select! {
        biased;
        _ = hard.cancelled() => zoid_tools::ToolOutput::err("[killed: hard-stop]"),
        o = async move {
            match tool_for_async.iter().find(|t| t.name() == name) {
                Some(t) => t.run_async(&args, &cwd).await,
                None => zoid_tools::ToolOutput::err(format!("unknown tool: {name}")),
            }
        } => o,
    };
    // ... same ToolResult emit + logging as the Local arm ...
}
```

The find-by-name + `run_async` mirrors the `Local` arm's `tools.clone()`/`name`/`args`/`cwd` capture and `run_tool`'s find-or-`unknown-tool` shape. No change to `Local`/`Emitting`/`Interactive`/`Mcp` arms.

### 4.2 The `zoid-web` leaf crate

New crate at `crates/zoid-web`, added to `Cargo.toml` `[workspace] members`. A pure leaf — depends only on `reqwest`, `serde_json`, `anyhow`, `tracing`, and (test) `tokio` with `net`+`io-util`. **No dependency on `zoid-tools`, `zoid-core`, `zoid-provider`, or the agent loop.** Matches the `zoid-model` leaf-crate precedent.

**`Cargo.toml` (`crates/zoid-web/Cargo.toml`):**
```toml
[package]
name = "zoid-web"
version.workspace = true
edition.workspace = true

[dependencies]
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
# HTML parser + readability + markdown deps: settled during implementation plan
# (candidates: scraper/readability + html2md/htmd, or a combined crate).

[dev-dependencies]
tokio = { workspace = true, features = ["net", "io-util", "macros", "rt"] }
```

The exact readability + HTML→markdown crates are selected during the implementation plan against maintenance status, MSRV compatibility, and the narrowness of the readability-produced HTML subset (a small hand-rolled converter may suffice and avoid a heavy dep). The selection does not affect the public API shape.

**Module layout (`crates/zoid-web/src/`):**

1. **`lib.rs`** — crate root. Re-exports `search`, `fetch`, `SearchResult`, `FetchResult`. Holds the shared `reqwest::Client` (connect timeout; a `User-Agent` identifying zoid so DDG doesn't block the default reqwest UA) and the idle-timeout helper. Public API:
   ```rust
   pub async fn search(query: &str) -> anyhow::Result<Vec<SearchResult>>;
   pub async fn fetch(url: &str, offset: usize, limit: usize) -> anyhow::Result<FetchResult>;
   ```

2. **`search.rs`** — DuckDuckGo HTML scrape. `search(client, query)` POSTs form-encoded `q=<query>` to `https://html.duckduckgo.com/html/`, parses up to 8 `SearchResult { title, url, snippet }` from the returned HTML using the HTML parser dep (not regex — DDG's HTML is nested). Returns `Err` on non-2xx/empty. Empty/whitespace query → `Err` before the network call.

3. **`fetch.rs`** — the fetch + extraction pipeline. `fetch(client, url, offset, limit)`:
   - Validates the URL scheme is `http`/`https` (rejects `file://`, `data:`, etc. with `Err`).
   - GETs the URL with the shared client; non-2xx → `Err` with status + body snippet.
   - Extracts readable content (readability: drop nav/ads/script/boilerplate).
   - Converts to markdown (HTML→markdown).
   - Builds a heading outline: `Vec<HeadingMark { level, text, char_offset }>` from the extracted content's headings.
   - Applies char paging: returns `content[offset..min(offset+limit, total)]`.
   - Returns `FetchResult { url, title, content, total_chars, offset, limit, outline, content_type }`. `outline` is populated only when `offset == 0` (the first fetch); subsequent fetches omit it to save tokens. If `offset >= total_chars` → `Err("offset {offset} past end (total {total_chars})")`. If readability yields no extractable content (JS-only page, empty body) → `Err("no extractable content")`.

4. **`extract.rs`** — pure functions for readability extraction + HTML→markdown + heading-outline building, factored out of `fetch.rs` so they're unit-testable with fixture HTML (no network). This is the dep-heavy module.

**Public types:**
```rust
pub struct SearchResult { pub title: String, pub url: String, pub snippet: String }

pub struct HeadingMark { pub level: u8, pub text: String, pub char_offset: usize }

pub struct FetchResult {
    pub url: String,
    pub title: String,
    pub content: String,       // the paged markdown window
    pub total_chars: usize,
    pub offset: usize,
    pub limit: usize,
    pub outline: Vec<HeadingMark>, // non-empty only when offset == 0
    pub content_type: String,
}
```

### 4.3 The two thin-shell tools in `zoid-tools`

`zoid-tools` gains a `zoid-web` dependency in its `Cargo.toml`.

**`web_search.rs`** — `WebSearch` implementing `Tool` with `kind() = Network`:
- `spec()`: name `web_search`, params `{query: string (required)}`, description telling the model it returns up to 8 results (title/URL/snippet) and to use `web_fetch` to read a result.
- `run()`: panics (unreachable — `kind != Local`).
- `run_async()`: `Box::pin(async move { … })` — `str_arg(args, "query")` → `zoid_web::search(&query).await` → `ToolOutput::ok(format_results(&results))` on success, `ToolOutput::err("web_search failed: {e}")` on error, `str_arg` error passthrough on missing arg.
- `format_results(&[SearchResult]) -> String`: numbered markdown (`1. [Title](url)\n   snippet…`), one line per result. Bounded (≤8).

**`web_fetch.rs`** — `WebFetch` implementing `Tool` with `kind() = Network`:
- `spec()`: name `web_fetch`, params `{url: string (required), offset: integer (default 0), limit: integer (default 20000)}`, description telling the model the first fetch includes a heading outline and to use offset/limit to page long pages (mirrors the `read` tool's paging language).
- `run()`: panics (unreachable).
- `run_async()`: `Box::pin(async move { … })` — `str_arg(args, "url")`, `offset = args["offset"].as_u64().unwrap_or(0)`, `limit = args["limit"].as_u64().unwrap_or(20_000)` → `zoid_web::fetch(&url, offset, limit).await` → `ToolOutput::ok(format_fetch(&r))` / `ToolOutput::err("web_fetch failed: {e}")`.
- `format_fetch(&FetchResult) -> String`: title, heading outline (when present, compact: `## Heading @offset`), the content window, and a trailing `[total_chars: N; showing offset..end; call web_fetch with offset=<X> for more]` note when `end < total_chars`.

**Registry wiring (`zoid-tools/src/lib.rs`):** add `Box::new(web_search::WebSearch)` and `Box::new(web_fetch::WebFetch)` to both `registry()` and `registry_with_kill(kill)`. Add `pub mod web_search; pub mod web_fetch;`. The `registry_has_unique_named_tools` test gains `assert!(names.contains(&"web_search"))` and `assert!(names.contains(&"web_fetch"))`.

### 4.4 Agent-loop integration

The new `Network` arm (§4.1) sits in `run_turn_inner`'s per-call loop, after `gate.check(&tc)` and the existing kind-dispatch `match`. The `ToolStarted`/`ToolResult` emit and the `hard.cancelled()` cancel path mirror the `Mcp` arm exactly — no new UI events, no new `AgentUpdate` variants. The tool result flows back to the model as a `Tool`-role message, identical to Local tools.

The `gate.check` call runs before the `Network` arm too (as it does for all kinds), but the default `AllowAll`/`BlacklistGate` never denies/prompts on web tools — `BlacklistGate` only inspects shell commands (it pattern-matches the `shell` tool's command string, not arbitrary tool names). See §5 for the auto-allow decision.

## 5. Approval / trust model

The tool-approvals spec's guiding principle: *"prompt only for actions that reach outside the working directory / are irreversible / are outward-facing."* Web search and fetch are outward-facing (network requests to model-chosen queries/URLs), so by that principle they should prompt.

This slice deliberately **does not prompt** on web tools by default. Rationale: the approvals spec was designed for **destructive, irreversible** shell actions (`rm -rf`, force-push, `curl -X POST`). Web search and fetch are **read-only GETs with no side effects** — a meaningfully lower risk class. A research loop that prompts on every search and every fetch (easily 5-10 calls) would be unusable; the friction would train the user to ⏎-through prompts, defeating the gate for the destructive actions it was built for.

This is a documented, conscious departure, not an oversight:
- The web tools are GET-only (§2 out-of-scope: any web mutation would go behind the gate).
- `web_fetch` validates the URL scheme (http/https only — no `file://`/`data://` exfiltration of local content).
- The auto-allow applies only to `web_search`/`web_fetch` specifically — it does **not** weaken `BlacklistGate`'s shell-command scrutiny (`curl`/`wget` in the shell tool are still flagged as before).
- If the risk profile changes (e.g. adding web mutation, or a fetched URL leaking internal hosts), wiring web tools into `Gate::Prompt` is a follow-up that reuses the existing `ask_user` overlay — no new UI.

The `--yolo`/`[approval]` config knobs are unaffected; they govern shell-command prompting, not the web tools' default-allow.

## 6. Data flow (a research turn)

1. The model calls `web_search({query: "rust async trait boxed future"})`.
2. `Network` arm → `WebSearch::run_async` → `zoid_web::search` → POST `html.duckduckgo.com/html`, parse ≤8 results → `ToolOutput::ok(numbered markdown list)`.
3. The model reads results, calls `web_fetch({url: "https://…", offset: 0})`.
4. `Network` arm → `WebFetch::run_async` → `zoid_web::fetch` → GET, readability→markdown, heading outline, first 20000 chars → `ToolOutput::ok(title + outline + window + "more at offset X" note)`.
5. If long, the model calls `web_fetch({url, offset: 20000})` for the next window (re-GET; caching is out-of-scope).
6. `ToolResult` events flow back like Local tools; the agent loop consumes them identically.

## 7. Error handling

- **Network errors** (connect, DNS, timeout): `zoid_web::Err` → `ToolOutput::err("web_{search,fetch} failed: {e}")`. Model recovers (retry, pick another result).
- **Non-2xx HTTP**: status + body snippet as `Err`. A 403/429 (DDG rate-limit) surfaces as an error the model can back off from.
- **Idle timeout / hard-cancel**: the `Network` arm's `hard.cancelled()` tokio::select abandons a stuck fetch → `ToolOutput::err("[killed: hard-stop]")` (mirrors the Mcp arm). The `zoid_web` client also has a connect timeout.
- **Empty/whitespace query** (search): `Err` before the network call.
- **Bad URL scheme** (fetch): `Err("web_fetch supports http/https only")` — no `file://`/`data://`.
- **Offset past end** (fetch): `Err("offset {offset} past end (total {total_chars})")`.
- **No extractable content** (fetch): `Err("no extractable content")` — not an empty success string.

## 8. Testing (offline, no live-endpoint CI)

- **`zoid-web/search.rs`**: fixture-HTML test — `tests/fixtures/ddg_sample.html` (a saved real DDG response) fed to the parser; asserts titles/URLs/snippets extract correctly. No live network.
- **`zoid-web/fetch.rs`**: `TcpListener` stub serving a fixture HTML page; asserts extracted markdown, heading outline, offset/limit paging (windowing, truncation note, offset-past-end error, empty-content error, bad-scheme error).
- **`zoid-web/extract.rs`**: pure unit tests on fixture HTML fragments — readability drops nav/script/boilerplate; markdown conversion handles headings/p/lists/code/pre/links/blockquote; heading-outline char offsets are correct.
- **`zoid-tools`**: `registry_has_unique_named_tools` gains `web_search`+`web_fetch`; `format_results`/`format_fetch` tested as pure functions; `run_async` is a thin delegate (verified by the `zoid-web` tests + agent-loop test).
- **`zoid` (agent loop)**: a stub `Tool` returning `ToolKind::Network` whose `run_async` sleeps → assert Esc/hard-stop yields `[killed: hard-stop]` (mirrors the Mcp-cancel test). A stub `Tool` returning `Network` whose `run_async` returns `ok("done")` → assert the `ToolResult` flows back like Local.
- **`#[ignore]` live smoke** (`zoid-web`): a `#[ignore]` test hitting real DDG + a real docs page, for manual verification only (never in CI).

## 9. Open questions (to resolve during implementation plan)

1. **Readability + markdown crate selection.** Candidates: `scraper` + a readability port + `html2md`/`htmd`, vs. a combined crate, vs. a hand-rolled converter for the narrow readability-produced HTML subset. Settle against maintenance/MSRV/dep weight; the public API shape is unaffected.
2. **DuckDuckGo HTML fixture freshness.** The fixture is a point-in-time snapshot; if DDG changes markup, the parser breaks and the fixture no longer matches. The `#[ignore]` live smoke test is the canary. A follow-up `SearchBackend` trait mitigates this (out of scope here).
3. **`run_async` ergonomics.** The boxed-future signature is the stable-Rust default; if `async-trait` (already a workspace dep) reads cleaner for the single optional method, the plan may use it — the external behavior is identical. Settle during the plan.
4. **User-Agent string.** The exact UA (e.g. `zoid/0.3.2 (web tool)`) is set during the plan; it must be non-empty and identifiable so DDG doesn't block the default reqwest UA, but shouldn't impersonate a browser.