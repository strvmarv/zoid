# Open-sourcing zoid — Phase 3b: Flip + hardening design

**Date:** 2026-08-15
**Approach:** A — full migration in one shot, then gilfoyle reviews plan + final diff before merge.

## Problem

`strvmarv/zoid` is private. All live infrastructure — releases, self-updater
target, plugin catalog, feedback issue filing — points at `strvmarv/zoid-releases`,
a separate public repo. Phase 0/2/3a cleaned up the repo for open source; the
audit (Phase 1) came back clean. Now the repo must flip to public, all
infrastructure must migrate to `strvmarv/zoid`, and `zoid-releases` must be
retired — as a single atomic transition with no broken-window state.

## Solution

Flip `strvmarv/zoid` to public, migrate the plugins catalog into this repo,
update all `crates/*/src/` references from `strvmarv/zoid-releases` to
`strvmarv/zoid`, cut the v1.0.0 release, harden the repo (branch protection,
scoped token permissions), and archive `strvmarv/zoid-releases`.

## Scope

**In scope:**
- Migrate `plugins/` (4 files), `scripts/gen_index.py`, and
  `.github/workflows/catalog-index.yml` from `zoid-releases` into this repo.
- Update 5 `crates/*/src/` files: replace all `strvmarv/zoid-releases` string
  references with `strvmarv/zoid` (self-updater, catalog, feedback, skill body,
  feedback tool doc comment).
- Update `docs/RELEASING.md` — remove the "Do not cut a release yet" warning.
- Update `public/index.html` — fix 4 `zoid-releases` install URLs to
  `strvmarv/zoid`; update beta-note from commercial to OSS voice (minimal —
  full OSS rewrite is Phase 4).
- Cut v1.0.0: bump version, add CHANGELOG entry, regenerate TUI snapshots,
  verify release gate, tag and push.
- Flip repo to public via `gh repo edit`.
- Set up branch protection on `main` (require PR review, no force-push,
  status checks).
- Set default `GITHUB_TOKEN` permissions to `read` (defense-in-depth;
  `release.yml` already scopes itself to `contents: write` per-workflow).
- Archive `strvmarv/zoid-releases` with a README pointer to the new home.

**Out of scope:**
- `CODE_OF_CONDUCT.md`, `docs/DEVELOPMENT.md` — Phase 4.
- Full `public/index.html` OSS rewrite (CTA, "get involved" section, GitHub
  stars) — Phase 4. This phase only fixes the install URLs and beta-note.
- GitHub Discussions, issue templates, PR template — Phase 5.
- Any product behavior change — only repo, docs, and string constants.

## Ordering

Code changes land on `main` while still private — no user sees them until the
flip. Then cut v1.0.0, flip, harden, verify, archive.

```
1. Migrate plugins catalog (files + workflow + script)
2. Update crates/*/src/ references (zoid-releases → zoid)
3. Update docs (RELEASING.md, public/index.html)
4. Cut v1.0.0 (version bump, CHANGELOG, snapshots, tag)
5. Flip repo to public
6. Set up branch protection on main
7. Set default GITHUB_TOKEN permissions to read
8. Verify: installer downloads, self-updater resolves, catalog fetches
9. Archive zoid-releases with README pointer
```

Steps 1-3 are a PR (reviewed by gilfoyle before merge). Step 4 is a tag push.
Steps 5-7 are GitHub settings. Step 8 is manual smoke. Step 9 is a repo action.

## 1. Migrate the plugins catalog

Move from `strvmarv/zoid-releases` into `strvmarv/zoid`:

| Source (zoid-releases) | Destination (zoid) |
|---|---|
| `plugins/index.json` | `plugins/index.json` |
| `plugins/github.toml` | `plugins/github.toml` |
| `plugins/superpowers.toml` | `plugins/superpowers.toml` |
| `plugins/README.md` | `plugins/README.md` |
| `scripts/gen_index.py` | `scripts/gen_index.py` |
| `.github/workflows/catalog-index.yml` | `.github/workflows/catalog-index.yml` |

