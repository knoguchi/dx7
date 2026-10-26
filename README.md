<p align="center">
  <img src="dx7.png" alt="Yamaha DX7" width="600">
</p>

# DX7

A Yamaha DX7 FM synthesizer emulator in Rust: a desktop synth and renderer, and the same engine as firmware for a Raspberry Pi Pico 2 W and an ESP32-S3 board with a round touch display.

## Demo

E.PIANO 1 solo — "Greatest Love of All" intro (DX7's iconic FM electric piano):

https://github.com/knoguchi/dx7/raw/main/demo/epiano1_intro_demo.mp4

## Structure

- **dx7-core** — Platform-independent synthesis engine (`no_std` capable, zero dependencies). See [dx7-core/README.md](dx7-core/README.md).
- **dx7-midi** — `no_std` USB-MIDI / BLE-MIDI parsers, message queue, General MIDI sound set. See [dx7-midi/README.md](dx7-midi/README.md).
- **dx7-app** — Desktop CLI: real-time audio, MIDI input (USB, optional BLE and RTP-MIDI), computer keyboard, MIDI file rendering.
- **dx7-rp2350** — Firmware for Raspberry Pi Pico 2 W (10 voices, BLE MIDI as the primary input, USB MIDI too, PWM audio). See [dx7-rp2350/README.md](dx7-rp2350/README.md).
- **dx7-esp32s3** — Firmware for ESP32-S3 boards (8 voices, BLE MIDI as the primary input, USB MIDI too, ES8311 I2S codec, round LCD with touch). See [dx7-esp32s3/README.md](dx7-esp32s3/README.md).

## Build

```
cargo build --release          # dx7-core, dx7-midi, dx7-app
cargo test                     # engine, parser and renderer tests
```

The firmware crates are excluded from the workspace and built from their own
directories with their own targets; see their READMEs.

Optional desktop features:

```
cargo build --release -p dx7-app --features bluetooth   # BLE MIDI peripheral (macOS, Linux/BlueZ)
cargo build --release -p dx7-app --features rtp-midi    # Apple Network MIDI (RTP-MIDI) listener
```

## Usage

### Interactive mode

Play with computer keyboard or a connected MIDI controller:

```
cargo run --release --package dx7-app
```

Keyboard controls:
- `ASDFGHJKL;` — white keys (C4-E5)
- `WETYUOP` — black keys
- `Z/X` — octave down/up
- `1-9,0` — select patch
- `Q`, `Esc`, `Ctrl-C` — quit

Key release needs a terminal that supports the kitty keyboard protocol
(iTerm2, kitty, WezTerm, foot). On Terminal.app a note sustains until the key
is pressed again. MIDI Program Change switches patches live.

### Load patches from SysEx

```
cargo run --release --package dx7-app -- --sysex sysex/factory/rom1a.syx --patch 0
```

### Render a single note to WAV

```
cargo run --release --package dx7-app -- --render output.wav --note 60 --velocity 100 --duration 3
```

### Render a MIDI file to WAV

```
cargo run --release --package dx7-app -- --midi-file song.mid --sysex sysex/factory/rom1a.syx --patch 3 --render output.wav
```

### GM mode

Render a MIDI file using the built-in General MIDI sound set (128 programs mapped to DX7 patches):

```
cargo run --release --package dx7-app -- --midi-file song.mid --render output.wav --gm
```

Use `--track 1 --track 2` to render specific tracks only. Other render
options: `--ch 2=file.syx:21@1.5,0.3` maps a channel to a patch with volume
and reverb send, `--dry` disables the effects chain, `--no-normalize` keeps
absolute levels, `--sample-rate 48000` sets the output rate (44100 or 48000).

### List MIDI ports

```
cargo run --release --package dx7-app -- --list-midi
```

## Hardware

The same engine runs as a standalone instrument on two boards. BLE MIDI is the
primary input: pair a keyboard or phone, pick a patch, play. USB MIDI works on
both as well.

|                | Raspberry Pi Pico 2 W (`dx7-rp2350`) | ESP32-S3 1.28" round-LCD board (`dx7-esp32s3`) |
|----------------|--------------------------------------|-------------------------------------------------|
| MCU            | RP2350, 2x Cortex-M33 @ 200 MHz      | ESP32-S3, 2x Xtensa LX7 @ 160 MHz               |
| Polyphony      | 10 voices (5 per core)               | 8 voices (4 per core)                           |
| Audio out      | PWM on GP15 fed by DMA ping-pong, RC low-pass to a jack | 16-bit 48 kHz I2S into the ES8311 codec and NS4150B speaker amp (LEDC PWM on GPIO4 for a bare module) |
| MIDI in        | BLE MIDI over the CYW43439, USB MIDI | BLE MIDI, USB MIDI                              |
| UI             | Onboard LED (blinks while advertising, solid when connected); mode and channel set by SysEx | 240x240 GC9A01 round LCD shows the patch name; CST816D touch: swipe up/down changes patch, swipe left plays a chord |
| Modes          | DX7 (one patch, like the original) and General MIDI (16 channels, drums on 10), switched by SysEx | DX7 |
| Output stage   | DC blocker, 4th-order Butterworth low-pass at 10.5 kHz, soft clip | same |
| Bank loading   | 32-voice SysEx bulk dump over BLE or USB | ROM1A built in                        |

Both firmwares advertise as **"DX7"** and implement the standard MIDI over
BLE service, so any BLE MIDI controller or macOS/iOS/Android host can connect.

### Signal path

```
 BLE / USB ──► MIDI parser ──► MidiQueue ──► dispatch: voice allocation, CCs, SysEx
                                                   │
                     ┌─────────────────────────────┴─────────────────────────────┐
             core 0: render voices 0..N/2                           core 1: render voices N/2..N
                     └─────────────────────────────┬─────────────────────────────┘
                                   mix one 64-sample block (1.33 ms at 48 kHz)
                                                   │
                       DC blocker ──► Butterworth LPF ──► soft clip ──► DMA ──► PWM / I2S
```

Core 0 runs the async executor (radio or USB stack, MIDI, half the voices,
DMA buffer refill). Core 1 does nothing but render its half of the voice pool,
handshaking with core 0 through two atomics once per block. The DMA engine
streams the finished block to the PWM slice or the I2S peripheral with no CPU
involvement, so a late block shows up as a counted underrun rather than a
crash. On the ESP32-S3 one voice block costs about 30k cycles, roughly 190 µs
at 160 MHz against the 1333 µs deadline.

### Bill of materials

- **Pico 2 W build**: a Pico 2 W, a 1 kΩ resistor and 100 nF capacitor for
  the PWM low-pass, and a headphone jack or small amplifier. Wiring and the
  flash procedure are in [dx7-rp2350/README.md](dx7-rp2350/README.md).
- **ESP32-S3 build**: an "ESP32-S3 1.28inch BOX" style board (ESP32-S3,
  ES8311 codec, speaker amp, GC9A01 LCD, CST816D touch), nothing external.
  Pin map, toolchain setup and flashing are in
  [dx7-esp32s3/README.md](dx7-esp32s3/README.md).

## SysEx Banks

The `sysex/` directory contains DX7 patch banks organized by source:

- `factory/` — 8 official factory ROM banks (ROM1A–ROM4B)
- `vrc/` — 24 Voice ROM Cartridge banks (VRC101–VRC112)
- `greymatter/` — Grey Matter E! expansion banks
- `KV-06 Shofuku Gaiden/` — community bank

## Engine

- 6 identical FM operators with 32 algorithm topologies and feedback
- 24-bit phase accumulator, 10-bit sine ROM in log domain
- Integer log/exp domain math in the style of the YM21280 OPS chip
- Per-operator amplitude envelopes with hardware-measured stage timing, pitch envelope, LFO
- Keyboard level/rate scaling, velocity, mod wheel, pitch bend with configurable range (RPN 0), sustain pedal
- Voice stealing with a one-block fade (no clicks)
- Desktop output chain: DC blocker, soft saturation, stereo reverb, chorus, tremolo, exciter
- Firmware output chain: DC blocker and 4th-order Butterworth low-pass at 10.5 kHz (DX7 reconstruction filter)
- GM sound set with 128 programs and synthesized drum machine (channel 10)

## License

Apache-2.0, see [LICENSE](LICENSE). Third-party attributions (MSFA, Freeverb)
and a note on the embedded factory voice data are in [NOTICE](NOTICE).

## Reference

- [music-synthesizer-for-android](https://github.com/google/music-synthesizer-for-android) (msfa) engine, Apache-2.0
- [OPL3 FPGA math notes](https://github.com/gtaylormb/opl3_fpga) for the log-domain operator technique
