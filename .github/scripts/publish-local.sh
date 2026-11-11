#!/usr/bin/env bash
# The `publish` job of release.yml, run by hand over a release run whose
# artifacts all built.
#
#   publish-local.sh <run-id> <tag> [work-dir]
#
# Run from the notinplus checkout, with `gh` logged in. Assets go up one
# at a time with retries, and the ones the release already has are
# skipped, so running it again finishes an interrupted upload. Pass the
# work dir a previous run printed to skip the download.
set -euo pipefail

run=$1
tag=$2
repo=SergioRibera/notinplus
work=${3:-}
if [ -n "$work" ] && [ -d "$work/dist" ]; then
  dist="$work/dist"
  echo "→ reusing $dist"
else
  work=$(mktemp -d)
  dist="$work/dist"
  mkdir -p "$dist"
  fetch=1
fi
echo "work dir: $work"

if [ -n "${fetch:-}" ]; then
echo "→ downloading the artifacts of run $run"
gh run download "$run" -R "$repo" -D "$work/raw" -p 'desktop-*' -p 'server-*' -p 'android'
# Merge every artifact folder into one flat dist/, like merge-multiple.
find "$work/raw" -type f -exec cp {} "$dist/" \;

# Names with spaces (the `NotIn Plus.app` zip) get renamed by GitHub.
for f in "$dist"/*' '*; do
  [ -e "$f" ] || continue
  mv "$f" "${f// /-}"
done

echo "→ release notes"
if [ -f "release-notes/$tag.md" ]; then
  cp "release-notes/$tag.md" "$work/CHANGES.md"
else
  nix run nixpkgs#git-cliff -- --config cliff.toml --current --strip header -o "$work/CHANGES.md"
fi
nix run nixpkgs#git-cliff -- --config cliff.toml --strip header -o "$dist/CHANGELOG.md"

echo "→ update manifest"
GITHUB_REPOSITORY=$repo bash .github/scripts/latest-manifest.sh "$dist" "$tag" "$work/CHANGES.md" > "$dist/latest.json"

echo "→ SHA256SUMS"
(cd "$dist" && find . -type f ! -name SHA256SUMS -printf '%P\0' | LC_ALL=C sort -z | xargs -0 sha256sum -- > SHA256SUMS)
fi

{
  cat "$work/CHANGES.md"
  cat <<EOF

## Install

- **Android**: \`notinplus-*-android-universal.apk\` works everywhere; the \`arm64-v8a\` / \`x86_64\` APKs are smaller.
- **Linux**: AppImage, \`.deb\`, \`.rpm\`, \`.flatpak\` or \`.tar.gz\`.
- **Nix**: \`nix run github:SergioRibera/notinplus/$tag\` (binary cache: \`cache.sergioribera.rs/main\`).
- **macOS**: \`.dmg\`, \`.pkg\` or the zipped \`.app\` (unsigned: right-click → Open the first time).
- **Windows**: \`-setup.exe\` (NSIS) or \`.msi\`.

### Relay server (\`notin-relay\`, Linux)

- \`notin-relay_*.deb\` / \`notin-relay-*.rpm\`: installs and starts the \`notin-relay\` systemd service (UDP 7720, data in \`/var/lib/notin-relay\`).
- \`notin-relay-*-linux.tar.gz\`: standalone binary.
- Docker: \`docker run -p 7720:7720/udp -v relay:/data ghcr.io/sergioribera/notin-relay:${tag#v} --port 7720 --data /data --advertise <public-host>\`
EOF
} > "$work/NOTES.md"

echo "→ assets"
ls -la "$dist"

flags=()
case "$tag" in
  *-*) flags+=(--prerelease --latest=false) ;;
  *) flags+=(--latest) ;;
esac

read -r -p "Publish $tag with these assets? [y/N] " answer
[ "$answer" = y ] || { echo "Left everything in $work"; exit 0; }

if ! gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
  gh release create "$tag" -R "$repo" --verify-tag --title "notinplus $tag" \
    --notes-file "$work/NOTES.md" "${flags[@]}"
fi

uploaded=$(gh release view "$tag" -R "$repo" --json assets -q '.assets[] | select(.state == "uploaded") | .name')
for file in "$dist"/*; do
  name=$(basename "$file")
  if grep -qxF "$name" <<<"$uploaded"; then
    echo "✓ $name (already there)"
    continue
  fi
  for attempt in 1 2 3 4 5; do
    if gh release upload "$tag" -R "$repo" --clobber "$file"; then
      echo "✓ $name"
      break
    fi
    if [ "$attempt" = 5 ]; then
      echo "✗ $name: giving up; run again with: $0 $run $tag $work" >&2
      exit 1
    fi
    echo "  retrying $name ($attempt/5)…"
    sleep $((attempt * 5))
  done
done
echo "Published. Scratch files: $work"
