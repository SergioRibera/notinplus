#!/bin/sh
# cargo-release pre-release hook: derive the Android versionCode
# (`[app] build` in istmo.toml) from the version being released, so every
# release installs over the previous one.
#
#   major * 1_000_000 + minor * 1_000 + patch   (0.1.0 → 1000, 1.2.3 → 1002003)
#
# Pre-release suffixes are ignored: 0.2.0-rc.1 and 0.2.0 share a code.
set -eu

version=${NEW_VERSION:?run through cargo-release}
core=${version%%-*}
major=${core%%.*}
rest=${core#*.}
minor=${rest%%.*}
patch=${rest#*.}
code=$((major * 1000000 + minor * 1000 + patch))

manifest="${WORKSPACE_ROOT:-.}/istmo.toml"
if [ "${DRY_RUN:-false}" = true ]; then
  echo "would set [app] build = $code in $manifest"
  exit 0
fi
if grep -q '^build[[:space:]]*=' "$manifest"; then
  sed -i "s/^build[[:space:]]*=.*/build           = $code/" "$manifest"
else
  sed -i "/^name[[:space:]]*=/a build           = $code" "$manifest"
fi
echo "[app] build = $code"
