set shell := ["bash", "-cu"]

image := "sergioribera/rust-android:1.96-sdk-37.0"
mount := "-v $(pwd)/../:/src -v $HOME/.android:/root/.android -v gradle-cache:/root/.gradle -v cargo-cache:/root/.cargo"

# Desktop build (host toolchain).
build:
    cargo build --release

test:
    cargo test --all-targets

fmt:
    cargo fmt --all
    cargo clippy --all-targets -- -D warnings

# Nuke every `libnotinplus.so` cargo/gradle keeps around.
#
# `gradle clean` alone only wipes `android/*/build/`; the cargo `target/`
# still holds stale `.so`s that Gradle then repackages into the next APK
# (its stage tasks stay up-to-date when the cargo output timestamp
# hasn't moved). Explicit `find … -delete` for the `.so`s so a bare
# `just clean` guarantees the next build re-links from scratch.
clean:
    sudo find . -type f -name "libnotinplus.so" -delete
    docker run --rm -it {{mount}} \
        -w /src/notinplus/android \
        --entrypoint bash {{image}} \
        -c 'gradle clean --no-daemon'

# Android — assemble debug APK via Gradle, which drives cargo for every ABI.
build-android:
    docker run --rm -it {{mount}} \
        -w /src/notinplus/android \
        --entrypoint bash {{image}} \
        -c 'gradle :app:assembleDebug --no-daemon'

# Play Store internal release bundle. Requires ANDROID_RELEASE_* env.
release-android:
    docker run --rm -it {{mount}} \
        -w /src/notinplus/android \
        --entrypoint bash {{image}} \
        -c 'gradle :app:bundleRelease --no-daemon'

install-android:
    adb install -r android/app/build/outputs/apk/debug/app-debug.apk

# iOS — regenerate the .xcodeproj (idempotent) and build for the simulator.
build-ios:
    cd ios && xcodegen generate && \
        xcodebuild -project NotinplusApp.xcodeproj \
                   -scheme  NotinplusApp \
                   -destination 'generic/platform=iOS Simulator' build

# Desktop distribution bundles via nix-bundle-app.
bundle-desktop:
    nix build .#bundle

# Run the desktop shell (uses the mock runtime + emulator thread).
android: (build-android) (install-android)

# Run the desktop shell (uses the mock runtime + emulator thread).
run:
    cargo run
