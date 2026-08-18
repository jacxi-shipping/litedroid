#!/usr/bin/env bash
# Build LiteDroid in release mode
set -euo pipefail

echo "Building LiteDroid (release)..."
cargo build --release 2>&1

echo ""
echo "Build complete. Binary sizes:"
echo "=========================="
printf "%-25s %s\n" "Binary" "Size"
printf "%-25s %s\n" "-------" "----"
for bin in litedroid litedroid-daemon litedroid-gui; do
    if [ -f "target/release/$bin" ]; then
        size=$(du -h "target/release/$bin" | cut -f1)
        printf "%-25s %s\n" "$bin" "$size"
    else
        printf "%-25s %s\n" "$bin" "(not built)"
    fi
done