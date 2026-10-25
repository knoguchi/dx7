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

/// Lock-free ring buffer carrying MIDI messages from the input tasks to the
/// audio task.
///
/// Concurrency contract (the only thing that makes the `unsafe` here sound):
///
/// * **One consumer.** Exactly one task calls [`pop`](Self::pop).
/// * **Producers must not interleave.** Several tasks may call
///   [`push`](Self::push) (USB reader, BLE handler, the SysEx mode switch), but
///   they must all run on the same executor on the same core, cooperatively
///   scheduled, so two `push` calls can never execute concurrently or preempt
///   each other. `push` contains no `.await`, so a cooperative task cannot be
///   suspended in the middle of one. Calling `push` from an interrupt handler
///   or from a second core violates this contract and is a data race on
///   `head`'s read-modify-write.
/// * Producer and consumer may be on different cores; the Release/Acquire
///   pairs on `head`/`tail` publish the slot contents across that boundary.
///
/// The storage lives in an `UnsafeCell` so that writing a slot through `&self`
/// is a legal interior mutation rather than a write through a pointer derived
/// from a shared reference.
pub struct MidiQueue {
    buf: core::cell::UnsafeCell<[MidiMessage; Self::CAPACITY]>,
    head: core::sync::atomic::AtomicUsize,
    tail: core::sync::atomic::AtomicUsize,
}

impl MidiQueue {
    /// Ring size. Usable capacity is `CAPACITY - 1` (one slot distinguishes
    /// full from empty).
    pub const CAPACITY: usize = 256;

    pub const fn new() -> Self {
        const EMPTY: MidiMessage = MidiMessage::NoteOff { channel: 0, note: 0, velocity: 0 };
        Self {
            buf: core::cell::UnsafeCell::new([EMPTY; Self::CAPACITY]),
            head: core::sync::atomic::AtomicUsize::new(0),
            tail: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Enqueue a message. Silently dropped if the queue is full.
    ///
    /// See the type-level docs for the producer constraint.
    pub fn push(&self, msg: MidiMessage) {
        use core::sync::atomic::Ordering;
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % Self::CAPACITY;
        if next != self.tail.load(Ordering::Acquire) {
            // SAFETY: slot `head` is owned by the producer side until the
            // Release store below publishes it; the consumer only reads slots
            // in `tail..head`. Producers never run concurrently (contract).
            unsafe {
                let slots = self.buf.get() as *mut MidiMessage;
                slots.add(head).write(msg);
            }
            self.head.store(next, Ordering::Release);
        }
    }

    /// Dequeue the oldest message, if any.
    pub fn pop(&self) -> Option<MidiMessage> {
        use core::sync::atomic::Ordering;
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: slot `tail` was published by the producer's Release store on
        // `head`, which the Acquire load above observed, and no producer will
        // write it again until we advance `tail`. `MidiMessage: Copy`.
        let msg = unsafe {
            let slots = self.buf.get() as *const MidiMessage;
            slots.add(tail).read()
        };
        self.tail.store((tail + 1) % Self::CAPACITY, Ordering::Release);
        Some(msg)
    }
}

impl Default for MidiQueue {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: see the concurrency contract on `MidiQueue`. The UnsafeCell contents
// are only touched under that protocol; `head`/`tail` are atomics.
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
