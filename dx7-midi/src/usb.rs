use crate::{MidiMessage, MidiQueue};

/// Parse a 4-byte USB MIDI Event Packet into a MidiMessage.
///
/// USB MIDI format: [cable_number(4) | CIN(4)] [status] [data1] [data2]
pub fn parse_usb_midi_event(packet: &[u8], queue: &MidiQueue) {
    if packet.len() < 4 {
        return;
    }

    let cin = packet[0] & 0x0F;
    let data1 = packet[2];
    let data2 = packet[3];

    match cin {
        0x08 => {
            // Note Off
            queue.push(MidiMessage::NoteOff {
                note: data1 & 0x7F,
                velocity: data2 & 0x7F,
            });
        }
        0x09 => {
            // Note On
            if data2 == 0 {
                queue.push(MidiMessage::NoteOff { note: data1 & 0x7F, velocity: 0 });
            } else {
                queue.push(MidiMessage::NoteOn {
                    note: data1 & 0x7F,
                    velocity: data2 & 0x7F,
                });
            }
        }
        0x0A => {
            // Poly Aftertouch
            queue.push(MidiMessage::PolyPressure {
                note: data1 & 0x7F,
                pressure: data2 & 0x7F,
            });
        }
        0x0B => {
            // Control Change
            queue.push(MidiMessage::ControlChange {
                controller: data1 & 0x7F,
                value: data2 & 0x7F,
            });
        }
        0x0C => {
            // Program Change
            queue.push(MidiMessage::ProgramChange { program: data1 & 0x7F });
        }
        0x0D => {
            // Channel Pressure
            queue.push(MidiMessage::ChannelPressure { pressure: data1 & 0x7F });
        }
        0x0E => {
            // Pitch Bend
            let lsb = (data1 & 0x7F) as u16;
            let msb = (data2 & 0x7F) as u16;
            queue.push(MidiMessage::PitchBend { value: (msb << 7) | lsb });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(packet: &[u8]) -> Vec<MidiMessage> {
        let q = MidiQueue::new();
        parse_usb_midi_event(packet, &q);
        let mut msgs = Vec::new();
        while let Some(m) = q.pop() {
            msgs.push(m);
        }
        msgs
    }

    #[test]
    fn too_short_ignored() {
        assert!(collect(&[0x09, 0x90, 0x3C]).is_empty());
        assert!(collect(&[]).is_empty());
    }

    #[test]
    fn note_on() {
        let msgs = collect(&[0x09, 0x90, 0x3C, 0x64]);
        assert_eq!(msgs, [MidiMessage::NoteOn { note: 60, velocity: 100 }]);
    }

    #[test]
    fn note_on_velocity_zero_becomes_note_off() {
        let msgs = collect(&[0x09, 0x90, 0x3C, 0x00]);
        assert_eq!(msgs, [MidiMessage::NoteOff { note: 60, velocity: 0 }]);
    }

    #[test]
    fn note_off() {
        let msgs = collect(&[0x08, 0x80, 0x3C, 0x40]);
        assert_eq!(msgs, [MidiMessage::NoteOff { note: 60, velocity: 64 }]);
    }

    #[test]
    fn poly_pressure() {
        let msgs = collect(&[0x0A, 0xA0, 0x3C, 0x50]);
        assert_eq!(msgs, [MidiMessage::PolyPressure { note: 60, pressure: 80 }]);
    }

    #[test]
    fn control_change() {
        let msgs = collect(&[0x0B, 0xB0, 0x01, 0x40]);
        assert_eq!(msgs, [MidiMessage::ControlChange { controller: 1, value: 64 }]);
    }

    #[test]
    fn program_change() {
        let msgs = collect(&[0x0C, 0xC0, 0x05, 0x00]);
        assert_eq!(msgs, [MidiMessage::ProgramChange { program: 5 }]);
    }

    #[test]
    fn channel_pressure() {
        let msgs = collect(&[0x0D, 0xD0, 0x60, 0x00]);
        assert_eq!(msgs, [MidiMessage::ChannelPressure { pressure: 96 }]);
    }

    #[test]
    fn pitch_bend() {
        let msgs = collect(&[0x0E, 0xE0, 0x00, 0x40]);
        assert_eq!(msgs, [MidiMessage::PitchBend { value: 0x2000 }]);
    }

    #[test]
    fn unknown_cin_ignored() {
        assert!(collect(&[0x0F, 0x00, 0x00, 0x00]).is_empty());
    }

    #[test]
    fn cable_number_masked() {
        // Cable 1 (upper nibble = 0x10), CIN = 0x09 note on
        let msgs = collect(&[0x19, 0x90, 0x3C, 0x64]);
        assert_eq!(msgs, [MidiMessage::NoteOn { note: 60, velocity: 100 }]);
    }
}
