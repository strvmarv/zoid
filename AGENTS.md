# AGENTS.md

Quick-reference conventions for coding agents working in this repo. Full
detail (crate architecture, full releasing runbook) lives in
**[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)** and
**[docs/RELEASING.md](docs/RELEASING.md)**.

## Building

```bash
cargo build --workspace --release --features zoid/local-embed
```

## Testing

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
```

Use **nextest**, not `cargo test` — nextest is the release gate.
`cargo test --workspace --features zoid/local-embed --no-fail-fast` works
as a fallback only if nextest isn't installed.

CI also gates on clippy and rustfmt, both with zero tolerance — run these
before committing, not after CI catches them:

```bash
cargo clippy --workspace --all-targets --features zoid/local-embed -- -D warnings
cargo fmt --all -- --check
```

### TUI snapshot tests

The TUI uses insta snapshots. If a UI change modifies rendered output:

```bash
cargo insta test --accept -p zoid-tui
```

Confirm `git diff` is intentional before committing — snapshot changes
should be reviewed, not blindly accepted.

## Terminal minimum size

The TUI enforces a hard minimum of 160×40 (`layout::MIN_WIDTH` /
`MIN_HEIGHT`). Renderers can assume at least 160 columns — no
narrow-terminal fallback or progressive-collapse logic is needed.
