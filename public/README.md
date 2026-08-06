# zoid teaser site

Self-contained, terminal-authentic teaser page. No build step at serve time —
`index.html` ships fully inlined.

## `index.html` is part hand-authored, part generated

Both, and the difference matters:

- **Hand-authored.** The static terminal mockups are hand-built HTML/CSS living
  directly in `index.html` (responsive, animated). Edit them in place.
- **Generated.** The three animated scenes — `context-economy`, `tools-models`,
  `extensibility` — are captured from the **live TUI renderer** and inlined
  between `<!--FRAMES:<scene>:BEGIN-->` / `<!--FRAMES:<scene>:END-->` markers.

> ⚠️ **Never hand-edit between the `FRAMES:` markers.** `assemble-site.sh`
> replaces everything in there from `frames/<scene>/`, so edits are silently
> lost on the next run. Change the scene fixtures
> (`crates/zoid-tui/examples/scenes/`) and re-capture instead.

## Regenerate the scenes

```sh
sh public/capture-preview.sh    # live renderer → public/frames/<scene>/NN.html
sh public/assemble-site.sh      # inline frames → public/index.html
sh public/assemble-preview.sh   # inline context-economy → public/preview.html
```

Both assemble steps are idempotent: same frames in, byte-identical file out.

## The site is deliberately version-free

The TUI status bar renders `v<CARGO_PKG_VERSION>`, resolved at **compile time**.
A capture therefore pins the page to whatever version it was built at, and every
release silently makes the site stale — `preview.html` sat at `v0.3.2` for two
releases this way.

So `capture-preview.sh` strips the version span from each frame, replacing it
with spaces of identical width. That is exactly what `title_line()`
(`crates/zoid-tui/src/render.rs`) renders when a terminal is too narrow to fit
the version, so the mockups stay authentic and the wordmark stays centered.
This was previously CI-enforced by `publish-site.yml`, now removed. Until a
replacement publish workflow exists, verify manually (`grep -rEo
'v[0-9]+\.[0-9]+\.[0-9]+' public/index.html public/preview.html
public/frames/`, expect no output) before shipping a new capture.

Keep it this way: there is no release step that touches `public/`, and adding
one is how the drift comes back.

## Hosting

`index.html` is portable — drop it on any static host.

## Publishing

There is currently no automated publish workflow for this site. The
previous pipeline mirrored `public/` into the (now-retired) private/public
repo split for GitHub Pages hosting; that pipeline was removed as part of
open-sourcing zoid (see
`docs/superpowers/specs/2026-08-05-open-source-zoid-design.md`). Once this
repo is public, GitHub Pages can be enabled directly on it (Settings →
Pages) without a separate mirror repo — setting that up is tracked as
follow-up work, not yet done.

For now, publish the built `index.html` manually to whatever static host you
choose, or run it locally.
