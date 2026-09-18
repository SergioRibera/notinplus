set shell := ["bash", "-cu"]

# Desktop build (host toolchain).
build:
    cargo build --release

test:
    cargo test --all-targets

fmt:
    cargo fmt --all
    cargo clippy --all-targets -- -D warnings


# Android — assemble debug APK via Gradle, which drives cargo for every ABI.
build-android:
    cd android && ./gradlew assembleDebug

# Play Store internal release bundle. Requires ANDROID_RELEASE_* env.
release-android:
    cd android && ./gradlew bundleRelease



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
run:
    cargo run
