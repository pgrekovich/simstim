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

`public/assets/app-summary*.{webp,png}` and `public/og.png` are derived from
`docs/images/beta-summary.png`. Regenerate them with `sharp` when the
screenshot changes; trim the transparent margins and keep the 1165 px and 2330 px widths.
