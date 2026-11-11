# Android build of notinplus, in two halves.
#
# Rust: one derivation per crate and ABI (nix/cross-crates.nix) against the
# NDK from nixpkgs' androidenv, so attic serves every crate on its own. The
# app crate's build script also writes the Gradle side's generated sources
# (`android/.istmo`, plugin dispatchers); `jniLibs` ships them next to the
# `.so` files.
#
# Gradle: `devShell` brings the SDK, JDK and Gradle, and `cargoShim` stands
# in for cargo so the istmo Gradle plugin packages the prebuilt `.so`
# instead of compiling again (see `nix/android-gradle.sh`).
{
  pkgs,
  lib,
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
      ccTarget = lib.replaceStrings [ "-" ] [ "_" ] target;
      cc = "${ndkBin}/${clang}${toString minSdk}-clang";
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

      cargoNix = import ./cross-crates.nix {
        inherit pkgs lib;
        # nixpkgs' android platforms name the wrong rustc target and
        # `target_os`.
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
        rustcFlags = [ "-Clinker=${cc}" ];
        env = {
          "CC_${ccTarget}" = cc;
          "CXX_${ccTarget}" = "${cc}++";
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
        };
        nativeBuildInputs = [ pkgs.jdk17 ];
        crateOverrides = perCrate.crateOverrides // {
          freya-skia-bindings = _: {
            SKIA_BINARIES_URL = "file://${skia}/skia-binaries-{key}.tar.gz";
          };
          # nixpkgs links the build machine's zstd; Android compiles the bundled one.
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
      crate = cargoNix.workspaceMembers.notinplus.build;
    in
    # `$out/android` holds what the build script generated for Gradle; its
    # paths into the build sandbox are rewritten to `@src@` and resolved
    # against the checkout by the Gradle step.
    pkgs.runCommand "${pname}-android-${abi}-${version}" { nativeBuildInputs = [ pkgs.jq ]; } ''
      install -Dm755 ${lib.getLib crate}/lib/lib${pname}.so $out/lib/${abi}/lib${pname}.so
      cp -r --no-preserve=mode ${crate}/android $out/android
      json=$out/android/.istmo/istmo.json
      substituteInPlace $json --replace-fail "$(cat $out/android/.build-root)" "@src@"
      rm $out/android/.build-root
      # Plugin native sources live in each crate's source in the store
      # (nix/cross-crates.nix): carry them along.
      jq -r '.plugins[] | "\(.id) \(.native_dir)"' $json | while read -r id native; do
        case "$native" in /nix/store/*) ;; *) continue ;; esac
        mkdir -p $out/android/.istmo/plugins/$id/native
        cp -r --no-preserve=mode "$native" $out/android/.istmo/plugins/$id/native/android
        substituteInPlace $json \
          --replace-fail "$native" "@src@/android/.istmo/plugins/$id/native/android"
      done
    '';

  perAbi = lib.mapAttrs mkAbi abis;

  # Every ABI's `.so` under `lib/<abi>/`, plus one copy of the generated
  # Gradle sources (identical across ABIs).
  jniLibs = pkgs.runCommand "${pname}-android-jnilibs-${version}" { } ''
    mkdir -p $out/lib
    ${lib.concatMapStrings (abi: ''
      ln -s ${perAbi.${abi}}/lib/${abi} $out/lib/${abi}
    '') (builtins.attrNames perAbi)}
    ln -s ${perAbi.arm64-v8a}/android $out/android
  '';

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
  gradle = pkgs.writeShellApplication {
    name = "${pname}-android-gradle";
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

  devShell = pkgs.mkShell (gradleEnv // { packages = gradleInputs; });
in
{
  inherit
    jniLibs
    gradle
    devShell
    ;
  abis = builtins.attrNames abis;
}
