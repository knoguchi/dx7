# dx7-esp32s3

Bare-metal DX7 FM synthesizer on ESP32-S3. Uses `dx7-core` for the synth engine (integer-only FM core, no libm). The firmware's own output stage (DC blocker + 4th-order Butterworth low-pass + soft clip) runs in `f32` on the S3's hardware FPU.

## Setup

```bash
# 1. Install cargo-binstall (downloads prebuilt binaries, avoids compile issues)
cargo install cargo-binstall

# 2. Install Rust ESP toolchain
cargo binstall espup
espup install

# 3. Add xtensa GCC to PATH (add to .bashrc / .zshrc; works on Linux and macOS)
XTENSA_GCC=$(find ~/.rustup/toolchains/esp -name 'xtensa-esp32s3-elf-gcc' -type f 2>/dev/null | head -1)
export PATH="$(dirname "$XTENSA_GCC"):$PATH"

# 4. Install flashing tool
cargo binstall espflash

# 5. Vendor + patch esp-rom-sys (REQUIRED before any cargo command, see below)
cd dx7-esp32s3 && bash patches/apply.sh

# 6. For QEMU (optional): install Espressif QEMU fork
#    https://github.com/espressif/qemu
```

### The esp-rom-sys patch

`Cargo.toml` has `[patch.crates-io] esp-rom-sys = { path = "patches/esp-rom-sys" }`.
That directory is **generated and gitignored**, so on a fresh checkout `cargo build`
(and even `cargo fetch`) fail with "failed to read patches/esp-rom-sys/Cargo.toml"
until `bash patches/apply.sh` has run. `run.sh` runs it for you.

`apply.sh` reads the esp-rom-sys version from `Cargo.lock`, copies the crate from the
cargo registry (or downloads it from crates.io if cargo has not fetched it yet), applies
`patches/esp-rom-sys.patch`, and writes a stamp with the version + patch checksum. Re-run
it after bumping esp-hal/esp-rom-sys or editing the patch; it detects the stale copy and
rebuilds it, and fails loudly (leaving no half-patched directory) if the patch no longer
applies.

The patch has two hunks: a `strcasecmp` signed/unsigned `char` fix (a real correctness
fix, worth upstreaming) and one that silences the `function_casts_as_integer` lint on
recent nightlies.

## Build

```bash
# QEMU (benchmark only, no hardware needed)
cargo +esp build --release --features qemu

# Real hardware — bare ESP32-S3, PWM audio on GPIO4
cargo +esp build --release --features pwm

# Real hardware — ESP32S3-1.28inch-BOX board, I2S audio via ES8311
cargo +esp build --release --features es8311

# Real hardware — BLE MIDI + ES8311 audio
cargo +esp build --release --features "ble-midi,es8311"

# Real hardware — BLE MIDI + ES8311 audio + round LCD / touch (patch name, swipe to change)
cargo +esp build --release --features "ble-midi,es8311,lcd"

# Real hardware — USB MIDI + ES8311 audio (+ optional lcd)
cargo +esp build --release --features "usb-midi,es8311"
```

## Run

### QEMU

```bash
./run.sh              # build + run in QEMU
./run.sh --build-only # build only, output in bin/
```

### Flash to hardware

```bash
cargo +esp build --release --features es8311
espflash flash --monitor target/xtensa-esp32s3-none-elf/release/dx7-esp32s3
```

## Audio output

### `pwm` — LEDC PWM (bare board)

For a bare ESP32-S3 with no audio codec. Outputs 8-bit 312 kHz PWM on GPIO4.

Minimal external circuit: GPIO4 → 1kΩ → 100nF cap → GND. Tap between R and C for audio.

### `es8311` — I2S + ES8311 codec (1.28" box board)

16-bit 48 kHz I2S audio through the ES8311 DAC and NS4150B Class-D speaker amplifier.

Pin mapping:

| Function | GPIO |
|----------|------|
| I2S BCLK | 9 |
| I2S LRCK | 45 |
| I2S DOUT | 8 |
| I2S MCLK | 16 |
| I2C SCL (codec) | 14 |
| I2C SDA (codec) | 15 |
| PA enable | 46 |

## BLE MIDI

The `ble-midi` feature enables wireless MIDI input over Bluetooth Low Energy. Combine with an audio output feature (e.g. `es8311`).

- Advertises as **"DX7"**
- Implements the standard [MIDI over BLE](https://www.midi.org/specifications/midi-transports-specifications/specification-for-midi-over-bluetooth-low-energy-ble-midi) service
- Supports Note On/Off, Control Change, and Program Change
- Uses [trouble-host](https://github.com/embassy-rs/trouble) for the BLE GATT server and [esp-radio](https://github.com/esp-rs/esp-hal) for the radio driver

Connect from any BLE MIDI controller or app (e.g. KORG microKEY Air, macOS Audio MIDI Setup, Android MIDI).

## Benchmark

The firmware runs the CPU at 160 MHz (`CPU_CLOCK` in `main.rs`). One voice rendering a 64-sample block takes ~30k cycles, i.e. ~190 µs at 160 MHz, against a real-time deadline of 1333 µs at 48 kHz. Polyphony is fixed at `MAX_VOICES = 8`, rendered 4 per core across both cores, which leaves headroom for the output filter, MIDI parsing and (with `ble-midi`) the BLE stack.
