#!/usr/bin/env bash
# Clean LiteDroid build artifacts
set -euo pipefail

echo "Cleaning build artifacts..."
cargo clean
echo "Done."
