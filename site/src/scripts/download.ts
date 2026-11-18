// Picks the visitor's platform, turns the hero button into that file,
// switches the download tabs, and swaps in a newer release than the one
// the page was built with.
import { fill, strings, type Lang } from '../i18n';
import { describe } from '../lib/describe';
import {
  PLATFORMS,
  RECOMMENDED,
  appSlots,
  filesOf,
  formatSize,
  slotKey,
  type Arch,
  type Platform,
  type Release,
  type Targets,
} from '../lib/release';

type Tab = Platform | 'ios';

interface Data {
  lang: Lang;
  targets: Targets;
  release: Release | null;
}

const REPO = 'SergioRibera/notinplus';

async function detect(): Promise<{ tab: Tab | null; arch: Arch }> {
  const ua = navigator.userAgent;
  const uaData = (navigator as Navigator & {
    userAgentData?: { getHighEntropyValues(h: string[]): Promise<{ architecture?: string }> };
  }).userAgentData;
  if (/iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1)) {
    return { tab: 'ios', arch: 'aarch64' };
  }
  if (/Android/.test(ua)) return { tab: 'android', arch: 'aarch64' };
  if (/Windows/.test(ua)) return { tab: 'windows', arch: 'x86_64' };
  if (/Macintosh|Mac OS X/.test(ua)) return { tab: 'macos', arch: 'aarch64' };
  if (/Linux|X11|CrOS/.test(ua)) {
    let arch: Arch = /aarch64|arm64|armv8/.test(ua) ? 'aarch64' : 'x86_64';
    try {
      const hints = await uaData?.getHighEntropyValues(['architecture']);
      if (hints?.architecture === 'arm') arch = 'aarch64';
    } catch {}
    return { tab: 'linux', arch };
  }
  return { tab: null, arch: 'x86_64' };
}

