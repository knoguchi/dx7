# dx7-midi

`no_std` MIDI building blocks for [`dx7-core`](https://crates.io/crates/dx7-core).

- `usb`: USB-MIDI event packet (CIN) decoding
- `ble`: BLE-MIDI 1.0 packet parser (timestamps, running status, SysEx
  across packets)
- `MidiQueue`: fixed-size single-consumer message queue with NoteOff
  priority and drop counting, for passing events from a transport task to an
  audio loop
- `gm`: General MIDI program map onto a compiled 128-patch DX7 sound set,
  plus drum-note patches

```rust
use dx7_midi::{MidiQueue, MidiMessage, ble::BleMidiParser};

static QUEUE: MidiQueue = MidiQueue::new();

let mut parser = BleMidiParser::new();
parser.parse(&[0x80, 0x81, 0x90, 60, 100], &QUEUE, &mut |_sysex_byte| {});

while let Some(msg) = QUEUE.pop() {
    if let MidiMessage::NoteOn { note, velocity, .. } = msg {
        // synth.note_on(note, velocity)
    }
}
```

Used by the RP2350 and ESP32-S3 firmware in the
[dx7 repository](https://github.com/knoguchi/dx7).

## License

Apache-2.0. The General MIDI sound set is compiled from community DX7 patch
banks; see NOTICE in the repository.
