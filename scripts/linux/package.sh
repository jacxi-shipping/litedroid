#!/usr/bin/env bash
# Package LiteDroid as a distributable tarball
set -euo pipefail

VERSION="0.1.0"
ARCH="$(uname -m)"
DIST_NAME="litedroid-${VERSION}-linux-${ARCH}"
DIST_DIR="target/${DIST_NAME}"

echo "Building release..."
cargo build --release -p litedroid-cli -p litedroid-daemon -p litedroid-gui 2>&1

echo "Packaging ${DIST_NAME}..."
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/scripts"

cp target/release/litedroid        "$DIST_DIR/"
cp target/release/litedroid-daemon "$DIST_DIR/"
cp target/release/litedroid-gui    "$DIST_DIR/"
cp scripts/linux/setup.sh         "$DIST_DIR/scripts/"

chmod +x "$DIST_DIR/litedroid" "$DIST_DIR/litedroid-daemon" "$DIST_DIR/litedroid-gui"
chmod +x "$DIST_DIR/scripts/setup.sh"

cd target
tar czf "${DIST_NAME}.tar.gz" "$DIST_NAME"
cd ..

echo ""
echo "Package created: target/${DIST_NAME}.tar.gz"
du -h "target/${DIST_NAME}.tar.gz"