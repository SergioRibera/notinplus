# Desktop build of notinplus, one derivation per crate (nix/per-crate.nix).
{
  pkgs,
  lib,
  system,
}:
let
  manifest = lib.importTOML ../Cargo.toml;
  pname = manifest.package.name;
  inherit (manifest.package) version;

  prebuilt = pkgs.callPackage ./prebuilt.nix { inherit system; };

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../build.rs
      ../istmo.toml
      ../assets
      ../locales
      ../crates
      ../src
      ../tools
    ];
  };

  # Mostly dlopen'd by winit / glutin / ash, so the Nix wrapper puts them
  # on LD_LIBRARY_PATH. Packaged bundles leave them to the host instead
  # (nix-bundle-app `info.hostLibs`), where the GPU driver stack must win.
  dlopenLibs = with pkgs; [
    libGL
    vulkan-loader
    wayland
    libxkbcommon
    libX11
    libXcursor
    libXrandr
    libXi
  ];

  buildInputs =
    lib.optionals pkgs.stdenv.hostPlatform.isLinux (
      with pkgs;
      [
        fontconfig
        freetype
        libxcb
      ]
      ++ dlopenLibs
    )
    ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];

  perCrate = import ./per-crate.nix {
    inherit
      pkgs
      lib
      src
      prebuilt
      buildInputs
      ;
  };
  unwrapped = perCrate.package;

  wrapped =
    if pkgs.stdenv.hostPlatform.isLinux then
      pkgs.runCommand "${pname}-${version}"
        {
          nativeBuildInputs = [ pkgs.makeWrapper ];
          meta.mainProgram = pname;
          passthru = { inherit unwrapped; };
        }
        ''
          mkdir -p $out/bin
          ln -s ${unwrapped}/lib $out/lib
          makeWrapper ${unwrapped}/bin/${pname} $out/bin/${pname} \
            --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath dlopenLibs}
        ''
    else
      unwrapped;
in
{
  inherit
    pname
    version
    unwrapped
    wrapped
    dlopenLibs
    prebuilt
    src
    perCrate
    ;
}
