# Phase 3b — Flip + Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate all infrastructure from `strvmarv/zoid-releases` into `strvmarv/zoid`, cut v1.0.0, flip the repo to public, harden it, and archive `zoid-releases`.

**Architecture:** Code changes (plugins migration, string references, docs) land as a PR while the repo is still private. Then v1.0.0 is tagged and pushed (CI creates the release). Then the repo is flipped to public, branch protection and token permissions are set, the release is verified, Pages is enabled, and `zoid-releases` is archived.

**Tech Stack:** Rust workspace (14 crates), cargo-dist 0.32.0, GitHub Actions, `gh` CLI.

## Global Constraints

- The spec is at `docs/superpowers/specs/2026-08-15-open-source-phase-3b-design.md`. All technical content must match the spec (which was reviewed by gilfoyle against source code).
- Replace **every** `zoid-releases` token — both qualified (`strvmarv/zoid-releases`) and bare (`zoid-releases` in doc comments) — with `strvmarv/zoid` / `zoid` respectively.
- Do not flip the repo to public until after v1.0.0 is tagged and pushed (the release must exist before the flip).
- Do not archive `zoid-releases` until Pages is enabled on `strvmarv/zoid` (the marketing site must not go dark).
- Branch protection uses Option A (no `required_pull_request_reviews`) so the catalog bot and release workflow can push directly.
- This plan does **not** include Phase 4 (CODE_OF_CONDUCT, DEVELOPMENT.md, full public/index.html OSS rewrite) or Phase 5 (issue templates, Discussions).

---

### Task 1: Migrate the plugins catalog from zoid-releases

**Files:**
- Create: `plugins/index.json`
- Create: `plugins/github.toml`
- Create: `plugins/superpowers.toml`
- Create: `plugins/README.md`
- Create: `scripts/gen_index.py`
- Create: `.github/workflows/catalog-index.yml`

**Interfaces:**
- Produces: `plugins/` directory with the catalog files; `catalog-index.yml` workflow that regenerates `index.json` on `plugins/*.toml` pushes. Task 2 updates `catalog.rs` to point at `strvmarv/zoid/main/plugins`.

- [ ] **Step 1: Create the `plugins/` directory with the manifest files**

Create `plugins/github.toml`:

```toml
[plugin]
id = "github"
schema = 1
kind = ["mcp"]
name = "GitHub"
description = "GitHub repositories, issues, and pull requests over MCP (stdio)."

[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "${GITHUB_PERSONAL_ACCESS_TOKEN}" }
```

Create `plugins/superpowers.toml`:

```toml
[plugin]
id = "superpowers"
schema = 1
kind = ["mode"]
name = "Superpowers"
description = "A curated skill set for structured software engineering workflows."

[source]
repo = "obra/superpowers"
ref = "d884ae04edebef577e82ff7c4e143debd0bbec99"
subtree = "skills"

[mode]
loader = "using-superpowers/SKILL.md"
strip_prefix = "skills/"
body = "from-skill-frontmatter"
description = "Superpowers — a curated skill set for structured software engineering workflows (TDD, debugging, code review, planning, parallel agents, git worktrees, verification), imported from obra/superpowers."
body_intro = """
You are operating in "Superpowers" mode, imported from obra/superpowers.

Before any task, check if an available skill applies and invoke it with invoke_skill. The skills are:
"""
body_outro = """

Always check for an applicable skill before starting work. If multiple skills apply, invoke the most specific one first. After completing work, invoke verification-before-completion before claiming success.

Skill work produces specs, plans, and debugging notes. Keep the running narration terse, and when the work is done do NOT reframe the whole effort in long paragraphs: close with a short recap of what changed and any next step.
"""

[[install]]
effect = "activate"

[[install]]
effect = "onboarding_hint"
text = "Superpowers mode installed and active."
```

Create `plugins/index.json`:

```json
{
  "schema": 1,
  "plugins": [
    {
      "id": "github",
      "name": "GitHub",
      "kind": [
        "mcp"
      ],
      "description": "GitHub repositories, issues, and pull requests over MCP (stdio)."
    },
    {
      "id": "superpowers",
      "name": "Superpowers",
      "kind": [
        "mode"
      ],
      "description": "A curated skill set for structured software engineering workflows.",
      "source": {
        "repo": "obra/superpowers",
        "ref": "d884ae04edebef577e82ff7c4e143debd0bbec99"
      }
    }
  ]
}
```

- [ ] **Step 2: Create `plugins/README.md`**

