# Cross build of notinplus for `x86_64-pc-windows-msvc` from Linux.
#
# rust-skia only publishes prebuilt Skia for the MSVC ABI and Skia's own
# GN files hardwire the MSVC toolchain on Windows, so a mingw build is
# off the table. Instead the MSVC target is linked here with LLVM:
# clang-cl + lld-link against the Windows SDK and CRT that `xwin`
# downloads from Microsoft (fixed-output, versions pinned below). Same
# setup as sismo's viewer.
{
  pkgs,
  lib,
  crane,
  src,
  pname,
  version,
}:
let
  target = "x86_64-pc-windows-msvc";
  targetEnv = "X86_64_PC_WINDOWS_MSVC";

  toolchain = pkgs.rust-bin.stable.latest.minimal.override { targets = [ target ]; };
  craneLib = (crane.mkLib pkgs).overrideToolchain (_: toolchain);
  llvm = pkgs.llvmPackages;

  # Windows SDK + Visual C++ CRT headers and import libraries. Pinned so the
  # output hash stays put; bump both together after re-running
  # `xwin --accept-license list` and refreshing `outputHash`.
  xwinSdk = pkgs.stdenvNoCC.mkDerivation {
    pname = "xwin-sdk";
    version = "10.0.26100-14.44.17.14";
    dontUnpack = true;
    nativeBuildInputs = [
      pkgs.xwin
      pkgs.cacert
    ];
    SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
    buildCommand = ''
      export HOME=$TMPDIR
      xwin --accept-license --arch x86_64 --cache-dir $TMPDIR/xwin \
        --manifest-version 17 --sdk-version 10.0.26100 --crt-version 14.44.17.14 \
        splat --output $TMPDIR/splat
      # xwin links from its cache into the output; keep both on one filesystem.
      cp -r $TMPDIR/splat $out
    '';
    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-UFQjsFVBwcF/9e9tVFoG0Z1JySxyTnFqoaRwr/tUWzA=";
  };

  # `freya-skia-bindings` downloads its prebuilt archive; the sandbox has
  # none, so it is pinned here. Same hash/features scheme as prebuilt.nix.
  # `-static`: built against the static CRT, matching `+crt-static` below so
  # the exe needs no VC++ redistributable.
  skiaKey = "746613b43b596b9d2114-${target}-ganesh-gl-jpegd-jpege-svg-textlayout-vulkan-webpd-webpe-static";
  skia = pkgs.runCommand "skia-binaries-0.104.0-${target}" { } ''
    mkdir -p $out
    ln -s ${
      pkgs.fetchurl {
        url = "https://github.com/marc2332/rust-skia/releases/download/0.104.0/skia-binaries-${skiaKey}.tar.gz";
        hash = "sha256-FDNepZ/1GyQ62bYJp/p/+yLC/nKYQYwav1ULoEJIvMY=";
      }
    } $out/skia-binaries-${skiaKey}.tar.gz
  '';

  # Same `chromium/7881` pdfium the other platforms pin; `freya-pdf` finds
  # `pdfium.dll` next to the exe.
  pdfium = pkgs.runCommand "pdfium-7881-win-x64" { } ''
    mkdir -p $out
    tar -xzf ${
      pkgs.fetchurl {
        url = "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-win-x64.tgz";
        hash = "sha256-c8wN5jisIJXnRFv1ajggClt8fKDp9LoURZjyRXN3rAg=";
      }
    } -C $out
  '';

  # clang-cl with the MSVC include tree, for the build scripts that compile
  # C for the target (sqlite, …).
  clangCl = pkgs.writeShellScriptBin "clang-cl" ''
    exec ${llvm.clang-unwrapped}/bin/clang-cl --target=${target} \
      /imsvc ${xwinSdk}/crt/include \
      /imsvc ${xwinSdk}/sdk/include/ucrt \
      /imsvc ${xwinSdk}/sdk/include/um \
      /imsvc ${xwinSdk}/sdk/include/shared \
      "$@"
  '';

  libDirs = [
    "${xwinSdk}/crt/lib/x86_64"
    "${xwinSdk}/sdk/lib/um/x86_64"
    "${xwinSdk}/sdk/lib/ucrt/x86_64"
  ];

  commonArgs = {
    inherit src pname version;
    # Same `-frandom-seed` in every layer: bindgen reruns (and with it
    # skia and everything above) whenever BINDGEN_EXTRA_CLANG_ARGS, which
    # carries NIX_CFLAGS_COMPILE, changes. The default seed is $out.
    NIX_OUTPATH_USED_AS_RANDOM_SEED = pname;
    strictDeps = true;
    cargoLock = ../Cargo.lock;
    cargoToml = ../Cargo.toml;
    cargoExtraArgs = "--locked --bin ${pname}";
    doCheck = false;

    CARGO_BUILD_TARGET = target;
    "CARGO_TARGET_${targetEnv}_LINKER" = "${llvm.lld}/bin/lld-link";
    "CARGO_TARGET_${targetEnv}_RUSTFLAGS" = lib.concatStringsSep " " (
      [ "-Ctarget-feature=+crt-static" ] ++ map (d: "-Lnative=${d}") libDirs
    );
    "CC_${target}" = "${clangCl}/bin/clang-cl";
    "CXX_${target}" = "${clangCl}/bin/clang-cl";
    "AR_${target}" = "${llvm.llvm}/bin/llvm-lib";
    # istmo-build compiles the `.exe` icon resource with embed-resource.
    "RC_${target}" = "${llvm.llvm}/bin/llvm-rc";

    SKIA_BINARIES_URL = "file://${skia}/skia-binaries-{key}.tar.gz";
  };

  # Third-party dependencies, then the workspace crates in layers
  # (nix/layers.nix); the package build only compiles the app crate.
  cargoArtifacts = (import ./layers.nix { inherit lib; }).build {
    inherit craneLib src;
    args = commonArgs;
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;
  };
in
{
  inherit xwinSdk cargoArtifacts;
  package = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;
      # The lib target is also a cdylib + staticlib (mobile entry); only
      # the exe ships, next to the pdfium it loads.
      installPhaseCommand = ''
        install -Dm755 target/${target}/release/${pname}.exe $out/bin/${pname}.exe
        install -Dm644 ${pdfium}/bin/pdfium.dll $out/bin/pdfium.dll
      '';
      meta = {
        description = "Crossplatform note-taking app with handwriting and PDF annotation";
        homepage = "https://github.com/SergioRibera/notinplus";
        license = with lib.licenses; [
          mit
          asl20
        ];
      };
    }
  );
}
