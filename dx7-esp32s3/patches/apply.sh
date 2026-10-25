#!/bin/bash
# Vendor esp-rom-sys into patches/esp-rom-sys and apply esp-rom-sys.patch.
#
# Cargo.toml's [patch.crates-io] points at patches/esp-rom-sys, which is a
# generated, gitignored directory, so `cargo build` AND `cargo fetch` fail
# until this script has run. The crate version is read from Cargo.lock; the
# version and the patch checksum that produced the current copy are recorded
# in a stamp file, so re-running after a dependency bump (or after editing
# the .patch) refreshes the copy instead of silently keeping a stale one.
#
# Works on Linux and macOS (bash 3.2+, POSIX awk/cksum/find, curl, tar).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET="$SCRIPT_DIR/esp-rom-sys"
PATCH="$SCRIPT_DIR/esp-rom-sys.patch"
STAMP="$TARGET/.dx7-patch-stamp"
TMP="$TARGET.tmp"

VER=$(awk '/^name = "esp-rom-sys"$/ { getline; if (sub(/^version = "/, "") && sub(/"$/, "")) print; exit }' "$CRATE_DIR/Cargo.lock")
if [ -z "$VER" ]; then
    echo "apply.sh: esp-rom-sys not found in $CRATE_DIR/Cargo.lock" >&2
    exit 1
fi

WANT="esp-rom-sys-$VER patch-cksum=$(cksum < "$PATCH" | awk '{print $1}')"
if [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$WANT" ]; then
    echo "patches/esp-rom-sys is up to date ($WANT)"
    exit 0
fi
if [ -d "$TARGET" ]; then
    echo "patches/esp-rom-sys is stale or incomplete (want $WANT); rebuilding"
fi

cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT
rm -rf "$TMP"

# Prefer the cargo registry copy, present once anything has fetched the crate.
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
SRC=$(find "$CARGO_HOME_DIR/registry/src" -maxdepth 2 -type d -name "esp-rom-sys-$VER" 2>/dev/null | head -1 || true)
if [ -n "$SRC" ]; then
    echo "Copying $SRC"
    cp -R "$SRC" "$TMP"
else
    # Cargo cannot fetch it for us: the [patch] entry is resolved before any
    # download happens. Pull the .crate straight from crates.io instead.
    URL="https://static.crates.io/crates/esp-rom-sys/esp-rom-sys-$VER.crate"
    echo "esp-rom-sys-$VER not in cargo registry; downloading $URL"
    mkdir -p "$TMP"
    curl -fsSL "$URL" | tar -xzf - -C "$TMP" --strip-components=1
fi

# -t: never prompt (batch mode); --forward: don't try to reverse-apply.
if ! (cd "$TMP" && patch -t -p1 --forward < "$PATCH"); then
    echo "apply.sh: esp-rom-sys.patch does not apply to esp-rom-sys-$VER; update the patch." >&2
    echo "          (partial copy removed; any previous patches/esp-rom-sys left untouched)" >&2
    exit 1
fi

rm -rf "$TARGET"
mv "$TMP" "$TARGET"
echo "$WANT" > "$STAMP"
echo "Patched esp-rom-sys-$VER at $TARGET"
