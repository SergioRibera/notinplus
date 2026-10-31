# Release matrix — single source of truth for what gets built where.
#
# `nix run .#matrix` serialises this into the JSON the release workflow
# fans out over, so adding a platform/format here is the only change
# needed to ship it.
{
  # The notinplus desktop app.
  #
  # kind = "nix"       → built and bundled natively on `runner` by Nix.
  # kind = "prebuilt"  → binary built by cargo on `runner` (no Nix there),
  #                      then bundled by Nix on `bundleRunner`.
  desktop = {
    x86_64-linux = {
      kind = "nix";
      runner = "ubuntu-latest";
      formats = [
        "appimage"
        "deb"
        "rpm"
        "tar.gz"
        "flatpak"
      ];
    };
    aarch64-linux = {
      kind = "nix";
      runner = "ubuntu-24.04-arm";
      formats = [
        "appimage"
        "deb"
        "rpm"
        "tar.gz"
      ];
    };
    aarch64-darwin = {
      kind = "nix";
      runner = "macos-latest";
      formats = [
        "app"
        "dmg"
        "pkg"
        "tar.gz"
      ];
    };
    # rust-skia publishes no `*-windows-gnu` binaries, so a mingw cross
    # build from Nix would compile Skia from source. MSVC on a Windows
    # runner downloads the prebuilt ones; Nix only does the packaging.
    x86_64-windows = {
      kind = "prebuilt";
      runner = "windows-latest";
      bundleRunner = "ubuntu-latest";
      formats = [
        "nsis"
        "msi"
        "zip"
      ];
    };
  };

  # notein-relay, Linux only. `dockerArch` is the OCI platform the image
  # is pushed under; the per-arch images are joined into one manifest.
  server = {
    x86_64-linux = {
      runner = "ubuntu-latest";
      formats = [
        "deb"
        "rpm"
        "tar.gz"
      ];
      dockerArch = "amd64";
    };
    aarch64-linux = {
      runner = "ubuntu-24.04-arm";
      formats = [
        "deb"
        "rpm"
        "tar.gz"
      ];
      dockerArch = "arm64";
    };
  };
}
