# zoid MCP Support — Design

**Date:** 2026-07-07
**Status:** Approved (design), pending implementation plan
**Author:** strvmarv (with Claude)

## Goal

Let zoid act as an **MCP (Model Context Protocol) client** so a user can point it
at existing MCP servers and have those servers' **tools** appear alongside zoid's
built-in tools. The model calls them identically to built-ins; zoid proxies the
call to the server and returns the result.

MCP is the de-facto plugin standard for agent tools. This makes zoid's tool set
extensible without recompiling — the architecture spec
(`docs/superpowers/specs/2026-06-30-zoid-core-architecture.md`) named MCP as the
`[POST-V1]` plugin surface and specified that server definitions be *read from
the ecosystem's locations, not redefined*. This design delivers the first,
deliberately narrow slice of that.

## Scoping decisions (locked)

| Decision | Choice | Rationale |
|---|---|---|
| Transport | **stdio only** | Covers the local server ecosystem (filesystem, git, sqlite, …). No network surface. Sits behind an `McpTransport` trait so HTTP is a later additive impl, not a rewrite. |
| Server config | **Ecosystem `.mcp.json`** (`{"mcpServers": {name: {command, args, env}}}`) | Honors the architecture spec's "read from the ecosystem, don't redefine" stance. Users reuse configs they already have. |
| Capabilities | **Tools only** | Tools are the only MCP capability that is *model-controlled* and thus maps onto zoid's existing tool-call loop. Resources/prompts/sampling/roots each need a new UI/turn surface and are deferred. |
| Client impl | **Hand-rolled**, behind a transport trait, in a new `zoid-mcp` crate | The protocol slice (JSON-RPC 2.0 + `initialize`/`tools/list`/`tools/call`) is small and stable. Hand-rolling adds **zero new dependencies** and matches zoid's minimal-deps ethos. The official `rmcp` SDK would add a dependency tree + pre-1.0 churn to buy breadth (transports, capabilities) that v1 deliberately declines. |
| Trust model | **Trust-on-configure** | A server the user put in `.mcp.json` is trusted; its tools run like built-ins, no per-call prompt. The `ToolGate` seam stays `AllowAll` for MCP and is the documented future home for approval. |
| TUI surface | **Read-only status view** | A small read-only overlay lists configured servers, connection state, and tool count. No management controls in v1. |
| Tool-name collisions | **Namespace as `server__tool`** | Multiple servers can expose a `search` tool; namespacing keeps the model's calls unambiguous. Matches the Claude Code convention. |

## Architecture

```
.mcp.json (project + user) ──► config::load ──► [McpServerConfig]
                                                      │
                                    McpManager::connect_all (background, per-server timeout)
                                                      │
                         ┌────────────────────────────┼────────────────────────────┐
                         ▼                             ▼                             ▼
                    McpClient(fs)                 McpClient(git)               McpClient(…)
                  StdioTransport               StdioTransport               StdioTransport
                  tokio::process::Child        (reader task demuxes         …
                  reader task ⇄ stdin/out       responses by id;
                                                tolerates inbound reqs)
                         │  initialize → tools/list (paginated)
                         └──────────────► aggregated tools  ─── namespaced server__tool ───┐
                                                                                            ▼
binary startup: tools = zoid_tools::registry() + manager.mcp_tools()  ──► agent turn loop
                                                                              │
                          provider request (ToolSpec[]) ◄── tool_specs(tools) │
                                                                              │
                          model emits ToolCall("git__commit", …)              │
                                                                              ▼
                          dispatch match on ToolKind:
                            Emitting / Interactive / **Mcp** (new) / Local
                            Mcp arm ──► manager.call_tool(name, args).await ──► ToolOutput
```

## Components

### A. New crate `zoid-mcp`

Mirrors `zoid-provider`'s role: isolates the protocol/subprocess concern behind a
narrow public surface (`McpManager`). The binary depends only on that surface, so
a future swap to `rmcp` (or an HTTP transport) is contained to this crate.

```
crates/zoid-mcp/src/
  lib.rs        # McpManager (public surface) + re-exports
  config.rs     # .mcp.json parsing + discovery + ${VAR} expansion
  jsonrpc.rs    # JSON-RPC 2.0 request/response/notification + id allocation
  transport.rs  # McpTransport trait + StdioTransport (tokio::process)
  client.rs     # McpClient: reader-task multiplexer; initialize / list_tools / call_tool
  manager.rs    # McpManager: owns N clients, aggregates tools, routes calls, status snapshot
  error.rs
```