The `catalog-index.yml` workflow regenerates `plugins/index.json` when any
`plugins/*.toml` changes on `main`. Same script, same trigger, new repo. No
changes to the workflow or script content are needed — they use relative paths
(`plugins/`, `scripts/gen_index.py`).

The `catalog.rs` constant `CATALOG_BASE` is updated in step 2 to point at
`strvmarv/zoid/main/plugins` instead of `strvmarv/zoid-releases/main/plugins`.

## 2. Update crates/*/src/ references

Mechanical replacement: `strvmarv/zoid-releases` → `strvmarv/zoid` in these
files:

| File | References | What it affects |
|---|---|---|
| `crates/zoid/src/update.rs:10` | `RELEASES_REPO` const | Self-updater checks `releases/latest` API |
| `crates/zoid/src/catalog.rs:10` | `CATALOG_BASE` const | Plugin catalog fetch URL |
| `crates/zoid-core/src/feedback.rs:89` | `REPO` const + doc comments + test URL assertions | Feedback/bug-report issue filing target |
| `crates/zoid-core/src/skill.rs` | `FEEDBACK_SKILL_BODY` + `REFRESHING_PROVIDER_MODELS_BODY` consts | Built-in skill text mentions `zoid-releases` |
| `crates/zoid-tools/src/feedback.rs:21` | Doc comment | Feedback tool description |

The `feedback.rs` test assertions (lines ~398, 450, 458, 473) reference
`zoid-releases` in expected URL strings — these must be updated to `zoid`
too, or the tests fail.

The `skill.rs` test `builtin_includes_feedback_skill` (line ~384) asserts
`fb.body.contains("strvmarv/zoid-releases")` — must be updated to
`strvmarv/zoid`.

After replacement, the self-updater resolves against
`https://api.github.com/repos/strvmarv/zoid/releases/latest`, the catalog
fetches from
`https://raw.githubusercontent.com/strvmarv/zoid/main/plugins/`, and feedback
files issues on `strvmarv/zoid`.

## 3. Update docs

### docs/RELEASING.md

Remove the "Do not cut a release yet" blockquote (lines 6-11). The repo is now
canonical — the warning is stale.

### public/index.html

