#!/bin/bash
# Clean all build artifacts for EBIDownload workspace

set -e

echo "🧹 Cleaning Rust workspace..."
cargo clean

echo "🧹 Cleaning GUI frontend..."
cd crates/ebidownload-gui
rm -rf node_modules dist
rm -rf src-tauri/target

echo "✅ All cleaned."
echo ""
echo "To rebuild, run:"
echo "  cd crates/ebidownload-gui && npm install && npm run tauri dev"
