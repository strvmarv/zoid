# zoid — URL Import Wizard (Slice 4) · Design

**Date:** 2026-07-05
**Status:** Approved design, ready for implementation plan
**Slice:** 4 of the mode/skill seam — the **URL import wizard** + **update reconciliation**. Network-aware, LLM-leaning, human-approved. The on-ramp that completes the Slice-3 promise ("drop a folder in, *or* paste a URL").
**Author:** gomanjoe (with Claude)

> **Spec set.** This continues the mode/skill direction:
> - **Slice 0 — runtime spike** → `2026-07-03-mode-skill-runtime-spike-design.md`. **Merged** `c79a1ed`; smoke PASS on `glm-5.2:cloud`.
> - **Slice 2 — SKILL.md importer** → `2026-07-04-skill-md-importer-design.md`. **Merged** `6cc9a4d`.
> - **Slice 3 — mode promotion + Shift+Tab quick-switch** → `2026-07-05-mode-promotion-quickswitch-design.md`. **Merged** `f52d928`; this slice is its §12.3 follow-on.
> - **Slice 4 — this doc** (URL import wizard + update).

---

## 1. Overview

A **mode** is a named agent that owns a scoped set of skills (Slice-3 §1). Slices 0–3 made modes work from **on-disk canonical files** — a `mode.md` + `*/SKILL.md` folder, parsed deterministically by `parse_skill_md`, discovered under convention dirs, cycled with Shift+Tab, ambient system-prompt overlay honored, per-session persistence, hot-reload. The runtime below the waist is finished and stable.

This slice builds the **on-ramp above the waist**: turn a URL into an on-disk mode folder by having the active model propose a mapping onto the canonical contract, the user approves it, zoid materializes canonical files + a provenance sidecar, then reloads — the mode is live with no restart. The same loop, run against the sidecar as a three-way base, is the **update** flow: re-fetch upstream, the model proposes a *merged* mapping that converges upstream changes with local edits, one approval gate, write changed files, refresh the sidecar, reload.

**In one sentence:** the wizard is a stateful chat session that turns a URL into an on-disk mode folder by having the active model propose a mapping the user approves, then reload — and update is the same loop with provenance.

### Why this slice now

Slices 0–3 retired the architectural and behavioral risks (the runtime runs; a small model drives `invoke_skill`; the canonical contract is crisp; the loader is total). The one remaining risk in the direction is **mapping quality**: can a small local model take a heterogeneous upstream tree and produce a *valid, useful* mapping onto the canonical contract, under human approval? That's a behavioral question, answered by the Tier-2 smoke (§11). Spiking it now retires the last scary unknown; if it FAILs, the fallback (deterministic mapping, model-only-for-descriptions) is already on the table (§11 decision gate).

---

## 2. North star (inherited from Slice 3)

**zoid is a thin, stable host; the ecosystem moves fast around it.** Modes and skills are drop-in extensions the user adds without waiting on a zoid release: drop a folder into `~/.config/zoid/modes/`, *or* (this slice) paste a URL. The LLM leaning lives entirely at the import boundary, under human approval — never in the runtime.

Design rule: **the wizard's only output is canonical files on disk.** The runtime below the waist is unchanged. A materialized mode is indistinguishable from a hand-authored one.

---

## 3. Scope

### In scope

- `:mode import <url>` — start an import wizard for a GitHub tree URL.
- `:mode update <name>` — start an update wizard for an existing imported mode.
- GitHub HTTP API fetcher (tree + blob, `$GITHUB_TOKEN` optional for rate limit / private repos).
- `propose_mode_mapping` tool (gated into the turn only while a wizard is active).
- `apply_mode_mapping` tool (the approval gate — raises AskUser; on Approve, materializes).
- Chat-iterate proposal loop: the user can reply "adjust" in chat and the model re-proposes against the same cached scan (no re-fetch).
- Canonical-file materializer writing to **user-global** `~/.config/zoid/modes/<slug>/`.
- `.zoid-provenance.json` sidecar (per-mode, co-located, human-readable, the three-way base for update).
- Update reconciliation: model-driven merged mapping, one approval gate, file-set reconciliation (add/update/drop), sidecar refresh, reload.

### Out of scope

- **Non-GitHub sources** — rejected with a clear error in v1 (§9). GitLab/Bitbucket/raw sites are a later slice.
- **A structured TUI form editor for the mapping** — chat is the editor (§1, §7).
- **Honoring a mode's `tools`/`model` fields** — still seamed from Slice-3 §12.1; the overlay body IS honored, the tool allow-list and model override are not.
- **The overlay picker** — still deferred from Slice-3 §12.2.
- **Git-as-source** — cloning a repo locally and pointing `[modes] source_dirs` at it already works via Slice 3; this slice is HTTP-tree-only.
- **Multi-mode-from-one-URL** — one URL → one mode. If a tree contains multiple mode candidates, the model picks one and skips the rest with reasons. (A "multi-mode import" is a possible later slice; not needed for the Superpowers case.)
- **Cross-machine sidecar portability** — `canonical_path`s are relative within the mode folder (asserted §11), but we don't test copying a mode folder between machines with different OS path conventions. Forward-compatible, not validated.
- **Concurrent wizards** — only one wizard at a time; a second `:mode import` cancels the first (§9).
- **Crash-recovery of staging** — the materializer snapshots to staging before writing and restores on error (§9). A crash *during* materialize leaves staging on disk; detecting and prompting to restore on next launch is P1 polish, out of scope for v1 (we best-effort-restore on caught errors only).

---

## 4. Vocabulary & invariants

