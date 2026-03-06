# dx7-esp32s3

Bare-metal DX7 FM synthesizer on ESP32-S3. Uses `dx7-core` for the synth engine — pure integer math, no floating point, no libm.

## Setup

```bash
# 1. Install cargo-binstall (downloads prebuilt binaries, avoids compile issues)
cargo install cargo-binstall

# 2. Install Rust ESP toolchain
cargo binstall espup
espup install

# 3. Add xtensa GCC to PATH (add to .bashrc / .zshrc)
XTENSA_BIN=$(find ~/.rustup/toolchains/esp -name 'xtensa-esp32s3-elf-gcc' -printf '%h' 2>/dev/null)
export PATH="$XTENSA_BIN:$PATH"

# 4. Install flashing tool
cargo binstall espflash

# 5. For QEMU (optional): install Espressif QEMU fork
#    https://github.com/espressif/qemu
```

## Build

```bash
# QEMU (benchmark only, no hardware needed)
cargo +esp build --release --features qemu

# Real hardware — bare ESP32-S3, PWM audio on GPIO4
cargo +esp build --release --features pwm

# Real hardware — ESP32S3-1.28inch-BOX board, I2S audio via ES8311
cargo +esp build --release --features es8311
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

## Benchmark

On ESP32-S3 @ 240 MHz, one voice rendering a 64-sample block takes ~30k cycles (~125 µs). The real-time deadline at 48 kHz is 1333 µs, leaving room for ~10 voices.
