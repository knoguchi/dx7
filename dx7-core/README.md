# dx7-core

Yamaha DX7 FM synthesizer engine in pure Rust. `no_std`, zero dependencies,
integer-only signal path.

- Six-operator FM with all 32 algorithms, feedback, and the DX7 bus routing
- Log-domain sine/exp operator kernel (the technique used by Yamaha's FM chips)
- Envelopes, LFO, pitch envelope, keyboard/velocity/rate scaling ported from
  MSFA with hardware-measured stage timings
- Parsers for 32-voice bulk dumps (packed) and single-voice (unpacked) SysEx,
  hardened against out-of-range data
- Polyphonic `Synth` with voice stealing, sustain pedal, mod wheel, pitch bend
  range (RPN 0), and 44.1/48 kHz output (`std` feature)
- The 32 ROM1A factory voices built in

Runs on desktop and on microcontrollers: 10 voices across the two 200 MHz
cores of an RP2350, 8 voices across the two 160 MHz cores of an ESP32-S3.

## Example

```rust
use dx7_core::{Synth, DxVoice, get_rom1a_preset};

let mut synth = Synth::new(48000);
synth.load_patch(get_rom1a_preset(10)); // E.PIANO 1
synth.note_on(60, 100);

let mut stereo = vec![0.0f32; 2 * 4800]; // 100 ms, interleaved L/R
synth.render(&mut stereo);
synth.note_off(60);
```

Load a bank from a `.syx` file:

```rust
use dx7_core::DxVoice;

let bytes = std::fs::read("rom1a.syx")?;
let bank = DxVoice::parse_bulk_dump(&bytes)?;
println!("{}", bank[3].name_str()); // "STRINGS 1 "
```

## `no_std`

```toml
dx7-core = { version = "0.1", default-features = false }
```

Without `std` you get `Voice`, `DxVoice`, the parsers and the ROM1A data;
drive `Voice::render` yourself in 64-sample blocks. Call
`tables::init_tables(sample_rate)`, `lfo::init_lfo`, and
`pitchenv::init_pitchenv` once at startup.

## Companion crates

- [`dx7-midi`](https://crates.io/crates/dx7-midi): USB/BLE MIDI parsers,
  message queue, General MIDI sound set.
- Desktop CLI and firmware for Pico 2 W and ESP32-S3 live in the
  [repository](https://github.com/knoguchi/dx7).

## License

Apache-2.0. Portions derived from Google's MSFA (Apache-2.0) and Freeverb
(public domain); see NOTICE in the repository. The embedded factory voice
data is Yamaha DX7 parameter data distributed for interoperability.
