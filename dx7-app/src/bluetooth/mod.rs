//! BLE MIDI peripheral mode.
//!
//! Advertises as a "DX7" BLE MIDI device. iPads, phones, and BLE MIDI
//! controllers can connect and send notes to the synth engine.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::BleHandler;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::BleHandler;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported {
    //! Stub so `--features bluetooth` still compiles on platforms without a
    //! BLE peripheral backend (e.g. Windows). `start` always fails.
    use crate::midi::CommandSink;
    use std::sync::Arc;

    pub struct BleHandler;

    impl BleHandler {
        pub fn start(_sink: Arc<CommandSink>) -> Result<Self, String> {
            Err("BLE MIDI peripheral mode is not supported on this platform".into())
        }
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use unsupported::BleHandler;

use crate::midi::{self, MidiMessage};

/// Parse a BLE MIDI packet into MIDI messages.
///
/// BLE-MIDI 1.0 wire format:
///
/// ```text
/// [header 10xxxxxx] { [timestamp 1xxxxxxx] [status] [data...]   full message
///                   | [timestamp 1xxxxxxx] [data...]            running status
///                   | [data...]                                 running status, shared timestamp
///                   }*
/// ```
///
/// Every message except running-status-with-shared-timestamp is preceded by
/// a timestamp-low byte (bit 7 set), so a bit-7 byte after a complete
/// message is always a timestamp, and the byte *after* it decides between a
/// new status byte (bit 7 set) and running-status data (bit 7 clear). SysEx
/// data runs until `[timestamp] [F7]`; real-time messages (F8-FF) may be
/// interleaved anywhere and do not disturb running status, while SysEx and
/// System Common messages clear it, as in MIDI 1.0.
///
/// The parser is stateless across packets: a SysEx that continues into the
/// next packet is simply skipped there (its data bytes arrive with no
/// running status and are ignored).
pub(crate) fn parse_ble_midi_packet(data: &[u8]) -> Vec<MidiMessage> {
    let mut messages = Vec::new();

    // Header + at least one timestamp + one status byte.
    if data.len() < 3 || data[0] & 0x80 == 0 {
        return messages;
    }

    let mut i = 1;
    let mut running_status: u8 = 0;

    while i < data.len() {
        let mut status = running_status;

        if data[i] & 0x80 != 0 {
            // Timestamp-low byte. What follows tells us the message form.
            i += 1;
            if i >= data.len() {
                break;
            }
            if data[i] & 0x80 != 0 {
                status = data[i];
                i += 1;
                if status >= 0xF0 {
                    i = skip_system_message(status, data, i, &mut running_status);
                    continue;
                }
                running_status = status;
            } else if running_status == 0 {
                // Data with no status to attach to (e.g. SysEx continuation).
                i += 1;
                continue;
            }
        } else if running_status == 0 {
            i += 1;
            continue;
        }

        // Channel voice message: read its data bytes.
        let data_len = midi_data_length(status);
        if data_len == 0 || i + data_len > data.len() {
            break;
        }
        let bytes = &data[i..i + data_len];
        if let Some(bad) = bytes.iter().position(|b| b & 0x80 != 0) {
            // Truncated message; resync on the bit-7 byte.
            i += bad;
            continue;
        }

        let mut msg = [status, 0, 0];
        msg[1..1 + data_len].copy_from_slice(bytes);
        i += data_len;

        if let Some(m) = midi::parse_midi_message(&msg[..1 + data_len]) {
            messages.push(m);
        }
    }

    messages
}

/// Skip a System message starting at `i` (just past its status byte) and
/// return the index of the next unparsed byte. Updates running status per
/// MIDI 1.0: SysEx and System Common clear it, real-time leaves it alone.
fn skip_system_message(status: u8, data: &[u8], mut i: usize, running_status: &mut u8) -> usize {
    match status {
        0xF0 => {
            *running_status = 0;
            while i < data.len() {
                if data[i] & 0x80 == 0 {
                    i += 1; // SysEx payload byte
                    continue;
                }
                // Timestamp byte inside SysEx: either [ts][F7] (end) or
                // [ts][F8-FF] (real-time interleaved). Anything else means
                // the SysEx continues in a later packet; hand the timestamp
                // back to the main loop.
                match data.get(i + 1) {
                    Some(&0xF7) => return i + 2,
                    Some(&b) if b >= 0xF8 => i += 2,
                    _ => return i,
                }
            }
            i
        }
        0xF8..=0xFF => i, // real-time: no data bytes, running status preserved
        0xF1 | 0xF3 => {
            *running_status = 0;
            i + 1
        }
        0xF2 => {
            *running_status = 0;
            i + 2
        }
        _ => {
            // F4-F7 (undefined / tune request / stray EOX): no data.
            *running_status = 0;
            i
        }
    }
}

/// Return the number of data bytes expected for a channel MIDI status.
fn midi_data_length(status: u8) -> usize {
    match status & 0xF0 {
        0x80 => 2, // Note Off
        0x90 => 2, // Note On
        0xA0 => 2, // Poly Aftertouch
        0xB0 => 2, // Control Change
        0xC0 => 1, // Program Change
        0xD0 => 1, // Channel Pressure
        0xE0 => 2, // Pitch Bend
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiMessageKind;
    use dx7_core::SynthCommand;

    fn synth_cmds(msgs: &[MidiMessage]) -> Vec<SynthCommand> {
        msgs.iter()
            .map(|m| match &m.kind {
                MidiMessageKind::Synth(c) => c.clone(),
                other => panic!("expected synth command, got {:?}", other),
            })
            .collect()
    }

    #[test]
    fn test_parse_note_on() {
        // BLE MIDI packet: header, timestamp, Note On C4 vel=100
        let packet = [0x80, 0x80, 0x90, 60, 100];
        let cmds = synth_cmds(&parse_ble_midi_packet(&packet));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::NoteOn { note, velocity } => {
                assert_eq!(*note, 60);
                assert_eq!(*velocity, 100);
            }
            other => panic!("Expected NoteOn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_note_off() {
        let packet = [0x80, 0x80, 0x80, 60, 0];
        let cmds = synth_cmds(&parse_ble_midi_packet(&packet));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::NoteOff { note } => assert_eq!(*note, 60),
            other => panic!("Expected NoteOff, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_note_on_zero_velocity_is_note_off() {
        let packet = [0x80, 0x80, 0x90, 60, 0];
        let cmds = synth_cmds(&parse_ble_midi_packet(&packet));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::NoteOff { note } => assert_eq!(*note, 60),
            other => panic!("Expected NoteOff, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_control_change() {
        // CC#1 (mod wheel) value 64
        let packet = [0x80, 0x80, 0xB0, 1, 64];
        let cmds = synth_cmds(&parse_ble_midi_packet(&packet));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::ControlChange { cc, value } => {
                assert_eq!(*cc, 1);
                assert_eq!(*value, 64);
            }
            other => panic!("Expected ControlChange, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_pitch_bend() {
        // Pitch bend center (64 << 7 | 0 = 8192, minus 8192 = 0)
        let packet = [0x80, 0x80, 0xE0, 0x00, 0x40];
        let cmds = synth_cmds(&parse_ble_midi_packet(&packet));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::PitchBend { value } => assert_eq!(*value, 0),
            other => panic!("Expected PitchBend, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_packet() {
        let cmds = parse_ble_midi_packet(&[]);
        assert!(cmds.is_empty());
    }

    #[test]
    fn test_packet_too_short() {
        let cmds = parse_ble_midi_packet(&[0x80, 0x80]);
        assert!(cmds.is_empty());
    }

    #[test]
    fn test_invalid_header() {
        // Header byte must have bit 7 set
        let cmds = parse_ble_midi_packet(&[0x00, 0x80, 0x90, 60, 100]);
        assert!(cmds.is_empty());
    }

    // ── Multi-message / running-status packets ───────────────────────

    fn note_ons(msgs: &[MidiMessage]) -> Vec<(u8, u8, u8)> {
        msgs.iter()
            .filter_map(|m| match &m.kind {
                MidiMessageKind::Synth(SynthCommand::NoteOn { note, velocity }) => {
                    Some((m.channel, *note, *velocity))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn running_status_with_timestamp_per_message() {
        // From issue #21: the 0x82 is a timestamp, not a NoteOff status.
        let packet = [0x80, 0x81, 0x90, 60, 100, 0x82, 62, 80];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100), (0, 62, 80)]);
    }

    #[test]
    fn running_status_with_shared_timestamp() {
        let packet = [0x80, 0x80, 0x90, 60, 100, 62, 80, 64, 90];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100), (0, 62, 80), (0, 64, 90)]);
    }

    #[test]
    fn two_full_messages() {
        let packet = [0x80, 0x80, 0x91, 60, 100, 0x81, 0x81, 60, 0];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].channel, 1);
        assert!(matches!(msgs[0].kind, MidiMessageKind::Synth(SynthCommand::NoteOn { note: 60, velocity: 100 })));
        assert_eq!(msgs[1].channel, 1);
        assert!(matches!(msgs[1].kind, MidiMessageKind::Synth(SynthCommand::NoteOff { note: 60 })));
    }

    #[test]
    fn mixed_lengths_program_change_then_bend() {
        let packet = [0x80, 0x80, 0xC2, 5, 0x81, 0xE0, 0x00, 0x40];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].channel, 2);
        assert!(matches!(msgs[0].kind, MidiMessageKind::ProgramChange(5)));
        assert!(matches!(msgs[1].kind, MidiMessageKind::Synth(SynthCommand::PitchBend { value: 0 })));
    }

    #[test]
    fn timestamp_byte_f7_is_not_sysex_end() {
        // Timestamp-low 0xF7 before a plain NoteOn must not be mistaken for EOX.
        let packet = [0x80, 0xF7, 0x90, 60, 100];
        assert_eq!(note_ons(&parse_ble_midi_packet(&packet)), vec![(0, 60, 100)]);
    }

    #[test]
    fn sysex_is_skipped_up_to_timestamped_f7() {
        // SysEx with a 0x77 data byte (would be 0xF7 with bit 7) and a
        // timestamp of 0xF7 inside; only the [ts][F7] pair ends it.
        let packet = [0x80, 0x80, 0xF0, 0x43, 0x77, 0x10, 0xF7, 0xF7, 0x82, 0x90, 60, 100];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100)]);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn sysex_clears_running_status() {
        // After SysEx, bare data bytes have no status and must be ignored.
        let packet = [0x80, 0x80, 0x90, 60, 100, 0x81, 0xF0, 0x01, 0x82, 0xF7, 0x83, 62, 80];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100)]);
    }

    #[test]
    fn system_common_clears_running_status() {
        // Song Position Pointer (F2 + 2 data) then running-status data.
        let packet = [0x80, 0x80, 0x90, 60, 100, 0x81, 0xF2, 0x00, 0x10, 0x82, 62, 80];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100)]);
    }

    #[test]
    fn realtime_preserves_running_status() {
        // Timing clock between two running-status notes.
        let packet = [0x80, 0x80, 0x90, 60, 100, 0x81, 0xF8, 0x82, 62, 80];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100), (0, 62, 80)]);
    }

    #[test]
    fn realtime_inside_sysex_is_tolerated() {
        let packet = [0x80, 0x80, 0xF0, 0x43, 0x81, 0xF8, 0x10, 0x82, 0xF7, 0x83, 0x90, 60, 100];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100)]);
    }

    #[test]
    fn truncated_trailing_message_is_dropped() {
        let packet = [0x80, 0x80, 0x90, 60, 100, 0x81, 0x90, 62];
        let msgs = parse_ble_midi_packet(&packet);
        assert_eq!(note_ons(&msgs), vec![(0, 60, 100)]);
    }

    #[test]
    fn sysex_continuation_packet_yields_nothing() {
        // A packet that is the middle of a SysEx started in a previous packet.
        let packet = [0x80, 0x01, 0x02, 0x03, 0x04];
        assert!(parse_ble_midi_packet(&packet).is_empty());
    }
}
