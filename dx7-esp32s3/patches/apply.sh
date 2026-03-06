#!/bin/bash
# Copy esp-rom-sys from cargo registry and apply our patch.
# Run once after `cargo +esp build` fetches dependencies.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="$SCRIPT_DIR/esp-rom-sys"

if [ -d "$TARGET" ]; then
    echo "patches/esp-rom-sys already exists, skipping."
    exit 0
fi

SRC=$(find ~/.cargo/registry/src -path '*/esp-rom-sys-0.1.3' -type d 2>/dev/null | head -1)
if [ -z "$SRC" ]; then
    echo "esp-rom-sys-0.1.3 not found in cargo registry. Run 'cargo +esp fetch' first."
    exit 1
fi

cp -r "$SRC" "$TARGET"
cd "$TARGET"
patch -p1 < "$SCRIPT_DIR/esp-rom-sys.patch"
echo "Patched esp-rom-sys at $TARGET"
