# zoid — One-Action Superpowers Mode Install · Design

**Date:** 2026-07-09
**Status:** Approved design, ready for implementation plan
**Author:** strvmarv (with Claude)

> **Spec set.** This builds directly on the mode/skill runtime and the URL-import wizard:
> - **Slice 3 — mode runtime** → `2026-07-05-mode-promotion-quickswitch-design.md` (on-disk modes, scoping, overlay, switch, hot-reload).
> - **Slice 4 — URL import wizard** → the AI-assisted `:mode import <url>` that scans an upstream skills repo, proposes a mapping, and `materialize`s canonical files + a `.zoid-provenance.json` sidecar. Lives in `mode_wizard.rs` / `github_fetch.rs` / `zoid_core::wizard`.
> - **This doc** — a **deterministic, model-free** one-action install of the canonical `obra/superpowers` skill set as a zoid mode, surfaced in-app (command + first-run onboarding). It reuses the wizard's fetch and writer; it replaces only the AI mapping step with a pinned, deterministic mapping.

---

## 1. Overview

zoid already imports skill sets as modes via the AI-assisted `:mode import <url>` wizard. This spec adds a **zero-friction, no-model path** for the *one* skill set that makes zoid feel complete out of the box: **Superpowers** (`obra/superpowers`).

The goal is a single user action — `:mode install superpowers`, or one keypress on the first-run screen — that fetches the pinned upstream, writes the same on-disk mode the wizard would, and reloads. No API key, no model turn, reproducible.

**Ethics constraint (governs the whole design).** zoid must **not bundle** the superpowers content into its repo or binary. The install **fetches from upstream on explicit user action** and records provenance. It is always **opt-in** — never automatic on launch or install.

---

## 2. North star

**Reuse the wizard's proven halves; add only what's genuinely new.** The wizard already does acquisition (`github_fetch::fetch_tree`) and writing (`mode_wizard::materialize`, including the `.zoid-provenance.json` schema). The AI-assisted `ProposeModeMappingTool` is the only piece that needs a model. This design keeps acquisition and writing verbatim and swaps the AI mapping for a **pure, pinned, deterministic mapping function**. One on-disk format, two ways to create it — so `:mode update superpowers` keeps working on the result.

---

## 3. The pinned source (reproducibility contract)

```
repo:         obra/superpowers
ref:          d884ae04edebef577e82ff7c4e143debd0bbec99   (pinned SHA, NOT "main")
subtree_path: skills
url form:     github.com/obra/superpowers/tree/<SHA>/skills
```

The SHA is pinned so re-installs are byte-stable and the generated `.zoid-provenance.json` matches across users. The pinned SHA is a single `const` in the recipe module; bumping it is a one-line change (and a deliberate act, reviewed like any dependency bump). It mirrors the SHA the official marketplace pins for the same plugin.

---

## 4. Architecture

Four steps; only step 2 is new code.

1. **Acquire (reuse).** `github_fetch::parse_github_url(PINNED_URL)` → `fetch_tree(&GithubClient, &url)` → `UpstreamScan`. This is the GitHub REST tree API (`api.github.com/repos/{owner}/{repo}/git/trees/{ref}?recursive=1`) behind the mockable `GithubApi` trait, returning every file under `skills/` with its content and per-file blob SHA.

2. **Map (new, pure).** `superpowers_mapping(&UpstreamScan) -> ModeMapping`:
   - `skills/using-superpowers/SKILL.md` → **generated** `mode.md` (see §5). This mirrors the wizard's judgment call: the `using-superpowers` loader skill becomes the mode's ambient overlay rather than a scoped skill.
   - every other `skills/<skill>/**` file → canonical path `<skill>/**`, copied **verbatim** (includes `references/*.md`, `*-prompt.md`, sub-docs — everything under each skill folder).
   - Produces the same `ModeMapping` shape the `ApplyModeMappingTool` produces, so step 3 is unchanged.

3. **Write (reuse).** `mode_wizard::materialize(&scan, &mapping, mode_name="Superpowers", fetched_at, modes_dir)` writes the canonical files and the `.zoid-provenance.json` (schema v1) into `<cfg>/modes/superpowers/`. Because it is the *same* writer, provenance is `:mode update`-compatible.

4. **Reload (reuse).** Emit the existing `ReloadModes` action; `build_mode_registry` re-scans and Superpowers joins the Shift+Tab cycle immediately. On success the handler switches to (or announces) the new mode.

### 4.1 Components & files

| File | Change |
|------|--------|
| `crates/zoid/src/superpowers_install.rs` *(new)* | `const PINNED_*`; `superpowers_mapping()` (pure); `generate_mode_md()` (pure); `install_superpowers()` async orchestrator (acquire → map → materialize → reload). |
| `crates/zoid-tui/src/command.rs` | Parse `:mode install superpowers` → `Command::ModeInstallSuperpowers`. |
| `crates/zoid/src/main.rs` | Handle the command (async, progress, success/failure status); palette entry; onboarding keypress dispatch. |
| `crates/zoid-tui/src/onboarding.rs` | Add the opt-in first-run affordance line (§6). |
| `crates/zoid/src/mode_wizard.rs` / `github_fetch.rs` / `zoid_core::wizard` | **No behavior change.** `materialize` and `fetch_tree` already live in the `zoid` bin crate alongside the new module, so they are reachable in-crate (widen to `pub(crate)` only if currently private to their module). |

---

## 5. `mode.md` generation (deterministic)

The wizard authored `mode.md` with a model. The recipe generates an equivalent from a **fixed template** plus **frontmatter extracted mechanically** from each skill's `SKILL.md`:

