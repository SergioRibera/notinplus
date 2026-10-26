# Pinned third-party binaries the desktop build consumes.
#
# Skia: the Nix sandbox has no network, so `freya-skia-bindings`'s
# build.rs gets its prebuilt archive through `SKIA_BINARIES_URL=file://…`.
# The key must match what the build script computes (crate version +
# repo hash + target + enabled features); a mismatch makes it fall back
# to a from-source build, which fails loudly offline. Bump the tag/hash
# together with `freya-skia-bindings` in Cargo.lock.
#
# pdfium: same `chromium/7881` release the Android and iOS builds pin,
# matching the ABI `pdfium-render 0.9` binds against.
{
  fetchurl,
  runCommand,
  system,
}:
let
  skiaTag = "0.104.0";
  skiaHash = "746613b43b596b9d2114";
  pdfiumRelease = "chromium%2F7881";

  linuxFeatures = "egl-ganesh-gl-jpegd-jpege-svg-textlayout-vulkan-wayland-webpd-webpe-x11";
  darwinFeatures = "ganesh-jpegd-jpege-metal-svg-textlayout-webpd-webpe";

  perSystem = {
    x86_64-linux = {
      skia = {
        triple = "x86_64-unknown-linux-gnu";
        features = linuxFeatures;
        hash = "sha256-3BJEhkPhqH6QOloL8UL3cueOS3g/56q8Xnn9sFD3wDw=";
      };
      pdfium = {
        name = "linux-x64";
        hash = "sha256-FHDiG4tKO0rX+FaE4toR2U87aahtgd7hG5tnCdknrB0=";
      };
    };
    aarch64-linux = {
      skia = {
        triple = "aarch64-unknown-linux-gnu";
        features = linuxFeatures;
        hash = "sha256-rRQJzPQAXfhdnfo2X9u+tjsNNbA5/11yZ1WohvkSEeo=";
      };
      pdfium = {
        name = "linux-arm64";
        hash = "sha256-7n97fVRolYM2qBjBzVgL3SCXKEa3N3sT+akj2S0dRnQ=";
      };
    };
    aarch64-darwin = {
      skia = {
        triple = "aarch64-apple-darwin";
        features = darwinFeatures;
        hash = "sha256-bzANEpQzqtfTll+fHMVJS0vnZECptrnZTteIp4BNMQY=";
      };
      pdfium = {
        name = "mac-arm64";
        hash = "sha256-UulMpaqIR5NDMNrz+BUMGQaCxcqTgxRoeU+LkNQ5LkA=";
      };
    };
  };

  cfg = perSystem.${system} or (throw "notinplus: no prebuilt skia/pdfium pinned for ${system}");
  skiaKey = "${skiaHash}-${cfg.skia.triple}-${cfg.skia.features}";
in
{
  # Directory holding `skia-binaries-<key>.tar.gz`, ready for the
  # `file://<dir>/skia-binaries-{key}.tar.gz` URL template.
  skia = runCommand "skia-binaries-${skiaTag}-${cfg.skia.triple}" { } ''
    mkdir -p $out
    ln -s ${
      fetchurl {
        url = "https://github.com/marc2332/rust-skia/releases/download/${skiaTag}/skia-binaries-${skiaKey}.tar.gz";
        inherit (cfg.skia) hash;
      }
    } $out/skia-binaries-${skiaKey}.tar.gz
  '';

  pdfium = runCommand "pdfium-7881-${cfg.pdfium.name}" { } ''
    mkdir -p $out
    tar -xzf ${
      fetchurl {
        url = "https://github.com/bblanchon/pdfium-binaries/releases/download/${pdfiumRelease}/pdfium-${cfg.pdfium.name}.tgz";
        inherit (cfg.pdfium) hash;
      }
    } -C $out
  '';
}
