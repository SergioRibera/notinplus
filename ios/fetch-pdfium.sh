#!/bin/bash
# Assembles `Vendor/pdfium.xcframework` from the per-slice tarballs
# published by `bblanchon/pdfium-binaries`. Wired as a preBuildScript
# in `project.yml` so xcodebuild pulls the framework on the first
# build (and re-uses the cached copy afterwards).
#
# bblanchon publishes one dylib per slice (device + simulator +
# catalyst). `xcodebuild -create-xcframework` bundles them into the
# xcframework Xcode expects. Pin `PDFIUM_RELEASE` to the same tag the
# Android Gradle task uses so behaviour stays consistent across
# platforms.
set -euo pipefail

PDFIUM_RELEASE="${PDFIUM_RELEASE:-chromium/8066}"
PROJECT_DIR="${PROJECT_DIR:-$(cd "$(dirname "$0")" && pwd)}"
VENDOR_DIR="${PROJECT_DIR}/Vendor"
XCFRAMEWORK_DIR="${VENDOR_DIR}/pdfium.xcframework"

if [[ -d "${XCFRAMEWORK_DIR}" ]]; then
    exit 0
fi

if ! command -v xcodebuild >/dev/null 2>&1; then
    echo "xcodebuild not on PATH — run this on a macOS host with Xcode installed" >&2
    exit 1
fi

mkdir -p "${VENDOR_DIR}"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

base_url="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE}"

fetch_slice() {
    local asset="$1"
    local dest="${tmp}/${asset%.tgz}"
    mkdir -p "${dest}"
    echo "Fetching ${base_url}/${asset}"
    curl -fL "${base_url}/${asset}" -o "${tmp}/${asset}"
    tar -xzf "${tmp}/${asset}" -C "${dest}"
}

fetch_slice "pdfium-ios-device-arm64.tgz"
fetch_slice "pdfium-ios-simulator-arm64.tgz"

xcodebuild -create-xcframework \
    -library  "${tmp}/pdfium-ios-device-arm64/lib/libpdfium.dylib"    \
    -headers  "${tmp}/pdfium-ios-device-arm64/include"                \
    -library  "${tmp}/pdfium-ios-simulator-arm64/lib/libpdfium.dylib" \
    -headers  "${tmp}/pdfium-ios-simulator-arm64/include"             \
    -output   "${XCFRAMEWORK_DIR}"