- **User-facing surface is "import wizard" / "update wizard"; internal model is `ModeImportWizard` state in `App`.** Never "agent" in the UI.
- **Imported modes are user-global, not repo-specific.** The materializer writes to `~/.config/zoid/modes/<slug>/` (the user-global convention dir, Slice-3 §7), never `<cwd>/.zoid/modes/`. An imported methodology like Superpowers is something the user wants everywhere, not tied to one repo. The repo-local dir stays for hand-authored repo-specific modes.
- **One URL → one mode.** The model proposes one `ModeMapping` per wizard.
- **The wizard never writes anything until the user approves.** The scan, the proposal, the chat iteration — all pure / in-memory. The only persistent effect is the materialize step (§8).
- **The wizard is model-agnostic.** It runs in the main turn with whatever model the user has active. No routing exception, no surprise bills. If the mapping is poor, the user switches models and retries — same loop they already know.
- **The LLM leaning lives at the boundary, under human approval.** The model proposes; the user approves; the materializer validates against the canonical contract *before* asking and again *before* writing. The runtime below the waist is deterministic and never sees a malformed file.
- **Provenance is co-located with the mode, not in the session DB.** A `.zoid-provenance.json` sidecar travels with the mode folder; no schema migration; survives DB deletion; human-readable (§7).

---

## 5. Architecture

Two layers, mirroring the Slice-3 split: a **pure `zoid-core` mapping model** and an **effectful `zoid` bin wizard**. The runtime below the waist is untouched — the wizard's only output is canonical files on disk; `mode_import.rs` reads them as before.

### Pure core (`zoid-core/src/wizard.rs` — new)

Value types the bin builds and the model proposes. No FS/network deps.

```rust
/// One file fetched from upstream at scan time. The bin fills this; the model
/// reads it as a tool result. `content` is the raw bytes decoded as UTF-8
/// (lossy); `sha` is the GitHub blob SHA (stable identity across ref moves).
pub struct ScannedFile {
    pub upstream_path: String,   // "skills/brainstorming/SKILL.md"
    pub sha: String,             // GitHub blob SHA
    pub content: String,         // raw decoded text
}

/// The scanned tree the wizard holds in App state. `resolved_ref` is the commit
/// SHA at scan time, so an update can re-fetch at the same ref OR at latest and
/// compare.
pub struct UpstreamScan {
    pub url: String,             // original URL the user pasted
    pub repo: String,            // "obra/superpowers"
    pub resolved_ref: String,    // commit SHA at scan time
    pub subtree_path: String,    // "skills" (the tree path within the repo)
    pub files: Vec<ScannedFile>,
}

/// The model's proposal for one canonical file. `source` is the upstream path
/// this canonical file should be materialized from; `canonical_path` is where
/// it lands in the mode folder (e.g. "brainstorming/SKILL.md" or "mode.md").
/// `skipped` files carry a `reason` instead.
pub enum MappingEntry {
    Materialize {
        canonical_path: String,
        source: String,         // upstream_path in the scan
        summary: String,         // one-line human-readable, shown in approval
    },
    Skip {
        upstream_path: String,
        reason: String,
    },
}

/// The full proposed mapping. The model emits this as args to
/// `apply_mode_mapping`; the user approves or adjusts; the bin materializes the
/// `Materialize` entries.
pub struct ModeMapping {
    pub mode_name: String,
    pub mode_description: String,
    pub mode_body: String,        // proposed mode.md body (the overlay text)
    pub entries: Vec<MappingEntry>,
}

/// One entry in the per-mode provenance sidecar. Read at update time.
pub struct ProvenanceEntry {
    pub canonical_path: String,
    pub upstream_path: String,
    pub upstream_ref: String,     // ref at last import
    pub upstream_sha: String,
    pub upstream_snapshot: String,// content we wrote from (== current canonical
                                  //   if the user never edited locally)
}

/// Classification of one canonical file for update, computed by the pure
/// `classify_update` helper. The bin builds a human-readable "reconciliation
/// brief" from these and passes it as the `propose_mode_mapping` tool result on
/// update, so the model's proposal is already informed by the three-way
/// analysis.
pub enum UpdateClass {
    Unchanged,                         // local==snapshot, upstream sha same
    UpstreamMoved { new_content: String },  // local==snapshot, upstream content differs
    LocalOnlyChanged,                  // local!=snapshot, upstream same
    BothChanged { new_upstream: String },   // local!=snapshot, upstream differs
    NewUpstream { new_content: String },    // in fresh scan, not in provenance
    UpstreamDeleted,                   // in provenance, not in fresh scan
}

pub fn classify_update(
    provenance: &ProvenanceEntry,
    local_canonical: &str,
    fresh_scan: &UpstreamScan,
) -> UpdateClass;
```

### Effectful bin (`zoid`)

