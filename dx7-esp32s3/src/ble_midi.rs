/// BLE MIDI transport.
///
/// Implements "MIDI over Bluetooth Low Energy" (BLE-MIDI) spec.
/// Uses trouble-host for the BLE GATT server.

/// MIDI message types we care about
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum MidiMessage {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    ControlChange { controller: u8, value: u8 },
    ProgramChange { program: u8 },
}

/// Simple lock-free single-producer single-consumer ring buffer for MIDI messages.
/// BLE task writes, audio task reads.
pub struct MidiQueue {
    buf: [MidiMessage; 32],
    head: core::sync::atomic::AtomicUsize,
    tail: core::sync::atomic::AtomicUsize,
}

impl MidiQueue {
    pub const fn new() -> Self {
        const EMPTY: MidiMessage = MidiMessage::NoteOff { note: 0 };
        Self {
            buf: [EMPTY; 32],
            head: core::sync::atomic::AtomicUsize::new(0),
            tail: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn push(&self, msg: MidiMessage) {
        use core::sync::atomic::Ordering;
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % self.buf.len();
        if next != self.tail.load(Ordering::Acquire) {
            // Safety: only one writer (BLE task)
            unsafe {
                let ptr = self.buf.as_ptr() as *mut MidiMessage;
                ptr.add(head).write(msg);
            }
            self.head.store(next, Ordering::Release);
        }
    }

    pub fn pop(&self) -> Option<MidiMessage> {
        use core::sync::atomic::Ordering;
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let msg = unsafe {
            let ptr = self.buf.as_ptr();
            ptr.add(tail).read()
        };
        self.tail.store((tail + 1) % self.buf.len(), Ordering::Release);
        Some(msg)
    }
}

// Safety: accessed from BLE task (push) and audio task (pop) — SPSC is safe
unsafe impl Sync for MidiQueue {}

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
        if b & 0x80 != 0 {
            if pos + 1 < data.len() && data[pos + 1] & 0x80 != 0 {
                // Timestamp followed by status byte
                pos += 1; // skip timestamp, next iteration handles status
                continue;
            } else if pos + 1 < data.len() && data[pos + 1] & 0x80 == 0 {
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
        0x90 if data.len() >= 2 => {
            let note = data[0] & 0x7F;
            let vel = data[1] & 0x7F;
            if vel == 0 {
                queue.push(MidiMessage::NoteOff { note });
            } else {
                queue.push(MidiMessage::NoteOn { note, velocity: vel });
            }
            Some(2)
        }
        0x80 if data.len() >= 2 => {
            queue.push(MidiMessage::NoteOff { note: data[0] & 0x7F });
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
        _ => None,
    }
}
