{
  description = "notinplus — Crossplatform Note App";

  nixConfig = {
    extra-substituters = [ "https://cache.sergioribera.rs/main" ];
    extra-trusted-public-keys = [ "main:vFI3N1JP9edRFwBwdk9ebUKTIPWK9R1ECbkdA7Q593M=" ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nix-bundle-app = {
      url = "github:SergioRibera/nix-bundle-app";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      crane,
      rust-overlay,
      nix-bundle-app,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            (import rust-overlay)
            nix-bundle-app.overlays.default
          ];
          config.allowUnfree = true;
        };
        lib = pkgs.lib;

        craneLib = (crane.mkLib pkgs).overrideToolchain (p: p.rust-bin.stable.latest.minimal);
        notinplus = import ./nix/package.nix {
          inherit
            pkgs
            lib
            craneLib
            system
            ;
        };
        inherit (notinplus) pname version;

        targets = import ./nix/targets.nix;
        bundler = pkgs.nixBundleApp;

        info = {
          name = pname;
          inherit version;
          license = "MIT OR Apache-2.0";
          maintainer = "Sergio Ribera <contact@sergioribera.rs>";
          description = "Crossplatform Note App";
          summary = "Crossplatform note-taking app with handwriting and PDF annotation";
          homepage = "https://github.com/SergioRibera/notinplus";
          bundleId = "rs.sergioribera.notinplus";
          minimumSystemVersion = "11.0";
          # Pinned so every future MSI upgrades the previous install.
          msiUpgradeCode = "19BFE419-C3D3-4DB6-BE04-BD4324279A4A";
          downloadUrl = "https://github.com/SergioRibera/notinplus/releases/download/v${version}";
          desktopEntries = [
            {
              name = "NotIn Plus";
              exec = "/opt/${pname}/bin/${pname} %F";
              categories = [
                "Office"
                "Utility"
              ];
            }
          ];
          flatpak = {
            runtimeVersion = "24.08";
            finishArgs = [
              "--share=ipc"
              "--share=network"
              "--socket=wayland"
              "--socket=fallback-x11"
              "--device=dri"
            ];
          };
        };

        # `tar.gz` → `tar_gz`: keeps attr names CLI-friendly (`.#bundle-tar_gz`).
        attrOf = format: lib.replaceStrings [ "." ] [ "_" ] format;

        # flatpak is a source kit finished by flatpak-builder in its own job,
        # so it's exposed as `flatpak-kit` instead of joining `bundles`.
        mkBundles =
          {
            drv,
            formats,
            info,
            name ? pname,
            target ? null,
          }:
          let
            built = lib.genAttrs (lib.remove "flatpak" formats) (
              format:
              bundler.bundle {
                inherit
                  drv
                  format
                  info
                  target
                  ;
              }
            );
          in
          lib.mapAttrs' (format: value: lib.nameValuePair "bundle-${attrOf format}" value) built
          // {
            bundles = pkgs.linkFarm "${name}-${info.version}-${system}-bundles" (
              lib.mapAttrsToList (name: path: { inherit name path; }) built
            );
          }
          // lib.optionalAttrs (builtins.elem "flatpak" formats) {
            flatpak-kit = bundler.bundle {
              inherit drv info target;
              format = "flatpak";
            };
          };

        nativeTarget = targets.desktop.${system} or null;
        nativeBundles = lib.optionalAttrs (nativeTarget != null && nativeTarget.kind == "nix") (mkBundles {
          drv = notinplus.unwrapped;
          inherit info;
          inherit (nativeTarget) formats;
        });

        # Windows: `cargo build` runs on a Windows runner (MSVC + prebuilt
        # Skia), the job uploads `notinplus.exe` + `pdfium.dll`, and this
        # wraps that directory into a derivation for nix-bundle-app. Impure
        # by necessity: `NOTINPLUS_WINDOWS_DIST=<dir> nix build --impure …`.
        windowsBundles =
          let
            dist = builtins.getEnv "NOTINPLUS_WINDOWS_DIST";
            drv =
              if dist == "" then
                throw "notinplus: set NOTINPLUS_WINDOWS_DIST to the Windows build dir and pass --impure"
              else
                pkgs.runCommand "${pname}-${version}-x86_64-windows" { } ''
                  mkdir -p $out/bin
                  cp -r ${
                    builtins.path {
                      path = dist;
                      name = "${pname}-windows-dist";
                    }
                  }/. $out/bin/
                '';
          in
          lib.mapAttrs' (name: value: lib.nameValuePair "windows-${name}" value) (mkBundles {
            inherit drv info;
            inherit (targets.desktop.x86_64-windows) formats;
            target = {
              arch = "x86_64";
              os = "windows";
            };
          });

        # notein-relay: `.#relay`, `.#relay-bundles` (deb/rpm/tar.gz with a
        # systemd unit) and `.#relay-image` (OCI tarball for `docker load`).
        relay = import ./nix/relay.nix {
          inherit pkgs lib craneLib;
          inherit (notinplus) src;
        };
        serverTarget = targets.server.${system} or null;
        relayPackages = lib.optionalAttrs (serverTarget != null) (
          lib.mapAttrs' (name: value: lib.nameValuePair "relay-${name}" value) (mkBundles {
            inherit (relay) drv info;
            inherit (serverTarget) formats;
            name = relay.pname;
          })
          // {
            relay = relay.drv;
            relay-cargoArtifacts = relay.cargoArtifacts;
            relay-image = relay.image;
          }
        );

        matrix =
          let
            entries = lib.mapAttrsToList (system: t: t // { inherit system; }) targets.desktop;
          in
          builtins.toJSON {
            server = lib.mapAttrsToList (system: t: {
              inherit system;
              inherit (t) runner dockerArch;
            }) targets.server;
            nix = map (t: { inherit (t) system runner; }) (builtins.filter (t: t.kind == "nix") entries);
            prebuilt = map (t: {
              inherit (t) system runner bundleRunner;
            }) (builtins.filter (t: t.kind == "prebuilt") entries);
          };
      in
      {
        packages = {
          default = notinplus.wrapped;
          ${pname} = notinplus.wrapped;
          "${pname}-unwrapped" = notinplus.unwrapped;
          inherit (notinplus) cargoArtifacts;
        }
        // nativeBundles
        // lib.optionalAttrs (system == "x86_64-linux") windowsBundles
        // relayPackages;

        apps.matrix = {
          type = "app";
          program = toString (pkgs.writeShellScript "notinplus-matrix" "echo '${matrix}'");
        };

        # Devshell — bring `just`, wayland/x11 dev libs for the desktop shell,
        # and the utilities the mobile builds shell out to.
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            just
            gcc
            claude-code-bin
            dioxus-cli
            pkg-config
            libGL
            wayland
            libxkbcommon

            libX11
            libXcursor
            libXrandr
            libXi

            # istmo-pen's Linux backend pulls `input` + `libudev-sys`
            # unconditionally on this target — provide both so cargo
            # can link the plugin without hunting for system libs.
            libinput
            udev
            # The camera plugin binds V4L2 headers with bindgen.
            llvmPackages.libclang.lib
            # freya-skia links against system freetype/fontconfig.
            freetype
            fontconfig
            # freya-pdf loads libpdfium at runtime via
            # `Pdfium::bind_to_system_library`. The upstream
            # `pdfium` derivation ships the prebuilt shared
            # library that pdfium-render expects.
            pdfium
            # Android tooling normally comes from Android Studio / a Docker image;
            # keep this shell focused on desktop dev + cargo.
            # iOS builds require Xcode + xcodegen on a macOS host.
          ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              libGL
              wayland
              libxkbcommon
              libX11
              libXcursor
              libXrandr
              libXi
              libinput
              udev
              freetype
              fontconfig
              pdfium
            ]
          );
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    );
}