| Unit | File | Responsibility |
|---|---|---|
| GitHub fetcher | **new** `crates/zoid/src/github_fetch.rs` | `reqwest` client (workspace dep, already used by `update.rs`); `$GITHUB_TOKEN` from env; tree + blob fetch via `api.github.com/repos/{owner}/{repo}/git/trees/{ref}?recursive=1` + per-blob `download_url`. URL parser: `github.com/{o}/{r}/tree/{ref}/{path}` (and `/blob/{ref}/{path}` → single-file scan). Non-GitHub → `Err`. Rate-limit-aware (reads `X-RateLimit-Remaining`, surfaces a "set GITHUB_TOKEN" error on 0). HTTP calls behind a `GithubApi` trait with a fake impl in tests — no real network in CI. |
| Wizard state | `crates/zoid/src/main.rs` `App` | New `wizard: Option<ModeImportWizard>` field. `ModeImportWizard { scan: UpstreamScan, mode_name_target: Option<String> }` (target set on update). Gated into the turn's tool set in `spawn_turn` (alongside the existing per-turn `invoke_skill` snapshot, Slice-3 §5). Cleared on approve/reject/cancel. |
| Mapping tools | **new** `crates/zoid/src/mode_wizard.rs` | `ProposeModeMappingTool` (impl `Tool`, name `propose_mode_mapping`) + `ApplyModeMappingTool` (impl `Tool`, name `apply_mode_mapping`). The propose tool's `run()` returns the cached scan (import) or the reconciliation brief (update) as its tool result — the model reads it and emits `ModeMapping` as args to `apply_mode_mapping`, which validates, raises `AgentUpdate::AskUser`, and on Approve materializes. |
| Materializer | `crates/zoid/src/mode_wizard.rs` | `materialize(mapping, scan, dest_dir) -> Result<PathBuf, MaterializeError>` writes canonical files + `.zoid-provenance.json` sidecar. Snapshot-to-staging-then-write for atomicity (§9). Idempotent: re-running on the same mapping overwrites cleanly. |
| Commands | `crates/zoid-tui/src/command.rs` | New `Command::ModeImport(String)` (`:mode import <url>`) and `Command::ModeUpdate(String)` (`:mode update <name>`). Reuses the existing `:mode` prefix family; parser disambiguates `import`/`update`/`reload`/`<name>` before falling through to `SwitchMode`. |
| Wiring | `crates/zoid/src/main.rs` | `ModeImport`/`ModeUpdate` handlers: spawn async fetch (status hint while in flight), build wizard state, push a seed user message ("Import wizard started. Call propose_mode_mapping…"), spawn a turn. Approval handler (existing `ask_user` reply path, `main.rs:2856`): materialize → reload (`build_mode_registry`, `main.rs:2933`) → clear wizard → confirm. |

### Data flow — one import

```
1. User: `:mode import github.com/obra/superpowers/tree/main/skills`
2. bin:  spawn async fetch → UpstreamScan { repo, resolved_ref=SHA,
          subtree_path="skills", files=[…] }
          (status hint while in flight: "fetching <url>…")
3. bin:  App.wizard = Some({ scan, mode_name_target: None });
          push user message: "Import wizard started for <url>.
          Call propose_mode_mapping to see the upstream scan, then call
          apply_mode_mapping with your proposed mapping onto the canonical
          contract."
4. spawn_turn: wizard is Some → include propose_mode_mapping + apply_mode_mapping
          in the turn's tool set (alongside the normal chat tools).
5. model: calls propose_mode_mapping() → tool result = the scan
          (paths + contents, as a structured text block)
6. model: reads scan, calls apply_mode_mapping({
            mode_name: "Superpowers",
            mode_description: "Superpowers methodology",
            mode_body: "<using-superpowers body>",
            entries: [
              Materialize { canonical_path: "brainstorming/SKILL.md",
                            source: "skills/brainstorming/SKILL.md",
                            summary: "brainstorming skill, verbatim" },
              Skip { upstream_path: "README.md",
                     reason: "repo readme, not a skill" },
              …
            ]
          })
7. apply_mode_mapping.run(): VALIDATE the mapping (§8) BEFORE raising AskUser.
          If valid → AgentUpdate::AskUser {
            question: "Proposed mode 'Superpowers': 13 skills, 2 skipped.
                       Approve?",
            choices: ["Approve", "Reject", "Adjust"] }
8. user:
   - Approve  → materialize to ~/.config/zoid/modes/superpowers/ +
                 write .zoid-provenance.json + reload + clear wizard +
                 "imported 'Superpowers' — Shift+Tab to it"
   - Reject   → clear wizard, "import cancelled"
   - Adjust   → the user's free-text reply is pushed as a user message;
                propose_mode_mapping returns the SAME cached scan (no re-fetch);
                the model re-proposes; loop to 6
```

### Data flow — one update

```
1. User: `:mode update Superpowers`
2. bin:  read ~/.config/zoid/modes/superpowers/.zoid-provenance.json
          → Vec<ProvenEntry> + source { repo, ref, subtree_path }
3. bin:  re-fetch upstream at the ORIGINAL ref (stable, not latest) → fresh scan
          with the same SHAs to compare against.
          (Note: we fetch at the original ref so SHA comparisons are apples-to-
          apples. If the user wants "update to latest", that's a follow-on
          `:mode update --latest Superpowers`; v1 updates at the original ref
          and a separate `:mode update --latest` advances the ref. See §12.)
4. bin:  for each provenance entry, classify_update() vs fresh scan + local file
          on disk → Vec<UpdateClass>
5. bin:  build reconciliation brief (human-readable text block):
          "brainstorming/SKILL.md: upstream unchanged.
           writing-plans/SKILL.md: upstream changed (diff summary).
           new-skill/SKILL.md: upstream added.
           removed-skill/SKILL.md: upstream deleted.
           using-superpowers/SKILL.md (mode.md): local-only changed."
6. bin:  App.wizard = Some({ scan: fresh_scan, mode_name_target:
          Some("Superpowers") });
          push user message: "Update wizard started for 'Superpowers'.
          Call propose_mode_mapping to see the reconciliation brief, then call
          apply_mode_mapping with your merged mapping."
7. model: calls propose_mode_mapping() → tool result = reconciliation brief
8. model: proposes merged ModeMapping:
          - carry unchanged entries as Materialize (no content change)
          - upstream-moved + local untouched → Materialize with new content
          - upstream-moved + local edited (BothChanged) → Materialize with the
            model's pick (carry local, take upstream, or merge), flagged in the
            summary so the user sees the decision at approval
          - local-only-changed → Materialize with local content (keep the edit)
          - new-upstream → Materialize (add the skill)
          - upstream-deleted → Skip { reason: "upstream deleted; keeping local
            copy" } OR Skip { reason: "upstream deleted; dropping local" } —
            the model decides, the user approves
9. apply_mode_mapping → VALIDATE → AskUser "Approve merged mapping?" → Approve →
   materialize (file-set reconciliation: add/update/drop, §8) + refresh sidecar
   + reload + clear wizard + "updated 'Superpowers'"
```

