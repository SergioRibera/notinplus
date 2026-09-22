{
  description = "notinplus — Crossplatform Note App";

  inputs = {
    nixpkgs.url      = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url  = "github:numtide/flake-utils";
    nix-bundle-app.url = "github:SergioRibera/nix-bundle-app";
  };

  outputs = { nixpkgs, flake-utils, nix-bundle-app, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in {
        # Devshell — bring `just`, wayland/x11 dev libs for the desktop shell,
        # and the utilities the mobile builds shell out to.
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            just
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
            # freya-skia links against system freetype/fontconfig.
            freetype
            fontconfig
            # Android tooling normally comes from Android Studio / a Docker image;
            # keep this shell focused on desktop dev + cargo.
            # iOS builds require Xcode + xcodegen on a macOS host.
          ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
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
          ]);
        };

        
        # nix-bundle-app packaging — one derivation per format. The
        # `bundle` combinator emits a `release/` tree with SHA256SUMS
        # ready to upload to GitHub Releases.
        packages = let
          bundler = nix-bundle-app.lib.mkLib pkgs;
          # TODO: replace with your `crane` / cargo build derivation. This
          # placeholder wraps `pkgs.hello` so `nix build .#bundle` works
          # out of the box and can be swapped incrementally.
          drv = pkgs.hello;
          info = {
            name        = "notinplus";
            version     = "0.1.0";
            license     = "MIT OR Apache-2.0";
            maintainer  = "Sergio Ribera <contact@sergioribera.rs>";
            description = "Crossplatform Note App";
            desktopEntries = [{
              name       = "NotIn Plus";
              exec       = "/opt/notinplus/bin/notinplus %F";
              categories = [ "Utility" ];
            }];
          };
        in {
          # Single-format shortcuts.
          deb      = bundler.bundle { inherit drv info; format = "deb"; };
          rpm      = bundler.bundle { inherit drv info; format = "rpm"; };
          appimage = bundler.bundle { inherit drv info; format = "appimage"; };
          dmg      = bundler.bundle { inherit drv info; format = "dmg"; };
          msi      = bundler.bundle { inherit drv info; format = "msi"; };

          # Cross-platform release: every format + install.sh + SHA256SUMS.
          bundle = bundler.release {
            inherit info;
            releaseUrl = "https://github.com/SergioRibera/notinplus/releases/download/v\${VERSION}";
            matrix = {
              "x86_64-linux"   = { inherit drv; formats = [ "tar.gz" "deb" "rpm" "appimage" ]; };
              "aarch64-linux"  = { inherit drv; formats = [ "tar.gz" ]; };
              "x86_64-darwin"  = { inherit drv; formats = [ "tar.gz" "dmg" ]; };
              "aarch64-darwin" = { inherit drv; formats = [ "tar.gz" "dmg" ]; };
              "x86_64-windows" = { inherit drv; formats = [ "zip" "msi" ]; };
            };
          };
        };
        

        formatter = pkgs.nixfmt-rfc-style;
      });
}
