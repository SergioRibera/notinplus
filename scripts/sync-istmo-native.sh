#!/bin/sh
# Pin istmo's native runtime to the istmo crates the app builds with:
# the Android runtime library and Gradle plugin, and the iOS Swift
# package. Their version is the one of `istmo-core` in Cargo.lock.
set -eu

root="${WORKSPACE_ROOT:-.}"
cd "$root"
version=$(awk '/^name = "istmo-core"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)
if [ -z "$version" ]; then
  echo "istmo-core is not in Cargo.lock" >&2
  exit 1
fi
if [ "${DRY_RUN:-false}" = true ]; then
  echo "would pin istmo's Android and iOS runtime to $version"
  exit 0
fi
sed -i "s|io.github.sergioribera:istmo-runtime:[^\"]*\"|io.github.sergioribera:istmo-runtime:$version\"|" android/app/build.gradle.kts
sed -i "s|id(\"io.github.sergioribera.istmo\") version \"[^\"]*\"|id(\"io.github.sergioribera.istmo\") version \"$version\"|" android/build.gradle.kts
sed -i "s|^\(    exactVersion: \).*|\1$version|" ios/project.yml