```
---
name: Superpowers
description: Superpowers — a curated skill set for structured software
  engineering workflows (TDD, debugging, code review, planning, parallel
  agents, git worktrees, verification), imported from obra/superpowers.
---
You are operating in "Superpowers" mode, imported from obra/superpowers (ref d884ae0).

This mode activates a curated set of skills for software engineering workflows.
Before any task, check if an available skill applies and invoke it with invoke_skill.
The skills are:

- <name>: <description>        # one line per skill, from each SKILL.md frontmatter
  ...

Always check for an applicable skill before starting work. If multiple skills
apply, invoke the most specific one first. After completing work, invoke
verification-before-completion before claiming success.
```

- The bullet list is built by parsing each non-loader `skills/<skill>/SKILL.md`'s frontmatter `name` + `description` (reuse `zoid_core::skill::parse_skill_md`). Deterministic ordering: alphabetical by skill folder (matches `build_mode_registry`'s sort).
- Uses zoid's `invoke_skill` mechanism (not Claude Code's Skill tool) — this is the concrete reason we template rather than copy `using-superpowers/SKILL.md` verbatim, whose body targets a different host.
- The generated `mode.md` is what `materialize` records under the `mode.md` provenance entry (`upstream_path: skills/using-superpowers/SKILL.md`), exactly as the wizard did.

---

## 6. In-app surface

Two surfaces, one code path.

**6.1 Command.** `:mode install superpowers` → `Command::ModeInstallSuperpowers`. Also a palette entry ("Install Superpowers mode"). The handler runs `install_superpowers()` asynchronously (network I/O off the UI thread), shows a progress hint while fetching, and reports success or a specific failure.

**6.2 Onboarding.** `onboarding.rs`'s first-time-user empty-state screen gains one interactive line:

```
  Press s to install the Superpowers skill set
  (brainstorming, TDD, systematic debugging, code review, planning…)
```

Shown only when `first_time_user` is true and Superpowers is not already installed. **Keypress gating (explicit, to avoid hijacking typing):** `s` triggers the install **only while the empty-state onboarding is on screen AND the message input buffer is empty** — the same "special key on empty buffer" convention `route.rs` already uses for `:` opening the palette (`Focus::Input` + `state.input_empty`). Once the user types any character, `s` is a literal again and the affordance is inert. Declining (typing a message, or ignoring the line) leaves zoid completely untouched — the opt-in guarantee.

---

## 7. Error handling & graceful degradation

Non-fatal, mirroring the mode importer's "degrade, never crash" stance.

- **Network / GitHub API / rate-limit** (unauthenticated API is 60 req/hr) → clear status message ("couldn't reach github.com" / "GitHub rate limit — try later"); **no partial mode** is left behind (materialize writes the folder atomically, or the handler removes a half-written folder on error).
- **Offline** → "Superpowers install requires network access."
- **Parse failure of an upstream SKILL.md** → that skill is skipped with a warning (the mode still installs), matching `build_mode_registry`'s per-skill tolerance. `mode.md`'s bullet list simply omits a skill it couldn't parse.
- **Already installed** → re-run overwrites `superpowers/` (same semantics as `:mode update`). Pinned SHA ⇒ byte-stable re-install.

---

## 8. Idempotency & update

- Re-running the command is safe and deterministic: same SHA → same bytes.
- Because the output is provenance-identical to the wizard's, the existing `:mode update superpowers` reconciliation works unchanged (diffs canonical files against a fresh scan at the recorded ref).
- Bumping the pinned SHA is the intended way to move Superpowers forward; after a bump, `:mode update` surfaces the drift.

---

## 9. Testing

- `superpowers_mapping()` — pure. Unit-test against a fixture `UpstreamScan`: asserts `using-superpowers/SKILL.md` maps to `mode.md`, every other file maps to `<skill>/**`, and supporting files (references, prompts) are included.
- `generate_mode_md()` — pure. Unit-test: given fixture skills, asserts frontmatter, the `invoke_skill` instruction, alphabetical bullet list of `name: description`, and the closing verification instruction.
- `fetch_tree` — already covered via the mockable `GithubApi`; add a recipe-level test that wires a fake API returning a small superpowers-shaped tree and asserts the materialized layout + provenance shape (schema, source.ref = pinned SHA, file entries). **No network in tests.**
- Onboarding line — snapshot test for the `first_time_user` screen (present when not installed, absent otherwise).
- Keypress gating — route unit test: `s` on the first-run empty-state with an **empty** buffer → install action; `s` with a **non-empty** buffer → literal character (`Edit`), no install.
- Command parse — `:mode install superpowers` → `Command::ModeInstallSuperpowers` (and that it does not collide with `:mode <name>` switching to a mode literally named "install").

---

## 10. Out of scope

- Installing arbitrary skill sets deterministically (this is Superpowers-specific; arbitrary repos stay with the AI `:mode import`).
- An installer-side (cargo-dist) prompt — explicitly rejected: non-interactive pipes can't prompt, it's install-channel-specific, customizing the generated script is fragile, and it can't re-offer or update.
- Auto-install on launch — violates the opt-in ethics constraint.
- Authenticated GitHub fetch / higher rate limits — unauthenticated is sufficient for a one-shot install; revisit only if rate limits bite.

---

## 11. Risks

- **Upstream restructure.** If `obra/superpowers` reorganizes `skills/`, the deterministic mapping may mis-map. Mitigation: the pinned SHA freezes structure; a SHA bump is a reviewed change where mapping is re-verified. The AI `:mode import` remains the escape hatch for a restructured repo.
- **GitHub API rate limit** on shared IPs. Mitigation: clear messaging; the operation is one-shot and cached on disk.
- **`invoke_skill` drift.** If zoid renames its skill-invocation mechanism, the templated `mode.md` must follow. Mitigation: the template is a single `const` in the recipe module.