**Key invariant:** the update fetches at the **original ref** (recorded in the sidecar), not latest. This keeps SHA comparisons meaningful (a file at the same SHA at the same ref is unchanged upstream). "Update to latest" is a separate `:mode update --latest <name>` that advances the recorded ref after materializing (§12 open question).

---

## 6. The canonical-contract mapping rules

The canonical contract (Slice-3 §3, enforced by `parse_skill_md`, `zoid-core/src/skill.rs`) is the target. The model's job is to decide, per upstream file, **which canonical role it plays** and **what its canonical content is**. The rules below constrain the proposal; the materializer validates them and rejects (with reasons) anything that violates them — the model never gets to write a malformed mode.

### Roles a file can map to

| Role | Canonical path | Rules |
|---|---|---|
| Mode manifest | `mode.md` | Exactly one per mapping. Frontmatter `name:` = `mode_name` (required, non-empty, must not collide with `default` or an existing `ModeRegistry::names()` entry; **update flow is exempt** — the target mode name *is* the existing one, by design). `description:` = `mode_description` (optional; model generates one if missing). Body = `mode_body` (the overlay; may be empty ⇒ behaves like Chat). |
| Scoped skill | `<slug>/SKILL.md` | Zero or more. `<slug>` = the skill's canonical `name` (the model proposes it; conventionally lowercased kebab-case matching the upstream folder, but the model may rename to resolve collisions or fix non-canonical shapes). Frontmatter `name:` required. Body verbatim. |
| Bundled sibling | `<slug>/<relative-path>` | A non-`SKILL.md` file inside a skill folder (e.g. `brainstorming/scripts/server.cjs`). Materialized verbatim under the skill's folder. NOT parsed by `mode_import` — it's payload the skill body may reference via its `base_dir` (Slice-2 `skill_import.rs:79`). |
| Skipped | — | `MappingEntry::Skip { upstream_path, reason }`. Anything the model judges irrelevant: READMEs, license files, CI configs, tests-for-upstream, non-`.md` noise. The reason is shown in the approval summary. |

### Content transformation rules (what the model is allowed to do)

- **Verbatim copy** for files that already match the canonical shape (a `SKILL.md` with valid frontmatter → materialize as-is).
- **Frontmatter synthesis** for `SKILL.md`-shaped files missing a `name` or `description`: the model generates them. Generated descriptions are flagged in the approval summary ("description generated by zoid").
- **Body extraction** for files that are *almost* `SKILL.md` but have extra wrapping (an HTML preamble, a code fence around the body): the model extracts the canonical body. Flagged in the summary.
- **Mode body synthesis** for `mode.md`: if upstream has no obvious "loader" file (no `using-superpowers.md` equivalent), the model may synthesize a minimal mode body or leave it empty. Flagged.
- **No content fabrication for skill bodies.** The model may reformat/extract but must not invent skill instructions that aren't upstream. If a skill body is missing, it's `Skip`, not synthesized.

### Materializer validation (enforced before AskUser and again before write)

The materializer enforces, on the proposed `ModeMapping`:
- `parse_skill_md` must succeed on every proposed `SKILL.md` content and the `mode.md` content.
- The mode name must not collide with `default` or existing `ModeRegistry::names()` (update flow exempt for the target name).
- Skill folder names (`<slug>`) must be unique within the mode.
- No two entries may map to the same `canonical_path`.
- Every `Materialize.source` must exist in the scan.
- Exactly one `mode.md` entry (or zero — an empty mode is allowed and behaves like Chat; flagged in the summary).

Violations → the whole proposal is rejected with a structured error listing every problem; the wizard stays open (no AskUser raised for an invalid mapping); the model self-corrects next round.

---

## 7. Provenance sidecar format

One file per materialized mode, at `<mode-folder>/.zoid-provenance.json`. Human-readable JSON, co-located with the mode. The runtime's `mode_import.rs` **ignores it** (it's not a `SKILL.md` or `mode.md`), so loading a mode is unaffected. No schema migration — it's just a file on disk.

```json
{
  "schema": 1,
  "source": {
    "url": "https://github.com/obra/superpowers/tree/main/skills",
    "repo": "obra/superpowers",
    "ref": "abc123def456...",
    "subtree_path": "skills",
    "fetched_at": "2026-07-05T12:00:00Z"
  },
  "mode_name": "Superpowers",
  "files": [
    {
      "canonical_path": "mode.md",
      "upstream_path": "skills/using-superpowers/SKILL.md",
      "upstream_sha": "blob-sha-1",
      "upstream_ref": "abc123def456...",
      "upstream_snapshot": "---\nname: using-superpowers\n...\n---\n<body as materialized>"
    },
    {
      "canonical_path": "brainstorming/SKILL.md",
      "upstream_path": "skills/brainstorming/SKILL.md",
      "upstream_sha": "blob-sha-2",
      "upstream_ref": "abc123def456...",
      "upstream_snapshot": "---\nname: brainstorming\n...\n---\n<body as materialized>"
    },
    {
      "canonical_path": "brainstorming/scripts/server.cjs",
      "upstream_path": "skills/brainstorming/scripts/server.cjs",
      "upstream_sha": "blob-sha-3",
      "upstream_ref": "abc123def456...",
      "upstream_snapshot": "<file contents as materialized>"
    }
  ]
}
```

