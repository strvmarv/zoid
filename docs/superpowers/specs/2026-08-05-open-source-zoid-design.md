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

- Add root `LICENSE-MIT` and `LICENSE-APACHE`. Set `license = "MIT OR Apache-2.0"`
  on `[workspace.package]` in `Cargo.toml` so it applies to all member crates.
- Retire `strvmarv/zoid-releases` (the current public binaries-only mirror)
  once `strvmarv/zoid` goes public — GitHub Releases can be cut directly from
  the now-public source repo. Archive it with a pointer to the new home.

### Phase 1 — Security/history audit (gate)

Nothing in Phase 3 onward proceeds until this resolves.

- Run a git-history secret scanner (`gitleaks` or `trufflehog`, full history,
  not just HEAD) across every commit for API keys, tokens, credentials.
- Triage findings:
  - **Clean** → flip `strvmarv/zoid` to public in place, preserving full
    commit history.
  - **Secrets found** → rotate them immediately regardless of what follows,
    and fall back to a fresh public repo with squashed/reset history for the
    source tree. The old private repo stays private/archived internally.

### Phase 2 — Code & repo cleanup

- Rewrite `AGENTS.md`: remove the "this is a closed-source product" section
  and the `RELEASES.md`-vs-`docs/CHANGELOG.md` public/private split
  rationale. Merge into a single `CHANGELOG.md` written for one audience —
  there's no longer a disclosure-level distinction to maintain.
- Update `dist-workspace.toml`: remove the `source-tarball = false` override
  entirely (dist's default already builds a source tarball) and delete
  `publish-jobs = ["./publish-public"]`. Delete
  `.github/workflows/publish-public.yml` and
  `.github/workflows/cleanup-private-release.yml` — there is no longer a
  private/public split to bridge.
- Trim `spikes/` and `docs/bugs/`/`docs/superpowers/specs/` rather than
  shipping them as-is:
  - Drop `spikes/` entirely. It's already excluded from the Cargo workspace
    (`exclude = ["spikes"]` in `Cargo.toml`) and has no bearing on building
    or understanding the shipped product.
  - Keep only `docs/bugs/`/`docs/superpowers/specs/` entries with lasting
    architectural value. Drop routine/noisy debugging-session artifacts and
    ones that lean heavily on internal AI-tooling specifics (total-recall,
    superpowers) that would confuse an external contributor with no context
    for them.
- Delete the stray untracked `rust_out` binary from the working tree
  (already gitignored — local hygiene, not a history concern).

### Phase 3 — Minimum viable OSS docs, then the quiet flip

Baseline files any OSS visitor expects. They need to be correct, not
polished — polish happens in Phase 4, after the flip.

- **`README.md`** (currently missing entirely): what zoid is, install
  instructions, quickstart, link to license. Can reuse language/positioning
  from the existing `public/index.html` marketing copy.
- **`CONTRIBUTING.md`**: how to build (`cargo build --workspace`), how to run
  tests (`cargo nextest run --workspace --features zoid/local-embed`), PR
  expectations, pointer to `AGENTS.md` for deeper repo conventions.
- **`SECURITY.md`**: how to privately report a vulnerability. Matters more
  than average here — zoid executes tools and calls external LLM providers.
- `LICENSE-MIT`, `LICENSE-APACHE` (from Phase 0).

Once these exist and Phase 1 has passed: **flip `strvmarv/zoid` to public
with no announcement.** This surfaces any problems (broken CI on a fresh
clone, a doc gap someone stumbles on) before anyone is watching.

### Phase 4 — Polish (runs after the quiet flip, in the open)

- Expand docs: an architecture overview (crate responsibilities —
  `zoid-core`, `zoid-tui`, `zoid-provider`, etc.), and a
  `docs/DEVELOPMENT.md` consolidating what's currently scattered across
  `AGENTS.md`/`docs/RELEASING.md`.
- Update `public/index.html`: shift from commercial-product framing to
  open-source project framing. Swap "customers/beta testers" language for
  "users/contributors," add a GitHub stars/link CTA, and a "get involved"
  section replacing any pricing/signup framing.
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
  (flip-in-place vs. fresh-history) depending on what it finds.
- No governance model beyond "single maintainer merges PRs." Fine at launch;
  revisit if contribution volume grows (explicitly out of scope now).
