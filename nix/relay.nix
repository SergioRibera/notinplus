# notin-relay — the always-on share relay, Linux server only.
#
# One derivation per crate, from the same `Cargo.nix` as the app
# (nix/per-crate.nix): as its own workspace member it resolves a much
# smaller feature set (no freya / skia / pdfium), and shares whatever
# crates both builds have in common.
{
  pkgs,
  lib,
  perCrate,
}:
let
  manifest = lib.importTOML ../tools/notin-relay/Cargo.toml;
  pname = manifest.package.name;
  inherit (manifest.package) version;

  # UDP (zudp); clients reach it through the invite address it prints.
  port = 7720;

  drv = perCrate.cargoNix.workspaceMembers.${pname}.build.overrideAttrs { meta.mainProgram = pname; };

  # Packaged installs run as a systemd service; the keypair and the
  # sealed shares live in the StateDirectory.
  info = {
    name = pname;
    inherit version;
    license = "MIT OR Apache-2.0";
    maintainer = "Sergio Ribera <contact@sergioribera.rs>";
    inherit (manifest.package) description;
    summary = "Store-and-forward relay for notinplus shares";
    homepage = "https://github.com/SergioRibera/notinplus";
    services = [
      {
        name = pname;
        exec = "/opt/${pname}/bin/${pname} --port ${toString port} --data /var/lib/${pname}";
        description = "notinplus share relay";
        after = [ "network-online.target" ];
        wants = [ "network-online.target" ];
        restart = "always";
        environment.RUST_LOG = "info";
        systemd.serviceOverrides = {
          DynamicUser = "yes";
          StateDirectory = pname;
          NoNewPrivileges = "yes";
          ProtectSystem = "strict";
          ProtectHome = "yes";
          PrivateTmp = "yes";
        };
      }
    ];
  };

  # `docker load < result` → `notin-relay:<version>`. Keypair + shares
  # persist in the `/data` volume; set `--advertise <public host>` when the
  # container IP isn't what clients should dial.
  image = pkgs.dockerTools.buildLayeredImage {
    name = pname;
    tag = version;
    config = {
      Entrypoint = [ (lib.getExe drv) ];
      Cmd = [
        "--port"
        (toString port)
        "--data"
        "/data"
      ];
      Env = [ "RUST_LOG=info" ];
      ExposedPorts."${toString port}/udp" = { };
      Volumes."/data" = { };
    };
  };
in
{
  inherit
    pname
    version
    drv
    info
    image
    ;
}
