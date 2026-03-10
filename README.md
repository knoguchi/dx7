<p align="center">
  <img src="dx7.png" alt="Yamaha DX7" width="600">
</p>

# DX7

A Yamaha DX7 FM synthesizer emulator in Rust.

## Demo

E.PIANO 1 solo — "Greatest Love of All" intro (DX7's iconic FM electric piano):

https://github.com/knoguchi/dx7/raw/main/demo/epiano1_intro_demo.mp4

## Structure

- **dx7-core** — Platform-independent synthesis engine (no-std capable, zero dependencies)
- **dx7-app** — CLI application with real-time audio, MIDI input, keyboard control, and MIDI file rendering

## Build

```
cargo build --release
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
- `Q/Esc` — quit

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

Use `--track 1 --track 2` to render specific tracks only.

### List MIDI ports

```
cargo run --release --package dx7-app -- --list-midi
```

## SysEx Banks

The `sysex/` directory contains DX7 patch banks organized by source:

- `factory/` — 8 official factory ROM banks (ROM1A–ROM4B)
- `vrc/` — 24 Voice ROM Cartridge banks (VRC101–VRC112)
- `greymatter/` — Grey Matter E! expansion banks

## Engine

- 6 identical FM operators with 32 algorithm topologies
- 24-bit phase accumulator, 10-bit sine ROM in log domain
- Integer log/exp domain math matching the YM21280 OPS chip
- Per-operator amplitude envelopes, pitch envelope, LFO
- 4th-order Butterworth output filter at 10.5 kHz (DX7 reconstruction filter)
- DC-blocking filter, stereo reverb, and soft saturation
- GM sound set with 128 programs and synthesized drum machine (channel 10)

## Reference

- [music-synthesizer-for-android](https://github.com/google/music-synthesizer-for-android) (msfa) engine
