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
