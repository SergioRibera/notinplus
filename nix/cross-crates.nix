# One derivation per crate for a target other than the build machine.
#
# `Cargo.nix` picks each target's dependencies from the `stdenv.hostPlatform`
# it is given and builds build scripts and proc-macros with
# `pkgs.buildPackages`. Rather than a full nixpkgs cross package set (its
# compilers are built from source), it gets the native one with only the
# host platform swapped:
#
# - target crates: `buildRustCrate` on the native stdenv whose
#   `hostPlatform` is the target, so it passes `--target`; a `rustc`
#   wrapper adds the linker and link flags to those invocations only, and
#   `env` reaches every build (the C compilers build scripts use);
# - build scripts and proc-macros: the desktop builder (nix/per-crate.nix),
#   so they are the same store paths as the desktop build's.
{
  pkgs,
  lib,
  # From `lib.systems.elaborate`; `rust.rustcTarget` and `rust.platform`
  # are what rustc and `cfg(...)` see.
  platform,
  # `cfg(target_env)`: "msvc", "gnu" or "" (Android).
  targetEnv,
  # `CARGO_CFG_TARGET_FEATURE` for build scripts.
  targetFeatures ? "",
  # Toolchain with the target's standard library.
  toolchain,
  # Appended to every rustc invocation compiling for the target.
  rustcFlags,
  # Environment of every target crate build.
  env ? { },
  nativeBuildInputs ? [ ],
  # The desktop build's (nix/per-crate.nix), extended for the target.
  # Build scripts and proc-macros see them too, so they stay the desktop
  # build's store paths as long as the extension leaves those crates be.
  crateOverrides,
  # The desktop `buildRustCrateForPkgs`, for build scripts and proc-macros.
  buildRustCrateForBuild,
  # The desktop root crate (nix/per-crate.nix): its graph is reused for
  # build scripts and proc-macros.
  desktopCrate,
}:
let
  # `Cargo.nix` resolves features once, for the target, so a build script
  # dependency whose features only the build machine needs (rustix's
  # `linux-raw-sys`, say) would miss them. Whatever the desktop build
  # already compiled is taken from it instead: right features, and the
  # same store paths attic already holds.
  desktopGraph = lib.unique (
    lib.concatMap (crate: [ crate ] ++ crate.completeBuildDeps) (
      [ desktopCrate ] ++ desktopCrate.completeDeps
    )
  );
  desktopBuilt = lib.listToAttrs (map (crate: lib.nameValuePair crate.name crate) desktopGraph);
  reuseDesktop =
    builder:
    lib.setFunctionArgs (
      attrs: desktopBuilt."rust_${attrs.crateName}-${attrs.version}" or (builder attrs)
    ) { }
    // {
      override = args: reuseDesktop (builder.override args);
    };

  rustc = pkgs.symlinkJoin {
    name = "rustc-${platform.rust.rustcTarget}-${toolchain.version}";
    paths = [ toolchain ];
    postBuild = ''
      rm $out/bin/rustc
      cat > $out/bin/rustc <<EOF
      #!${pkgs.runtimeShell}
      # `buildRustCrate` keeps its link flags in `\$LINK`, which MSVC-style
      # linkers (lld-link) read as extra arguments.
      unset LINK
      for arg in "\$@"; do
        if [ "\$arg" = --target ]; then
          exec ${toolchain}/bin/rustc "\$@" ${lib.escapeShellArgs rustcFlags}
        fi
      done
      exec ${toolchain}/bin/rustc "\$@"
      EOF
      chmod +x $out/bin/rustc
    '';
    inherit (toolchain) version;
  };

  # `buildRustCrate` hardcodes a unix, gnu target for build scripts; the
  # exports below come after its own and win.
  family = lib.head platform.rust.platform.target-family;
  anchor = "export CARGO_CFG_TARGET_ENDIAN=";
  cfg = ''
    unset CARGO_CFG_UNIX CARGO_CFG_WINDOWS
    export CARGO_CFG_TARGET_FAMILY="${family}"
    export CARGO_CFG_${lib.toUpper family}=1
    export CARGO_CFG_TARGET_ENV="${targetEnv}"
    export CARGO_CFG_TARGET_FEATURE="${targetFeatures}"
  '';
  fixCfg =
    phase:
    if lib.hasInfix anchor phase then
      lib.replaceStrings [ anchor ] [ (cfg + anchor) ] phase
    else
      throw "nix/cross-crates.nix: buildRustCrate's build script environment changed";

  # What a crate exports to its dependents (`DEP_<links>_<key>`), as cargo
  # would: `buildRustCrate` adds a copy of every variable with the version
  # in its name, which istmo-build reads as a second plugin, and paths into
  # the build directory, gone once the crate is built, now point at its
  # source in the store.
  depEnvFix = ''
    if [ -f "$lib/env" ]; then
      ver=$(echo "$version" | tr '.+-' '___' | tr '[:lower:]' '[:upper:]')
      sed -i "/^export DEP_[A-Z0-9_]*_''${ver}_/d" "$lib/env"
      if [ -d "$src" ]; then
        sed -i "s|$NIX_BUILD_TOP/$sourceRoot|$src|g" "$lib/env"
      fi
    fi
  '';

  stdenv = pkgs.stdenv // {
    hostPlatform = platform;
    targetPlatform = platform;
    # No native `cc` as the linker; the rustc wrapper names the target's.
    hasCC = false;
    mkDerivation =
      args:
      pkgs.stdenv.mkDerivation (
        env
        // args
        // {
          nativeBuildInputs = (args.nativeBuildInputs or [ ]) ++ nativeBuildInputs;
          configurePhase = fixCfg args.configurePhase;
          postInstall = (args.postInstall or "") + depEnvFix;
        }
      );
  };

  buildRustCrate = pkgs.buildRustCrate.override {
    inherit stdenv rustc;
    cargo = toolchain;
  };

  hostPkgs = pkgs // {
    inherit stdenv;
    buildPackages = pkgs;
  };
in
import ./Cargo.nix {
  pkgs = hostPkgs;
  inherit stdenv;
  buildRustCrateForPkgs =
    p:
    if p.stdenv.hostPlatform.config == platform.config then
      buildRustCrate
    else
      reuseDesktop (buildRustCrateForBuild p);
  defaultCrateOverrides = crateOverrides;
  extraTargetFlags = {
    env = targetEnv;
  };
}
