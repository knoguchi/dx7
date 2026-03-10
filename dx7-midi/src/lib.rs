#![cfg_attr(not(test), no_std)]

pub mod ble;
pub mod drum;
pub mod gm;
mod gm_rom;
pub mod usb;

/// All channel voice MIDI messages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiMessage {
    NoteOff { channel: u8, note: u8, velocity: u8 },
    NoteOn { channel: u8, note: u8, velocity: u8 },
    PolyPressure { channel: u8, note: u8, pressure: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    ProgramChange { channel: u8, program: u8 },
    ChannelPressure { channel: u8, pressure: u8 },
    PitchBend { channel: u8, value: u16 },
}

/// Simple lock-free single-producer single-consumer ring buffer for MIDI messages.
/// BLE task writes, audio task reads.
pub struct MidiQueue {
    buf: [MidiMessage; 256],
    head: core::sync::atomic::AtomicUsize,
    tail: core::sync::atomic::AtomicUsize,
}

impl MidiQueue {
    pub const fn new() -> Self {
        const EMPTY: MidiMessage = MidiMessage::NoteOff { channel: 0, note: 0, velocity: 0 };
        Self {
            buf: [EMPTY; 256],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pop_empty_returns_none() {
        let q = MidiQueue::new();
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn push_pop_round_trip() {
        let q = MidiQueue::new();
        q.push(MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 });
        assert_eq!(q.pop(), Some(MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn fifo_ordering() {
        let q = MidiQueue::new();
        q.push(MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 });
        q.push(MidiMessage::NoteOff { channel: 0, note: 60, velocity: 0 });
        q.push(MidiMessage::ProgramChange { channel: 0, program: 5 });
        assert_eq!(q.pop(), Some(MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }));
        assert_eq!(q.pop(), Some(MidiMessage::NoteOff { channel: 0, note: 60, velocity: 0 }));
        assert_eq!(q.pop(), Some(MidiMessage::ProgramChange { channel: 0, program: 5 }));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn full_queue_drops_message() {
        let q = MidiQueue::new();
        // Fill all 255 slots (capacity = buf.len() - 1)
        for i in 0..255 {
            q.push(MidiMessage::NoteOn { channel: 0, note: (i % 128) as u8, velocity: 100 });
        }
        // This should be silently dropped
        q.push(MidiMessage::NoteOn { channel: 0, note: 99, velocity: 127 });
        // Drain and verify the 256th message was dropped
        for i in 0..255 {
            assert_eq!(q.pop(), Some(MidiMessage::NoteOn { channel: 0, note: (i % 128) as u8, velocity: 100 }));
        }
        assert_eq!(q.pop(), None);
    }
}
