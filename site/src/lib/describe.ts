import type { Lang } from '../i18n';
import { strings } from '../i18n';
import type { Slot } from './release';

/** File type as the download button names it. */
const SHORT: Record<string, string> = {
  appimage: 'AppImage',
  deb: '.deb',
  rpm: '.rpm',
  flatpak: 'Flatpak',
  'tar.gz': '.tar.gz',
  nsis: '.exe',
  msi: '.msi',
  zip: '.zip',
  dmg: '.dmg',
  pkg: '.pkg',
  app: '.app',
  apk: 'APK',
};

/** Name, button word and one line about a release file. */
export function describe(lang: Lang, slot: Slot) {
  const d = strings[lang].download;
  if (slot.platform === 'android') {
    const [name, text] = d.apk[slot.arch];
    return { name, text, short: SHORT.apk };
  }
  const key = slot.platform === 'macos' && slot.kind === 'tar.gz' ? 'darwin-tar.gz' : slot.kind;
  const [name, text] = d.formats[key as keyof typeof d.formats] ?? [slot.kind, ''];
  return { name, text, short: SHORT[slot.kind] ?? slot.kind };
}
