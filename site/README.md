# simstim.tech

The landing page for Simstim: one static HTML page, one stylesheet, no
JavaScript, no analytics, no cookies. It is served by Cloudflare Workers as
static assets, which is what Cloudflare Pages has become.

## How it is built

`node build.mjs` writes `dist/`:

- reads the latest stable GitHub release and fills in the version, file
  name, size, date and SHA-256 in `src/index.html`;
- writes `_redirects` so `https://simstim.tech/download` sends the visitor
  to that release's DMG on GitHub;
- copies `public/` and gives everything in `public/assets/` a content hash
  so `_headers` can cache it forever.

The build needs a GitHub token while the repository is private. CI uses the
workflow token; locally the `gh` CLI token is picked up automatically.

Preview locally with Cloudflare's dev server, which honours `_redirects`
and `_headers`:

```sh
pnpm --dir site preview
```

## Deployment

`wrangler.toml` names the Worker `simstim` and its custom domains;
Cloudflare creates the DNS records for them on the first deploy.
`.github/workflows/site.yml` runs `wrangler deploy` on every push to `main`
that touches `site/`, and `release.yml` calls it after a stable release so
the download link moves to the new version without a manual step.

CI needs two repository secrets:

- `CLOUDFLARE_ACCOUNT_ID`: the account id shown by `wrangler whoami`.
- `CLOUDFLARE_API_TOKEN`: an API token created in the Cloudflare dashboard
  from the "Edit Cloudflare Workers" template, limited to this account.

A manual deploy from a logged-in machine is `wrangler deploy` inside `site/`
after `node build.mjs`.

Downloads come from GitHub Releases. Visitors can only download while the
repository is public.

## Images

`public/assets/app-{summary,loops,recording}*` are the app screenshots,
`public/assets/call-banner*` the floating call banner, `public/og.png` the
link preview and `docs/images/*.png` the README copies. All of them come
from one script:

```sh
(cd apps/desktop && pnpm exec vite --config vite.demo.config.ts)  # the fictional preview on port 1422
pnpm --dir site install
pnpm --dir site screenshots
```

`screenshots.mjs` renders the preview in headless Google Chrome at 2x,
puts each render into `screenshot-frame.png`, a capture of the real app
window's shadow and corners with the content removed, and writes the PNG,
WebP and 2x WebP variants. It prints each image's size; copy it to the
`width` and `height` of the `<img>` in `src/index.html` when it changes.
The preview's fixtures live in `apps/desktop/demo/tauri.ts`.
