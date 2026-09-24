#!/usr/bin/env bash
# Local Linux equivalent of .github/workflows/vivida-headless-e2e.yml.
#
# Builds vivida with the pinned Rust toolchain, then runs the hermetic
# headless smoke (tests/e2e/vivida_headless_smoke.py): start
# `vivida --headless --session`, drive layout, text, input, search, exec, and
# split over host IPC, then quit. Server stderr stays on this terminal.
#
# Usage: vivida/scripts/headless_e2e.sh [--install-deps] [--skip-build]
#   --install-deps  apt-get install the CI build inputs (Debian/Ubuntu, sudo)
#   --skip-build    reuse the existing target/debug/vivida
set -euo pipefail

install_deps=0
skip_build=0
for arg in "$@"; do
  case "$arg" in
    --install-deps) install_deps=1 ;;
    --skip-build) skip_build=1 ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != Linux ]]; then
  echo "this script targets Linux (the CI job runs on ubuntu-24.04)" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if (( install_deps )); then
  sudo apt-get update
  sudo apt-get install -y build-essential cmake git pkg-config \
    libasound2-dev libfontconfig1-dev libfreetype6-dev \
    libwayland-dev libxkbcommon-dev \
    libavcodec-dev libavdevice-dev libavutil-dev libswscale-dev libswresample-dev \
    libvulkan1 mesa-vulkan-drivers
fi

if (( ! skip_build )); then
  rust_toolchain="$(python3 -c 'import json; print(json.load(open("installer/release.json"))["rust_toolchain"])')"
  rustup toolchain install "$rust_toolchain" --profile minimal
  (cd vivida && cargo "+$rust_toolchain" build --locked --bin vivida)
fi

vivida_bin="$repo_root/vivida/target/debug/vivida"
if [[ ! -x "$vivida_bin" ]]; then
  echo "missing $vivida_bin; run without --skip-build" >&2
  exit 1
fi

python3 tests/e2e/vivida_headless_smoke.py \
  --vivida "$vivida_bin" \
  --session "local-headless-$$-$(date +%s)"
