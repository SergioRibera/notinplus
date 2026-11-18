# NotIn Plus site

The landing page at <https://sergioribera.rs/notinplus/>, in English and
Spanish (`/es/`). Static Astro build served by a Cloudflare Worker, the
same setup as the istmo docs.

## Where the data comes from

- **What is offered**: `nix/targets.nix`, the matrix the release workflow
  builds. `scripts/data.mjs` evaluates it with `nix eval` before every
  build, so adding or removing a format or an Android ABI there changes
  the download section.
- **Files, sizes, version and date**: the newest GitHub release with a
  `latest.json` (every release is a pre-release, so `releases/latest` is
  not used). Fetched at build time; in the browser the page asks GitHub
  again and switches to a newer release if one exists, so a page built
  before a release still offers it.
- **Screenshots**: `src/assets/shots/`, headless renders of the app
  (Astro converts them to WebP at build time).

The only JavaScript: platform detection and the download tabs, the
feature tour, the phone carousel dots and the language menu. Without it
the page still lists every file.

## Commands

```sh
cd site
pnpm install
pnpm dev        # http://localhost:4321/notinplus/
pnpm build      # dist/notinplus
pnpm run deploy # build + wrangler deploy (needs Cloudflare credentials)
```

`nix` must be on the PATH (the build evaluates `nix/targets.nix`).

## Releases

The `site` job of `.github/workflows/release.yml` rebuilds and deploys the
site after each published release. It runs once the repository has a
`CLOUDFLARE_API_TOKEN` secret (and `CLOUDFLARE_ACCOUNT_ID` when the token
covers more than one account).
