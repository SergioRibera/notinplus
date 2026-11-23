# notin-pack: the tool plugin authors run to build a `.ntp` package
# (crates/notin-plugin, feature `pack`). Released next to the app with the
# same version: it loads plugins with the SDK of that release.
#
# `cargoNix` is the desktop one (nix/per-crate.nix) or a cross one
# (nix/windows.nix); `exe` is the binary's file name on that target.
# nix-bundle-app turns `drv` into the release archive (tar.gz with its
# libraries, zip on Windows) that plugin-action downloads.
{
  pkgs,
  lib,
  cargoNix,
  exe ? "notin-pack",
  # The file the crate build leaves in `bin/`, when it differs from `exe`.
  built ? exe,
}:
let
  manifest = lib.importTOML ../Cargo.toml;
  pname = "notin-pack";
  # Versioned like the app it ships with, not like the SDK crate.
  inherit (manifest.package) version;

  crate = cargoNix.internal.buildRustCrateWithFeatures {
    packageId = "notin-plugin";
    features = [ "pack" ];
  };

  drv = pkgs.runCommand "${pname}-${version}" { meta.mainProgram = pname; } ''
    install -Dm755 ${crate}/bin/${built} $out/bin/${exe}
  '';

  info = {
    name = pname;
    inherit version;
    license = "MIT OR Apache-2.0";
    maintainer = "Sergio Ribera <contact@sergioribera.rs>";
    description = "Builds .ntp plugin packages for NotIn Plus";
    summary = "NotIn Plus plugin packaging tool";
    homepage = "https://github.com/SergioRibera/notinplus";
  };
in
{
  inherit pname drv info;
}
