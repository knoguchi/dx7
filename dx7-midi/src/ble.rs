use crate::{MidiMessage, MidiQueue};

/// Parse raw MIDI bytes (from BLE MIDI characteristic write) into messages.
/// BLE MIDI format: [header] [timestamp+status] [data...]
pub fn parse_ble_midi_packet(data: &[u8], queue: &MidiQueue) {
    if data.len() < 3 {
        return;
    }

    let mut pos = 1; // skip header byte
    let mut running_status: u8 = 0;

    while pos < data.len() {
        let b = data[pos];

        // Timestamp byte (bit7 set) before a status or data byte
        if b & 0x80 != 0 && pos + 1 < data.len() {
            if data[pos + 1] & 0x80 != 0 {
                // Timestamp followed by status byte — skip timestamp,
                // fall through to status handler below
                pos += 1;
            } else {
                // Timestamp followed by data byte (running status)
                pos += 1;
                if running_status != 0 {
                    if let Some(advance) = try_parse(running_status, &data[pos..], queue) {
                        pos += advance;
                        continue;
                    }
                }
                break;
            }
        }

        let b = data[pos];

        // Status byte
        if b & 0x80 != 0 {
            if b >= 0xF0 {
                // System messages — skip
                pos += 1;
                if b == 0xF0 {
                    while pos < data.len() && data[pos] != 0xF7 { pos += 1; }
                    pos += 1;
                }
                continue;
            }
            running_status = b;
            pos += 1;
            if let Some(advance) = try_parse(b, &data[pos..], queue) {
                pos += advance;
            }
        } else {
            pos += 1;
        }
    }
}

fn try_parse(status: u8, data: &[u8], queue: &MidiQueue) -> Option<usize> {
    match status & 0xF0 {
        0x80 if data.len() >= 2 => {
            queue.push(MidiMessage::NoteOff {
                note: data[0] & 0x7F,
                velocity: data[1] & 0x7F,
            });
            Some(2)
        }
        0x90 if data.len() >= 2 => {
            let note = data[0] & 0x7F;
            let vel = data[1] & 0x7F;
            if vel == 0 {
                queue.push(MidiMessage::NoteOff { note, velocity: 0 });
            } else {
                queue.push(MidiMessage::NoteOn { note, velocity: vel });
            }
            Some(2)
        }
        0xA0 if data.len() >= 2 => {
            queue.push(MidiMessage::PolyPressure {
                note: data[0] & 0x7F,
                pressure: data[1] & 0x7F,
            });
            Some(2)
        }
        0xB0 if data.len() >= 2 => {
            queue.push(MidiMessage::ControlChange {
                controller: data[0] & 0x7F,
                value: data[1] & 0x7F,
            });
            Some(2)
        }
        0xC0 if !data.is_empty() => {
            queue.push(MidiMessage::ProgramChange { program: data[0] & 0x7F });
            Some(1)
        }
        0xD0 if !data.is_empty() => {
            queue.push(MidiMessage::ChannelPressure { pressure: data[0] & 0x7F });
            Some(1)
        }
        0xE0 if data.len() >= 2 => {
            let lsb = (data[0] & 0x7F) as u16;
            let msb = (data[1] & 0x7F) as u16;
            queue.push(MidiMessage::PitchBend { value: (msb << 7) | lsb });
            Some(2)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(data: &[u8]) -> Vec<MidiMessage> {
        let q = MidiQueue::new();
        parse_ble_midi_packet(data, &q);
        let mut msgs = Vec::new();
        while let Some(m) = q.pop() {
            msgs.push(m);
        }
        msgs
    }

    #[test]
    fn too_short_ignored() {
        assert!(collect(&[0x80, 0x80]).is_empty());
        assert!(collect(&[]).is_empty());
    }

    #[test]
    fn note_on() {
        // header, timestamp, status 0x90, note 60, velocity 100
        let msgs = collect(&[0x80, 0x80, 0x90, 0x3C, 0x64]);
        assert_eq!(msgs, [MidiMessage::NoteOn { note: 60, velocity: 100 }]);
    }

    #[test]
    fn note_on_velocity_zero_becomes_note_off() {
        let msgs = collect(&[0x80, 0x80, 0x90, 0x3C, 0x00]);
        assert_eq!(msgs, [MidiMessage::NoteOff { note: 60, velocity: 0 }]);
    }

    #[test]
    fn note_off() {
        let msgs = collect(&[0x80, 0x80, 0x80, 0x3C, 0x40]);
        assert_eq!(msgs, [MidiMessage::NoteOff { note: 60, velocity: 64 }]);
    }

    #[test]
    fn poly_pressure() {
        let msgs = collect(&[0x80, 0x80, 0xA0, 0x3C, 0x50]);
        assert_eq!(msgs, [MidiMessage::PolyPressure { note: 60, pressure: 80 }]);
    }

    #[test]
    fn control_change() {
        // CC#1 (mod wheel) value 64
        let msgs = collect(&[0x80, 0x80, 0xB0, 0x01, 0x40]);
        assert_eq!(msgs, [MidiMessage::ControlChange { controller: 1, value: 64 }]);
    }

    #[test]
    fn program_change() {
        let msgs = collect(&[0x80, 0x80, 0xC0, 0x05]);
        assert_eq!(msgs, [MidiMessage::ProgramChange { program: 5 }]);
    }

    #[test]
    fn channel_pressure() {
        let msgs = collect(&[0x80, 0x80, 0xD0, 0x60]);
        assert_eq!(msgs, [MidiMessage::ChannelPressure { pressure: 96 }]);
    }

    #[test]
    fn pitch_bend() {
        // Pitch bend center = 0x2000 (MSB=0x40, LSB=0x00)
        let msgs = collect(&[0x80, 0x80, 0xE0, 0x00, 0x40]);
        assert_eq!(msgs, [MidiMessage::PitchBend { value: 0x2000 }]);
    }

    #[test]
    fn pitch_bend_max() {
        // Pitch bend max = 0x3FFF (MSB=0x7F, LSB=0x7F)
        let msgs = collect(&[0x80, 0x80, 0xE0, 0x7F, 0x7F]);
        assert_eq!(msgs, [MidiMessage::PitchBend { value: 0x3FFF }]);
    }

    #[test]
    fn multiple_messages() {
        // Two note-ons in one packet, each with its own timestamp
        let msgs = collect(&[
            0x80,
            0x80, 0x90, 0x3C, 0x64, // note on C4
            0x80, 0x90, 0x40, 0x50, // note on E4
        ]);
        assert_eq!(msgs, [
            MidiMessage::NoteOn { note: 60, velocity: 100 },
            MidiMessage::NoteOn { note: 64, velocity: 80 },
        ]);
    }

    #[test]
    fn running_status() {
        // Note on, then running status (timestamp + data bytes, no new status)
        let msgs = collect(&[
            0x80,
            0x80, 0x90, 0x3C, 0x64, // note on C4
            0x80, 0x40, 0x50,        // running status: note on E4
        ]);
        assert_eq!(msgs, [
            MidiMessage::NoteOn { note: 60, velocity: 100 },
            MidiMessage::NoteOn { note: 64, velocity: 80 },
        ]);
    }

    #[test]
    fn sysex_skipped() {
        // SysEx followed by a note on
        let msgs = collect(&[
            0x80,
            0x80, 0xF0, 0x7E, 0x7F, 0xF7, // SysEx (identity request)
            0x80, 0x90, 0x3C, 0x64,          // note on
        ]);
        assert_eq!(msgs, [MidiMessage::NoteOn { note: 60, velocity: 100 }]);
    }
}
