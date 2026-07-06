# zoid teaser site

Self-contained, terminal-authentic teaser page. No build step at serve time —
`index.html` is fully inlined.

## Regenerate

```sh
sh public/capture.sh   # re-render TUI frames from the live renderer → public/frames/
sh public/build.sh     # inject frames into template.html → public/index.html
```

## Hosting

`index.html` is portable — drop it on any static host. For GitHub Pages, publish
from a **public** repo (e.g. `zoid-site`), since the source repo is private.
Do not enable Pages on the private source repo.

## Publishing

The site is mirrored into the public `strvmarv/zoid-releases` repo (the same
repo that hosts binary release artifacts) and served via GitHub Pages from its
`docs/` folder.

- **Workflow:** `.github/workflows/publish-site.yml` in the private source repo.
- **Trigger:** push to `main` touching `public/**`, or `workflow_dispatch`.
- **Secret:** reuses `RELEASES_REPO_TOKEN` (same PAT the binary-release mirror
  uses to write to `strvmarv/zoid-releases`). No separate secret needed.
- **What ships:** `index.html` + `frames/`, copied as-is. The dev-only
  `build.sh`, `capture.sh`, and this README are excluded from the published
  payload.
- **Pages config (one-time, on `strvmarv/zoid-releases`):** Settings → Pages →
  Source: Deploy from a branch → Branch: `main` / `/docs`.
