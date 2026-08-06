# Open-sourcing zoid — design

## Goal

Open-source zoid to grow adoption and community, reversing the current
closed-source/binary-only distribution model described in `AGENTS.md`.

## Locked decisions

| Decision | Choice |
|---|---|
| License | Dual **MIT OR Apache-2.0** (Rust ecosystem convention) |
| Contributor agreement | None — no CLA, fully community-owned, no reserved relicensing path |
| Commercial protection | None needed — pre-revenue, no monetization plan to protect |
| Rollout shape | **Staged**: audit → cleanup → quiet flip → polish → coordinated launch |

## Non-goals

- Deciding a future monetization strategy (hosted service, support
  contracts). Explicitly rejected for now. If this changes later it is a
  separate future decision — and harder post-hoc, since there's no CLA
  reserving relicensing rights. That trade-off is accepted.
- A governance model beyond "single maintainer accepts PRs." No foundation,
  no maintainer team, no RFC process. Can evolve later as a separate effort
  if the project grows.
- Any change to zoid's actual product behavior. This plan touches
  repo/docs/site/process only, not `crates/*` functionality.

## Phases

### Phase 0 — Formalize decisions

- Add root `LICENSE-MIT` and `LICENSE-APACHE`. Set
  `license = "MIT OR Apache-2.0"` on `[workspace.package]` in `Cargo.toml`,
  **and** add `license.workspace = true` to every member crate's own
  `Cargo.toml` (`zoid`, `zoid-core`, `zoid-tui`, `zoid-provider`, etc.).
  Setting the field only at the workspace level does nothing by itself —
  Cargo's workspace inheritance is opt-in per field per crate, and none of
  the member manifests currently declare `license.workspace = true` (they
  do declare `version.workspace = true` / `edition.workspace = true`, which
  confirms the pattern to follow). Verify with `cargo metadata` or
  `cargo-license` that every published crate actually reports the license
  before calling this done.

### Phase 1 — Security/history audit (gate)

Nothing in Phase 2 onward proceeds until this resolves. This phase covers
three distinct leak surfaces — a git-history scan alone is not sufficient,
since flipping repo visibility exposes more than just git history.

- **Git history**: run a secret scanner (`gitleaks` or `trufflehog`, full
  history, not just HEAD) across every commit for API keys, tokens,
  credentials.
- **Historical GitHub Actions run logs**: these become publicly visible the
  moment the repo flips, independent of git history. This repo's
  `release.yml`, `publish-public.yml`, and `cleanup-private-release.yml`
  workflows reference `RELEASES_REPO_TOKEN` and other repo internals — a
  clean git-history scan does not prove a token was never echoed into a run
  log (e.g. via `set -x` or a debug step). Review historical run logs for
  these workflows before flipping; delete/redact any run that leaked
  something sensitive. This is a known, deliberately accepted scope: we are
  not archiving/re-auditing every run ever executed, only reviewing the
  workflows that had access to real secrets.
- **Current workflow hazards for a public repo**: review
  `.github/workflows/*.yml` for "pwn request" risk (workflows triggered by
  fork PRs that still have secret access), confirm default `GITHUB_TOKEN`
  permissions are scoped down (public repos default more permissively than
  private ones expect), and set up branch protection on `main` before
  external PRs can land.
- Triage findings:
  - **Clean** → flip `strvmarv/zoid` to public in place, preserving full
    commit history.
  - **Secrets found (in git history or Actions logs)** → rotate them
    immediately regardless of what follows. Then, **before Phase 2 starts**:
    rename the tainted private repo aside (e.g.
    `strvmarv/zoid-legacy-private`, kept private/archived internally), and
    create a fresh `strvmarv/zoid` seeded with the current source at a
    single squashed initial commit. All of Phase 2's cleanup work then lands
    on this clean base from the start, rather than needing to be redone or
    replayed later. Before Phase 3's flip, manually recreate the existing
    `vX.Y.Z` release tags/GitHub Releases on the new repo (the `zoid update`
    self-updater resolves against a specific repo — it must have continuity
    of releases to update against) and recreate CI secrets and branch
    protection rules on the new repo, since none of that carries over
    automatically from a rename.

### Phase 2 — Code & repo cleanup

- Rewrite `AGENTS.md`: remove the "this is a closed-source product" section
  and the `RELEASES.md`-vs-`docs/CHANGELOG.md` public/private split
  rationale. Merge into a single `CHANGELOG.md` written for one audience —
  there's no longer a disclosure-level distinction to maintain. Leave the
  "Memory" section's total-recall reference in place as a deliberate,
  explicit exception: it documents the maintainer's own workflow tooling for
  AI agents working in the repo, not sensitive or disclosure-inappropriate
  information, so it doesn't need to be scrubbed under the same principle as
  the docs cleanup below.
- Rewrite `docs/RELEASING.md` in this phase (not deferred to Phase 4): it
  currently opens with "source is private" and documents the token-based
  publish-to-`zoid-releases` pipeline whose workflow files this same phase
  deletes. Left untouched, it would ship live and actively misleading during
  Phase 3's quiet-flip window. While in there, fix the pre-existing stale
  claim that `create-release = false` (the actual `dist-workspace.toml` has
  `create-release = true`, with a comment explaining why).
