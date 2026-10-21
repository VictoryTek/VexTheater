#!/usr/bin/env bash
# preflight.sh — VexTheater pre-push validation. Also what CI runs.
# Usage, from the repository root: bash scripts/preflight.sh
#
# `path:.` rather than `.` so untracked new files are part of the flake.
set -euo pipefail

dev() { nix develop path:. -c "$@"; }

echo "[1/6] cargo fmt";        dev cargo fmt --check
echo "[2/6] cargo clippy";     dev cargo clippy --all-targets -- -D warnings
echo "[3/6] cargo test";       dev cargo test
echo "[4/6] desktop file";     dev desktop-file-validate data/io.github.vextheater.desktop
echo "[5/6] AppStream";        dev appstreamcli validate --no-net data/io.github.vextheater.metainfo.xml
echo "[6/6] nix build";        nix build path:. --no-link --print-build-logs

echo "Preflight passed."
