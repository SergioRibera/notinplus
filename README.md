# notinplus

Crossplatform Note App


Multi-platform application built on [istmo](https://crates.io/crates/istmo).


Author: Sergio Ribera <contact@sergioribera.rs>
License: MIT OR Apache-2.0

## Selected features

- Archetype: `app`
- Platforms: android ios desktop
- CI: github-actions 

- Bundled plugins: DeepLinks google-sign-in 
- UI framework hint: `freya` (see comments in `src/app.rs`)
- Desktop packaging: nix-bundle-app (`nix build .#bundle`)

- Publish targets: github-release 

## Quickstart

```sh
just build          # cargo build for the current host
just build-android  # cargo build for aarch64-linux-android + Gradle assemble
just build-ios      # cargo build for aarch64-apple-ios + xcodegen + xcodebuild
just bundle-desktop # nix build .#bundle (deb/rpm/appimage/dmg/msi via nix-bundle-app)
```

## Wiring notes


See `src/lib.rs` for the `istmo::runtime!` macro invocation and the commented
placeholder where extra plugin clients get registered. `src/app.rs` is the
shared UI shell — replace the `freya` scaffold if you picked a
different toolkit.


## Continuous integration


`.github/workflows/ci.yml` runs `cargo test`, `cargo fmt --check`, `cargo clippy`
on every push. `release.yml` fires on tags matching `v*`.


