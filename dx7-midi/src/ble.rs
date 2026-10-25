use crate::{MidiMessage, MidiQueue};

/// Incremental BLE-MIDI packet parser.
///
/// Packet layout (BLE-MIDI 1.0, section 4):
///
/// ```text
/// [header] ( [timestamp] [status] [data...] | [timestamp] [data...] | [data...] )*
/// ```
///
/// * `header`: bit 7 set, bit 6 clear, bits 0-5 = timestamp high.
/// * Every message starts with a `timestamp` byte (bit 7 set). It is *never*
///   a status byte, no matter what its low bits look like, so `0x80..=0xEF`
///   in timestamp position is not a channel status and `0xF7` in timestamp
///   position does not end a SysEx.
/// * A `status` byte (bit 7 set) may follow the timestamp. If instead a data
///   byte follows, the message uses running status from earlier in the same
///   packet. A data byte directly after a complete message (no timestamp) is
///   also running status and reuses the previous timestamp.
/// * Running status does not survive across packets; System Common and
///   System Exclusive status bytes clear it (MIDI 1.0). System Real-Time
///   (`0xF8..=0xFF`) is a single byte that leaves running status untouched.
/// * SysEx may span packets: a packet can end inside the payload and the next
///   packet continues with `[header] [data...]`. The terminating `0xF7` is a
///   status byte and is therefore preceded by a timestamp. Real-Time
///   messages (`[timestamp] [0xF8..=0xFF]`) may be interleaved inside a
///   SysEx payload.
///
/// Channel messages are pushed to the [`MidiQueue`]; every SysEx byte from
/// `0xF0` through `0xF7` inclusive is handed to the `sysex` sink so callers
/// can run the same accumulator they use for USB.
#[derive(Debug, Default, Clone, Copy)]
pub struct BleMidiParser {
    /// A SysEx started in an earlier packet and has not seen its `0xF7` yet.
    in_sysex: bool,
}

impl BleMidiParser {
    pub const fn new() -> Self {
        Self { in_sysex: false }
    }

    /// True while a SysEx payload is open across packet boundaries.
    pub fn in_sysex(&self) -> bool {
        self.in_sysex
    }

    /// Parse one BLE-MIDI packet (one characteristic write).
    pub fn parse(&mut self, data: &[u8], queue: &MidiQueue, sysex: &mut dyn FnMut(u8)) {
        // Header plus at least one more byte.
        if data.len() < 2 {
            return;
        }

        let mut pos = 1; // skip header
        let mut running_status: u8 = 0;

        while pos < data.len() {
            // --- inside a SysEx payload (possibly continued from a previous packet)
            if self.in_sysex {
                let b = data[pos];
                if b & 0x80 == 0 {
                    sysex(b);
                    pos += 1;
                    continue;
                }
                // Timestamp. The byte after it is a status byte.
                pos += 1;
                let Some(&status) = data.get(pos) else { break };
                if status == 0xF7 {
                    sysex(0xF7);
                    self.in_sysex = false;
                    pos += 1;
                } else if status >= 0xF8 {
                    // Real-Time interleaved in SysEx: ignore, stay in SysEx.
                    pos += 1;
                } else {
                    // Any other status terminates the SysEx abnormally. Leave
                    // `pos` on the status byte so it is parsed as a message.
                    self.in_sysex = false;
                    pos -= 1;
                }
                continue;
            }

            let b = data[pos];

            // --- data byte with no timestamp: running status, previous timestamp
            if b & 0x80 == 0 {
                if running_status == 0 {
                    // Stray data byte with nothing to attach it to.
                    pos += 1;
                    continue;
                }
                match try_parse(running_status, &data[pos..], queue) {
                    Some(adv) => pos += adv,
                    None => break, // truncated
                }
                continue;
            }

            // --- timestamp byte
            pos += 1;
            let Some(&next) = data.get(pos) else { break };

            if next & 0x80 == 0 {
                // Timestamp followed by data: running status.
                if running_status == 0 {
                    break; // desync: cannot interpret
                }
                match try_parse(running_status, &data[pos..], queue) {
                    Some(adv) => pos += adv,
                    None => break,
                }
                continue;
            }

            // --- status byte
            let status = next;
            pos += 1;
            match status {
                0xF0 => {
                    running_status = 0;
                    self.in_sysex = true;
                    sysex(0xF0);
                }
                0xF1..=0xF7 => {
                    // System Common: clears running status; skip its data.
                    running_status = 0;
                    let data_len = match status {
                        0xF1 | 0xF3 => 1,
                        0xF2 => 2,
                        _ => 0, // F4/F5 undefined, F6 tune request, F7 stray end
                    };
                    pos += data_len;
                }
                0xF8..=0xFF => {
                    // System Real-Time: single byte, running status untouched.
                }
                _ => {
                    running_status = status;
                    match try_parse(status, &data[pos..], queue) {
                        Some(adv) => pos += adv,
                        None => break, // truncated
                    }
                }
            }
        }
    }
}

