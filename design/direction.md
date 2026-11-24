# Landing page: direction

Register: product persuade, restraint mode. The visitor wants to know what
NotIn Plus is, trust that it runs on their device and get the right file in
one click. Convention leads (Jakob); identity comes from the existing brand
kit (`assets/brand/`), not from a new concept.

## Inventory

1. The name: "notes in" the margin; the mark is a handwritten n beside the red
   margin rule of a notebook page.
2. Rubric `#C0482F`: the red ink manuscripts and annotated Bibles used for what
   matters. Brand rule: a line, never body text.
3. Paper `#EFE6D4`, ink `#1F1B18`, night `#16181C` (approved grounds: white,
   paper, night).
4. Real screenshots: desk with Ana and Marco editing live (1440x900), Romans 8
   with the Greek beside it, Home, four phone screens (390x844).
5. Handwriting on the canvas ("Romans 8", "Kept", red circle): the product's
   own ink, already in the screenshots.
6. Four platforms, 23 files per release: APK (arm64, x86_64, universal),
   AppImage / deb / rpm / flatpak / tar.gz, dmg / pkg / app.zip (Apple
   silicon only, unsigned), setup.exe / msi / zip. Every release ships
   `latest.json` (platform, arch, kind, size, sha256) and `SHA256SUMS`.
7. Alpha: every release is a pre-release; formats and sync may change.
8. Free, MIT / Apache-2.0, Rust + Freya + istmo, `nix run` works.
9. Optional relay server that forwards sealed records it cannot read.
10. Interface in English and Spanish, six themes, any accent.

## Argument

| Section | Claim | Proof |
|---|---|---|
| Hero | Handwriting and notes, on every device | Desk screenshot; download button already set for the visitor's OS |
| Notes and canvas, built in | The core: pen and keyboard in the same library | Phone sketch + phone note screenshots |
| PDF and Bible come as plugins | Useful from the first minute, not part of the core; can be turned off | Bible plugin screenshot (no PDF screenshot exists yet) |
| Organize and sync | Your library on every device, shared live when you want | Home screenshot; pointer to the relay section |
| Download | Every platform and format, verified | Full list from `latest.json`, sizes, SHA256SUMS, Nix line |
| notin-relay | For people who run a server: what it is, why run it, what it can see, what it needs | Six server packages, container image, docker command, invite address flow |
| Footer | Open source, alpha | License, GitHub, backup warning |

No logo strip, no testimonials, no pricing: none exist.

## Directions considered

- **A. The margin (chosen).** White page, one rubric margin rule running the
  full height beside the content column, like the notebook in the mark;
  Noto Sans as in the app; screenshots carry the colour. Only this product
  owns the margin rule, it is the logo stretched over the page.
- **B. Paper field.** Whole page on paper with ink type and rubric details.
  Rejected: cream + red accent is a third-wave tell, and it would make the
  real (dark) screenshots look pasted on.
- **C. Night.** Dark page matching the app's default theme. Rejected as
  default (screenshots would merge into the ground); kept as the
  `prefers-color-scheme: dark` variant.

## Direction sheet

**Type.** Noto Sans only (the app's UI face and the brand's companion face):
H1 56/64 semibold, H2 36/44 semibold, body 18/28 regular, small 14/20. Mobile
H1 36/44, H2 28/36, body 17/26. Wordmark only as the logo SVG. Numbers in
the download list tabular.

**Palette (proportions).** White ground ~80 %; ink `#1F1B18` text and the one
filled button ~12 %; paper `#EFE6D4` as one full-width field behind the
download section ~7 %; rubric `#C0482F` ~1 %: the margin rule and the I in
the wordmark, nothing else. Muted text `#5E564F` (6.9:1 on white). Dark:
night `#16181C` ground, paper text, rubric rule unchanged (3.6:1, graphic only).

**Hero.** Left: logo, H1, one supporting sentence, the detected download
button (ink fill, paper text) with a meta line (format, arch, size, version),
a text link "Other platforms and formats", and the alpha line. Right: the desk
screenshot at full width of its column, 12 px radius, 1 px border, no shadow.
Margin rule at the left edge of the content column, top of hero to footer.

