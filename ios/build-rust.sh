#!/bin/bash
# Xcode pre-build hook. Maps $PLATFORM_NAME + $ARCHS to a rust target
# triple, runs cargo, and stages the resulting .a into LIBRARY_SEARCH_PATHS.
set -euo pipefail

: "${PLATFORM_NAME:=iphonesimulator}"
: "${ARCHS:=arm64}"
: "${CONFIGURATION:=Debug}"
: "${PROJECT_DIR:=$(cd "$(dirname "$0")" && pwd)}"

WORKSPACE_DIR="$(cd "${PROJECT_DIR}/.." && pwd)"
STAGING_ROOT="${PROJECT_DIR}/build/rust"

profile_flag=""
profile_dir="debug"
if [[ "${CONFIGURATION}" == "Release" ]]; then
    profile_flag="--release"
    profile_dir="release"
fi

for arch in ${ARCHS}; do
    case "${PLATFORM_NAME}-${arch}" in
        iphoneos-arm64)         triple="aarch64-apple-ios" ;;
        iphonesimulator-arm64)  triple="aarch64-apple-ios-sim" ;;
        iphonesimulator-x86_64) triple="x86_64-apple-ios" ;;
        *)
            echo "unsupported PLATFORM_NAME/ARCH: ${PLATFORM_NAME}/${arch}" >&2
            exit 1
            ;;
    esac

    cargo build \
        --manifest-path "${WORKSPACE_DIR}/Cargo.toml" \
        -p notinplus \
        --target "${triple}" \
        ${profile_flag}

    src="${WORKSPACE_DIR}/target/${triple}/${profile_dir}/libnotinplus.a"
    dst_dir="${STAGING_ROOT}/${PLATFORM_NAME}/${arch}"
    mkdir -p "${dst_dir}"
    cp "${src}" "${dst_dir}/libnotinplus.a"
done