/// Parse a single self-contained BLE-MIDI packet, discarding any SysEx.
///
/// Convenience wrapper over [`BleMidiParser`] for callers that do not need
/// SysEx or cross-packet state.
pub fn parse_ble_midi_packet(data: &[u8], queue: &MidiQueue) {
    BleMidiParser::new().parse(data, queue, &mut |_| {});
}

fn try_parse(status: u8, data: &[u8], queue: &MidiQueue) -> Option<usize> {
    let channel = status & 0x0F;
    match status & 0xF0 {
        0x80 if data.len() >= 2 => {
            queue.push(MidiMessage::NoteOff {
                channel,
                note: data[0] & 0x7F,
                velocity: data[1] & 0x7F,
            });
            Some(2)
        }
        0x90 if data.len() >= 2 => {
            let note = data[0] & 0x7F;
            let vel = data[1] & 0x7F;
            if vel == 0 {
                queue.push(MidiMessage::NoteOff { channel, note, velocity: 0 });
            } else {
                queue.push(MidiMessage::NoteOn { channel, note, velocity: vel });
            }
            Some(2)
        }
        0xA0 if data.len() >= 2 => {
            queue.push(MidiMessage::PolyPressure {
                channel,
                note: data[0] & 0x7F,
                pressure: data[1] & 0x7F,
            });
            Some(2)
        }
        0xB0 if data.len() >= 2 => {
            queue.push(MidiMessage::ControlChange {
                channel,
                controller: data[0] & 0x7F,
                value: data[1] & 0x7F,
            });
            Some(2)
        }
        0xC0 if !data.is_empty() => {
            queue.push(MidiMessage::ProgramChange { channel, program: data[0] & 0x7F });
            Some(1)
        }
        0xD0 if !data.is_empty() => {
            queue.push(MidiMessage::ChannelPressure { channel, pressure: data[0] & 0x7F });
            Some(1)
        }
        0xE0 if data.len() >= 2 => {
            let lsb = (data[0] & 0x7F) as u16;
            let msb = (data[1] & 0x7F) as u16;
            queue.push(MidiMessage::PitchBend { channel, value: (msb << 7) | lsb });
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
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }]);
    }

    #[test]
    fn note_on_velocity_zero_becomes_note_off() {
        let msgs = collect(&[0x80, 0x80, 0x90, 0x3C, 0x00]);
        assert_eq!(msgs, [MidiMessage::NoteOff { channel: 0, note: 60, velocity: 0 }]);
    }

    #[test]
    fn note_off() {
        let msgs = collect(&[0x80, 0x80, 0x80, 0x3C, 0x40]);
        assert_eq!(msgs, [MidiMessage::NoteOff { channel: 0, note: 60, velocity: 64 }]);
    }

    #[test]
    fn poly_pressure() {
        let msgs = collect(&[0x80, 0x80, 0xA0, 0x3C, 0x50]);
        assert_eq!(msgs, [MidiMessage::PolyPressure { channel: 0, note: 60, pressure: 80 }]);
    }

    #[test]
    fn control_change() {
        // CC#1 (mod wheel) value 64
        let msgs = collect(&[0x80, 0x80, 0xB0, 0x01, 0x40]);
        assert_eq!(msgs, [MidiMessage::ControlChange { channel: 0, controller: 1, value: 64 }]);
    }

    #[test]
    fn program_change() {
        let msgs = collect(&[0x80, 0x80, 0xC0, 0x05]);
        assert_eq!(msgs, [MidiMessage::ProgramChange { channel: 0, program: 5 }]);
    }

    #[test]
    fn channel_pressure() {
        let msgs = collect(&[0x80, 0x80, 0xD0, 0x60]);
        assert_eq!(msgs, [MidiMessage::ChannelPressure { channel: 0, pressure: 96 }]);
    }

    #[test]
    fn pitch_bend() {
        // Pitch bend center = 0x2000 (MSB=0x40, LSB=0x00)
        let msgs = collect(&[0x80, 0x80, 0xE0, 0x00, 0x40]);
        assert_eq!(msgs, [MidiMessage::PitchBend { channel: 0, value: 0x2000 }]);
    }

    #[test]
    fn pitch_bend_max() {
        // Pitch bend max = 0x3FFF (MSB=0x7F, LSB=0x7F)
        let msgs = collect(&[0x80, 0x80, 0xE0, 0x7F, 0x7F]);
        assert_eq!(msgs, [MidiMessage::PitchBend { channel: 0, value: 0x3FFF }]);
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
            MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 },
            MidiMessage::NoteOn { channel: 0, note: 64, velocity: 80 },
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
            MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 },
            MidiMessage::NoteOn { channel: 0, note: 64, velocity: 80 },
        ]);
    }

    #[test]
    fn sysex_skipped() {
        // SysEx followed by a note on. Per spec the terminating F7 is a status
        // byte and so carries its own timestamp.
        let msgs = collect(&[
            0x80,
            0x80, 0xF0, 0x7E, 0x7F, 0x80, 0xF7, // SysEx (identity request)
            0x80, 0x90, 0x3C, 0x64,             // note on
        ]);
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }]);
    }

    /// Collect channel messages and SysEx bytes with a persistent parser
    /// across several packets.
    fn collect_packets(packets: &[&[u8]]) -> (Vec<MidiMessage>, Vec<u8>) {
        let q = MidiQueue::new();
        let mut sysex = Vec::new();
        let mut parser = BleMidiParser::new();
        for p in packets {
            parser.parse(p, &q, &mut |b| sysex.push(b));
        }
        let mut msgs = Vec::new();
        while let Some(m) = q.pop() {
            msgs.push(m);
        }
        (msgs, sysex)
    }

    #[test]
    fn multi_message_timestamps_are_not_status_bytes() {
        // Issue #21: the second timestamp 0x82 looks like a NoteOff status
        // but is a timestamp; 62/80 are running-status NoteOn data.
        let msgs = collect(&[0x80, 0x81, 0x90, 60, 100, 0x82, 62, 80]);
        assert_eq!(msgs, [
            MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 },
            MidiMessage::NoteOn { channel: 0, note: 62, velocity: 80 },
        ]);
    }

    #[test]
    fn running_status_without_timestamp() {
        // Data bytes directly after a complete message reuse status and timestamp.
        let msgs = collect(&[0x80, 0x80, 0x90, 60, 100, 62, 80, 64, 90]);
        assert_eq!(msgs, [
            MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 },
            MidiMessage::NoteOn { channel: 0, note: 62, velocity: 80 },
            MidiMessage::NoteOn { channel: 0, note: 64, velocity: 90 },
        ]);
    }

    #[test]
    fn running_status_mixed_with_new_status() {
        let msgs = collect(&[
            0x80,
            0x80, 0x90, 60, 100,  // note on
            0x81, 62, 80,         // running status note on
            0x82, 0x80, 60, 0,    // explicit note off
            0x83, 62, 0,          // running status note off
        ]);
        assert_eq!(msgs, [
            MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 },
            MidiMessage::NoteOn { channel: 0, note: 62, velocity: 80 },
            MidiMessage::NoteOff { channel: 0, note: 60, velocity: 0 },
            MidiMessage::NoteOff { channel: 0, note: 62, velocity: 0 },
        ]);
    }

    #[test]
    fn sysex_bytes_delivered_to_sink() {
        let (msgs, sysex) = collect_packets(&[&[
            0x80,
            0x80, 0xF0, 0x7D, 0x01, 0x01, 0x80, 0xF7, // mode switch SysEx
            0x80, 0x90, 60, 100,
        ]]);
        assert_eq!(sysex, [0xF0, 0x7D, 0x01, 0x01, 0xF7]);
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }]);
    }

    #[test]
    fn sysex_spans_packets() {
        let (msgs, sysex) = collect_packets(&[
            &[0x80, 0x80, 0xF0, 0x43, 0x00],  // start, ends mid-payload
            &[0x80, 0x09, 0x20],              // continuation: header + data
            &[0x80, 0x00, 0x80, 0xF7, 0x80, 0x90, 60, 100], // end + note on
        ]);
        assert_eq!(sysex, [0xF0, 0x43, 0x00, 0x09, 0x20, 0x00, 0xF7]);
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }]);
    }

    #[test]
    fn sysex_only_ends_at_f7_in_status_position() {
        // A timestamp byte that happens to be 0xF7 (followed by a Real-Time
        // clock 0xF8) must not terminate the SysEx.
        let (msgs, sysex) = collect_packets(&[&[
            0x80,
            0x80, 0xF0, 0x7E, 0xF7, 0xF8, 0x7F, 0x80, 0xF7,
            0x80, 0x90, 60, 100,
        ]]);
        assert_eq!(sysex, [0xF0, 0x7E, 0x7F, 0xF7]);
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }]);
    }

    #[test]
    fn system_common_clears_running_status() {
        // Song Select (F3 + 1 data byte) between a NoteOn and running-status
        // data: the data after it has no status and must be dropped.
        let msgs = collect(&[
            0x80,
            0x80, 0x90, 60, 100,
            0x80, 0xF3, 0x05,
            0x80, 62, 80,
        ]);
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }]);
    }

    #[test]
    fn realtime_keeps_running_status() {
        let msgs = collect(&[
            0x80,
            0x80, 0x90, 60, 100,
            0x80, 0xF8,       // clock
            0x80, 62, 80,     // running status still valid
        ]);
        assert_eq!(msgs, [
            MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 },
            MidiMessage::NoteOn { channel: 0, note: 62, velocity: 80 },
        ]);
    }

    #[test]
    fn truncated_message_ignored() {
        assert!(collect(&[0x80, 0x80, 0x90, 60]).is_empty());
        assert!(collect(&[0x80, 0x80]).is_empty());
        assert!(collect(&[0x80]).is_empty());
    }

    #[test]
    fn running_status_does_not_cross_packets() {
        let (msgs, _) = collect_packets(&[
            &[0x80, 0x80, 0x90, 60, 100],
            &[0x80, 0x80, 62, 80], // no status in this packet: dropped
        ]);
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }]);
    }

    #[test]
    fn channel_extracted() {
        // Note on channel 5: status = 0x95
        let msgs = collect(&[0x80, 0x80, 0x95, 0x3C, 0x64]);
        assert_eq!(msgs, [MidiMessage::NoteOn { channel: 5, note: 60, velocity: 100 }]);
    }
}
