#!/usr/bin/env bash
# The `publish` job of release.yml, run by hand over a release run whose
# artifacts all built.
#
#   publish-local.sh <run-id> <tag>        # e.g. publish-local.sh 1234567890 v0.1.0-alpha.1
#
# Run from the notinplus checkout, with `gh` logged in.
set -euo pipefail

run=$1
tag=$2
repo=SergioRibera/notinplus
work=$(mktemp -d)
dist="$work/dist"
mkdir -p "$dist"

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

{
  cat "$work/CHANGES.md"
  cat <<EOF

## Install

- **Android**: \`notinplus-*-android-universal.apk\` works everywhere; the \`arm64-v8a\` / \`x86_64\` APKs are smaller.
- **Linux**: AppImage, \`.deb\`, \`.rpm\`, \`.flatpak\` or \`.tar.gz\`.
- **Nix**: \`nix run github:SergioRibera/notinplus/$tag\` (binary cache: \`cache.sergioribera.rs/main\`).
- **macOS**: \`.dmg\`, \`.pkg\` or the zipped \`.app\` (unsigned: right-click → Open the first time).
- **Windows**: \`-setup.exe\` (NSIS) or \`.msi\`.

### Relay server (\`notein-relay\`, Linux)

- \`notein-relay_*.deb\` / \`notein-relay-*.rpm\`: installs and starts the \`notein-relay\` systemd service (UDP 7720, data in \`/var/lib/notein-relay\`).
- \`notein-relay-*-linux.tar.gz\`: standalone binary.
- Docker: \`docker run -p 7720:7720/udp -v relay:/data ghcr.io/sergioribera/notein-relay:${tag#v} --port 7720 --data /data --advertise <public-host>\`
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

gh release create "$tag" -R "$repo" --verify-tag --title "notinplus $tag" \
  --notes-file "$work/NOTES.md" "${flags[@]}" "$dist"/*
echo "Published. Scratch files: $work"