Fix 4 URLs from `strvmarv/zoid-releases` to `strvmarv/zoid`:
- Line 313: install button `href`
- Line 314: curl one-liner
- Line 315: beta-note (update from "from the public releases repo" to "from
  GitHub Releases")
- Line 854: install button `href`

The beta-note's "Evaluation builds expire 30 days after release" framing is
commercial-era copy — update to a neutral OSS note (e.g. "Download the latest
release or build from source."). Full OSS rewrite of the marketing copy is
Phase 4.

## 4. Cut v1.0.0

1. Bump `[workspace.package].version` in root `Cargo.toml` from `0.9.1` to
   `1.0.0`.
2. Add `## 1.0.0` section to the top of `CHANGELOG.md`.
3. Regenerate TUI snapshots: `cargo insta test --accept -p zoid-tui`, confirm
   `git diff` is version-token-only.
4. Verify the release gate:
   `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast`.
5. Commit, then `git tag v1.0.0 && git push origin main --tags`. The tag push
   fires `release.yml`, which creates the GitHub Release with installer
   scripts and build artifacts on `strvmarv/zoid`.

### CHANGELOG entry

```markdown
## 1.0.0

zoid is now open source under MIT OR Apache-2.0. The source repo
(strvmarv/zoid) is the canonical home for releases, the self-updater, and the
plugin catalog; the former strvmarv/zoid-releases distribution repo is
archived.

### Open source

- **Dual-licensed MIT OR Apache-2.0** — see `LICENSE-MIT` and `LICENSE-APACHE`.
- **Single repo** — source, releases, installer scripts, and plugin catalog
  all live on strvmarv/zoid.
- **Contributing** — see `CONTRIBUTING.md` and `SECURITY.md`.

### Fixes

- **Provider model registry skill** — a new built-in skill
  (`refreshing-provider-models`) guides refreshing the static provider/model
  catalog against live endpoints.
```

## 5. Flip repo to public

```bash
gh repo edit strvmarv/zoid --visibility public
```

This is the irreversible step. Everything before it (steps 1-4) is staged on
the still-private repo. After the flip, the repo is visible, the v1.0.0
release is public, and install URLs resolve.

## 6. Branch protection on main

After the flip (private repos on free GitHub can't set branch protection):

```bash
gh api repos/strvmarv/zoid/branches/main/protection -X PUT \
  -f required_pull_request_reviews.required_approving_review_count=0 \
  -f required_status_checks.strict=true \
  -f required_status_checks.contexts='[]' \
  -f enforce_admins=true \
  -f restrictions='{}' \
  -f required_linear_history=true
```

- `required_approving_review_count=0` — single maintainer; PRs are required
  but self-merge is allowed (no other reviewers exist yet).
- `required_linear_history=true` — no merge commits.
- `enforce_admins=true` — the rules apply to everyone.

## 7. Default GITHUB_TOKEN permissions

```bash
gh api repos/strvmarv/zoid/actions/permissions -X PUT \
  -f default_workflow_permissions=read \
  -f can_approve_pull_request_reviews=true
```

The `release.yml` workflow already declares `permissions: contents: write`
per-workflow, so it still works. New workflows default to read-only, which is
the safe pattern for a public repo with fork PRs.

## 8. Verify

After the flip and v1.0.0 release:

1. **Installer download:** `curl -sL
   https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.sh
   | head -5` — should return the installer script, not a 404.
2. **Self-updater API:** `curl -s
   https://api.github.com/repos/strvmarv/zoid/releases/latest | jq
   .tag_name` — should return `"v1.0.0"`.
3. **Plugin catalog:** `curl -s
   https://raw.githubusercontent.com/strvmarv/zoid/main/plugins/index.json |
   jq .schema` — should return `1`.
4. **Feedback repo:** confirm `https://github.com/strvmarv/zoid/issues` is
   accessible (public repo, issues enabled).

## 9. Archive zoid-releases

After verification:

1. Update `zoid-releases` README to point at `strvmarv/zoid` as the new home.
2. Archive the repo:

```bash
gh api repos/strvmarv/zoid-releases -X PATCH -f archived=true
```

The existing 14 releases (v0.2.0–v0.9.1) remain accessible at their archived
URLs for historical continuity — existing installs that haven't updated yet
can still download their version. New installs and updates resolve against
`strvmarv/zoid`.

## Testing

- `cargo test --workspace --features zoid/local-embed --no-fail-fast` — the
  code changes touch `feedback.rs` and `skill.rs` test assertions.
- `cargo check --workspace` — confirm all references compile.
- Manual smoke (step 8): installer, self-updater API, catalog fetch, issues
  page.

## Risks

- **The flip is irreversible.** If something is wrong (broken install,
  missing release), it's visible to anyone who finds the repo. Mitigation:
  everything is staged and verified before the flip; the v1.0.0 tag is pushed
  before the flip so the release exists the moment the repo goes public.
- **Existing installs on v0.9.1** will check `strvmarv/zoid-releases` for
  updates (their self-updater still has the old const). They'll see no new
  release there (v0.9.1 is the last one). They must manually update to v1.0.0
  (which points at `strvmarv/zoid`). This is a one-time migration step for
  existing users — documented in the v1.0.0 CHANGELOG entry.
- **The catalog-index workflow** already declares `permissions: contents: write`
  in its own workflow file, so it works regardless of the repo default being
  `read`. No fix needed — this was verified against the source workflow.