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
/// BLE MIDI wire format:
///   [header: 0b10xxxxxx] [timestamp: 0b1xxxxxxx] [status] [data...] ...
///
/// Multiple MIDI messages can be packed into a single BLE packet.
/// Running status is supported within a packet.
pub(crate) fn parse_ble_midi_packet(data: &[u8]) -> Vec<MidiMessage> {
    let mut commands = Vec::new();

    if data.len() < 3 {
        return commands;
    }

    // First byte is the header (bit 7 set)
    if data[0] & 0x80 == 0 {
        return commands;
    }

    let mut i = 1;
    let mut running_status: u8 = 0;

    while i < data.len() {
        // Skip timestamp bytes (bit 7 set, appear before status/data)
        if data[i] & 0x80 != 0 && (i + 1 < data.len()) && is_midi_status(data[i + 1]) {
            // This is a timestamp byte followed by a status byte
            i += 1; // skip timestamp
            continue;
        }

        if data[i] & 0x80 != 0 && !is_midi_status(data[i]) {
            // Timestamp byte (not a MIDI status)
            i += 1;
            continue;
        }

        // MIDI status byte or data byte (running status)
        if is_midi_status(data[i]) {
            let status = data[i];

            // Skip SysEx
            if status == 0xF0 {
                while i < data.len() && data[i] != 0xF7 {
                    i += 1;
                }
                i += 1; // skip F7
                continue;
            }

            // Skip system common/realtime
            if status >= 0xF0 {
                i += 1;
                continue;
            }

            running_status = status;
            i += 1;
        }

        if running_status == 0 {
            i += 1;
            continue;
        }

        // Extract data bytes based on the running status
        let data_len = midi_data_length(running_status);
        if i + data_len > data.len() {
            break;
        }

        let mut msg = vec![running_status];
        for j in 0..data_len {
            msg.push(data[i + j]);
        }
        i += data_len;

        if let Some(msg) = midi::parse_midi_message(&msg) {
            commands.push(msg);
        }
    }

    commands
}

/// Check if a byte is a MIDI status byte (channel voice/mode message).
fn is_midi_status(byte: u8) -> bool {
    byte >= 0x80 && byte <= 0xEF
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
}
