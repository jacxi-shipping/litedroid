#!/usr/bin/env bash
# Run all LiteDroid tests
set -euo pipefail

echo "Running LiteDroid test suite..."
cargo test --workspace 2>&1

echo ""
echo "All tests passed."