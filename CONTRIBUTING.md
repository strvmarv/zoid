# Contributing to zoid

Thanks for your interest in contributing.

## Building

```bash
cargo build --workspace
```

## Running tests

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

(`cargo test --workspace --no-fail-fast` also works if you don't have
`cargo-nextest` installed.)

## Before opening a pull request

- Run `cargo fmt --all` and `cargo clippy --workspace --all-targets` before
  opening a PR.
- If your change touches the TUI's rendering, you may need to regenerate
  snapshot tests: `cargo insta test --accept -p zoid-tui`. Review the diff
  (`git diff`) before committing regenerated snapshots — an unexpected visual
  change usually means a real bug, not a snapshot to blindly accept.
- Keep pull requests focused on one change. Larger changes are easier to
  review (and merge) as a sequence of smaller ones.

## Repository conventions

`AGENTS.md` documents repository-specific conventions for anyone (human or
AI agent) working in this codebase — release process, the TUI's minimum
terminal size, and similar cross-cutting rules. Read it before touching
anything related to releases or changelogs.

## Reporting bugs / requesting features

Open a GitHub issue. For security vulnerabilities, see
[SECURITY.md](SECURITY.md) instead of opening a public issue.
