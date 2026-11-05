# Workspace crates compiled in layers on top of crane's dependency build.
#
# Each layer is a derivation building the app against a source tree where
# only the crates of that layer and the ones below are real; the rest are
# crane's dummies. Cargo then finds the lower crates fresh in the next
# layer (same paths, sources at the store's fixed mtime), so a change in
# one crate rebuilds only its layer and the ones above, and attic serves
# the rest. Features match the final build because every layer builds the
# same target with the real manifests.
{ lib }:
let
  # Least to most often changed. A crate may only depend on crates of its
  # own or an earlier group (checked below); the app crate always builds
  # last, in the package derivation itself.
  groups = [
    [
      "notein-i18n"
      "notein-align"
      "notein-handwriting"
      "istmo-installer"
      "freya-canvas-bg"
      "freya-pdf"
    ]
    [
      "freya-markdown-editor"
      "freya-markdown-math"
      "freya-markdown-diagrams"
      "notein-sync"
      "notein-ui-kit"
    ]
    [
      "notein-plugin"
      "notein-sdk"
      "notein-notes"
      "notein-bible"
    ]
  ];

  layered = lib.concatLists groups;

  # One crate's sources as their own store path, so a layer's inputs
  # change with its crates only, not with the rest of the checkout.
  crateSrc =
    src: crate:
    builtins.path {
      name = "${crate}-source";
      path = "${src}/crates/${crate}";
    };

  pathDepsOf =
    crate:
    let
      manifest = lib.importTOML ../crates/${crate}/Cargo.toml;
      tables = [
        (manifest.dependencies or { })
        (manifest.build-dependencies or { })
      ]
      ++ lib.concatMap (t: [
        (t.dependencies or { })
        (t.build-dependencies or { })
      ]) (builtins.attrValues (manifest.target or { }));
    in
    lib.concatMap (
      t: lib.mapAttrsToList (name: _: name) (lib.filterAttrs (_: dep: dep ? path) t)
    ) tables;

  checked = lib.foldl' (
    below: group:
    let
      allowed = below ++ group;
      bad = lib.concatMap (
        crate: map (dep: "${crate} → ${dep}") (lib.subtractLists allowed (pathDepsOf crate))
      ) group;
    in
    if bad == [ ] then
      allowed
    else
      throw "nix/layers.nix: a crate depends on a later layer: ${lib.concatStringsSep ", " bad}"
  ) [ ] groups;
in
{
  # The `cargoArtifacts` the app's own build starts from: `cargoArtifacts`
  # (crane's dependency build) plus every layer, built with `args` (the
  # package's common crane arguments, minus the final-only bits).
  build =
    {
      craneLib,
      src,
      args,
      cargoArtifacts,
    }:
    assert checked == layered;
    (lib.foldl'
      (
        acc: group:
        let
          real = acc.real ++ group;
        in
        {
          inherit real;
          index = acc.index + 1;
          artifacts = craneLib.cargoBuild (
            args
            // {
              pname = "${args.pname}-layer${toString acc.index}";
              cargoArtifacts = acc.artifacts;
              src = craneLib.mkDummySrc {
                inherit src;
                extraDummyScript = lib.concatMapStrings (crate: ''
                  rm -rf $out/crates/${crate}
                  cp -r ${crateSrc src crate} $out/crates/${crate}
                '') real;
              };
              doCheck = false;
            }
          );
        }
      )
      {
        real = [ ];
        index = 1;
        artifacts = cargoArtifacts;
      }
      groups
    ).artifacts;
}