Copy the full `plugins/README.md` from `zoid-releases` verbatim (92 lines — the contributing guide for plugin manifests). The content is repo-agnostic (uses relative paths like `plugins/<id>.toml`).

- [ ] **Step 3: Create `scripts/gen_index.py`**

```python
#!/usr/bin/env python3
"""Regenerate plugins/index.json from plugins/*.toml. Stdlib only (tomllib)."""
import json, sys, tomllib
from pathlib import Path

def build_index(plugins_dir: Path) -> dict:
    entries = []
    for toml_path in sorted(plugins_dir.glob("*.toml")):
        with toml_path.open("rb") as fh:
            data = tomllib.load(fh)
        p = data["plugin"]
        entry = {
            "id": p["id"], "name": p.get("name", p["id"]),
            "kind": p["kind"], "description": p.get("description", ""),
        }
        # `mcp` manifests declare no [source]; emit source only when present.
        if "source" in data:
            s = data["source"]
            entry["source"] = {"repo": s["repo"], "ref": s["ref"]}
        if "license" in p:
            entry["license"] = p["license"]
        entries.append(entry)
    entries.sort(key=lambda e: e["id"])
    return {"schema": 1, "plugins": entries}

def main(argv):
    plugins_dir = Path(argv[1]) if len(argv) > 1 else Path(__file__).resolve().parent.parent / "plugins"
    index = build_index(plugins_dir)
    out = json.dumps(index, indent=2, ensure_ascii=False) + "\n"
    (plugins_dir / "index.json").write_text(out, encoding="utf-8")
    print(f"wrote {plugins_dir/'index.json'} ({len(index['plugins'])} plugins)")

if __name__ == "__main__":
    main(sys.argv)
```

- [ ] **Step 4: Create `.github/workflows/catalog-index.yml`**

```yaml
name: catalog-index
on:
  push:
    paths: [ "plugins/*.toml" ]
permissions:
  contents: write
jobs:
  regen:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with: { python-version: "3.11" }
      - run: python scripts/gen_index.py plugins
      - name: Commit index if changed
        run: |
          if ! git diff --quiet plugins/index.json; then
            git config user.name "catalog-bot"
            git config user.email "catalog-bot@users.noreply.github.com"
            git add plugins/index.json
            git commit -m "chore(catalog): regenerate index.json"
            git pull --rebase origin main
            git push
          fi
```

- [ ] **Step 5: Verify the catalog script regenerates index.json correctly**

Run:

```bash
python3 scripts/gen_index.py plugins && diff <(git show HEAD:plugins/index.json 2>/dev/null || echo "new") plugins/index.json
```

Expected: no diff (the index.json was already generated from the same toml files). If python3 is unavailable, skip — the workflow handles regeneration on push.

- [ ] **Step 6: Commit**

```bash
git add plugins/ scripts/gen_index.py .github/workflows/catalog-index.yml
git commit -m "feat(catalog): migrate plugins catalog from zoid-releases into this repo"
```

---

### Task 2: Update crates/*/src/ references from zoid-releases to zoid

**Files:**
- Modify: `crates/zoid/src/update.rs:10`
- Modify: `crates/zoid/src/catalog.rs:1,10,85,209,211`
- Modify: `crates/zoid-core/src/feedback.rs:1,9,89,165,398,450,458,473`
- Modify: `crates/zoid-core/src/skill.rs:90,95,277,384`
- Modify: `crates/zoid-tools/src/feedback.rs:21`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: all `crates/*/src/` references point at `strvmarv/zoid` instead of `strvmarv/zoid-releases`. The self-updater, catalog fetch, feedback issue filing, and built-in skill text all resolve against `strvmarv/zoid`.

- [ ] **Step 1: Replace all references in `crates/zoid/src/update.rs`**

Line 10:

```rust
const RELEASES_REPO: &str = "strvmarv/zoid";
```

- [ ] **Step 2: Replace all references in `crates/zoid/src/catalog.rs`**

Line 1:

```rust
//! Fetch + cache + parse the public zoid plugin catalog.
```

Line 10:

```rust
    "https://raw.githubusercontent.com/strvmarv/zoid/main/plugins";
```

Line 85:

```rust
/// One-shot async raw GET of a public zoid text file (unauthenticated).
```

Line 209:

```rust
            "https://raw.githubusercontent.com/strvmarv/zoid/main/plugins/index.json");
```

Line 211:

```rust
            "https://raw.githubusercontent.com/strvmarv/zoid/main/plugins/ok-skills.toml");
```

