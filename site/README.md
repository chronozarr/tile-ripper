# Tile-Ripper documentation site

Vocs 1.4.1 and React render the source Markdown listed in `content.mjs`. The only
handwritten site page is `docs/pages/index.mdx`; generated `.md` pages and raw
copies are ignored. Edit repository documentation, then rebuild. Local links go
to published routes when available and otherwise to the source repository.

The visual direction is a quiet scientific workbench: snow `#f8fafc`, ink
`#162a3d`, sea `#0a6b78`, slate `#546778` and selection `#d9eff1`. Optima/Candara
and a system monospace stack avoid external font requests. A source-grid
schematic carries the landing page; installation, product contracts and evidence
remain ordinary readable documentation. No performance metric is invented for
the illustration.

```sh
cd site
npm ci --min-release-age=7
npm audit
npm test
npm run build
./node_modules/.bin/wrangler dev --ip 127.0.0.1 --port 4173
# Production, after reviewing the build and account/domain bindings:
npm run deploy
```

The Worker serves `docs/dist`, redirects www to the apex and preserves actual
404 responses. Only `tileripper.com` and `www.tileripper.com` are bound;
`data.tileripper.com`, storage buckets and ChronoZarr deployments are outside
this Worker. Documentation deployment needs the configured Cloudflare account.

Versions and dependency overrides follow the existing ChronoZarr docs stack.
`scripts/prepare-mermaid.mjs` is adapted from the Apache-2.0 ChronoZarr source at
revision `4154135987db0a756e354e0348c0ec5681cac193`. It rebuilds the installed
Mermaid classic renderer with the pinned DOMPurify/KaTeX dependencies rather
than leaving an older bundled copy. The original upstream licenses remain in
the installed packages. Build output is separate from Rust acquisition code.
