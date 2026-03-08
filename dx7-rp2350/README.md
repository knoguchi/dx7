# dx7-rp2350 — DX7 FM Synth on RP2350

DX7 FM synthesizer running on the RP2350 (Cortex-M33 @ 200 MHz, dual-core).

## Prerequisites

```bash
# ARM Cortex-M33 target
rustup target add thumbv8m.main-none-eabihf

# Flash tool (pick one)
cargo install probe-rs-tools   # debug probe (SWD)
cargo install elf2uf2-rs       # UF2 drag-and-drop
```

## Build

```bash
cd dx7-rp2350

# Benchmark only (no audio)
cargo build --release

# PWM audio demo (hardcoded note)
cargo build --release --features pwm

# USB MIDI synth with PWM audio
cargo build --release --features "usb-midi,pwm"
```

## Flash

### Via debug probe (SWD)

```bash
probe-rs run --chip RP2350 target/thumbv8m.main-none-eabihf/release/dx7-rp2350
```

### Via UF2 (hold BOOTSEL + plug USB)

```bash
elf2uf2-rs target/thumbv8m.main-none-eabihf/release/dx7-rp2350 dx7-rp2350.uf2
# Copy dx7-rp2350.uf2 to the RPI-RP2 USB drive
```

### Via picotool

```bash
picotool load target/thumbv8m.main-none-eabihf/release/dx7-rp2350 -t elf -f
picotool reboot
```

## Features

| Feature    | Description                    | Audio | MIDI Input |
|------------|--------------------------------|-------|------------|
| `pwm`      | PWM audio on GP15              | Yes   | No (demo)  |
| `usb-midi` | USB MIDI class device          | —     | Yes        |
| `i2s`      | PIO I2S for external DAC       | Yes   | No         |
| `ble-midi` | BLE MIDI via CYW43439          | —     | Yes        |
| `uart-midi`| Classic 31250 baud MIDI        | —     | Yes        |

Typical combinations:
- `--features pwm` — demo playback, no MIDI
- `--features "usb-midi,pwm"` — live synth, plug into DAW

## Architecture

Dual-core rendering with 4-voice polyphony:
- **Core 0**: embassy async — USB MIDI + renders voices 0-1 + pushes to ring buffer
- **Core 1**: TIMER0 ALARM3 ISR at 48kHz for PWM output + renders voices 2-3 on demand

## Pin Mapping

| Function   | GPIO  | Feature     | Notes                          |
|------------|-------|-------------|--------------------------------|
| PWM audio  | GP15  | `pwm`       | RC filter → headphones         |
| I2S BCK    | GP16  | `i2s`       | PCM5102A DAC                   |
| I2S LRCK   | GP17  | `i2s`       |                                |
| I2S DOUT   | GP18  | `i2s`       |                                |
| UART RX    | GP1   | `uart-midi` | DIN-5 / TRS connector          |
| CYW43 SPI  | GP23,24,25,29 | `ble-midi` | Hardwired on Pico 2 W   |
| USB        | —     | `usb-midi`  | Internal USB controller        |

## PWM Audio Wiring

For headphone output from the PWM pin, use a simple RC low-pass filter:

```
GP15 ──[1kΩ]──┬── audio out
              [100nF]
               │
              GND
```

Cutoff frequency: ~1.6 kHz (adequate for demo; use I2S + DAC for quality audio).

## Performance

- RP2350: 200 MHz Cortex-M33, 520 KB SRAM
- Block size: 64 samples @ 48 kHz = 1333 us deadline
- ~25% CPU per voice per core
- 4 voices across 2 cores at ~50% utilization each
