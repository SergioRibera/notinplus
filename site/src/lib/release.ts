// What the page offers (from nix/targets.nix) and where each file is
// (from a GitHub release). Shared by the build and the browser script, so
// both read release files the same way.

export type Platform = 'android' | 'windows' | 'macos' | 'linux';
export type Arch = 'x86_64' | 'aarch64' | 'universal';

export interface Targets {
  desktop: Record<string, { formats: string[] }>;
  server: Record<string, { formats: string[] }>;
  android: { abis: string[] };
}

export interface ReleaseAsset {
  name: string;
  size: number;
  url: string;
}

export interface Release {
  tag: string;
  version: string;
  publishedAt: string;
  url: string;
  assets: ReleaseAsset[];
}

/** One downloadable file the page lists. */
export interface Slot {
  platform: Platform | 'relay';
  arch: Arch;
  kind: string;
}

export const slotKey = (s: Slot) => `${s.platform}/${s.arch}/${s.kind}`;

/** Platforms in the order the tabs show them. */
export const PLATFORMS: Platform[] = ['android', 'windows', 'macos', 'linux'];

/** The file most people on a platform want. */
export const RECOMMENDED: Record<Platform, { kind: string; arch: Arch }> = {
  android: { kind: 'apk', arch: 'aarch64' },
  windows: { kind: 'nsis', arch: 'x86_64' },
  macos: { kind: 'dmg', arch: 'aarch64' },
  linux: { kind: 'appimage', arch: 'x86_64' },
};

const systemArch = (system: string): Arch =>
  system.startsWith('aarch64') ? 'aarch64' : 'x86_64';

const systemPlatform = (system: string): Platform | null => {
  if (system.endsWith('-linux')) return 'linux';
  if (system.endsWith('-darwin')) return 'macos';
  if (system.endsWith('-windows')) return 'windows';
  return null;
};

const ABI_ARCH: Record<string, Arch> = { 'arm64-v8a': 'aarch64', x86_64: 'x86_64' };

/** Every app file the release matrix produces, per platform, in order. */
export function appSlots(targets: Targets): Record<Platform, Slot[]> {
  const out: Record<Platform, Slot[]> = { android: [], windows: [], macos: [], linux: [] };
  for (const [system, t] of Object.entries(targets.desktop)) {
    const platform = systemPlatform(system);
    if (!platform) continue;
    for (const kind of t.formats) out[platform].push({ platform, arch: systemArch(system), kind });
  }
  for (const abi of targets.android.abis) {
    const arch = ABI_ARCH[abi];
    if (arch) out.android.push({ platform: 'android', arch, kind: 'apk' });
  }
  out.android.push({ platform: 'android', arch: 'universal', kind: 'apk' });
  // Recommended architecture first (arm64 phones, x86_64 desktops).
  out.android.sort((a, b) => rank(a.arch) - rank(b.arch));
  return out;
}

const rank = (arch: Arch) => ({ aarch64: 0, universal: 1, x86_64: 2 })[arch];

/** notin-relay files, by format then architecture. */
export function relaySlots(targets: Targets): Slot[] {
  const slots: Slot[] = [];
  for (const [system, t] of Object.entries(targets.server)) {
    for (const kind of t.formats) slots.push({ platform: 'relay', arch: systemArch(system), kind });
  }
  const order = (k: string) => ['deb', 'rpm', 'tar.gz'].indexOf(k);
  return slots.sort((a, b) => order(a.kind) - order(b.kind) || desktopRank(a.arch) - desktopRank(b.arch));
}

/** Architectures a platform ships, recommended first. */
export const archsOf = (slots: Slot[]) =>
  [...new Set(slots.map((s) => s.arch))].sort((a, b) => desktopRank(a) - desktopRank(b));

const desktopRank = (arch: Arch) => ({ x86_64: 0, aarch64: 1, universal: 2 })[arch];

const archOf = (name: string): Arch => {
  if (/universal/.test(name)) return 'universal';
  if (/arm64-v8a|aarch64|arm64/.test(name)) return 'aarch64';
  return 'x86_64';
};

/** Same rules as .github/scripts/latest-manifest.sh, plus the relay. */
export function classify(name: string): Slot | null {
  const arch = archOf(name);
  if (name.startsWith('notin-relay')) {
    if (name.endsWith('.deb')) return { platform: 'relay', arch, kind: 'deb' };
    if (name.endsWith('.rpm')) return { platform: 'relay', arch, kind: 'rpm' };
    if (name.endsWith('-linux.tar.gz')) return { platform: 'relay', arch, kind: 'tar.gz' };
    return null;
  }
  const app = (platform: Platform, kind: string): Slot => ({ platform, arch, kind });
  if (name.endsWith('.apk')) return app('android', 'apk');
  if (name.endsWith('.AppImage')) return app('linux', 'appimage');
  if (name.endsWith('.deb')) return app('linux', 'deb');
  if (name.endsWith('.rpm')) return app('linux', 'rpm');
  if (name.endsWith('.flatpak')) return app('linux', 'flatpak');
  if (name.endsWith('-linux.tar.gz')) return app('linux', 'tar.gz');
  if (name.endsWith('.dmg')) return app('macos', 'dmg');
  if (name.endsWith('.pkg')) return app('macos', 'pkg');
  if (name.endsWith('.app.zip')) return app('macos', 'app');
  if (/darwin.*\.tar\.gz$/.test(name)) return app('macos', 'tar.gz');
  if (name.endsWith('-setup.exe')) return app('windows', 'nsis');
  if (name.endsWith('.msi')) return app('windows', 'msi');
  if (/windows.*\.zip$/.test(name)) return app('windows', 'zip');
  return null;
}

/** Release files by slot key, plus the checksum list. */
export function filesOf(release: Release | null) {
  const files = new Map<string, ReleaseAsset>();
  let sums: string | undefined;
  for (const asset of release?.assets ?? []) {
    if (asset.name === 'SHA256SUMS') sums = asset.url;
    const slot = classify(asset.name);
    if (slot) files.set(slotKey(slot), asset);
  }
  return { files, sums };
}

/** "53.6 MB", decimal like the release page. */
export const formatSize = (bytes: number) =>
  bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${(bytes / 1e6).toFixed(1)} MB`;