**Dependencies:** no new *workspace* crates — uses `tokio`, `serde`/`serde_json`,
`anyhow`, `tracing`, all already present. The workspace `tokio` feature set gains
**`process`** (subprocess spawn) and **`io-util`** (buffered line reads); these are
feature additions to the existing `tokio` dependency, not new crates.

**Transport seam.** `McpTransport` is an async trait: send a framed message, and
expose an inbound stream of framed messages. `StdioTransport` is the only v1 impl,
wrapping `tokio::process::Child`. The JSON-RPC layer above it is transport-agnostic
(a future `HttpTransport` reuses it unchanged).

**Framing.** MCP stdio transport is **newline-delimited JSON-RPC 2.0** — one JSON
object per line, no `Content-Length` headers. Read a line, parse it.

**Client multiplexer.** `McpClient` owns a background reader task that reads lines
from the transport and demultiplexes: responses are matched to pending requests by
JSON-RPC `id` (a map of `id → oneshot::Sender`); inbound **notifications**
(e.g. `notifications/tools/list_changed`) are handled/ignored; inbound **requests**
from the server (e.g. `ping`) are answered with a JSON-RPC `method not found` error
rather than deadlocking. Each child's **stderr is drained on its own task** to
prevent a full pipe from blocking the child; drained lines are surfaced as
`tracing` diagnostics.

**Methods (the whole client surface):**
- `initialize` — send our latest supported `protocolVersion` + capabilities +
  `clientInfo`. The server replies with the version it will use (MCP negotiates,
  it need not equal ours); accept it if we support it, otherwise disconnect and
  mark the server `failed`. On success, send the `initialized` notification.
- `list_tools` — call `tools/list`, **looping on `nextCursor`** so the full tool
  set is discovered, not just the first page.
- `call_tool(name, args)` — call `tools/call`; map the result content to
  `ToolOutput` text; `isError: true` becomes a `ToolOutput` error.

### B. Manager & the dispatch seam

`McpManager` owns all `McpClient`s. `connect_all` spawns and initializes each
configured server **in the background** with a per-server timeout (default 10s),
then aggregates discovered tools with `server__tool` namespacing. It exposes:

- `mcp_tools() -> Vec<Box<dyn Tool>>` — one `McpTool` per discovered tool.
- `call_tool(namespaced_name, args) -> ToolOutput` — maps the namespaced name back
  to `(server, tool)` and routes to the owning client; returns a clean error if the
  server is unavailable.
- `status() -> Vec<ServerStatus>` — a read-only snapshot for the TUI.

**The sync/async bridge — reuse of an existing pattern.** zoid's `Tool` trait is
synchronous (`run(&self, args, cwd) -> ToolOutput`), but MCP calls are async I/O.
The codebase already solves this exact shape: `ToolKind::Emitting` and
`ToolKind::Interactive` tools implement `Tool` for their `name()`/`spec()`/`kind()`
but their `run()` is **never called** — the agent loop intercepts them *by kind*
before the synchronous fallback arm (`crates/zoid/src/agent.rs:687–1181`).

MCP becomes a **fourth intercepted kind**:

- Add `ToolKind::Mcp` to `crates/zoid-tools/src/lib.rs`.
- `McpTool` is a thin spec-carrier: `name()` → namespaced `server__tool`,
  `spec()` → the `ToolSpec` built from the server's discovered JSON Schema,
  `kind()` → `Mcp`, `run()` → unreachable (bypassed, exactly like `Emitting`).
- A new dispatch arm `Some(ToolKind::Mcp) => { let out = mcp.call_tool(&tc.name,
  &tc.args).await; … }` **awaits the call directly** — the natural async path,
  no `spawn_blocking`, no `block_on`-inside-sync.
- The binary builds the tool list as `zoid_tools::registry()` +
  `manager.mcp_tools()`. Downstream (`tool_specs()` at `agent.rs:150`, the provider
  request at `agent.rs:218`) iterates `&[Box<dyn Tool>]` unchanged, so the model
  sees MCP tools identically to built-ins and **the provider crate is untouched**.
- Execution routing is threaded to the turn loop as an added
  `Option<&McpManager>` parameter (`run_agent_turn` / `run_agent_turn_cancellable`
  / `run_turn_inner`); the `Mcp` arm awaits it. `McpTool` itself holds no manager
  reference — it is a pure spec carrier — so tool specs and execution stay cleanly
  separated.

Net structural change: one enum variant, one dispatch arm, one added loop
parameter, and the startup tool-list concatenation. The sync `Tool` execution path
(`run_tool` at `lib.rs:106`) and the provider layer are unchanged.

### C. Config discovery — `crates/zoid-mcp/src/config.rs`

