# Agents as an Entity — Design

## Motivation

The harness currently has exactly one hardcoded subagent profile —
`AgentProfile::builtin()` ("delegate") — used for every `dispatch_subagent` call.
The `AgentProfile` struct already mirrors the `.claude/agents` file schema
(`name`, `description`, `system_prompt`, `tools`, `model`), and the code
anticipates a file loader and named registry (the `agent_profile.rs` module doc
says: *"the file loader and named registry are POST-V1 — loaders built on
demand"*).

This slice adds **agents as a first-class filesystem entity**: the user drops
agent files on disk, the harness loads them into a registry at startup, and the
orchestrator picks one by name when dispatching a subagent.

**Scope is deliberately narrow:** add the entity, load it, expose it, wire the
dispatch selection. The `tools` allow-list and `model` override fields are
**parsed and stored but seamed** (not enforced by the runtime) — matching how
`mode.md` already seams those same fields. Enforcing them is a follow-up slice.

## Decisions (from brainstorming)

| Decision | Resolution |
|---|---|
| Relationship to modes | Agents are a **distinct entity** from modes; they do not interact with the mode system. |
| Purpose | Agents are profiles for **subagent delegation** — replacing the hardcoded `AgentProfile::builtin()`. |
| Selection mechanism | The model picks by name via an `agent` parameter on `dispatch_subagent` (default: "delegate" when unspecified). |
| Discovery | A separate `list_agents` tool the model calls to see available agents. |
| Unknown agent name | Reject with an error that lists available agents (so the model self-corrects in one step). |
| File format | Frontmatter + markdown body (same `---`-fenced YAML-scalar pattern as `SKILL.md` / `mode.md`), with `tools` and `model` as additional frontmatter fields. |
| File layout | `<dir>/<agent-name>/agent.md` (folder-per-entity, mirroring `mode.md` / `SKILL.md`). |
| Discovery dirs | Two convention dirs (`<user_cfg_dir>/agents`, `<cwd>/.zoid/agents`) plus configurable `[agents] source_dirs` (unioned across config layers) — same shape as skills and modes. |
| Built-in "delegate" | Pre-seeded in the registry at index 0; first-wins collision protection means an import named "delegate" is silently skipped (no overwriting). |
| `tools` field | Parsed and stored on the profile, **seamed** (runtime does not filter the tool set). Follow-up to enforce. |
| `model` field | Parsed and stored on the profile, **seamed** (runtime always inherits the orchestrator's model). Follow-up to honor. |
| Architecture | Mirror the skill/mode pattern: `AgentRegistry` + `parse_agent_md` in `zoid-core` (pure, unit-tested), `agent_import.rs` in the bin (filesystem adapter). |

## Architecture

The design mirrors the proven skill/mode pattern at every layer:

```
zoid-core/src/agent_profile.rs
  ├── AgentProfile          (existing, unchanged)
  ├── ParsedAgent           (new — result of parse_agent_md)
  ├── parse_agent_md()      (new — pure frontmatter+body parser)
  └── AgentRegistry         (new — mirrors SkillRegistry)

crates/zoid/src/agent_import.rs
  ├── resolve_agent_dirs()  (mirrors resolve_skill_dirs)
  ├── import_agents()       (mirrors import_skills)
  └── build_agent_registry()(mirrors build_registry)

crates/zoid-core/src/config.rs
  ├── AgentsConfig           (new — mirrors SkillsConfig)
  └── PartialAgents         (new — mirrors PartialSkills)

crates/zoid-tools/src/list_agents.rs
  └── ListAgents             (new tool — mirrors invoke_skill's structure)

crates/zoid-tools/src/subagent_dispatch.rs
  └── DispatchSubagent      (gains `agent` parameter)
```

## Component Design

### 1. `AgentRegistry` (zoid-core, pure)

New struct in `agent_profile.rs`, mirroring `SkillRegistry`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRegistry {
    agents: Vec<AgentProfile>,
}
```

Methods (all mirror `SkillRegistry`):

- `new(agents: Vec<AgentProfile>) -> Self` — construct from a list.
- `builtin() -> Self` — pre-seeds `AgentProfile::builtin()` ("delegate") at
  index 0.
- `push_unique(profile: AgentProfile) -> bool` — appends unless a profile with
  the same name already exists. Returns `false` on collision (first-wins). An
  import named "delegate" is silently rejected — the built-in is protected.
- `get(name: &str) -> Option<&AgentProfile>` — lookup by exact name.
- `names() -> Vec<String>` — all names in registry order.
- `all() -> &[AgentProfile]` — read-only view of all profiles in order.
- `menu() -> String` — one `- name: description` line per agent, for the
  `list_agents` tool result. Empty string when there are no agents (never empty
  in practice — "delegate" is always present).

### 2. `parse_agent_md` (zoid-core, pure)

New pure parser in `agent_profile.rs`. Same frontmatter+body structure as
`parse_skill_md` (YAML-style scalars inside `---` fences, body = everything
after the first closing fence, verbatim), but extracts five fields.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAgent {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub model: Option<String>,
}
```

Parsing rules:

- **Frontmatter**: `---`-fenced block at the start. The opening `---` must be
  the first line. The closing `\n---` ends the frontmatter; everything after it
  (minus one leading newline) is the body.
- `name` (required): `name: value` — one-line scalar, one pair of surrounding
  quotes stripped. `Err` if missing or empty.
- `description` (optional): `description: value` — one-line scalar, quotes
  stripped. Defaults to empty string.
- `tools` (optional): a YAML-style list under a `tools:` key. Each list item is
  a line starting with `- ` (dash-space) after the `tools:` line. Absent or
  empty = all tools permitted (empty `Vec`). Example:
  ```
  tools:
    - read
    - grep
    - glob
  ```
  The parser collects these by detecting the `tools:` key line, then consuming
  subsequent lines that start with `- ` until a non-`- `-prefixed line or the
  closing fence. This is a minimal inline list parser — not a full YAML parser
  (consistent with `parse_skill_md`'s single-line-scalar-only approach).
- `model` (optional): `model: value` — one-line scalar, quotes stripped.
  Absent = `None` (inherit the orchestrator's model).
- **Body**: everything after the first closing `---` fence (verbatim, including
  internal `---` lines), assigned to `system_prompt`.

Returns `Err(String)` with a human-readable reason if:
- No frontmatter opening `---`.
- No frontmatter closing `---`.
- `name` is missing or empty.

This is a **new parser**, not a modification to `parse_skill_md`. The agent
format carries `tools` and `model` which are semantically meaningless to
skills. Keeping them separate preserves the single-responsibility boundary.

### 3. `agent_import.rs` (bin, filesystem adapter)

New module in `crates/zoid/src/`, mirroring `skill_import.rs` exactly.

**`resolve_agent_dirs`**: ordered list of directories to scan.

```rust
pub fn resolve_agent_dirs(
    source_dirs: &[String],
    user_cfg_dir: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf>
```

Returns:
1. `user_cfg_dir.join("agents")`
2. `cwd.join(".zoid").join("agents")`
3. Each `source_dirs` entry with `~` / `~/` expanded against `home`.

Pure path arithmetic — existence is checked by `import_agents`.

**`import_agents`**: scan each directory for immediate `<name>/agent.md`
children, parse them, return the resulting `AgentProfile`s. A directory that
does not exist is skipped silently. A present-but-unreadable directory, an
unreadable file, or a malformed `agent.md` is skipped with a warning to stderr.
Never panics. Also supports one level of nesting (`<root>/<pack>/<agent>/agent.md`)
mirroring the skill importer's pack-dir support.

```rust
pub fn import_agents(dirs: &[PathBuf]) -> Vec<AgentProfile>
```

**`build_agent_registry`**: pre-seed `AgentRegistry::builtin()` then merge
imports with first-wins collision protection.

```rust
pub fn build_agent_registry(dirs: &[PathBuf]) -> AgentRegistry
```

### 4. Config: `AgentsConfig` (zoid-core)

New config section mirroring `SkillsConfig` / `ModesConfig`.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentsConfig {
    pub source_dirs: Vec<String>,
}
```

Added to `Config`:
```rust
pub struct Config {
    // ... existing fields ...
    pub agents: AgentsConfig,
}
```

Partial for deserialization:
```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialAgents {
    pub source_dirs: Option<Vec<String>>,
}
```

Added to `PartialConfig`. Merged via the same union-across-layers logic as
`skills.source_dirs` and `modes.source_dirs` (duplicates skipped, later layers
append new dirs).

TOML usage:
```toml
[agents]
source_dirs = ["~/my-agents", "/shared/agents"]
```

Unknown-key surfacing: `[agents]` is a known section, so `source_dirs` inside it
is consumed. Any unknown key inside `[agents]` is collected by
`serde_ignored` and surfaced as a warning (same as every other section).

### 5. `list_agents` Tool (zoid-tools)

New tool in `crates/zoid-tools/src/list_agents.rs`. Mirrors the structure of
`InvokeSkillTool` but simpler — it's a read-only listing tool.

The tool holds an `Arc<AgentRegistry>` (injected at construction). Its `run()`
returns the registry's `menu()` as a plain-text tool result:

```
Available agents:
- delegate: Complete one discrete unit of work autonomously.
- code-reviewer: Reviews code changes for quality and correctness
- researcher: Deep research across the codebase
```

Tool spec:
```json
{
  "name": "list_agents",
  "description": "List the available subagent agent profiles by name and \
                   description. Call this before dispatch_subagent to see which \
                   agents are available, then pass one's name to dispatch_subagent's \
                   'agent' parameter.",
  "parameters": {
    "type": "object",
    "properties": {},
    "required": []
  }
}
```

`kind() -> ToolKind::Local` (a normal synchronous tool, not `Emitting` — its
`run()` is called directly by the agent loop, returns immediately with no side
effects on the session state). The `Tool` trait defaults `kind()` to `Local`,
so the impl can simply omit the override — but being explicit is clearer.

### 6. `dispatch_subagent` Tool Spec Change (zoid-tools)

`DispatchSubagent::spec()` gains an `agent` parameter:

```json
{
  "name": "dispatch_subagent",
  "description": "Dispatch a subagent to execute a task in isolation. ...",
  "parameters": {
    "type": "object",
    "properties": {
      "task": { "type": "string", "description": "The task description for the subagent" },
      "agent": { "type": "string", "description": "The agent profile name to use (default: 'delegate'). Call list_agents to see available agents.", "default": "delegate" },
      "worktree": { "type": "boolean", "description": "Isolate in a git worktree (default: false)", "default": false }
    },
    "required": ["task"]
  }
}
```

The tool's `run()` remains unreachable (the agent loop branches on `Emitting`
before calling `run()`). The spec change is what matters — the model sees the
`agent` parameter in the tool definition.

### 7. Dispatch Site Wiring (agent.rs)

The `dispatch_subagent` handling in `agent.rs` (the `Some(ToolKind::Emitting)
if tc.name == "dispatch_subagent"` branch) gains agent-name resolution.

After extracting `task` and `want_worktree`, extract `agent`:

```rust
let agent_name = tc
    .args
    .get("agent")
    .and_then(|v| v.as_str())
    .unwrap_or("delegate")
    .to_string();
```

Resolve against the registry (which is passed into the turn config / made
available to the dispatch branch via `TurnConfig` or an `Arc<AgentRegistry>`
field):

- **Known name**: use `registry.get(&agent_name)` → `AgentProfile` clone. Pass
  it to `spawn_subagent` instead of `AgentProfile::builtin()`.
- **Unknown name**: emit a `ToolResult` error that lists available agents:
  ```
  dispatch_subagent: unknown agent 'code-review'. Available: delegate, researcher, ...
  ```
  and `continue` (do not dispatch). This is the self-correcting path — the
  model gets the menu in the error and retries.
- **Empty/absent `agent`**: defaults to "delegate" (the built-in), so existing
  behavior is preserved. The model doesn't have to specify an agent for
  simple delegations.

The `spawn_subagent` function signature changes: instead of always passing
`&AgentProfile::builtin()`, it receives `&AgentProfile` (the resolved profile).
This is a one-line change at the call site — the function already takes a
`&AgentProfile`, it just always receives `builtin()` today.

### 8. Startup Wiring (main.rs)

At startup (alongside skill/mode registry construction, ~line 2060):

```rust
let agents = {
    let dirs = zoid::agent_import::resolve_agent_dirs(
        &config.agents.source_dirs,
        &cfg_dir,
        std::path::Path::new(&root),
        home.as_deref(),
    );
    std::sync::Arc::new(zoid::agent_import::build_agent_registry(&dirs))
};
```

The `Arc<AgentRegistry>` is:
1. Passed to `chat_tools` so the `ListAgents` tool can be constructed with it.
2. Made available to the `dispatch_subagent` dispatch branch (via `TurnConfig`
   or the app state that the turn closure captures).

`chat_tools` signature gains an `agents: Arc<AgentRegistry>` parameter:

```rust
pub fn chat_tools(
    skills: Arc<SkillRegistry>,
    agents: Arc<AgentRegistry>,
    kill: zoid_tools::KillSlot,
) -> Vec<Box<dyn Tool>>
```

It pushes `Box::new(ListAgents::new(agents))` onto the tool list.

## Data Flow

```
Startup:
  config.agents.source_dirs
    → resolve_agent_dirs (convention dirs + configured)
    → import_agents (scan <dir>/<name>/agent.md, parse_agent_md each)
    → build_agent_registry (builtin "delegate" + imports, first-wins)
    → Arc<AgentRegistry>
    → chat_tools (ListAgents tool bound to registry)
    → TurnConfig (registry available to dispatch branch)

Runtime (delegation):
  Model calls list_agents
    → ListAgents::run → registry.menu() → "Available agents:\n- delegate: ..."
    → tool result fed back to model

  Model calls dispatch_subagent { task, agent: "code-reviewer", worktree: true }
    → agent.rs dispatch branch extracts agent_name
    → registry.get("code-reviewer") → Some(profile)
    → spawn_subagent(task, ..., &profile, ...)
    → subagent runs with the selected profile's system_prompt

  Model calls dispatch_subagent { task } (no agent param)
    → agent_name defaults to "delegate"
    → registry.get("delegate") → Some(builtin profile)
    → spawn_subagent with builtin (unchanged behavior)

  Model calls dispatch_subagent { task, agent: "typo-name" }
    → registry.get("typo-name") → None
    → emit ToolResult error: "unknown agent 'typo-name'. Available: ..."
    → model self-corrects on next turn
```

## Error Handling

All error handling follows the established "bad input returns a result, never
aborts startup" rule:

- **Missing convention dir**: skipped silently (normal — not every install has
  `<cfg>/agents` or `<cwd>/.zoid/agents`).
- **Missing configured source dir**: skipped silently.
- **Unreadable directory**: skipped with `eprintln!` warning, continue.
- **Unreadable `agent.md`**: skipped with `eprintln!` warning, continue.
- **Malformed `agent.md`** (no frontmatter, missing name): skipped with
  `eprintln!` warning, continue. (Unlike modes, which create a `Broken` entry
  to stay visible in the cycle, agents have no UI cycle — a bad agent is simply
  dropped from the registry. The warning to stderr is the only surfacing.)
- **Name collision with built-in "delegate"**: silently skipped (first-wins).
  No warning — this is expected behavior (protecting the built-in), matching how
  skills silently protect "spike-plan".
- **Name collision between two imports**: first-wins (the earlier dir's agent
  is kept). No warning — matching skill/mode behavior.
- **Unknown agent name at dispatch time**: `ToolResult` error listing available
  agents. The model self-corrects.
- **Empty/absent `agent` parameter**: defaults to "delegate", no error.

## Seamed Fields

Both `tools` and `model` are parsed from the frontmatter and stored on the
`AgentProfile`, but the runtime does not act on them:

- **`tools`**: The subagent runtime builds its tool set from the global
  registry regardless of the profile's `tools` field. Enforcing the allow-list
  (filtering the tool set per dispatch) is a follow-up slice. The field is
  stored so the profile is a faithful representation of the file, and the
  enforcement slice can wire it without re-parsing.
- **`model`**: The subagent always inherits the orchestrator's model. Honoring
  the per-agent model override is a follow-up slice. The field is stored for
  the same reason.

This matches how `mode.md` already seams these fields (`mode_import.rs` sets
`tools: vec![]` and `model: None` with "SEAMED" comments).

## Testing

### Core (pure, unit-tested in `agent_profile.rs`)

- `parse_agent_md` parses all five fields (name, description, tools list, model,
  body/system_prompt).
- `parse_agent_md` returns `Err` for missing frontmatter, missing closing
  fence, missing/empty name.
- `parse_agent_md` handles absent `tools` (empty vec) and absent `model` (None).
- `parse_agent_md` preserves body verbatim including internal `---` lines.
- `parse_agent_md` strips one pair of surrounding quotes from scalar values.
- `AgentRegistry::builtin()` contains "delegate" at index 0.
- `AgentRegistry::push_unique` rejects a name collision with the built-in
  "delegate" (returns `false`, registry unchanged).
- `AgentRegistry::push_unique` appends a genuinely new agent.
- `AgentRegistry::get` hits known, misses unknown.
- `AgentRegistry::names` and `all` return in order.
- `AgentRegistry::menu` renders one line per agent.

### Bin (agent_import.rs)

- `resolve_agent_dirs` prepends convention dirs and expands `~`.
- `import_agents` reads valid agents and skips malformed (mirrors
  `import_skills` tests).
- `import_agents` skips missing dir without panic.
- `import_agents` supports per-pack subdirs (`<root>/<pack>/<agent>/agent.md`).
- `build_agent_registry` merges builtins and imports with first-wins (an
  import named "delegate" does not shadow the built-in).

### Config (config.rs)

- `[agents] source_dirs` parses correctly.
- Merge unions `agents.source_dirs` across layers (mirrors existing
  `skills`/`modes` merge tests).

### Tools (zoid-tools)

- `ListAgents` spec name, kind, and empty parameters.
- `ListAgents::run` returns the registry menu.
- `DispatchSubagent` spec includes the `agent` parameter with default
  "delegate".
- `DispatchSubagent` spec `required` is still `["task"]` only (`agent` is
  optional).

### Integration (agent.rs or test)

- Dispatch with known agent name uses that profile (not the builtin).
- Dispatch with unknown agent name emits an error listing available agents.
- Dispatch with absent `agent` param defaults to "delegate" (unchanged
  behavior).

## Out of Scope

- Enforcing the `tools` allow-list during subagent execution (follow-up slice).
- Honoring the `model` override during subagent execution (follow-up slice).
- Hot-reloading agents at runtime (agents load at startup only, same as
  skills/modes).
- UI surfaces for browsing/managing agents (no status bar, no picker, no
  config screen — agents are filesystem-only and model-discovered).
- Agent-scoped skills (modes own scoped skills; agents do not — an agent is
  just a profile, not a skill namespace).