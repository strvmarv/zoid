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
`publish-site.yml` fails the build if a version token reappears anywhere in the
site.

Keep it this way: there is no release step that touches `public/`, and adding
one is how the drift comes back.

## Hosting

`index.html` is portable — drop it on any static host. For GitHub Pages, publish
from a **public** repo, since the source repo is private. Do not enable Pages on
the private source repo.

## Publishing

The site is mirrored into the public `strvmarv/zoid-releases` repo (the same
repo that hosts binary release artifacts) and served via GitHub Pages from its
`docs/` folder.

- **Workflow:** `.github/workflows/publish-site.yml` in the private source repo.
- **Trigger:** push to `main` touching `public/**`, or `workflow_dispatch`.
- **Secret:** reuses `RELEASES_REPO_TOKEN` (same PAT the binary-release mirror
  uses to write to `strvmarv/zoid-releases`). No separate secret needed.
- **What ships:** `index.html`, `preview.html`, and `frames/`, copied as-is.
  The authoring scripts, `preview.template.html`, and this README are stripped
  from the published payload — **if you add a dev-only file to `public/`, add it
  to that `rm -f` list too**, or it ships. The workflow fails if any `.sh`
  survives into `docs/`.
- **Pages config (one-time, on `strvmarv/zoid-releases`):** Settings → Pages →
  Source: Deploy from a branch → Branch: `main` / `/docs`.