Parse the ecosystem format:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/src"],
      "env": { "SOME_TOKEN": "${SOME_TOKEN}" }
    }
  }
}
```

- **Locations, merged by server name (project wins over user):**
  - User: `~/.config/zoid/mcp.json` (via zoid's existing `resolve_config_dir()`).
  - Project: `./.mcp.json` (repo root — the ecosystem-standard location).
- **`${VAR}` expansion** in `args` and `env` **values**, resolved from zoid's
  environment, so configs can reference secrets without hardcoding them. Unset
  variables expand to empty and are logged (name only).
- The spawned child **inherits zoid's parent environment**, with the config's `env`
  layered on top.
- **`env` values are never logged** — they carry secrets.
- A malformed `.mcp.json` is reported (path + parse error) and skipped; it does not
  abort startup.

### D. Lifecycle & failure handling

- **Startup is non-blocking.** The TUI launches immediately; MCP tools appear as
  each server completes `initialize` + `tools/list`. A slow `npx` cold-start never
  blocks the user.
- **Server fails to spawn / times out / protocol-mismatch:** logged, contributes
  zero tools, status `failed`. zoid runs normally.
- **Server crashes mid-session:** the reader task sees EOF, fails in-flight calls
  with a clean `ToolOutput` error ("mcp server `X` unavailable"), status
  `disconnected`. No zoid crash.
- **`tools/call` returns `isError: true`:** a normal tool error routed back to the
  model.
- **Shutdown:** closing a client drops the transport (closes the child's stdin);
  children are reaped (`kill_on_drop`) so no zombies remain.

### E. TUI surface — read-only

A read-only `Overlay::Mcp` (added to the `Overlay` enum in
`crates/zoid-tui/src/state.rs`), reachable from the command palette (e.g. `/mcp`).
It lists each configured server: **name · state
(`connecting`/`ready`/`failed`/`disconnected`) · tool count**. Enough to see what's
connected and debug a failed start. No connect/disconnect/reload controls — that is
the deferred management overlay. The status data is the `McpManager::status()`
snapshot.

## Data flow (a single MCP tool call)

1. At startup, `McpManager::connect_all` discovers `git__commit` from the git
   server and adds an `McpTool` for it to the tool list.
2. `tool_specs(tools)` serializes every tool — including `git__commit` — into the
   provider request. The model sees it as an ordinary tool.
3. The model emits `ToolCall { name: "git__commit", args }`.
4. The dispatch loop looks up the tool's `kind` → `Mcp` → the `Mcp` arm calls
   `manager.call_tool("git__commit", args).await`.
5. The manager maps `git__commit` → (`git` server, `commit` tool), sends
   `tools/call` over that client's transport, awaits the response, maps content to
   `ToolOutput`.
6. The `ToolOutput` is appended as the tool result and the turn continues —
   identical to a built-in tool from the loop's perspective.

## Testing

**Unit (no subprocess), via a fake in-process transport fed canned lines:**

| Area | Assert |
|---|---|
| JSON-RPC framing | request serializes to one line; a response line parses back |
| id demux | two concurrent requests get their matching responses, not swapped |
| pagination | a two-page `tools/list` (`nextCursor` then none) yields all tools |
| namespacing | two servers each exposing `search` yield `a__search` / `b__search` |
| inbound tolerance | a server-sent notification / `ping` request does not stall a pending call |
| config parse | `.mcp.json` parses; `${VAR}` expands; project overrides user by name |
| call error | `isError: true` becomes a `ToolOutput` error, not a protocol failure |

**Integration, via a fixture server shipped in-repo:** a ~40-line script that
speaks `initialize` / `tools/list` / `tools/call` over stdio. Spawn it for real and
assert the full round-trip, plus a **crash-mid-call** test (fixture exits during a
call → clean `ToolOutput` error + `disconnected` status). Using an in-repo fixture
instead of a real npm server keeps CI hermetic (no network, no `npx`).

## Non-goals (explicit, each with a seam left)

- **Resources, prompts, sampling, roots** — not model-controlled; each needs a new
  UI/turn surface. Deferred.
- **HTTP / remote transport** — the `McpTransport` trait is the seam; not
  implemented in v1.
- **Per-call approval / allowlists** — `ToolGate` stays `AllowAll` for MCP; it is
  the documented future home for approval.
- **Full server-management UI** (connect/disconnect/reload, live logs) — v1 is
  read-only.
- **Hot config reload** — servers are read once at startup.
- **Defeating a malicious server** — trust-on-configure assumes the user trusts what
  they put in `.mcp.json`. Out of scope, consistent with treating configured servers
  like built-in tools.
