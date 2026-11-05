#!/usr/bin/env bash
# Writes the update manifest the app polls to offer a new version:
#
#   latest-manifest.sh <dist-dir> <tag> <notes.md> > latest.json
#
# Published as a release asset, so the newest stable release is always at
# https://github.com/<repo>/releases/latest/download/latest.json
# (GitHub skips prereleases for `latest`). Shape:
#
#   { "version": "0.2.0", "tag": "v0.2.0", "published_at": "…",
#     "url": "<release page>", "notes": "<markdown>",
#     "assets": [ { "platform": "android", "arch": "aarch64",
#                   "kind": "apk", "name": "…", "url": "…",
#                   "size": 123, "sha256": "…" }, … ] }
#
# platform ∈ android | linux | macos | windows; arch ∈ x86_64 | aarch64 |
# universal. Server artifacts (notein-relay) are left out — the manifest
# is for the app.
set -euo pipefail
shopt -s nullglob

dist=$1
tag=$2
notes=$3
repo=${GITHUB_REPOSITORY:?}
base="https://github.com/$repo/releases/download/$tag"

arch_of() {
  case "$1" in
    *universal*) echo universal ;;
    *arm64-v8a* | *aarch64* | *arm64*) echo aarch64 ;;
    *x86_64* | *amd64* | *x64*) echo x86_64 ;;
    *) echo unknown ;;
  esac
}

# Prints "<platform> <kind>" or nothing for files the app doesn't install.
classify() {
  case "$1" in
    notein-relay*) ;;
    *.apk) echo "android apk" ;;
    *.AppImage) echo "linux appimage" ;;
    *.deb) echo "linux deb" ;;
    *.rpm) echo "linux rpm" ;;
    *.flatpak) echo "linux flatpak" ;;
    *-linux.tar.gz) echo "linux tar.gz" ;;
    *.dmg) echo "macos dmg" ;;
    *.pkg) echo "macos pkg" ;;
    *.app.zip) echo "macos app" ;;
    *darwin*.tar.gz) echo "macos tar.gz" ;;
    *-setup.exe) echo "windows nsis" ;;
    *.msi) echo "windows msi" ;;
    *windows*.zip) echo "windows zip" ;;
  esac
}

assets='[]'
for file in "$dist"/*; do
  name=$(basename "$file")
  read -r platform kind <<<"$(classify "$name")" || true
  [ -n "${platform:-}" ] || continue
  assets=$(jq -c \
    --arg platform "$platform" \
    --arg arch "$(arch_of "$name")" \
    --arg kind "$kind" \
    --arg name "$name" \
    --arg url "$base/$name" \
    --argjson size "$(stat -c %s "$file")" \
    --arg sha256 "$(sha256sum "$file" | cut -d' ' -f1)" \
    '. + [{platform: $platform, arch: $arch, kind: $kind, name: $name, url: $url, size: $size, sha256: $sha256}]' \
    <<<"$assets")
  platform=""
done

jq -n \
  --arg version "${tag#v}" \
  --arg tag "$tag" \
  --arg published_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg url "https://github.com/$repo/releases/tag/$tag" \
  --rawfile notes "$notes" \
  --argjson assets "$assets" \
  '{version: $version, tag: $tag, published_at: $published_at, url: $url, notes: $notes, assets: ($assets | sort_by(.platform, .kind, .arch))}'