- Update `dist-workspace.toml`: remove the `source-tarball = false` override
  entirely (dist's default already builds a source tarball) and delete
  `publish-jobs = ["./publish-public"]`. Delete
  `.github/workflows/publish-public.yml` and
  `.github/workflows/cleanup-private-release.yml` — there is no longer a
  private/public split to bridge.
- Review **all** of `docs/` for internal-tooling-specific and
  disclosure-inappropriate content, not just the two directories that came
  up during brainstorming. As of this writing that's `docs/bugs/` (4 files),
  `docs/superpowers/specs/` (107 files), `docs/superpowers/plans/` (114
  files — larger than `specs/`, same category, easy to miss if only `specs/`
  is named explicitly), `docs/superpowers/archive/` (8 files),
  `docs/superpowers/runbooks/` (2 files), and `docs/ux/` (5 files). Keep
  only entries with lasting architectural value; drop routine/noisy
  debugging-session artifacts and ones that lean heavily on internal
  AI-tooling specifics (total-recall, superpowers) that would confuse an
  external contributor with no context for them.
- Drop root `spikes/` entirely. It's already excluded from the Cargo
  workspace (`exclude = ["spikes"]` in `Cargo.toml`) and has no bearing on
  building or understanding the shipped product. Note there is a
  **second, separate** `docs/superpowers/spikes/` directory — confusingly
  similarly named but a different location — which falls under the `docs/`
  review above, not this bullet. Treat them as two distinct cleanup items.
- Delete the stray untracked `rust_out` binary from the working tree
  (already gitignored — local hygiene, not a history concern).

### Phase 3 — Minimum viable OSS docs, then the quiet flip

Split into two sub-phases: writing docs is low-risk and reversible; flipping
repo visibility is the single highest-stakes, effectively-irreversible
action in the whole plan. They shouldn't be tracked as one undifferentiated
checklist.

**3a — Baseline docs.** Need to be correct, not polished — deeper polish
happens in Phase 4, after the flip.

- **`README.md`** (currently missing entirely): what zoid is, install
  instructions, quickstart, link to license. Write this directly in
  open-source/community voice from the start — do not port
  `public/index.html`'s current commercial "customers/beta testers" framing
  verbatim and plan to fix it later. Reusing factual product description
  from the marketing copy is fine; the tone needs to be right at first
  publish, since nothing later in the plan is scheduled to revisit
  `README.md`'s framing.
- **`CONTRIBUTING.md`**: how to build (`cargo build --workspace`), how to run
  tests (`cargo nextest run --workspace --features zoid/local-embed`), PR
  expectations, pointer to `AGENTS.md` for deeper repo conventions.
- **`SECURITY.md`**: how to privately report a vulnerability. Matters more
  than average here — zoid executes tools and calls external LLM providers.
- `LICENSE-MIT`, `LICENSE-APACHE` (from Phase 0).

**3b — Flip + hardening.** Once 3a is done and Phase 1 has passed:

- Confirm branch protection on `main` and fork-PR workflow permissions are
  in place (set up in Phase 1's workflow-hazard review — verify here, don't
  assume it silently carried over).
- Flip `strvmarv/zoid` to public with **no announcement**. This surfaces any
  problems (broken CI on a fresh clone, a doc gap someone stumbles on)
  before anyone is watching.
- Retire `strvmarv/zoid-releases` (the binaries-only mirror used while
  private) now that GitHub Releases can be cut directly from the public
  source repo. Archive it with a pointer to the new home. (This can only
  happen now, not earlier — its precondition is that `strvmarv/zoid` is
  already public.)

### Phase 4 — Polish (runs after the quiet flip, in the open)

- Expand docs: an architecture overview (crate responsibilities —
  `zoid-core`, `zoid-tui`, `zoid-provider`, etc.), and a
  `docs/DEVELOPMENT.md` consolidating developer-facing conventions currently
  scattered across `AGENTS.md` and `docs/RELEASING.md` (both already
  rewritten for the public repo in Phase 2 — this is about consolidating
  into one place, not fixing stale framing, which is already done by now).
- Update `public/index.html`: shift from commercial-product framing to
  open-source project framing. Swap "customers/beta testers" language for
  "users/contributors," add a GitHub stars/link CTA, and a "get involved"
  section replacing any pricing/signup framing. (`README.md` does not need
  this pass — it was written in OSS voice from the start in Phase 3a.)
- Add `CODE_OF_CONDUCT.md` (standard Contributor Covenant is sufficient for
  a single-maintainer project).

### Phase 5 — Community infra

- GitHub issue templates (bug report, feature request) and a PR template.
- Enable GitHub Discussions (lighter-weight than Discord/Slack for a project
  with no community yet; real-time chat can be added later if demand shows
  up).
- CI badge in `README.md` once test/release workflows are confirmed working
  against the public repo — public CI runs are a trust signal contributors
  look for.

### Phase 6 — Coordinated public launch

- Timing: after Phases 4 and 5 are both done. Announce only once the repo
  would survive first impressions (working CI badge, real README, real
  docs).
- Channels: Hacker News ("Show HN"), r/rust, r/programming, X/Twitter —
  standard dev-tool launch channels. Exact copy/timing is a separate, later
  decision, not part of this plan.
- Success criteria: track stars, forks, issues/PRs opened, and Discussions
  activity in the following weeks. No revenue metric — the stated goal is
  adoption/community, not monetization.

## Risks / open items carried forward

- Phase 1's audit outcome is unknown until it runs. The plan branches
  (flip-in-place vs. rename-aside-and-recreate) depending on what it finds.
- Phase 1's Actions-log review is scoped to the workflows known to have had
  secret access, not an exhaustive audit of every historical run. This is a
  deliberate, accepted scope limit, not an oversight.
- No governance model beyond "single maintainer merges PRs." Fine at launch;
  revisit if contribution volume grows (explicitly out of scope now).
