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
  pname,
  version,
  perCrate,
}:
let
  target = "x86_64-pc-windows-msvc";

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

  # One derivation per crate (nix/cross-crates.nix).
  cargoNixWindows = import ./cross-crates.nix {
    inherit pkgs lib;
    platform = lib.systems.elaborate lib.systems.examples.x86_64-windows;
    targetEnv = "msvc";
    # `cc` picks the static CRT from this, and rust-skia its `-static` build.
    targetFeatures = "crt-static,fxsr,sse,sse2";
    toolchain = pkgs.rust-bin.stable.${perCrate.channel}.minimal.override { targets = [ target ]; };
    rustcFlags = [
      "-Clinker=${llvm.lld}/bin/lld-link"
      "-Ctarget-feature=+crt-static"
    ]
    ++ map (d: "-Lnative=${d}") libDirs;
    env = {
      "CC_${target}" = "${clangCl}/bin/clang-cl";
      "CXX_${target}" = "${clangCl}/bin/clang-cl";
      "AR_${target}" = "${llvm.llvm}/bin/llvm-lib";
      "RC_${target}" = "${llvm.llvm}/bin/llvm-rc";
    };
    crateOverrides = perCrate.crateOverrides // {
      # nixpkgs links the system (Linux) zstd; Windows compiles the bundled one.
      zstd-sys = _: { };
      # Its build script points the linker at the import libraries in its
      # own source dir, which is gone once the crate is built.
      windows_x86_64_msvc = _: {
        postInstall = ''
          mkdir -p $lib/lib/import
          cp lib/*.lib $lib/lib/import/
          for file in $lib/lib/link $lib/env; do
            if [ -f "$file" ]; then
              sed -i "s|$PWD/lib|$lib/lib/import|g" "$file"
            fi
          done
        '';
      };
      freya-skia-bindings = _: {
        SKIA_BINARIES_URL = "file://${skia}/skia-binaries-{key}.tar.gz";
      };
      notinplus =
        attrs:
        perCrate.crateOverrides.notinplus attrs
        // {
          # Only the exe ships; the cdylib and staticlib are the mobile entry.
          type = [ "rlib" ];
        };
    };
    buildRustCrateForBuild = perCrate.buildRustCrateForPkgs;
    desktopCrate = perCrate.crate;
  };

  crate = cargoNixWindows.workspaceMembers.notinplus.build;
in
{
  inherit xwinSdk;
  # The exe, next to the pdfium it loads.
  package =
    pkgs.runCommand "${pname}-windows-${version}"
      {
        meta = {
          description = "Crossplatform note-taking app with handwriting and PDF annotation";
          homepage = "https://github.com/SergioRibera/notinplus";
          license = with lib.licenses; [
            mit
            asl20
          ];
        };
      }
      ''
        install -Dm755 ${crate}/bin/${pname}.exe $out/bin/${pname}.exe
        install -Dm644 ${pdfium}/bin/pdfium.dll $out/bin/pdfium.dll
      '';
}
