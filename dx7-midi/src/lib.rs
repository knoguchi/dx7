//! `no_std` MIDI building blocks for the `dx7-core` synthesizer: USB-MIDI and
//! BLE-MIDI packet parsers, a fixed-size message queue, and a General MIDI
//! program map onto a compiled DX7 sound set.


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
/// Overflow policy: the last [`RESERVE`](Self::RESERVE) slots are only usable
/// by *release-class* messages (NoteOff and ControlChange, which carry sustain
/// off / All Sound Off / All Notes Off). A burst of NoteOns therefore cannot
/// fill the queue to the point where a NoteOff is lost, which is what turns a
/// burst into a stuck note. Anything dropped is counted in
/// [`dropped`](Self::dropped) so the firmware can report it from a low-priority
/// task.
///
/// The storage lives in an `UnsafeCell` so that writing a slot through `&self`
/// is a legal interior mutation rather than a write through a pointer derived
/// from a shared reference.
pub struct MidiQueue {
    buf: core::cell::UnsafeCell<[MidiMessage; Self::CAPACITY]>,
    head: core::sync::atomic::AtomicUsize,
    tail: core::sync::atomic::AtomicUsize,
    dropped: core::sync::atomic::AtomicU32,
}

impl MidiQueue {
    /// Ring size. Usable capacity is `CAPACITY - 1` (one slot distinguishes
    /// full from empty).
    pub const CAPACITY: usize = 256;

    /// Slots held back for release-class messages (NoteOff, ControlChange).
    /// Other messages are refused once fewer than this many slots are free.
    pub const RESERVE: usize = 32;

    pub const fn new() -> Self {
        const EMPTY: MidiMessage = MidiMessage::NoteOff { channel: 0, note: 0, velocity: 0 };
        Self {
            buf: core::cell::UnsafeCell::new([EMPTY; Self::CAPACITY]),
            head: core::sync::atomic::AtomicUsize::new(0),
            tail: core::sync::atomic::AtomicUsize::new(0),
            dropped: core::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Messages that end or silence notes. These may use the reserved slots.
    fn is_release_class(msg: &MidiMessage) -> bool {
        matches!(msg, MidiMessage::NoteOff { .. } | MidiMessage::ControlChange { .. })
    }

    /// Enqueue a message. Returns `false` (and bumps the drop counter) if the
    /// queue is full for this message class; see the type-level docs for the
    /// overflow policy and the producer constraint.
    pub fn push(&self, msg: MidiMessage) -> bool {
        use core::sync::atomic::Ordering;
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let used = (head + Self::CAPACITY - tail) % Self::CAPACITY;
        let limit = if Self::is_release_class(&msg) {
            Self::CAPACITY - 1
        } else {
            Self::CAPACITY - 1 - Self::RESERVE
        };
        if used >= limit {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // SAFETY: slot `head` is owned by the producer side until the
        // Release store below publishes it; the consumer only reads slots
        // in `tail..head`. Producers never run concurrently (contract).
        unsafe {
            let slots = self.buf.get() as *mut MidiMessage;
            slots.add(head).write(msg);
        }
        self.head.store((head + 1) % Self::CAPACITY, Ordering::Release);
        true
    }

    /// Total messages refused by `push` since construction.
    pub fn dropped(&self) -> u32 {
        self.dropped.load(core::sync::atomic::Ordering::Relaxed)
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

    /// NoteOns may use every slot except the reserve.
    const NOTE_ON_LIMIT: usize = MidiQueue::CAPACITY - 1 - MidiQueue::RESERVE;

    #[test]
    fn full_queue_drops_message_and_counts_it() {
        let q = MidiQueue::new();
        for i in 0..NOTE_ON_LIMIT {
            assert!(q.push(MidiMessage::NoteOn { channel: 0, note: (i % 128) as u8, velocity: 100 }));
        }
        assert_eq!(q.dropped(), 0);
        // One more NoteOn is refused and counted
        assert!(!q.push(MidiMessage::NoteOn { channel: 0, note: 99, velocity: 127 }));
        assert_eq!(q.dropped(), 1);
        // Drain: the refused message is absent
        for i in 0..NOTE_ON_LIMIT {
            assert_eq!(q.pop(), Some(MidiMessage::NoteOn { channel: 0, note: (i % 128) as u8, velocity: 100 }));
        }
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn note_off_and_cc_use_reserved_slots() {
        let q = MidiQueue::new();
        for i in 0..NOTE_ON_LIMIT {
            q.push(MidiMessage::NoteOn { channel: 0, note: (i % 128) as u8, velocity: 100 });
        }
        assert!(!q.push(MidiMessage::NoteOn { channel: 0, note: 1, velocity: 1 }));
        assert!(!q.push(MidiMessage::ProgramChange { channel: 0, program: 1 }));
        // Release-class messages still get through, up to the hard limit
        for i in 0..MidiQueue::RESERVE {
            let msg = if i % 2 == 0 {
                MidiMessage::NoteOff { channel: 0, note: i as u8, velocity: 0 }
            } else {
                MidiMessage::ControlChange { channel: 0, controller: 64, value: 0 }
            };
            assert!(q.push(msg), "reserve slot {i}");
        }
        // Now truly full: even a NoteOff is dropped
        assert!(!q.push(MidiMessage::NoteOff { channel: 0, note: 5, velocity: 0 }));
        assert_eq!(q.dropped(), 3);
        // Draining frees space again
        q.pop();
        assert!(q.push(MidiMessage::NoteOff { channel: 0, note: 5, velocity: 0 }));
    }

    #[test]
    fn wraparound_preserves_order() {
        let q = MidiQueue::new();
        // Push/pop past the end of the ring several times
        for round in 0..3u8 {
            for i in 0..200u8 {
                assert!(q.push(MidiMessage::ProgramChange { channel: round, program: i % 128 }));
            }
            for i in 0..200u8 {
                assert_eq!(q.pop(), Some(MidiMessage::ProgramChange { channel: round, program: i % 128 }));
            }
            assert_eq!(q.pop(), None);
        }
        assert_eq!(q.dropped(), 0);
    }
}
