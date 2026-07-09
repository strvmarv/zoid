#!/usr/bin/env bash
# Phase-0 musl LINK probe, run INSIDE a rust docker container.
# Attempts to build each probe crate for x86_64-unknown-linux-musl and reports
# PASS/FAIL per path. No model download — this is a compile+link test only.
set -u
TARGET=x86_64-unknown-linux-musl
RESULTS=/work/PHASE0-RESULTS.txt
: > "$RESULTS"

echo "== toolchain setup ==" | tee -a "$RESULTS"
rustc --version | tee -a "$RESULTS"
apt-get update -qq && apt-get install -y -qq musl-tools >/dev/null 2>&1 && echo "musl-tools installed" | tee -a "$RESULTS"
rustup target add "$TARGET" 2>&1 | tail -1 | tee -a "$RESULTS"

probe() {
  local name="$1" dir="$2"
  echo "" | tee -a "$RESULTS"
  echo "==================== PROBE: $name ====================" | tee -a "$RESULTS"
  local log="/work/${name}-build.log"
  if (cd "$dir" && CARGO_TARGET_DIR=/work/target-$name cargo build --release --target "$TARGET" ) >"$log" 2>&1; then
    echo "RESULT $name: PASS (linked for $TARGET)" | tee -a "$RESULTS"
    ls -la /work/target-$name/$TARGET/release/ 2>/dev/null | grep -E "$name" | tee -a "$RESULTS"
  else
    echo "RESULT $name: FAIL" | tee -a "$RESULTS"
    echo "--- last 25 lines of $name build log ---" | tee -a "$RESULTS"
    tail -25 "$log" | tee -a "$RESULTS"
  fi
}

probe candle-probe    /work/candle-probe
probe fastembed-probe /work/fastembed-probe

echo "" | tee -a "$RESULTS"
echo "==================== SUMMARY ====================" | tee -a "$RESULTS"
grep -E "^RESULT" "$RESULTS" | tee -a "$RESULTS"
