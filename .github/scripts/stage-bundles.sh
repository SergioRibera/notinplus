#!/usr/bin/env bash
# Flattens a nix-bundle-app `bundles` linkFarm (<format>/<artifact>) into
# a release-ready directory.
#
#   stage-bundles.sh <bundles-dir> <out-dir> <nix-system>
#
# - Directory artifacts (`notinplus.app`) are archived: `ditto` on macOS
#   keeps symlinks/xattrs the bundle needs, tar.gz elsewhere.
# - Names lacking an arch get the system prepended, so the per-system
#   uploads can be merged into one release without overwriting each other.
set -euo pipefail
shopt -s nullglob

src=$1
out=$2
system=$3
mkdir -p "$out"

with_arch() {
  case "$1" in
    *x86_64* | *aarch64* | *amd64* | *arm64* | *x64*) echo "$1" ;;
    *) echo "$system-$1" ;;
  esac
}

for entry in "$src"/*/*; do
  name=$(basename "$entry")
  case "$name" in sign.sh) continue ;; esac
  real=$(readlink -f "$entry")
  if [ -d "$real" ]; then
    if command -v ditto >/dev/null 2>&1; then
      ditto -c -k --keepParent "$real" "$out/$(with_arch "$name").zip"
    else
      tar -C "$(dirname "$real")" -czf "$out/$(with_arch "$name").tar.gz" "$(basename "$real")"
    fi
  else
    cp -L "$real" "$out/$(with_arch "$name")"
  fi
done

chmod -R u+w "$out"
ls -la "$out"
