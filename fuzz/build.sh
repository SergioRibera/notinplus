#!/usr/bin/env bash
# Builds the fuzz targets with libFuzzer + ASan. With rustup + cargo-fuzz
# installed, `cargo +nightly fuzz run <target>` does the same; this script
# only needs a nightly toolchain on PATH (`NIGHTLY` may point at its bin/).
set -eu
cd "$(dirname "$0")"
bin=${NIGHTLY:+$NIGHTLY/}
export RUSTFLAGS="-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters -Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares --cfg fuzzing -Zsanitizer=address \
-Cdebug-assertions -Coverflow-checks"
exec "${bin}cargo" build --release --target x86_64-unknown-linux-gnu --bins
