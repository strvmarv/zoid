# Deterministic Claude-Plugin Importer + Plugin Generalization — Design

- **Date:** 2026-07-13
- **Status:** Approved (brainstorming); pending implementation plan
- **Scope:** **Spec 1 of 3.** Make *any* Claude Code plugin importable into zoid — not just the bundled Superpowers pilot — by (a) generalizing zoid's mode-body generator and adding a `skills` manifest kind, and (b) building a deterministic hybrid converter that reads existing Claude-plugin manifests and emits zoid artifacts. **Out of scope (own specs):** Spec 2 = curated catalog hosting + compatibility surfacing in `zoid-releases`; Spec 3 = mega-catalog per-skill selective install + wholesale bloat guard + HTTP MCP transport.

## 1. Motivation

zoid already has a manifest-driven plugin host (`zoid-plugin`, schema v1; see `2026-07-09-zoid-plugin-support-design.md`), but it is effectively single-tenant:

1. **Only Superpowers is bundled.** `bundled.rs` compiles in one manifest; adding another means a PR against zoid.
2. **The mode-body generator is hardcoded to Superpowers.** `plan.rs::generate_body_from_frontmatter` emits the literal text *"You are operating in \"Superpowers\" mode, imported from obra/superpowers … invoke verification-before-completion before claiming success."* Any *other* plugin installed today would derive a correct name/description/skill-list but a **wrong, Superpowers-branded body**.
3. **Only `kind = ["mode"]` validates.** `manifest.validate()` hard-rejects every other kind, even though the config seam it would need (`skills.source_dirs`, already `Safe`-classified) exists.

The maintainer's goal is a **curated set of pre-validated manifests hosted in the public `zoid-releases` repo** that officially advertise compatibility — *not* more bundled (compiled-in) plugins. Superpowers stays the sole first-class bundled plugin. To make that catalog **truthful and low-effort**, the recipe for turning an upstream plugin into zoid artifacts must move out of bespoke authoring and into a **deterministic converter**, and zoid's mode plumbing must stop assuming it is always Superpowers.

The unlock that makes determinism cheap: the official `claude-plugins-official` `marketplace.json` **already pins a full 40-char commit `sha` per external plugin**, and Claude plugins expose capabilities purely by directory convention. So the converter can copy the pin rather than resolve a moving `HEAD`.

## 2. Decisions (locked during brainstorming)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Deliverable | **Curated manifests hosted in `zoid-releases`, surfaced/advertised** — *not* bundled. Superpowers remains the only compiled-in plugin. (Hosting/surfacing itself is Spec 2.) |
| 2 | Body generator | **Generalize it.** Move Superpowers' preamble into manifest fields; keep its output **byte-identical** via the existing golden snapshot. Provide a **generic name/repo-parameterized default** for packs that supply no body text. |
| 3 | New `skills` kind | **In scope now.** `kind = ["skills"]` materializes skills into `skills.source_dirs` with **no mode overlay**. |
| 4 | Mode vs. skills | **Default keyed on loader presence; flags override.** A pack with a natural loader/index skill (`using-*`, `find-skills`, `*-overview`) defaults to `kind = ["mode"]`; a loader-less pack defaults to `["skills"]`. `--mode` / `--skills` override at **both** import (converter) and install (`:plugin install`). |
| 5 | Converter shape | **Hybrid:** a `bulk` front-end (ingest `marketplace.json`, use its pinned SHAs) and a `per-repo` front-end (a single plugin repo/dir; resolve SHA via `git ls-remote`). |
| 6 | Converter home | **New Rust bin in the zoid workspace** (`crates/zoid-plugin-import`), reusing `zoid-plugin`'s manifest/plan types so emitted output is verified by the same code that installs it. |
| 7 | MCP handling | **stdio only, generically.** Normalize a stdio `.mcp.json` (bare or `mcpServers`-wrapped) into zoid's `{ mcpServers: { name: { command, args, env } } }` shape. `type:"http"`/remote servers → **skip + flag** (blocked until Spec 3's `HttpTransport`). |
| 8 | Unsupported capabilities | `commands/`, `agents/`, `hooks/` in a Claude plugin have **no zoid seam** → **drop + record** in the per-plugin report; never silently ignore. |
| 9 | Scope guard | **Mega-catalogs deferred.** Spec 1 imports only packs that install **wholesale** (curated set, all ≤ ~30 skills). Selective per-skill install + a wholesale bloat guard are **Spec 3**. |

### 2.1 Why the bloat guard is deferred, not ignored

zoid injects **every registered skill's name + description into the system prompt** (`invoke_skill`'s spec: *"Available skills are listed in your system prompt"*; `build_registry` walks every `*/SKILL.md` in a source dir). Bodies stay lazy, but the **listing** is always-on context. A 1016-skill catalog (e.g. `TerminalSkills/skills`) would therefore add tens of thousands of tokens of permanent overhead. Spec 1 sidesteps this by importing only small curated packs; Spec 3 adds selective install + a guard so large catalogs can be mined a few skills at a time.