### Field semantics

- **`upstream_snapshot`** is the content we materialized *from* (post any frontmatter-synthesis/body-extraction the model did at import time). This is the three-way base. If the user never edits a canonical file locally, `local == snapshot`. If upstream moves but local == snapshot, update re-materializes safely (pure upstream change). If both diverge from snapshot, it's a `BothChanged` conflict the model reconciles in its proposal.
- **`upstream_sha`** is the GitHub blob SHA at import time. On update at the same ref, a matching SHA means upstream unchanged; a differing SHA means upstream moved (the fetcher returns the new content).
- **`upstream_ref`** is the commit SHA at scan time, recorded once at import and carried on every entry. Update fetches at this ref (§5).
- **`canonical_path`** is relative to the mode folder (never absolute) — keeps the sidecar portable across machines and OS path conventions.

### Skipped files are NOT in the sidecar

They weren't materialized, so they have no provenance. On update, a previously-skipped file appearing upstream is just `NewUpstream` from the fresh scan's perspective — the model sees it and decides whether to materialize this time.

### Update writes a fresh sidecar

After materialization: any file that changed gets its `upstream_sha`/`upstream_ref`/`upstream_snapshot` updated to the new ref. Any file materialized for the first time this round is added. Any file dropped this round (§8 file-set reconciliation) is removed from the sidecar.

---

## 8. Materialization & file-set reconciliation

### The materialize step (after Approve)

`materialize(mapping, scan, dest_dir) -> Result<PathBuf, MaterializeError>`:

1. **Validate** the mapping against the rules in §6 (parse every proposed file, check name collisions, dedupe paths, check sources exist). Any violation → `Err(MaterializeError { problems: Vec<…> })`, no files written, wizard stays open.
2. **Snapshot to staging.** If `dest_dir` already exists (update flow), copy it to `<dest_dir>.zoid-staging-<mode>/` in the same parent. This is the rollback base on any mid-write failure.
3. **Reconcile the file set** (update flow only; import flow is "write everything fresh"):
   - For each `Materialize` entry: write/overwrite `dest_dir/<canonical_path>` with the proposed content.
   - For each canonical path in the **old sidecar** that is **not** in the new mapping: **delete it from disk** (the user approved dropping it). Log each deletion.
   - New files (in mapping, not in old sidecar) → created.
   - Unchanged files (in both, same content) → untouched on disk (skip the write; cheap).
4. **Write the sidecar.** Fresh `.zoid-provenance.json` reflecting the new file set + new SHAs/refs/snapshots.
5. **Delete staging** on success.

