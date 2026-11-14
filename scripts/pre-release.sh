#!/bin/sh
# cargo-release pre-release hook: everything the version bump touches
# besides the manifests, committed with `chore(release)`.
#
# - the Android versionCode (scripts/set-version-code.sh);
# - nix/Cargo.nix, which pins every crate's version: CI fails a release
#   whose Cargo.nix still names the previous one.
set -eu

here=$(dirname "$0")
"$here/set-version-code.sh"

root="${WORKSPACE_ROOT:-.}"
if [ "${DRY_RUN:-false}" = true ]; then
  echo "would regenerate $root/nix/Cargo.nix"
  exit 0
fi
cd "$root"
nix run nixpkgs#crate2nix -- generate -f Cargo.toml -o nix/Cargo.nix -h nix/crate-hashes.json
