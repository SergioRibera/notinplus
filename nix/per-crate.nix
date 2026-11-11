# Desktop build of notinplus with one derivation per crate.
#
# `Cargo.nix` (crate2nix, regenerated with `just cargo-nix` whenever
# Cargo.lock changes) lists every crate of the lock file; nixpkgs'
# `buildRustCrate` compiles each one in its own derivation, so a binary
# cache holds every crate separately and a change rebuilds only that crate
# and the ones depending on it.
#
# The compiler comes from rust-overlay at the channel `rust-toolchain.toml`
# pins, without its mobile targets: those are part of the toolchain's
# store path and with it of every crate's.
{
  pkgs,
  lib,
  src,
  prebuilt,
  buildInputs,
}:
let
  channel = (lib.importTOML ../rust-toolchain.toml).toolchain.channel;
  toolchain = pkgs.rust-bin.stable.${channel}.minimal;

  buildRustCrateForPkgs =
    p:
    p.buildRustCrate.override {
      rustc = toolchain;
      cargo = toolchain;
    };

  bindgen = {
    nativeBuildInputs = [ pkgs.rustPlatform.bindgenHook ];
  };

  # Cargo always sets these for build scripts; `buildRustCrate` does not.
  cargoEnv = {
    CARGO_ENCODED_RUSTFLAGS = "";
  };

  crateOverrides = pkgs.defaultCrateOverrides // {
    av-scenechange = _: cargoEnv // { nativeBuildInputs = [ pkgs.nasm ]; };
    rav1e = _: cargoEnv // { nativeBuildInputs = [ pkgs.nasm ]; };
    freya-skia-bindings = _: {
      SKIA_BINARIES_URL = "file://${prebuilt.skia}/skia-binaries-{key}.tar.gz";
      nativeBuildInputs = [ pkgs.pkg-config ];
      buildInputs = with pkgs; [
        fontconfig
        freetype
      ];
    };
    istmo-camera = _: bindgen;
    v4l2-sys-mit = _: bindgen;
    # cdylib: links Skia, whose GL and Wayland libraries must be found.
    notin-bible = _: { inherit buildInputs; };
    notinplus = _: {
      inherit src;
      inherit buildInputs;
      nativeBuildInputs = [ pkgs.pkg-config ];
    };
  };

  cargoNix = import ./Cargo.nix {
    inherit pkgs buildRustCrateForPkgs;
    defaultCrateOverrides = crateOverrides;
  };

  crate = cargoNix.workspaceMembers.notinplus.build;

  # `freya-pdf` probes `<exe>/../lib` for pdfium and resolves `<exe>`
  # through symlinks, so the binary is copied next to it; the store
  # reference also pulls pdfium into the closure nix-bundle-app copies.
  package = pkgs.runCommand "notinplus-${crate.version}" { meta.mainProgram = "notinplus"; } ''
    install -Dm755 ${crate}/bin/notinplus $out/bin/notinplus
    mkdir -p $out/lib
    for lib in ${prebuilt.pdfium}/lib/*pdfium*; do
      ln -s "$lib" $out/lib/
    done
  '';
in
{
  inherit
    channel
    toolchain
    cargoNix
    crate
    package
    crateOverrides
    buildRustCrateForPkgs
    ;
}
