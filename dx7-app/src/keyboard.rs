//! Computer keyboard → MIDI note mapping using crossterm.

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::time::Duration;

/// Keyboard handler that maps computer keys to MIDI notes.
pub struct KeyboardHandler {
    /// Map from KeyCode to MIDI note number
    key_map: HashMap<KeyCode, u8>,
    /// Track which keys are currently held
    held_keys: HashMap<KeyCode, u8>, // KeyCode -> MIDI note
    /// Current octave offset
    octave: i8,
}

impl KeyboardHandler {
    pub fn new() -> Self {
        let mut key_map = HashMap::new();

        // Bottom row: white keys C4-B4
        key_map.insert(KeyCode::Char('a'), 60); // C4
        key_map.insert(KeyCode::Char('s'), 62); // D4
        key_map.insert(KeyCode::Char('d'), 64); // E4
        key_map.insert(KeyCode::Char('f'), 65); // F4
        key_map.insert(KeyCode::Char('g'), 67); // G4
        key_map.insert(KeyCode::Char('h'), 69); // A4
        key_map.insert(KeyCode::Char('j'), 71); // B4
        key_map.insert(KeyCode::Char('k'), 72); // C5
        key_map.insert(KeyCode::Char('l'), 74); // D5
        key_map.insert(KeyCode::Char(';'), 76); // E5

        // Top row: black keys
        key_map.insert(KeyCode::Char('w'), 61); // C#4
        key_map.insert(KeyCode::Char('e'), 63); // D#4
        key_map.insert(KeyCode::Char('t'), 66); // F#4
        key_map.insert(KeyCode::Char('y'), 68); // G#4
        key_map.insert(KeyCode::Char('u'), 70); // A#4
        key_map.insert(KeyCode::Char('o'), 73); // C#5
        key_map.insert(KeyCode::Char('p'), 75); // D#5

        KeyboardHandler {
            key_map,
            held_keys: HashMap::new(),
            octave: 0,
        }
    }

    /// Poll for keyboard events. Returns a list of synth commands.
    /// Call this in a loop with a short timeout.
    pub fn poll(&mut self, timeout: Duration) -> Vec<KeyboardEvent> {
        let mut events = Vec::new();

        if event::poll(timeout).unwrap_or(false) {
            if let Ok(Event::Key(key_event)) = event::read() {
                events.extend(self.process_key(key_event));
            }
        }

        events
    }

    fn process_key(&mut self, key: KeyEvent) -> Vec<KeyboardEvent> {
        let mut events = Vec::new();

        match key.kind {
            KeyEventKind::Press => {
                // Ctrl-C arrives as Char('c') + CONTROL in raw mode.
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    events.push(KeyboardEvent::Quit);
                    return events;
                }

                // Check for special keys first
                match key.code {
                    KeyCode::Char('z') => {
                        // Octave down
                        self.octave = (self.octave - 1).max(-3);
                        events.push(KeyboardEvent::OctaveChange(self.octave));
                        return events;
                    }
                    KeyCode::Char('x') => {
                        // Octave up
                        self.octave = (self.octave + 1).min(3);
                        events.push(KeyboardEvent::OctaveChange(self.octave));
                        return events;
                    }
                    KeyCode::Char(c @ '0'..='9') => {
                        let idx = if c == '0' { 9 } else { c as u8 - b'1' };
                        events.push(KeyboardEvent::PatchChange(idx));
                        return events;
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        events.push(KeyboardEvent::Quit);
                        return events;
                    }
                    _ => {}
                }

                // Musical key handling
                if let Some(&base_note) = self.key_map.get(&key.code) {
                    let note = (base_note as i8 + self.octave * 12).clamp(0, 127) as u8;

                    if self.held_keys.contains_key(&key.code) {
                        // Already held — auto-repeat. Send NoteOff + NoteOn to
                        // retrigger instead of stacking voices.
                        events.push(KeyboardEvent::NoteOff(note));
                    }

                    self.held_keys.insert(key.code, note);
                    events.push(KeyboardEvent::NoteOn(note, 100));
                }
            }
            KeyEventKind::Release => {
                if let Some(note) = self.held_keys.remove(&key.code) {
                    events.push(KeyboardEvent::NoteOff(note));
                }
            }
            _ => {}
        }

        events
    }
}

/// Events produced by the keyboard handler.
#[derive(Debug)]
pub enum KeyboardEvent {
    NoteOn(u8, u8),
    NoteOff(u8),
    OctaveChange(i8),
    PatchChange(u8),
    Quit,
}

// ── Terminal raw-mode guard ──────────────────────────────────────────

/// Whether raw mode is currently enabled by us.
static RAW_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Whether we pushed keyboard-enhancement flags that must be popped.
static ENHANCEMENT_PUSHED: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK_INSTALLED: Once = Once::new();

/// Undo whatever terminal state we set up. Idempotent and safe to call
/// from a panic hook.
fn restore_terminal() {
    if ENHANCEMENT_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    if RAW_MODE_ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = terminal::disable_raw_mode();
    }
}

/// RAII guard for terminal raw mode.
///
/// On creation it enables raw mode and, when the terminal supports the
/// kitty keyboard protocol, pushes `REPORT_EVENT_TYPES` so that key
/// *release* events are delivered (crossterm only reports releases on
/// Unix when this flag is active). On drop — including during panic
/// unwinding — it pops the flags and restores cooked mode. A panic hook
/// is also installed so the panic message itself is printed on a sane
/// terminal rather than inside raw mode.
pub struct RawModeGuard {
    release_supported: bool,
}

impl RawModeGuard {
    /// Enter raw mode and enable key-release reporting where available.
    pub fn enter() -> io::Result<Self> {
        PANIC_HOOK_INSTALLED.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                prev(info);
            }));
        });

        terminal::enable_raw_mode()?;
        RAW_MODE_ACTIVE.store(true, Ordering::SeqCst);

        // Must be queried after raw mode is on, otherwise the terminal's
        // reply would be echoed / line-buffered.
        let release_supported = match terminal::supports_keyboard_enhancement() {
            Ok(true) => {
                match execute!(
                    io::stdout(),
                    PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
                ) {
                    Ok(()) => {
                        ENHANCEMENT_PUSHED.store(true, Ordering::SeqCst);
                        true
                    }
                    Err(_) => false,
                }
            }
            _ => false,
        };

        Ok(RawModeGuard { release_supported })
    }

    /// True when the terminal reports key-release events, so NoteOff
    /// fires on key lift. False on terminals without the kitty keyboard
    /// protocol (e.g. macOS Terminal.app), where notes sustain until the
    /// same key is pressed again.
    pub fn release_supported(&self) -> bool {
        self.release_supported
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}
