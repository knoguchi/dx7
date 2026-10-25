#!/bin/bash
# Build the QEMU benchmark image and run it.
# Portable to Linux and macOS: no GNU-only `find -printf`, `stat -c` or bare
# `timeout`; failures are reported instead of swallowed.
set -euo pipefail
cd "$(dirname "$0")"

# Ensure xtensa GCC is on PATH
XTENSA_GCC=$(find ~/.rustup/toolchains/esp -name 'xtensa-esp32s3-elf-gcc' -type f 2>/dev/null | head -1 || true)
[ -n "$XTENSA_GCC" ] && export PATH="$(dirname "$XTENSA_GCC"):$PATH"

ELF=target/xtensa-esp32s3-none-elf/release/dx7-esp32s3
BIN=bin/dx7-esp32s3.bin

bash patches/apply.sh
cargo +esp build --release --features qemu

mkdir -p bin
esptool --chip esp32s3 elf2image --ram-only-header -fs 4MB -fm dio -ff 40m -o bin/dx7.bin "$ELF" 2>&1 | tail -1
esptool --chip esp32s3 merge-bin --pad-to-size 4MB -fm dio -ff 40m --output "$BIN" 0x0000 bin/dx7.bin 2>&1 | tail -1
rm bin/dx7.bin

echo "Image: $BIN ($(wc -c < "$BIN" | tr -d ' ') bytes)"

if [ "${1:-}" = "--build-only" ]; then
    exit 0
fi

QEMU=$(find ~/.espressif/tools/qemu-xtensa -name 'qemu-system-xtensa' -type f 2>/dev/null | head -1 || true)
if [ -z "$QEMU" ]; then
    echo "run.sh: qemu-system-xtensa not found under ~/.espressif/tools/qemu-xtensa" >&2
    echo "        install the Espressif QEMU fork (https://github.com/espressif/qemu) or use --build-only" >&2
    exit 1
fi

# GNU timeout is `timeout` on Linux and `gtimeout` (coreutils) on macOS. With
# neither, QEMU runs until you quit it (Ctrl-A X).
RUN_FOR=15
TIMEOUT=()
if command -v timeout >/dev/null 2>&1; then
    TIMEOUT=(timeout "$RUN_FOR")
elif command -v gtimeout >/dev/null 2>&1; then
    TIMEOUT=(gtimeout "$RUN_FOR")
else
    echo "run.sh: no timeout(1) found; QEMU will run until you quit it (Ctrl-A X)"
fi

set +e
# ${arr[@]+"${arr[@]}"} keeps `set -u` happy on bash 3.2 when the array is empty.
${TIMEOUT[@]+"${TIMEOUT[@]}"} "$QEMU" -machine esp32s3 -nographic -no-reboot -serial mon:stdio \
    -drive file="$BIN",if=mtd,format=raw
rc=$?
set -e

if [ ${#TIMEOUT[@]} -gt 0 ] && [ "$rc" -eq 124 ]; then
    echo "QEMU stopped after ${RUN_FOR}s (timeout, expected)"
elif [ "$rc" -ne 0 ]; then
    echo "QEMU exited with status $rc" >&2
    exit "$rc"
else
    echo "QEMU exited normally"
fi
