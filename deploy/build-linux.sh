#!/usr/bin/env bash
# deploy/build-linux.sh
#
# Run this ON the Linux build host (the production machine), from the repo root.
# It compiles duapi/dudb natively, builds the SPA, and stages everything under
# dist/ ready to be packaged into a container image.
#
#   ./deploy/build-linux.sh
#
# Env:
#   DIST   staging directory (default: dist)
set -euo pipefail
cd "$(dirname "$0")/.."

DIST=${DIST:-dist}

command -v cargo >/dev/null || { echo "cargo not found — install rustup first"; exit 1; }
command -v npm   >/dev/null || { echo "npm not found — install Node 22 first";  exit 1; }

echo "==> host glibc: $(ldd --version 2>/dev/null | head -1)"

echo "==> building Rust binaries"
# Build every binary: duapi/dudb go into the image, while duscan/dusum stay in
# target/release for building the DB on the scan host (see deploy/README.md).
cargo build --release --locked

echo "==> building SPA"
# svelte.config.js (adapter-static) writes the SPA to browser/public, NOT browser/build.
( cd browser && npm ci && npm run build )

echo "==> staging into $DIST"
rm -rf "$DIST"
mkdir -p "$DIST"
cp target/release/duapi target/release/dudb "$DIST/"
cp -r browser/public "$DIST/public"

# The highest GLIBC symbol version the binary references is the floor for the
# runtime base image. A UBI9 runtime provides 2.34; UBI8 provides 2.28.
if command -v objdump >/dev/null; then
  floor=$(objdump -T "$DIST/duapi" | sed -n 's/.*GLIBC_\([0-9.]*\).*/\1/p' | sort -uV | tail -1)
  echo "==> duapi requires glibc >= ${floor:-unknown}"
  echo "    the runtime base image must provide at least this version"
fi

echo "==> done. Contents of $DIST:"
ls -la "$DIST"
