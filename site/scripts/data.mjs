// Writes the data the page is built from:
//
// - src/data/targets.json: nix/targets.nix as JSON, the same matrix the
//   release workflow builds. Platforms, formats and Android ABIs on the
//   page come from here, so adding a format in Nix adds it to the site.
// - src/data/release.json: the newest published release (files, sizes,
//   version, date). Every release is a pre-release, so `releases/latest`
//   is not used. Without network the page still builds; its script asks
//   GitHub again in the browser.
import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const REPO = 'SergioRibera/notinplus';
const out = (name) => fileURLToPath(new URL(`../src/data/${name}`, import.meta.url));
const targetsFile = fileURLToPath(new URL('../../nix/targets.nix', import.meta.url));

const targets = execFileSync('nix', ['eval', '--json', '--file', targetsFile], {
  encoding: 'utf8',
});
writeFileSync(out('targets.json'), targets);

let release = null;
try {
  const headers = { Accept: 'application/vnd.github+json' };
  if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const res = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=10`, { headers });
  if (!res.ok) throw new Error(`GitHub answered ${res.status}`);
  const list = await res.json();
  const newest = list.find((r) => !r.draft && r.assets.some((a) => a.name === 'latest.json'));
  if (newest) {
    release = {
      tag: newest.tag_name,
      version: newest.tag_name.replace(/^v/, ''),
      publishedAt: newest.published_at,
      url: newest.html_url,
      assets: newest.assets.map((a) => ({ name: a.name, size: a.size, url: a.browser_download_url })),
    };
  }
} catch (err) {
  console.warn(`release data unavailable, the page will fetch it in the browser: ${err.message}`);
}
writeFileSync(out('release.json'), JSON.stringify(release, null, 2));
console.log(`targets.json written; release ${release ? release.tag : 'missing'}`);
