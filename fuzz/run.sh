#!/usr/bin/env bash
# Runs every fuzz target for SECONDS seconds (default 300). Any single
# allocation over 1 GiB counts as a finding.
set -eu
cd "$(dirname "$0")"
secs=${1:-300}
export ASAN_OPTIONS=detect_leaks=0:allocator_may_return_null=0
status=0
for t in $(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | grep -v notinplus-fuzz); do
  mkdir -p "corpus/$t" "artifacts/$t"
  ./target/x86_64-unknown-linux-gnu/release/"$t" "corpus/$t" \
    -artifact_prefix="artifacts/$t/" -max_total_time="$secs" -max_len=65536 \
    -malloc_limit_mb=1024 -rss_limit_mb=4096 -timeout=10 || status=1
done
exit $status