- [ ] **Step 3: Replace all references in `crates/zoid-core/src/feedback.rs`**

Line 1:

```rust
//! User feedback & bug-report submission to GitHub issues on strvmarv/zoid.
```

Line 9:

```rust
/// What kind of feedback this is. Maps to a GitHub label on zoid.
```

Line 89:

```rust
pub const REPO: &str = "strvmarv/zoid";
```

Line 165:

```rust
    /// Create an issue on `repo` (e.g. "strvmarv/zoid"). Returns
```

Line 398:

```rust
            "https://github.com/strvmarv/zoid/issues/new?title="
```

Line 450:

```rust
            "https://github.com/strvmarv/zoid/issues/7",
```

Line 458:

```rust
                    "https://github.com/strvmarv/zoid/issues/7"
```

Line 473:

```rust
                    "https://github.com/strvmarv/zoid/issues/new?"
```

- [ ] **Step 4: Replace all references in `crates/zoid-core/src/skill.rs`**

Line 90:

```rust
/// tool and the `strvmarv/zoid` repo.
```

Line 95 (inside `FEEDBACK_SKILL_BODY`):

```rust
`strvmarv/zoid`. The `submit_feedback` tool proposes a report; the
```

Line 277 (inside the feedback `Skill`'s `description` literal):

```rust
                    strvmarv/zoid, with the user confirming before anything \
```

Line 384 (test assertion):

```rust
        assert!(fb.body.contains("strvmarv/zoid"));
```

- [ ] **Step 5: Replace the reference in `crates/zoid-tools/src/feedback.rs`**

Line 21:

```rust
                maintainers (GitHub issues on strvmarv/zoid). The user MUST \
```

- [ ] **Step 6: Verify no zoid-releases references remain in crates/*/src/**

Run:

```bash
grep -rn 'zoid-releases' crates/ --include='*.rs'
```

Expected: no output (every reference has been replaced).

- [ ] **Step 7: Run the affected tests**

Run:

```bash
cargo test -p zoid-core skill::tests
cargo test -p zoid-core -- feedback
cargo test -p zoid -- catalog
```

Expected: all passing. The `urls_are_raw_unauthenticated` test, the feedback URL test assertions, and the `builtin_includes_feedback_skill` test assertion all now expect `strvmarv/zoid`.

- [ ] **Step 8: Run the full workspace test suite**

Run:

```bash
cargo test --workspace --features zoid/local-embed --no-fail-fast 2>&1 | tail -20
```

Expected: no new failures (pre-existing TUI snapshot drift from version bump is expected and handled in Task 5).

- [ ] **Step 9: Commit**

```bash
git add crates/zoid/src/update.rs crates/zoid/src/catalog.rs crates/zoid-core/src/feedback.rs crates/zoid-core/src/skill.rs crates/zoid-tools/src/feedback.rs
git commit -m "refactor: point all zoid-releases references to strvmarv/zoid

Self-updater, plugin catalog, feedback issue filing, and built-in skill
text now resolve against strvmarv/zoid instead of strvmarv/zoid-releases."
```

---

### Task 3: Update docs (RELEASING.md, public/index.html)

**Files:**
- Modify: `docs/RELEASING.md` — remove the "Do not cut a release yet" warning block (lines 6-11)
- Modify: `public/index.html` — fix 3 URLs + 2 betanotes

**Interfaces:**
- Consumes: nothing from Tasks 1-2.
- Produces: docs consistent with the repo being canonical.

- [ ] **Step 1: Remove the "Do not cut a release yet" block from `docs/RELEASING.md`**

Delete lines 6–12 (the blockquote starting with `> **Do not cut a release yet.**` through the line ending `steps below.`) and the blank line at line 13. The file should start:

```markdown
# Releasing zoid

Users install prebuilt binaries via the installer script or `zoid update`
(anonymous, checksum-verified). No tokens live on user machines.

## How a release is wired
```

- [ ] **Step 2: Fix the 3 `zoid-releases` URLs in `public/index.html`**

Line 313:

```html
    <a class="btn" href="https://github.com/strvmarv/zoid/releases/latest">Install zoid</a>
```

Line 314:

```html
    <code class="oneliner">curl --proto '=https' --tlsv1.2 -LsSf https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.sh | sh</code>
```

Line 854:

```html
  <p style="margin:16px 0 0;"><a class="btn" href="https://github.com/strvmarv/zoid/releases/latest">Install zoid</a></p>
```

- [ ] **Step 3: Update the 2 betanotes in `public/index.html`**

Line 315 — change "from the public releases repo" to "from GitHub Releases":

```html
    <p class="betanote">Evaluation builds expire 30 days after release — run <code>zoid update</code> to stay current (anonymous, checksum-verified, from GitHub Releases). PowerShell installer &amp; per-platform archives on the releases page.</p>
```

Line 855 — change the beta/expiry copy to neutral OSS framing:

```html
  <p style="margin:12px 0 0;font-size:12px;color:var(--dim);">Download the latest release or build from source.</p>
```

- [ ] **Step 4: Verify no zoid-releases references remain in docs and config**

Run:

```bash
grep -rn 'zoid-releases' docs/RELEASING.md public/index.html README.md AGENTS.md CONTRIBUTING.md SECURITY.md dist-workspace.toml .github/workflows/release.yml .github/workflows/catalog-index.yml
```

Expected: no output. The `dist-workspace.toml` and `release.yml` are verified clean (no `zoid-releases` references exist in them today), but including them in this grep confirms nothing was missed.

- [ ] **Step 5: Commit**

```bash
git add docs/RELEASING.md public/index.html
git commit -m "docs: remove release warning, fix install URLs to strvmarv/zoid

Remove the 'Do not cut a release yet' block from RELEASING.md (the repo is
now canonical). Fix 3 install URLs and 2 betanotes in public/index.html
from zoid-releases to zoid."
```

---

### Task 4: Cut v1.0.0

**Files:**
- Modify: `Cargo.toml` — bump `[workspace.package].version` from `0.9.1` to `1.0.0`
- Modify: `CHANGELOG.md` — add `## 1.0.0` section at the top
- Modify: TUI snapshot tests (regenerated via `cargo insta test --accept`)

**Interfaces:**
- Consumes: Tasks 1-3 (all code changes must be committed before the version bump).
- Produces: `v1.0.0` tag on `main` that fires `release.yml` to create the GitHub Release.

- [ ] **Step 1: Bump the workspace version**

In `Cargo.toml`, change:

```toml
version = "0.9.1"
```

to:

```toml
version = "1.0.0"
```

- [ ] **Step 2: Add the CHANGELOG entry**

Add to the top of `CHANGELOG.md` (before the existing `## 0.9.1` section):

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

### Updating from v0.9.x

If you're running v0.9.x, your self-updater still checks
`strvmarv/zoid-releases` (the archived repo). It will report "already up to
date" because v0.9.1 is the last release there. To migrate to v1.0.0,
reinstall with the new installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.sh | sh
```

After that, `zoid update` checks `strvmarv/zoid` going forward.

### New

- **Provider model registry skill** — a new built-in skill
  (`refreshing-provider-models`) guides refreshing the static provider/model
  catalog against live endpoints.
```

- [ ] **Step 3: Regenerate TUI snapshots**

Run:

```bash
cargo insta test --accept -p zoid-tui
```

Then verify the diff is version-token-only:

```bash
git diff -p zoid-tui/ | grep '^[+-]' | grep -v '0.9.1\|1.0.0\|zoid-releases\|^[+-][+-][+-]' | head -10
```

Expected: no output (all changes are `0.9.1` → `1.0.0` token replacements, plus `zoid-releases` → `zoid` URL changes from Task 2 that appear in any snapshot capturing feedback/upgrade prompts). If non-version, non-URL changes appear, investigate before continuing.

- [ ] **Step 4: Verify the release gate**

Run:

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast 2>&1 | tail -10
```

Expected: all passing (0 failures). If `cargo nextest` is unavailable, use `cargo test --workspace --features zoid/local-embed --no-fail-fast`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml CHANGELOG.md zoid-tui/
git commit -m "release: v1.0.0 — open source, single-repo distribution"
```

- [ ] **Step 6: Tag and push**

```bash
git tag v1.0.0
git push origin main --tags
```

The tag push fires `release.yml`, which builds the three targets and creates
the GitHub Release with installer scripts and build artifacts on
`strvmarv/zoid`. Wait for CI to complete before proceeding to the flip.

- [ ] **Step 7: Verify the release was created**

```bash
gh api repos/strvmarv/zoid/releases/latest --jq '.tag_name'
```

Expected: `"v1.0.0"`. If the release is missing, wait for CI or check the
Actions tab.

---

### Task 5: Flip repo to public + harden

**Files:**
- No file changes. All steps are GitHub settings via `gh` CLI.

**Interfaces:**
- Consumes: Task 4 (v1.0.0 release must exist on `strvmarv/zoid`).
- Produces: a public repo with branch protection and scoped token permissions.

- [ ] **Step 1: Flip the repo to public**

```bash
gh repo edit strvmarv/zoid --visibility public --accept-visibility-change-consequences
```

This is the irreversible step. The v1.0.0 release is already public, so install
URLs resolve immediately.

- [ ] **Step 2: Set up branch protection on main (Option A — no PR requirement)**

```bash
gh api repos/strvmarv/zoid/branches/main/protection -X PUT \
  -f required_status_checks.strict=true \
  -f required_status_checks.contexts='[]' \
  -f enforce_admins=true \
  -f restrictions='{}' \
  -f required_linear_history=true
```

This prevents force-push, requires linear history, and enforces rules for
admins — but allows direct pushes (including the catalog bot's automated
commits and the maintainer's release commits). PRs are encouraged by
convention but not enforced. Migrate to PR requirement in Phase 5 when CI
workflows exist to gate PRs on.

- [ ] **Step 3: Set default GITHUB_TOKEN permissions to read**

```bash
gh api repos/strvmarv/zoid/actions/permissions -X PUT \
  -f default_workflow_permissions=read \
  -f can_approve_pull_request_reviews=true
```

The `release.yml` and `catalog-index.yml` workflows both declare their own
`permissions: contents: write`, so they still work. New workflows default to
read-only.

- [ ] **Step 4: Verify the public repo**

```bash
# Installer downloads
curl -sL https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.sh | head -5

# Self-updater API
curl -s https://api.github.com/repos/strvmarv/zoid/releases/latest | jq -r '.tag_name'

# Plugin catalog
curl -s https://raw.githubusercontent.com/strvmarv/zoid/main/plugins/index.json | jq -r '.schema'

# Issues page accessible
gh api repos/strvmarv/zoid --jq '.has_issues'
```

Expected:
- Installer script content (not 404)
- `"v1.0.0"`
- `1`
- `true`

---

### Task 6: Enable Pages + archive zoid-releases

**Files:**
- No file changes in this repo. All steps are GitHub settings on both repos.

**Interfaces:**
- Consumes: Task 5 (repo must be public for Pages to work).
- Produces: marketing site served from `strvmarv/zoid`, `zoid-releases` archived.

- [ ] **Step 1: Enable GitHub Pages on strvmarv/zoid**

```bash
gh api repos/strvmarv/zoid/pages -X POST \
  -f "source[branch]=main" \
  -f "source[path]=/public" 2>&1
```

If the API returns an error, this step requires manual intervention via the
GitHub web UI (Settings → Pages → Source: `main` / `/public`). Note: some
GitHub configurations expect `public` without the leading `/`; try both.
This step is semi-manual — stop and hand off to a human if the API call fails.

- [ ] **Step 2: Verify the marketing site renders**

Wait 1-2 minutes for Pages to build, then:

```bash
curl -sL https://strvmarv.github.io/zoid/ | head -5
```

Expected: HTML content (the marketing site). If it returns a build-in-progress
page, wait and retry.

- [ ] **Step 3: Update zoid-releases README to point at the new home**

```bash
gh api repos/strvmarv/zoid-releases/contents/README.md --jq '.sha' > /tmp/zr-sha.txt
README_SHA=$(cat /tmp/zr-sha.txt)

# Create the new README content
cat > /tmp/zr-readme.md << 'EOF'
# zoid-releases (archived)

This repo was the binary distribution point for zoid while the source was
private. zoid is now open source at **[strvmarv/zoid](https://github.com/strvmarv/zoid)**.

All releases, installer scripts, the plugin catalog, and the marketing site
now live there. Existing releases (v0.2.0–v0.9.1) remain downloadable here for
historical continuity.
EOF

gh api repos/strvmarv/zoid-releases/contents/README.md \
  -X PUT \
  -f message="docs: point to strvmarv/zoid as the new home" \
  -f content=$(base64 < /tmp/zr-readme.md | tr -d '\n') \
  -f sha="$README_SHA" 2>&1
```

- [ ] **Step 4: Archive zoid-releases**

```bash
gh api repos/strvmarv/zoid-releases -X PATCH -f archived=true
```

The existing 14 releases (v0.2.0–v0.9.1) remain accessible at their archived
URLs. New installs and updates resolve against `strvmarv/zoid`.

- [ ] **Step 5: Verify zoid-releases is archived**

```bash
gh api repos/strvmarv/zoid-releases --jq '.archived'
```

Expected: `true`.