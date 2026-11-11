# Desktop build of notinplus (crane).
{
  pkgs,
  lib,
  craneLib,
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

    nativeBuildInputs = [
      pkgs.pkg-config
    ]
    # The camera plugin binds V4L2 headers with bindgen.
    ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.rustPlatform.bindgenHook ];
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

    SKIA_BINARIES_URL = "file://${prebuilt.skia}/skia-binaries-{key}.tar.gz";
  };

  # Third-party dependencies, then the workspace crates in layers
  # (nix/layers.nix); the package build only compiles the app crate.
  cargoArtifacts = (import ./layers.nix { inherit lib; }).build {
    inherit craneLib src;
    args = commonArgs;
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;
  };

  unwrapped = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;
      # The lib target is also a cdylib + staticlib (mobile entry); only
      # the desktop binary ships.
      installPhaseCommand = ''
        install -Dm755 target/release/${pname} $out/bin/${pname}
      '';
      # `freya-pdf` probes `<exe>/../lib` for pdfium, and the store
      # reference pulls it into the closure nix-bundle-app copies from.
      postInstall = ''
        mkdir -p $out/lib
        for lib in ${prebuilt.pdfium}/lib/*pdfium*; do
          ln -s "$lib" $out/lib/
        done
      '';
      meta.mainProgram = pname;
    }
  );

  # Same desktop binary, one derivation per crate (nix/per-crate.nix).
  perCrate = import ./per-crate.nix {
    inherit
      pkgs
      lib
      src
      prebuilt
      ;
    inherit (commonArgs) buildInputs;
  };

  wrap =
    unwrapped:
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

  wrapped = wrap unwrapped;
  perCrateWrapped = wrap perCrate.package;
in
{
  inherit
    pname
    version
    cargoArtifacts
    unwrapped
    wrapped
    dlopenLibs
    prebuilt
    src
    perCrate
    perCrateWrapped
    ;
}