**Sections.** Text column 520 px max on the margin side, screenshots on the
other side, alternating; never cards. Section gap 160 px desktop, 96 px
mobile. Download section is the paper field: detected platform first, then
four platform blocks with one row per file (name, arch, size, Download link).

**Imagery.** Only the seven real screenshots; phone shots shown at 1:1 crop
ratio 390x844 side by side, desktop shots at 16:10. No device frames.

**Motion.** One moment: on first load the margin rule draws from top to
bottom (scaleY, 700 ms, exponential ease-out). Reduced motion: drawn already.
Hover / focus on links and button only.

## OS detection rules

| Detected | Primary file | Alternatives shown under the button |
|---|---|---|
| Windows x64 | `-x64-setup.exe` | `.msi`, `.zip` |
| macOS | `-arm64.dmg` | `.pkg`, `.app.zip`; note: Apple silicon only, unsigned (right-click, then Open) |
| Linux x86_64 | `-x86_64.AppImage` | `.deb`, `.rpm`, `.flatpak`, `.tar.gz` |
| Linux aarch64 | `-aarch64.AppImage` | `.deb`, `.rpm`, `.tar.gz` |
| Android | `-android-arm64-v8a.apk` | universal APK, x86_64 APK |
| iPhone / iPad | none | "Not on iPhone or iPad yet" + link to other platforms |
| Unknown | none | button scrolls to the download list |

Architecture from `navigator.userAgentData.getHighEntropyValues` when present,
UA string otherwise. Data from the newest release's `latest.json` (all
releases are pre-releases, so `releases/latest` 404s); when the fetch fails
the button links to the GitHub releases page.

## Round 2 decisions (2026-11-18)

- English and Spanish. Initial language from `navigator.languages`; the nav
  link ("Español" / "English") switches and is remembered in localStorage.
- Lives in `site/` in this repo, published with GitHub Pages.
- Android default: arm64 APK; universal as the alternative.
- No Intel Mac build; the page says so.
- Core = notes + canvas. PDF and Bible are plugins installed by default
  (Settings, Plugins can turn them off); the hero no longer names them.
- notin-relay gets its own section on a night field (different audience:
  people who run a server). Never auto-detected; lists the six server
  packages and the container image. Facts: UDP 7720 by default, sessions end
  to end encrypted through it, direct connection tried first, `--no-retain`
  writes nothing to disk, access is the operator's choice, prints an invite
  address `host:port/key` for Settings, Devices, Server.

## Round 3 (2026-11-18)

- **Composition, less boxy.** Hero overlaps a phone screenshot on the
  desktop one (proves "every device"), the desktop shot bleeds off the
  right edge. Notes and canvas = feature tour: a list where the active
  feature expands and the stage zooms into the matching crop of a real
  screenshot; advances every few seconds, pauses on hover/focus, static
  with reduced motion. Mobile turns the tour into a swipe carousel with the
  next card peeking. Bible screenshot bleeds left, Home + phone overlap.
- **Feature lists** become term + one sentence (definition grid), never
  bullet walls; plugins shown as the two rows of Settings, Plugins with
  their switch on.
- **Downloads.** Platform tabs (detected one preselected, "this computer"
  hint), a recommended file with what it is and how to open it, an
  architecture switch where two exist, then other formats each with who
  it is for. Mobile: chips that scroll, same card.
- **Data.** Catalog (platforms, formats, ABIs) from `nix/targets.nix` and
  `nix/android.nix` via a new `.#site` output; files, sizes, sha256,
  version and date from the release's `latest.json`. The release workflow
  rebuilds and deploys the site after publishing; the page also asks the
  GitHub API on load and uses a newer release if one exists, else the
  baked one.
- **Relay** downloads match `targets.server`: .deb / .rpm / .tar.gz for
  x86_64 and aarch64 Linux, plus the multi-arch OCI image.
- **Language.** Globe + current language + chevron, menu lists each
  language in itself; `/en/` and `/es/` URLs.
