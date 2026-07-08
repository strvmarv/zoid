# Status Bar Binary Version — Design

**Date:** 2026-07-07
**Status:** Approved, ready for implementation plan

## Goal

Display the running binary's semantic version in the top status bar so users can
tell at a glance which build they are running. The version fills the currently
empty left zone of the bar, giving it a symmetric three-zone read:

```
v0.1.2              zoid           Esc · : command · ^P palette
└ left            center                  right ┘
```

"zoid" stays exactly where it renders today; the palette hint stays flush-right.
The version is purely additive.

## Scope decisions (settled during brainstorming)

- **Placement:** far-left edge, as a distinct zone — NOT appended to the wordmark.
  Keeps the wordmark's centering math untouched.
- **Content:** bare crate semver only, e.g. `v0.1.2`. No git SHA, no build date,
  no dirty flag. This needs zero new build machinery.
- **Non-goals:** no `build.rs` for `zoid-tui`, no git-describe, no embedded commit
  metadata. If richer build identity is ever wanted, it is a separate effort
  (add a `build.rs` to `crates/zoid-tui` embedding a SHA via `option_env!`).

## The change

Single function: `render_title` in `crates/zoid-tui/src/render.rs` (currently
lines 220-241). No new files, no new dependencies, no layout refactor.

### Version source

A compile-time `&'static str` — zero allocation, no plumbing:

```rust
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION")); // "v0.1.2"
```

`zoid-tui` inherits `version.workspace = true` from the workspace `Cargo.toml`
(`[workspace.package] version = "0.1.2"`), so this always matches
`zoid --version` (`crates/zoid/src/cli.rs::version_string`).

### Layout arithmetic

The wordmark's centering and the hint's right-alignment are deliberately left
byte-for-byte unchanged. The version simply *overlays the left padding* that the
current code already emits:

```
w         = area.width
wm_w      = width("zoid")
ver_w     = width(VERSION)
hint_w    = width(hint)

pad       = (w - wm_w) / 2                 // UNCHANGED → wordmark stays centered
right_pad = w - (pad + wm_w) - hint_w      // UNCHANGED → hint stays flush-right

left zone:
  if pad >= ver_w + 1:                     // room for version + ≥1 space gap
      [VERSION][ (pad - ver_w) spaces ]
  else:
      [ pad spaces ]                       // fallback: original bar, no version
```

Only the left-padding span is replaced. `used`, `right_pad`, and the hint span
are computed exactly as before, so the wordmark and hint render pixel-identically
to today — the diff reviewer only has to reason about the new left span.

### Styling

`Style::new().fg(color::DIM)` — the same token as the wordmark and hint
(`DIM = Rgb(0x6e, 0x76, 0x81)`, `crates/zoid-tui/src/tokens.rs`). The version
reads as the same quiet chrome, not a shout.

### Graceful degradation

`render_title` must never overflow or shift the wordmark. When the left pad
cannot hold the version (`pad < ver_w + 1`, i.e. a very narrow terminal), it
falls back to the exact original bar (leading spaces only, no version). At
zoid's 100×24 snapshot floor `pad = 48` and `ver_w = 6`, so the version always
shows in practice.

## Performance

`concat!("v", env!(...))` resolves at compile time to a single static string
baked into the binary — no per-frame formatting or allocation in the render hot
path, consistent with the existing BodyCache work that keeps frames ~7ms median.

## Testing

- **Existing insta snapshots** that capture the title row will now include the
  version. These are intentional diffs — regenerate with `cargo insta accept`
  after visual confirmation.
- **New coverage** (via the shell render snapshot, since `render_title` is
  private):
  - Normal width: version appears flush-left, wordmark centered, hint flush-right.
  - Narrow width (below the fit threshold): version is dropped cleanly, no
    overflow, wordmark unmoved.
- Full workspace test suite stays green.

## Files touched

- `crates/zoid-tui/src/render.rs` — `render_title` (the only logic change).
- Snapshot fixtures under `crates/zoid-tui` (regenerated, not hand-edited).