/** Newest published release with an update manifest, or null. */
async function newestRelease(): Promise<Release | null> {
  const res = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=10`, {
    headers: { Accept: 'application/vnd.github+json' },
  });
  if (!res.ok) throw new Error(String(res.status));
  const list = (await res.json()) as Array<{
    draft: boolean;
    tag_name: string;
    published_at: string;
    html_url: string;
    assets: Array<{ name: string; size: number; browser_download_url: string }>;
  }>;
  const r = list.find((x) => !x.draft && x.assets.some((a) => a.name === 'latest.json'));
  if (!r) return null;
  return {
    tag: r.tag_name,
    version: r.tag_name.replace(/^v/, ''),
    publishedAt: r.published_at,
    url: r.html_url,
    assets: r.assets.map((a) => ({ name: a.name, size: a.size, url: a.browser_download_url })),
  };
}

export function start() {
  const raw = document.getElementById('download-data')?.textContent;
  if (!raw) return;
  const data = JSON.parse(raw) as Data;
  const t = strings[data.lang];
  const slots = appSlots(data.targets);
  let release = data.release;
  let detected: { tab: Tab | null; arch: Arch } = { tab: null, arch: 'x86_64' };

  const tabs = [...document.querySelectorAll<HTMLButtonElement>('.platform-tabs [role=tab]')];
  const select = document.querySelector<HTMLSelectElement>('[data-platform-select]');

  const showPlatform = (tab: Tab, focus = false) => {
    for (const b of tabs) {
      const on = b.dataset.platform === tab;
      b.setAttribute('aria-selected', String(on));
      b.tabIndex = on ? 0 : -1;
      if (on && focus) b.focus();
    }
    for (const p of document.querySelectorAll<HTMLElement>('[data-panel]')) {
      p.toggleAttribute('data-active', p.dataset.panel === tab);
    }
    if (select) select.value = tab;
  };

  const showArch = (platform: string, arch: string) => {
    const panel = document.querySelector<HTMLElement>(`[data-panel="${platform}"]`);
    if (!panel) return;
    for (const v of panel.querySelectorAll<HTMLElement>('.arch-view')) {
      const on = v.dataset.arch === arch;
      v.toggleAttribute('data-active', on);
      for (const r of v.querySelectorAll<HTMLInputElement>('input[type=radio]')) r.checked = r.value === arch;
    }
  };

  tabs.forEach((b, i) => {
    b.addEventListener('click', () => showPlatform(b.dataset.platform as Tab));
    b.addEventListener('keydown', (e) => {
      const step = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
      if (step === undefined) return;
      e.preventDefault();
      showPlatform(tabs[(i + step + tabs.length) % tabs.length].dataset.platform as Tab, true);
    });
  });
  select?.addEventListener('change', () => showPlatform(select.value as Tab));
  for (const r of document.querySelectorAll<HTMLInputElement>('[data-arch-pick]')) {
    r.addEventListener('change', () => {
      showArch(r.dataset.archPick!, r.value);
      document.querySelector<HTMLElement>(`[data-panel="${r.dataset.archPick}"] .arch-view[data-active] input:checked`)?.focus();
    });
  }

  const renderHero = () => {
    const button = document.getElementById('hero-download') as HTMLAnchorElement | null;
    const label = document.querySelector<HTMLElement>('[data-hero-label]');
    const meta = document.querySelector<HTMLElement>('[data-hero-meta]');
    const ios = document.querySelector<HTMLElement>('[data-hero-ios]');
    if (!button || !label || !meta || !ios) return;
    const { tab, arch } = detected;
    ios.hidden = tab !== 'ios';
    if (!tab || tab === 'ios') {
      label.textContent = t.hero.ctaChoose;
      button.href = '#download';
      meta.textContent = '';
      return;
    }
    const platform: Platform = tab;
    const kind = RECOMMENDED[platform].kind;
    const wanted = platform === 'android' ? RECOMMENDED.android.arch : arch;
    const slot =
      slots[platform].find((s) => s.kind === kind && s.arch === wanted) ??
      slots[platform].find((s) => s.kind === kind) ??
      slots[platform][0];
    if (!slot) return;
    const file = filesOf(release).files.get(slotKey(slot));
    label.textContent = fill(t.hero.cta, { os: t.download.platforms[platform] });
    button.href = file?.url ?? release?.url ?? `https://github.com/${REPO}/releases`;
    meta.textContent = file && release
      ? fill(platform === 'android' ? t.hero.meta.replace("{arch}, ", "") : t.hero.meta, {
          format: describe(data.lang, slot).name,
          arch: t.arch[slot.arch],
          size: formatSize(file.size),
          version: release.version,
        })
      : '';
  };

  const renderRelease = () => {
    if (!release) return;
    const { files, sums } = filesOf(release);
    for (const a of document.querySelectorAll<HTMLAnchorElement>('[data-slot]')) {
      const f = files.get(a.dataset.slot!);
      a.href = f?.url ?? release.url;
    }
    for (const s of document.querySelectorAll<HTMLElement>('[data-size]')) {
      const f = files.get(s.dataset.size!);
      s.textContent = f ? formatSize(f.size) : '';
    }
    const date = new Intl.DateTimeFormat(data.lang, { dateStyle: 'long' }).format(new Date(release.publishedAt));
    const line = document.querySelector('[data-version-line]');
    if (line) line.textContent = fill(t.download.version, { version: release.version, date });
    const link = document.querySelector<HTMLAnchorElement>('[data-release-link]');
    if (link) link.href = release.url;
    const sumsLink = document.querySelector<HTMLAnchorElement>('[data-sums]');
    if (sumsLink && sums) sumsLink.href = sums;
    renderHero();
  };

  detect().then((d) => {
    detected = d;
    if (d.tab) {
      showPlatform(d.tab);
      const hint = document.querySelector(`#tab-${d.tab} .detected-hint`);
      if (hint) (hint as HTMLElement).hidden = false;
      if (d.tab !== 'ios') showArch(d.tab, d.arch);
    } else {
      showPlatform(PLATFORMS[0]);
    }
    renderHero();
  });

  newestRelease()
    .then((fresh) => {
      if (fresh && fresh.tag !== release?.tag) {
        release = fresh;
        renderRelease();
      }
    })
    .catch(() => {
      if (!release) {
        const note = document.querySelector<HTMLElement>('[data-unavailable]');
        if (note) note.hidden = false;
        // Nothing to wait for any more: drop the "looking for" line.
        const line = document.querySelector<HTMLElement>('[data-version-line]');
        if (line) line.hidden = true;
      }
    });
}