**The dropped-file rule (stated explicitly because it's the one subtlety):** if the new mapping skips a file that was previously materialized, the materializer **deletes the old canonical file**. Otherwise the mode would carry a skill the user thought they were dropping — `mode_import` would still load it as a scoped skill. The materializer reconciles the *file set* too, not just content.

### Atomicity

On any mid-materialize failure (OS error on a write, sidecar write fails, deletion fails):
- Best-effort delete any files written in this attempt.
- If staging exists (update flow), restore `dest_dir` from staging.
- Return `Err`. The wizard stays open. AskUser re-raises: `"materialize failed: <error>. Retry / Cancel?"` The user can Retry (re-attempt with the same approved mapping) or Cancel (clear wizard, no disk changes).

A mode folder without a sidecar is valid for the runtime (it loads fine) but **breaks update** — so we treat sidecar-write as mandatory and roll back if it fails.

### Destination is user-global

`dest_dir` is always `~/.config/zoid/modes/<slug>/`, where `<slug>` is derived from `mode_name` (lowercased, spaces→hyphens, filesystem-safe). Never `<cwd>/.zoid/modes/`. The materializer takes the user-global dir as a parameter (testable with a tempdir); the bin passes the resolved user cfg dir.

---

## 9. Error handling & degradation

The wizard inherits the slice's governing principle: **a bad input produces a value, never aborts.** Every failure surfaces as a recoverable state — the wizard stays open, the user can adjust/retry/cancel — never a panic, never a half-written mode folder.

### Fetch failures (before the wizard opens)

| Failure | Behavior |
|---|---|
| Non-GitHub URL | Status hint: `"URL import supports github.com URLs only (got 'gitlab.com/...'). See :help mode import."` No wizard opened. |
| Malformed GitHub URL (not `/tree/` or `/blob/`) | Status hint with the expected shape + an example. No wizard. |
| Network error / timeout | Status hint: `"fetch failed: <error>. Retry with :mode import <url>."` No wizard. |
| HTTP 404 (tree/path not found) | Status hint: `"GitHub returned 404 for <path>. Check the ref/path."` No wizard. |
| HTTP 403 with rate-limit headers (`X-RateLimit-Remaining=0`) | Status hint: `"GitHub rate-limited. Set $GITHUB_TOKEN for a higher limit."` No wizard. |
| HTTP 403/401 on a private repo without/with bad token | Status hint: `"GitHub denied access (401). Set $GITHUB_TOKEN with repo scope for private repos."` No wizard. |
| Empty tree (zero files at path) | Status hint: `"no files found at <path> on ref <ref>."` No wizard. |
| Fetch succeeds but no file looks like a skill (no `SKILL.md`, no `*.md`) | **Wizard opens anyway.** The model decides whether to propose an empty mode or skip everything. The approval summary will say "0 skills, mode body empty — this will behave like Chat." User can reject. |

**Rationale for "wizard opens on empty-ish tree":** the model might judge that a tree of scripts is actually a single mode with bundled siblings and no skills. That's the model's call, not the fetcher's. The fetcher's job is "get the bytes"; the model's job is "propose a mapping." We don't pre-filter at the fetch boundary.

### Scan/wizard-open failures (after wizard opens, before proposal)

Once `App.wizard = Some(scan)`, the scan is in memory. No IO failure is possible in this phase — `propose_mode_mapping` just returns the cached scan (or the cached reconciliation brief on update). This is deliberate: the chat-iterate loop is **pure** and never re-fetches, so a network blip mid-wizard can't strand the user.

### `apply_mode_mapping` failures (pre-approval)

The tool validates the model's `ModeMapping` *before* showing AskUser:

| Failure | Behavior |
|---|---|
| Malformed `ModeMapping` args (missing `mode_name`, bad `entries` shape) | `ToolOutput::err` with "apply_mode_mapping: <reason>. Re-propose with valid args." The model self-corrects next round. No AskUser raised. |
| Mode name collides with `default` or existing `ModeRegistry::names()` (import flow) | `ToolOutput::err`: "mode name 'X' collides with existing mode 'X'. Choose a different name." (Update flow is exempt for the target name.) |
| Two entries map to the same `canonical_path` | `ToolOutput::err`: "duplicate canonical path 'brainstorming/SKILL.md' in entries." |
| A `Materialize.source` doesn't exist in the scan | `ToolOutput::err`: "entry references upstream path 'foo.md' not in scan. Available: <list>." |
| Proposed `SKILL.md` or `mode.md` content fails `parse_skill_md` | `ToolOutput::err` with the parse reason. (Happens when the model synthesizes bad frontmatter — it gets the parse error and re-proposes.) |

All pre-approval. The user never sees a broken proposal; the model fixes it before asking. This keeps the approval gate meaningful — "Approve" is always a *valid* mapping.

### Materialize failures (after approval)

| Failure | Behavior |
|---|---|
| Destination dir not writable / disk full / OS error on one file | **Abort the whole materialize.** Any files already written in this attempt are deleted; if staging exists (update), restore from staging. The wizard stays open. AskUser re-raises: `"materialize failed: <error>. Retry / Cancel?"` No partial mode on disk. |
| Sidecar write fails but canonical files wrote OK | Same: abort, delete canonical files, restore from staging, re-raise. A mode folder without a sidecar breaks update — treat sidecar-write as mandatory. |
| Update flow: deleting a dropped file fails (OS error) | Abort the whole materialize, restore from staging, re-raise. Update must be atomic-ish; a half-updated mode is worse than no update. |

### Runtime degradation after a bad materialize (backstop)

Even if materialize somehow produces a malformed mode (a bug in the materializer, not a model error — the validators should catch everything), the Slice-3 loader's totality is the backstop: `mode_import.rs::load_mode` yields `Mode::Broken` for a bad `mode.md`, and a bad `SKILL.md` is skip-and-warn (`skill_import.rs:87`). The wizard never makes zoid unbootable. This is inherited, not new — but stated: **the runtime below the waist was already designed to survive bad on-disk modes; the wizard's output is just another input to it.**

### Cancel / quit-mid-wizard

- `:mode import` while a wizard is already open → the open wizard is **cancelled** (cleared) and a new one starts. Status hint: `"previous import wizard cancelled."` No partial state.
- `:mode update <name>` while an import wizard is open → same: cancel the import, start the update.
- App quit while a wizard is open → `App.wizard` is in-memory only, dropped on quit. No disk residue. (If the user had approved and materialize was mid-flight in a spawned task, the spawned task completes or is killed; on next launch the mode either loads or shows `Broken`, same as any other. The staging-restore from §8 covers the "killed mid-write" case best-effort for caught errors; crash-during-write leaves staging on disk, detected/restored in P1 polish — out of scope v1.)

---

## 10. Commands & UX

- **`:mode import <url>`** — starts an import wizard. `Command::ModeImport(String)`.
- **`:mode update <name>`** — starts an update wizard for an existing imported mode. `Command::ModeUpdate(String)`. If the named mode has no `.zoid-provenance.json`, surface: `"mode '<name>' has no import provenance; it was not imported from a URL. Use :mode import <url> instead."` and don't open a wizard.
- **`:mode update`** (bare) — usage hint: `"usage: :mode update <name> · :mode update --latest <name>"`.
- **Parser disambiguation** (`command.rs::parse_command`): `mode reload` (existing) → `ReloadModes`; `mode import …` → `ModeImport`; `mode update …` → `ModeUpdate`; `mode <name>` → `SwitchMode` (existing). Order matters: check `reload`/`import`/`update` prefixes before the fallthrough.
- **No new TUI surface.** The proposal renders in the conversation as the model's reply; the approval uses the **existing `AskUser` overlay** (`agent.rs:810`, `main.rs:1727`); the seed user message and the post-approve/reject status hints render in the status bar. The wizard adds no new render surface — chat is the editor, the existing overlay is the gate.
- **While a wizard is open, the mode chip** (Slice-3 §8) shows `"importing…"` or `"updating <name>…"` so the user knows a wizard is in flight. Cleared on wizard exit.

---

## 11. Testing & the go/no-go protocol

Two tiers, mirroring Slice-3. The behavioral question ("does the model produce a *good* mapping?") is a Tier-2 smoke; Tier-1 is deterministic wiring.

### Tier 1 — deterministic (`cargo test`, no network, no model)

**Pure core (`zoid-core/src/wizard.rs`):**
- `classify_update` classification: every `UpdateClass` variant is reachable and correct given constructed `{provenance, local, fresh_scan}` triples. Specifically: `Unchanged` (local==snapshot, upstream sha same), `UpstreamMoved` (local==snapshot, upstream content differs), `LocalOnlyChanged` (local≠snapshot, upstream same), `BothChanged` (local≠snapshot, upstream differs), `NewUpstream` (in fresh scan, not in provenance), `UpstreamDeleted` (in provenance, not in fresh scan).
- `ModeMapping`/`MappingEntry`/`ScannedFile`/`UpstreamScan`/`ProvenanceEntry` construct and access as documented; no hidden mutation.
- Sidecar (de)serialization round-trips: a `ProvenanceEntry` → JSON → back == original (the schema-1 shape). `canonical_path` is always relative (no absolute paths survive a round-trip).

**Effectful bin (`zoid`, temp dirs, no network — fetcher behind a trait):**
- `github_fetch.rs`: URL parser accepts `github.com/{o}/{r}/tree/{ref}/{path}` and `/blob/{ref}/{path}`; rejects `gitlab.com/...`, `github.com/{o}/{r}` (no tree), malformed shapes. The HTTP calls are behind a `GithubApi` trait with a fake impl in tests — no real network. Fake impl returns canned trees/blobs; the fetcher assembles an `UpstreamScan` with the right `resolved_ref`/`subtree_path`/files.
- Materializer: a valid `ModeMapping` + `UpstreamScan` → canonical files written to the **user-global temp dir** (`tempdir` standing in for `~/.config/zoid/modes`), each `SKILL.md`/`mode.md` parses via `parse_skill_md`, sidecar round-trips, mode name doesn't collide with `default`.
- Materializer validation: collision with `default`/existing names, duplicate canonical paths, bad upstream source ref, unparseable proposed `SKILL.md` → all produce a structured `MaterializeError`, no files written.
- Materializer atomicity: inject a failing write (a `WriteFail` fake FS) mid-materialize → previously-written files in this attempt are deleted, staging restored, error surfaced. No partial mode.
- **File-set reconciliation on update:** an old sidecar with files `{A, B, C}` + a new mapping that materializes `{A, B', D}` (B content changed, C dropped, D new) → on disk after materialize: `A` untouched, `B` overwritten, `C` deleted, `D` created, sidecar reflects `{A, B', D}`. The dropped `C` is gone (not left orphaned).
- `propose_mode_mapping` tool: `run()` returns the cached scan (import) or the reconciliation brief (update) from `App.wizard`. No IO.
- `apply_mode_mapping` tool: valid mapping → raises `AgentUpdate::AskUser`; invalid mapping → `ToolOutput::err` (no AskUser). The AskUser reply (Approve/Reject/Adjust) is wired through the existing `ask_user` seam — test the three branches: Approve → materialize+reload+clear-wizard; Reject → clear-wizard + status hint; Adjust → user message pushed, wizard stays, model re-proposes.
- **Tool gating:** `spawn_turn` includes `propose_mode_mapping` + `apply_mode_mapping` only when `App.wizard.is_some()`; absent otherwise. Asserted via a scripted provider that probes the tool list.
- **Destination is user-global:** materializer writes to the user-global dir, not `<cwd>/.zoid/modes/`. Asserted by pointing the materializer at a temp "home" and checking the file lands under `<home>/.config/zoid/modes/<slug>/`, not under the cwd temp dir.

**Integration (scripted provider, `zoid/tests/`):**
- `mode_import_wiring.rs`: a scripted provider that calls `propose_mode_mapping` then `apply_mode_mapping` with a canned mapping. Assert: AskUser raised, on "Approve" the canonical files appear at the user-global path, sidecar parses, `mode_import::build_mode_registry` loads the new mode as `Ready`, and the wizard is cleared. No real fetch (scan injected directly into `App.wizard`); no real model (scripted tool calls).
- `mode_update_wiring.rs`: pre-seed a mode folder + sidecar, inject a "fresh scan" that moves one file / adds one / drops one, scripted provider proposes a merged mapping, Approve → assert the on-disk file set reconciled correctly and the sidecar refreshed.

**TUI (`TestBackend`/`insta` snapshots):**
- `:mode import` and `:mode update` parse to the right `Command` variants (extend `command.rs` tests).
- AskUser overlay renders the proposal summary (mode name, skill count, skipped count) — reuses the existing AskUser render path, so a snapshot of the overlay with wizard-shaped content suffices.
- Status hints for the fetch failures (non-GitHub URL, rate-limited, 404) render in the status bar.

### Tier 2 — real-model go/no-go smoke (manual, documented)

A smoke-test runbook for this feature was created during implementation (not part of this spec) but was an internal-process artifact not carried into the public repo.

**Import smoke:** fresh session, `$GITHUB_TOKEN` set, `github.com/obra/superpowers/tree/main/skills` as the input.
- **PASS** = the model calls `propose_mode_mapping`, proposes a mapping with `Superpowers` as the mode name, the ~13 methodology skills as scoped skills, `using-superpowers` as the `mode.md` body, and skips the genuinely-irrelevant files (README, license, tests-for-upstream). User approves → folder materializes at `~/.config/zoid/modes/superpowers/` → `:mode` shows `Superpowers` → switching to it loads the skills → `invoke_skill("brainstorming")` returns its body.
- **PARTIAL** = proposes a mapping but gets the mode/skill split wrong (e.g. puts `using-superpowers` as a skill instead of the mode body), or skips too much, or generates bad frontmatter that the materializer rejects more than once.
- **FAIL** = never calls `propose_mode_mapping`, or proposes an empty/trivial mapping, or loops without converging.

**Update smoke:** after a successful import, hand-edit one local skill body, simulate upstream changing two files (by pointing at a different ref or editing the sidecar's `ref`), run `:mode update Superpowers`:
- **PASS** = the model's merged mapping carries the local edit, re-materializes the upstream-only-changed file, flags the both-changed one with its pick, and the result on disk matches the approved mapping.
- **PARTIAL** = reconciles structure but drops or clobbers the local edit against the model's stated intent.
- **FAIL** = can't produce a coherent merged proposal.

### Decision gate

- **Import PASS + update PASS** → the wizard ships; the on-ramp is real.
- **Import PARTIAL** → prompt-engineering on the `propose_mode_mapping` tool description / the wizard's seed user message before shipping.
- **Import FAIL** → the model can't drive the mapping; fall back to deterministic mapping (model-only-for-descriptions, the "Map-half first" scope option) with the provenance sidecar still shipping (forward-compatible).
- **Update FAIL specifically** → ship import-only this slice, defer update to a follow-on; the provenance sidecar still ships (it's forward-compatible; a later update slice lights it up).

---

## 12. Seams & deferred work

- **`:mode update --latest <name>`** — v1 updates at the **original ref** (recorded in the sidecar) so SHA comparisons are apples-to-apples. "Update to latest" advances the recorded ref: fetch at `HEAD` of the default branch, treat every file as `NewUpstream` or `UpstreamMoved` against the old-ref snapshots, propose a merged mapping, on Approve write the new ref into the sidecar. Seamed this slice (the `--latest` flag parses but surfaces "not yet implemented"); the materializer already writes `upstream_ref` so the data shape is forward-compatible.
- **Non-GitHub sources** — the `GithubApi` trait is the seam. A later `GitlabApi`/`RawHttpApi` impl extends the fetcher; the rest of the wizard is source-agnostic once the `UpstreamScan` is built.
- **Multi-mode-from-one-URL** — one URL → one mode in v1. A later slice could let the model propose multiple `ModeMapping`s from one scan; the `ModeImportWizard` state would hold `Vec<ModeMapping>` and materialize each to its own folder. Not needed for the Superpowers case.
- **Crash-recovery of staging** — the materializer snapshots to `.zoid-staging-<mode>/` before writing and restores on caught errors (§8). Detecting stale staging on next launch (a crash left it behind) and prompting to restore is P1 polish.
- **Honoring a mode's `tools`/`model` fields** — still seamed from Slice-3 §12.1; the wizard materializes them into `mode.md` frontmatter if the model proposes them, but the runtime doesn't apply them yet.

---

## 13. Out of scope (consolidated)

- Non-GitHub sources (§3, §12).
- A structured TUI form editor for the mapping (§1, §7 — chat is the editor).
- Honoring a mode's `tools`/`model` fields (§12 — seamed from Slice 3).
- The overlay picker (deferred from Slice-3 §12.2).
- Git-as-source (clone + point `source_dirs` at it already works via Slice 3).
- Multi-mode-from-one-URL (§12).
- `:mode update --latest` (§12 — seamed, not implemented v1).
- Cross-machine sidecar portability testing (§3 — forward-compatible, not validated).
- Concurrent wizards (§3, §9 — one at a time; a second cancels the first).
- Crash-recovery of staging on next launch (§9, §12 — P1 polish).
- Real GitHub network in CI (§11 — the `GithubApi` trait keeps the fetcher deterministic).

---

## 14. Risks

1. **Mapping quality is the real risk.** A small local model may produce poor mappings (wrong mode/skill split, bad frontmatter, over-skipping). Mitigation: the materializer validates *before* asking and *before* writing, so a bad mapping never lands on disk; the chat-iterate loop lets the user steer; the decision gate (§11) has an explicit fallback to deterministic mapping. The Tier-2 smoke retires this risk before shipping.
2. **GitHub rate limits could make the wizard flaky unauthenticated.** Mitigation: the fetcher surfaces rate-limit state clearly (§9); `$GITHUB_TOKEN` is optional but recommended; the scan is cached in `App.wizard` so chat-iterate never re-fetches.
3. **Provenance sidecar drift** — if a user hand-edits a canonical file *and* the sidecar's `upstream_snapshot` (unlikely but possible), `classify_update` mis-classifies. Mitigation: the sidecar is human-readable and clearly labeled "do not edit"; the update flow is best-effort, not safety-critical; a mis-classification yields a wrong `UpdateClass` but the model still sees the actual local + upstream content in the brief and can recover. Acceptable.
4. **File-set reconciliation deletes user data on update.** If the model proposes dropping a file the user wanted to keep, and the user approves, the file is deleted. Mitigation: the approval summary lists every dropped file with the model's reason; the staging snapshot means a regretted delete can be restored manually from `.zoid-staging-<mode>/` before the next materialize. The staging dir is cleaned on success, so the restore window is "until the next update of that mode" — documented in the runbook.
5. **`apply_mode_mapping` is a tool that writes to disk on approval.** This is the first non-`write_file` tool that persists state from the main turn. Mitigation: it only writes *after* AskUser approval; the materializer is atomic-ish (§8); the runtime degrades gracefully to `Broken` (§9). The risk is low and the seam is the existing `ask_user` approval path, already proven by `ToolGate`/interactive tools.
6. **The wizard is model-agnostic, so a weak model produces a weak proposal.** This is by design (§1, §4 — no routing exception, no surprise bills), but it means the Tier-2 smoke must run against the *default* model the user will actually use, not a capable model that masks mapping-quality issues. The runbook specifies the model.