## 3. Architecture

### 3.1 Enabling changes in `zoid-plugin` (pure, IO-free)

**3.1a — Generalize the mode body (`plan.rs`, `manifest.rs`).**
Move the two hardcoded text blocks out of `generate_body_from_frontmatter` and into the `[mode]` recipe as manifest fields:

```toml
[mode]
loader       = "using-superpowers/SKILL.md"
strip_prefix = "skills/"
body         = "from-skill-frontmatter"
description  = "…"
body_intro   = """You are operating in "Superpowers" mode, imported from obra/superpowers.

Before any task, check if an available skill applies and invoke it with invoke_skill. The skills are:
"""
body_outro   = """
Always check for an applicable skill before starting work. …
… invoke verification-before-completion before claiming success.

Skill work produces specs, plans, and debugging notes. …
"""
```

The generator becomes `body_intro + skill-bullets + body_outro`. Superpowers' bundled manifest carries **its exact current strings**, so `tests/superpowers_body_golden.txt` stays byte-identical (the test is the guardrail: any drift fails).

When `body_intro`/`body_outro` are **absent** (e.g. a loader-less pack promoted with `--mode`), the generator synthesizes a **generic default** parameterized by `manifest.name` and `source.repo`:

```
You are operating in "{name}" mode, imported from {repo}.

Before any task, check if an available skill applies and invoke it with invoke_skill. The skills are:

{skill bullets}

Always check for an applicable skill before starting work. If multiple skills apply, invoke the most specific one first.
```

`BodyStrategy::FromSkillFrontmatter` is retained; only its text source changes (manifest fields, else generic default). No new strategy variant is introduced.

**3.1b — Add `kind = ["skills"]` (`manifest.rs`, `plan.rs`).**
- `validate()` accepts `"skills"` alongside `"mode"`. A `skills` plugin requires **no `[mode]` table**.
- `build_plan` gains a skills branch: materialize every `<skill>/SKILL.md` (plus sibling files) under `strip_prefix`, with **no `mode.md` entry and no overlay body**. The plan's effects register the destination via the already-`Safe` `skills.source_dirs` config key (applied through the existing `skill_import` path — see §3.3).
- A manifest may declare `kind = ["mode"]` **or** `["skills"]` (v1 does not combine them for one install; the converter picks one per §2 decision 4 + §4).

### 3.2 The converter bin (`crates/zoid-plugin-import`)

```
crates/zoid-plugin-import/src/
  main.rs        — CLI: `bulk <marketplace.json>` | `repo <owner/name>[/subpath]` [--mode|--skills] [--out DIR]
  claude.rs      — parse Claude marketplace.json + plugin.json (serde)
  classify.rs    — capability detection by convention → target kind (pure)
  emit.rs        — build zoid plugin.toml + normalized .mcp.json + report (reuses zoid_plugin types)
  fetch.rs       — GitHub API tree/blob fetch at a pinned sha; git ls-remote for per-repo SHA
```

`classify.rs` and `emit.rs` are **pure** (fed the fetched file listing + blobs); `fetch.rs`/`main.rs` are the effectful shell — mirroring zoid's pure/effectful split so classification and emission are table-testable.

**Reuse guarantee:** `emit.rs` constructs `zoid_plugin::manifest::PluginManifest` values and serializes them, then re-parses via `parse_manifest` + `validate()` before writing — so **nothing is emitted that the installer can't consume**. This is the determinism-and-correctness backbone.

### 3.3 Install-side changes (`crates/zoid/src/plugin_install.rs`)

- `finish_plugin_install` gains a **skills branch**: for a `skills`-kind plan, materialize skill files into the destination and register it as a skills source dir (via the existing `skill_import::resolve_skill_dirs`/`build_registry` machinery), **without** writing `mode.md` or activating a mode.
- `:plugin install <id|url>` gains **`--mode` / `--skills`** flags that override the manifest's declared kind at install time (promote a `skills` pack to a mode, or vice-versa). Promotion to mode with no manifest body text uses the §3.1a generic default.

## 4. Classification rules (deterministic, no judgment)

Given a fetched plugin directory (at a pinned sha), the converter classifies purely by presence:

| Observed | Emitted |
|----------|---------|
| `skills/<name>/SKILL.md` present, **loader skill present** (`using-*` / `find-skills` / `*-overview`), no `--skills` | `plugin.toml` `kind = ["mode"]` (loader → `mode.loader`) |
| `skills/<name>/SKILL.md` present, **no loader** (or `--skills`) | `plugin.toml` `kind = ["skills"]` |
| either of the above with `--mode` | `kind = ["mode"]` (generic default body if no loader) |
| root `.mcp.json`, all servers stdio (`command`) | normalized `{ mcpServers: { … command/args/env } }` snippet |
| `.mcp.json` server with `type:"http"` / `url` | **skip that server + flag** "needs HttpTransport (Spec 3)" |
| `commands/` \| `agents/` \| `hooks/` | **drop + record** in report ("no zoid seam") |

SHA source: `marketplace.json` entry pin (`bulk`) or `git ls-remote <repo> <branch>` (`per-repo`). The loader-detection glob is a small allowlist of index-skill name patterns, overridable by `--loader <path>`.

## 5. Testing

Golden round-trip fixtures drawn from the **real local plugin cache** (`~/.claude/plugins/marketplaces/claude-plugins-official/`), copied into `crates/zoid-plugin-import/tests/fixtures/`:

| Fixture | Asserts |
|---------|---------|
| `frontend-design` (skills/, no loader) | emits `kind = ["skills"]`; no `mode.md`; parses+validates |
| `superpowers` (skills/ + `using-superpowers` loader) | emits `kind = ["mode"]`; body **byte-identical** to the existing golden |
| a stdio `.mcp.json` (e.g. `@playwright/mcp`) | normalized to zoid `mcpServers` stdio shape |
| the github `.mcp.json` (`type:"http"`) | server **skipped + flagged**, not emitted |
| a plugin with `commands/` | commands **dropped + reported** |

Plus `zoid-plugin` unit tests for: the generalized body (manifest fields → interpolation; absent fields → generic default; Superpowers golden unchanged) and the `skills` kind (validate accepts it; `build_plan` emits no `mode.md`). Plus `plugin_install` tests for the install-time `--mode`/`--skills` override.

## 6. Approved candidate pool (validation fixtures + Spec 2 seed)

Not required to *build* Spec 1, but the vetted, SHA-pinned set the tests draw from and Spec 2 will host. All layout-verified `<subtree>/<skill>/SKILL.md`; all live-checked against the GitHub API on 2026-07-13.

**Skill packs** (default kind per §2 decision 4):
| Repo | Skills | Loader → default | License | Pinned sha |
|------|--------|------------------|---------|-----------|
| `obra/superpowers` | 14 | ✅ → mode (bundled) | MIT | `d884ae04edebef577e82ff7c4e143debd0bbec99` |
| `addyosmani/agent-skills` | 24 | ✅ → mode | MIT | `98967c45a42b88d6b8fb3a88b7ff6273920763d6` |
| `veniceai/skills` | 19 | ✅ → mode | MIT | `de089fac4e2e4a51be2ee701eaec97fd0a60b9d3` |
| `mxyhi/ok-skills` | ~30 | ✅ (`find-skills`) → mode | Apache-2.0 | `0fda62be082575ffb450ce9708bf74e37754d061` |
| `anthropics/skills` | ~19 | ❌ → skills | ⚠️ **no license file** | `9d2f1ae187231d8199c64b5b762e1bdf2244733d` |
| `arpitg1304/robotics-agent-skills` | 10 | ❌ → skills | Apache-2.0 | `54f7b578f3dc269d29c0beb623b3f2611fd3a430` |

**Additive stdio MCP servers** (none re-wrap zoid built-ins — file/shell/web_fetch/web_search/recall):
`@playwright/mcp` (browser), `mcp-server-time` (uvx, no secret), `postgres-mcp` (uvx, `DATABASE_URI`), `mongodb-mcp-server` (npx), `@benborla29/mcp-server-mysql` (npx), `mcp-server-motherduck` (uvx, local no-secret), `redis-mcp-server` (uvx), `mcp-server-kubernetes` (npx, kubeconfig), `mcp-server-docker` (uvx, socket), `awslabs.aws-api-mcp-server` (uvx, AWS creds), `mcp-clickhouse` (uvx).
No-secret tier (advertise as drop-in): `playwright`, `time`, `docker`, `kubernetes`, `motherduck`-local.

**Open flags for Spec 2:** `anthropics/skills` has no license file (redistribution/advertising risk — hold pending terms); HTTP-only servers (github, linear, notion, atlassian, cloudflare, sentry-hosted) are demand evidence for Spec 3's `HttpTransport`.

## 7. Non-goals (explicit)

- Catalog **hosting, index format, `:plugin` discovery UX, and public advertising** → Spec 2.
- **Mega-catalog** support: per-skill selective install, `--skill a,b,c`, interactive picker, wholesale bloat guard/threshold → Spec 3.
- **HTTP/SSE MCP transport** → Spec 3.
- Combining `mode` + `skills` in a single install; `set_config` for non-allowlisted keys (still `Dangerous`, unchanged).
