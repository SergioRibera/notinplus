# Android build of notinplus, in two halves.
#
# Rust: one crane build per ABI against the NDK from nixpkgs' androidenv,
# split like the desktop one into a dependency layer and the app crate so
# attic serves the dependencies until Cargo.lock moves. The build script
# also writes the Gradle side's generated sources (`android/.istmo`, plugin
# dispatchers); `jniLibs` ships them next to the `.so` files.
#
# Gradle: `devShell` brings the SDK, JDK and Gradle, and `cargoShim` stands
# in for cargo so the istmo Gradle plugin packages the prebuilt `.so`
# instead of compiling again (see `nix/android-gradle.sh`).
{
  pkgs,
  lib,
  crane,
  src,
  pname,
  version,
  perCrate,
}:
let
  ndkVersion = "29.0.14206865";
  buildToolsVersion = "36.1.0";
  # Matches `[min_versions] android` in istmo.toml.
  minSdk = 26;

  abis = {
    arm64-v8a = {
      target = "aarch64-linux-android";
      arch = "aarch64";
      features = "neon";
      clang = "aarch64-linux-android";
      skiaHash = "sha256-0SUm0ZnodtCHd/hlNqilNP+W6+vqdMGYP5CslxbJV/I=";
    };
    x86_64 = {
      target = "x86_64-linux-android";
      arch = "x86_64";
      features = "fxsr,sse,sse2";
      clang = "x86_64-linux-android";
      skiaHash = "sha256-3/dViGgFQBdLGmwvxb895A9rrQvPgPgBZjalu8OSBHU=";
    };
  };

  androidComposition = pkgs.androidenv.composeAndroidPackages {
    platformVersions = [
      "34"
      "36"
    ];
    # 34.0.0: the default of AGP 8.2, which it would try to install.
    buildToolsVersions = [
      "34.0.0"
      buildToolsVersion
    ];
    includeNDK = true;
    ndkVersions = [ ndkVersion ];
    includeEmulator = false;
    includeSystemImages = false;
    includeSources = false;
  };
  sdk = androidComposition.androidsdk;
  sdkRoot = "${sdk}/libexec/android-sdk";
  ndk = "${sdkRoot}/ndk/${ndkVersion}";
  ndkBin = "${ndk}/toolchains/llvm/prebuilt/linux-x86_64/bin";

  toolchain = pkgs.rust-bin.stable.latest.minimal.override {
    targets = lib.mapAttrsToList (_: a: a.target) abis;
  };
  craneLib = (crane.mkLib pkgs).overrideToolchain (_: toolchain);

  # `istmo-build` only generates the Kotlin side when an `android/` folder
  # sits next to the crate; `[app] android_package` in istmo.toml names
  # the package, so an empty one is enough and Gradle edits never rebuild
  # the Rust half.
  withAndroidDir = ''
    mkdir -p android/app
  '';

  mkAbi =
    abi:
    {
      target,
      arch,
      features,
      clang,
      skiaHash,
    }:
    let
      envTarget = lib.toUpper (lib.replaceStrings [ "-" ] [ "_" ] target);
      ccTarget = lib.replaceStrings [ "-" ] [ "_" ] target;
      skiaKey = "746613b43b596b9d2114-${target}-ganesh-gl-jpegd-jpege-svg-textlayout-webpd-webpe";
      skia = pkgs.runCommand "skia-binaries-0.104.0-${target}" { } ''
        mkdir -p $out
        ln -s ${
          pkgs.fetchurl {
            url = "https://github.com/marc2332/rust-skia/releases/download/0.104.0/skia-binaries-${skiaKey}.tar.gz";
            hash = skiaHash;
          }
        } $out/skia-binaries-${skiaKey}.tar.gz
      '';

      commonArgs = {
        inherit src version;
        pname = "${pname}-android-${abi}";
        # Same `-frandom-seed` in every layer (see nix/package.nix).
        NIX_OUTPATH_USED_AS_RANDOM_SEED = pname;
        strictDeps = true;
        cargoLock = ../Cargo.lock;
        cargoToml = ../Cargo.toml;
        cargoExtraArgs = "--locked --lib";
        doCheck = false;

        CARGO_BUILD_TARGET = target;
        "CARGO_TARGET_${envTarget}_LINKER" = "${ndkBin}/${clang}${toString minSdk}-clang";
        "CC_${ccTarget}" = "${ndkBin}/${clang}${toString minSdk}-clang";
        "CXX_${ccTarget}" = "${ndkBin}/${clang}${toString minSdk}-clang++";
        "AR_${ccTarget}" = "${ndkBin}/llvm-ar";
        "RANLIB_${ccTarget}" = "${ndkBin}/llvm-ranlib";
        ANDROID_NDK_ROOT = ndk;
        ANDROID_NDK_HOME = ndk;
        # `mundy`'s build script compiles and dexes Java helpers.
        ANDROID_HOME = sdkRoot;
        ANDROID_SDK_ROOT = sdkRoot;
        ANDROID_JAR = "${sdkRoot}/platforms/android-36/android.jar";
        ANDROID_BUILD_TOOLS_VERSION = buildToolsVersion;
        JAVA_HOME = pkgs.jdk17.home;
        nativeBuildInputs = [ pkgs.jdk17 ];

        SKIA_BINARIES_URL = "file://${skia}/skia-binaries-{key}.tar.gz";
      };

      # Third-party dependencies, then the workspace crates in layers
      # (nix/layers.nix); `lib'` only compiles the app crate.
      cargoArtifacts = (import ./layers.nix { inherit lib; }).build {
        inherit craneLib src;
        args = commonArgs;
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      };

      lib' = craneLib.buildPackage (
        commonArgs
        // {
          inherit cargoArtifacts;
          preBuild = withAndroidDir;
          # `$out/android` holds what the build script generated for
          # Gradle; its paths into this sandbox are rewritten to `@src@`
          # and resolved against the checkout by the Gradle step.
          installPhaseCommand = ''
            install -Dm755 target/${target}/release/lib${pname}.so $out/lib/${abi}/lib${pname}.so
            cp -r android $out/android
            json=$out/android/.istmo/istmo.json
            substituteInPlace $json --replace-fail "$PWD" "@src@"
            # Plugins from crates.io / git live in the vendor dir, whose
            # references crane strips from outputs: carry their native
            # sources along instead.
            for native in $(jq -r '.plugins[].native_dir' $json | grep '^/nix/store/'); do
              crate=''${native%/native/android}
              name=$(basename "$crate")
              mkdir -p $out/android/.istmo/plugins/$name/native
              cp -r --no-preserve=mode "$native" $out/android/.istmo/plugins/$name/native/android
              substituteInPlace $json --replace-fail "$crate/" "@src@/android/.istmo/plugins/$name/"
            done
          '';
          nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.jq ];
        }
      );

      # One derivation per crate (nix/cross-crates.nix).
      cargoNixAndroid = import ./cross-crates.nix {
        inherit pkgs lib;
        platform = lib.systems.elaborate {
          config = "${arch}-unknown-linux-android";
          rust = {
            rustcTarget = target;
            rustcTargetSpec = target;
            platform = {
              inherit arch;
              os = "android";
              vendor = "unknown";
              target-family = [ "unix" ];
            };
          };
        };
        targetEnv = "";
        targetFeatures = features;
        toolchain = pkgs.rust-bin.stable.${perCrate.channel}.minimal.override { targets = [ target ]; };
        rustcFlags = [ "-Clinker=${commonArgs."CARGO_TARGET_${envTarget}_LINKER"}" ];
        env = lib.filterAttrs (
          name: _:
          lib.hasPrefix "ANDROID_" name
          || lib.elem name [
            "JAVA_HOME"
            "CC_${ccTarget}"
            "CXX_${ccTarget}"
            "AR_${ccTarget}"
            "RANLIB_${ccTarget}"
          ]
        ) commonArgs;
        nativeBuildInputs = [ pkgs.jdk17 ];
        crateOverrides = perCrate.crateOverrides // {
          freya-skia-bindings = _: {
            SKIA_BINARIES_URL = "file://${skia}/skia-binaries-{key}.tar.gz";
          };
          zstd-sys = _: { };
          notinplus =
            attrs:
            perCrate.crateOverrides.notinplus attrs
            // {
              # The `.so` the app loads; the rlib and staticlib are unused here.
              type = [ "cdylib" ];
              preConfigure = withAndroidDir;
              # What the build script generated for Gradle.
              postInstall = ''
                cp -r android $out/android
                echo "$PWD" > $out/android/.build-root
              '';
            };
        };
        buildRustCrateForBuild = perCrate.buildRustCrateForPkgs;
        desktopCrate = perCrate.crate;
      };
      crate = cargoNixAndroid.workspaceMembers.notinplus.build;

      perCrateLib =
        pkgs.runCommand "${pname}-android-${abi}-${version}" { nativeBuildInputs = [ pkgs.jq ]; }
          ''
            install -Dm755 ${lib.getLib crate}/lib/lib${pname}.so $out/lib/${abi}/lib${pname}.so
            cp -r --no-preserve=mode ${crate}/android $out/android
            json=$out/android/.istmo/istmo.json
            substituteInPlace $json --replace-fail "$(cat $out/android/.build-root)" "@src@"
            rm $out/android/.build-root
            # Plugin native sources live in each crate's source in the store
            # (nix/cross-crates.nix): carry them along, as the crane build does.
            jq -r '.plugins[] | "\(.id) \(.native_dir)"' $json | while read -r id native; do
              case "$native" in /nix/store/*) ;; *) continue ;; esac
              mkdir -p $out/android/.istmo/plugins/$id/native
              cp -r --no-preserve=mode "$native" $out/android/.istmo/plugins/$id/native/android
              substituteInPlace $json \
                --replace-fail "$native" "@src@/android/.istmo/plugins/$id/native/android"
            done
          '';
    in
    {
      inherit cargoArtifacts crate perCrateLib;
      lib = lib';
    };

  perAbi = lib.mapAttrs mkAbi abis;

  # Every ABI's `.so` under `lib/<abi>/`, plus one copy of the generated
  # Gradle sources (identical across ABIs).
  mkJniLibs =
    name: libOf:
    pkgs.runCommand "${pname}-android-${name}-${version}" { } ''
      mkdir -p $out/lib
      ${lib.concatMapStrings (abi: ''
        ln -s ${libOf perAbi.${abi}}/lib/${abi} $out/lib/${abi}
      '') (builtins.attrNames perAbi)}
      ln -s ${libOf perAbi.arm64-v8a}/android $out/android
    '';
  jniLibs = mkJniLibs "jnilibs" (p: p.lib);
  # Same layout from the per-crate build.
  jniLibsPerCrate = mkJniLibs "jnilibs-per-crate" (p: p.perCrateLib);

  cargoShim = pkgs.writeShellApplication {
    name = "cargo";
    text = builtins.readFile ./android-cargo-shim.sh;
  };

  gradleEnv = {
    ANDROID_HOME = sdkRoot;
    ANDROID_SDK_ROOT = sdkRoot;
    ANDROID_NDK_HOME = ndk;
    JAVA_HOME = pkgs.jdk17.home;
    # AGP otherwise downloads a glibc-linked aapt2 from Maven.
    AAPT2 = "${sdkRoot}/build-tools/${buildToolsVersion}/aapt2";
    ISTMO_CARGO = lib.getExe cargoShim;
  };
  gradleInputs = [
    sdk
    pkgs.jdk17
    pkgs.gradle_8
    pkgs.gnused
    pkgs.coreutils
  ];

  # `nix run .#android-gradle -- :app:assembleRelease [gradle args]`, from
  # the repo root: builds (or fetches from attic) every ABI, drops the
  # generated Gradle sources into `android/` and runs Gradle on them.
  mkGradle =
    name: jniLibs:
    pkgs.writeShellApplication {
      name = "${pname}-${name}";
      runtimeInputs = gradleInputs;
      runtimeEnv = gradleEnv // {
        NOTINPLUS_JNI_DIR = "${jniLibs}/lib";
      };
      text = ''
        [ -f android/settings.gradle.kts ] || { echo "run from the notinplus checkout root" >&2; exit 1; }
        cp -r --no-preserve=mode ${jniLibs}/android/. android/
        sed -i "s|@src@|$PWD|g" android/.istmo/istmo.json
        # Newer than Cargo.lock / istmo.toml, so the plugin trusts it
        # instead of refreshing it through `cargo check`.
        touch android/.istmo/istmo.json
        cd android
        exec gradle --no-daemon -Pandroid.aapt2FromMavenOverride="$AAPT2" "$@"
      '';
    };
  gradle = mkGradle "android-gradle" jniLibs;
  gradlePerCrate = mkGradle "android-gradle-per-crate" jniLibsPerCrate;

  devShell = pkgs.mkShell (gradleEnv // { packages = gradleInputs; });
in
{
  inherit
    jniLibs
    jniLibsPerCrate
    gradle
    gradlePerCrate
    devShell
    ;
  abis = builtins.attrNames abis;
  cargoArtifacts = pkgs.linkFarm "${pname}-android-cargoArtifacts-${version}" (
    lib.mapAttrsToList (abi: p: {
      name = abi;
      path = p.cargoArtifacts;
    }) perAbi
  );
}
