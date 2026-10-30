# simstim.tech

The landing page for Simstim: one static HTML page, one stylesheet, no
JavaScript, no analytics, no cookies. It is hosted on Cloudflare Pages.

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

`.github/workflows/site.yml` builds and deploys on every push to `main`
that touches `site/`, and `release.yml` calls it after a stable release so
the download link moves to the new version without a manual step.

One-time Cloudflare setup:

1. Create the Pages project: `npx wrangler pages project create simstim
   --production-branch main` (or in the dashboard with Direct Upload).
2. Add the custom domain `simstim.tech` to the project and let Cloudflare
   create the DNS records. Add `www.simstim.tech` as well, or a Bulk
   Redirect rule from `www` to the apex.
3. Create an API token with the "Cloudflare Pages: Edit" permission and
   store it as the `CLOUDFLARE_API_TOKEN` repository secret, with the
   account id as `CLOUDFLARE_ACCOUNT_ID`.
4. Run the Site workflow once by hand from the Actions tab.

Downloads come from GitHub Releases. Visitors can only download while the
repository is public.

## Images

`public/assets/app-summary*.{webp,png}` and `public/og.png` are derived from
`docs/images/beta-summary.png`. Regenerate them with `sharp` when the
screenshot changes; trim the transparent margins and keep the 1165 px and 2330 px widths.
