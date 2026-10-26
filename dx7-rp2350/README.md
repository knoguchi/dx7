# dx7-rp2350 — DX7 FM Synth on RP2350

DX7 FM synthesizer running on the RP2350 (Cortex-M33 @ 200 MHz, dual-core).

## Prerequisites

```bash
# ARM Cortex-M33 target
rustup target add thumbv8m.main-none-eabihf

# Flash tool (pick one)
cargo install probe-rs-tools   # debug probe (SWD)
# or picotool (used by the uf2-deploy runner): https://github.com/raspberrypi/picotool
```

Note: if you also have a Homebrew/system `cargo` on your PATH, make sure
`~/.cargo/bin` comes first so the rustup toolchain (which has the
`thumbv8m` target) is the one that runs.

## Build

```bash
cd dx7-rp2350

# BLE MIDI synth with PWM audio (Pico 2 W only) — primary configuration
cargo build --release --features "ble-midi,pwm"

# USB MIDI synth with PWM audio
cargo build --release --features "usb-midi,pwm"

# USB + BLE MIDI synth with PWM audio (Pico 2 W only)
cargo build --release --features "usb-midi,ble-midi,pwm"
```

## Flash

### Via uf2-deploy runner (recommended)

Hold BOOTSEL and plug in the Pico, then:

```bash
cargo run --release --features "usb-midi,ble-midi,pwm"
```

The `uf2-deploy` script converts the ELF to UF2 with `picotool` and copies it to the `RP2350` BOOTSEL volume (macOS path `/Volumes/RP2350`; edit the script for other OSes).

### Via debug probe (SWD)

```bash
probe-rs run --chip RP2350 target/thumbv8m.main-none-eabihf/release/dx7-rp2350
```

### Via UF2 manual copy

```bash
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/dx7-rp2350 dx7-rp2350.uf2 --family rp2350-arm-s
# Copy dx7-rp2350.uf2 to the RP2350 USB drive
```

## Features

| Feature    | Description                    | Audio | MIDI Input | Board         |
|------------|--------------------------------|-------|------------|---------------|
| `pwm`      | PWM audio on GP15              | Yes   | No (demo)  | Any Pico 2    |
| `usb-midi` | USB MIDI class device          | —     | Yes        | Any Pico 2    |
| `ble-midi` | BLE MIDI via CYW43439          | —     | Yes        | Pico 2 **W**  |

BLE MIDI is the primary way to connect; USB MIDI is also supported. Typical combinations:
- `--features "ble-midi,pwm"` — wireless BLE MIDI synth (primary)
- `--features "usb-midi,ble-midi,pwm"` — BLE plus USB MIDI simultaneously
- `--features "usb-midi,pwm"` — USB MIDI only

`i2s` and `uart-midi` are declared in `Cargo.toml` as placeholders but have no code behind them yet.

## Architecture

Dual-core rendering with 10-voice polyphony:
- **Core 0**: embassy async — MIDI input (USB and/or BLE) + renders voices 0-4 + DMA buffer fill
- **Core 1**: renders voices 5-9 on demand, synchronized via atomic flags

Audio output uses DMA ping-pong double-buffering: two DMA channels alternate between two buffers, writing PWM duty values at 48 kHz with zero CPU involvement during transfer.

### Synth Modes

The firmware supports two operating modes, switchable at runtime via custom SysEx:

- **DX7 mode** (power-up default): Classic monotimbral operation — one patch for all voices, like a real DX7. Boots with BRASS 1 (ROM1A patch 0). ProgramChange selects from the ROM1A bank (0-31) or a loaded SysEx bank. Pitch bend, mod wheel, and sustain pedal apply globally to all active voices. MIDI receive channel is configurable (default: OMNI).

- **GM mode**: Multitimbral General MIDI operation — per-channel patches from the GM voice map, drums on channel 10 (MIDI ch 9). Intended for MIDI file playback. GM programs are level-normalised at mix time (`dx7_midi::gm::program_gain`: bass ×4, solo strings ×1.5, reeds/pipes ×1.3, …) because the underlying DX7 patches differ widely in output level.

### Custom SysEx Commands

Configure the synth from the host (Pico has no UI). Manufacturer ID `7D` = educational/non-commercial.

| SysEx Message           | Description                              |
|-------------------------|------------------------------------------|
| `F0 7D 01 00 F7`       | Switch to DX7 mode (monotimbral)         |
| `F0 7D 01 01 F7`       | Switch to GM mode (multitimbral)         |
| `F0 7D 02 <ch> F7`     | Set MIDI receive channel (DX7 mode only) |

Channel values: `00`-`0F` = channel 1-16, `7F` = OMNI (respond to all channels).

Mode switches release all active voices (CC 123 on every channel) to prevent stuck notes.

SysEx is accepted on every MIDI transport (USB and BLE). On BLE a SysEx message may span several packets, as the BLE-MIDI specification allows, so the 4104-byte bank dump works over Bluetooth too.

### BLE MIDI

On the Pico 2 W, the CYW43 WiFi/BLE chip provides Bluetooth Low Energy. The firmware advertises as "DX7" and accepts BLE MIDI connections. The onboard LED blinks while advertising and goes solid when a device is connected.

BLE packets are parsed per the BLE-MIDI 1.0 spec: header byte, then per message a timestamp byte, optional status, data. Running status is honoured within a packet; SysEx may span packets and feeds the same SysEx handler as USB.

### Voice Management

- 10-voice polyphony across 2 cores (5 per core)
- Steal priority: inactive > released (already decaying) > sustain-held (full level, pedal only) > oldest active
- Same-note deduplication prevents stuck notes on retrigger (the old instance is killed and its sustain-held flag cleared)
- CC 120 (All Sound Off) silences voices immediately, with no release stage; CC 123 (All Notes Off) releases them so the envelope tail plays out. In DX7 mode both act on every voice; in GM mode on the message's channel only
- SysEx bank reception for loading custom DX7 32-voice bulk dumps (USB or BLE)
- The MIDI input queue (256 slots) keeps 32 slots in reserve for NoteOff and Control Change so a NoteOn burst cannot cause a dropped NoteOff; dropped messages are counted and reported over defmt every 500 ms from the CPU-meter task

## Pin Mapping

| Function   | GPIO  | Feature     | Notes                          |
|------------|-------|-------------|--------------------------------|
| PWM audio  | GP15  | `pwm`       | RC filter → speaker/headphones |
| CYW43 SPI  | GP23,24,25,29 | `ble-midi` | Hardwired on Pico 2 W   |
| USB        | —     | `usb-midi`  | Internal USB controller        |

## PWM Audio Wiring

For speaker/headphone output from the PWM pin, use a simple RC low-pass filter:

```
GP15 ──[1kΩ]──┬── audio out
              [100nF]
               │
              GND
```

Cutoff frequency: ~1.6 kHz (adequate for a demo; an I2S DAC output is planned but not implemented).

## Performance

- RP2350: 200 MHz Cortex-M33, 520 KB SRAM
- Block size: 64 samples @ 48 kHz = 1333 us deadline
- 10 voices across 2 cores (5 per core)
- DMA audio output with zero CPU overhead during transfer
