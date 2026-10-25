//! Built-in factory presets (ROM1A bank).
//!
//! These provide fallback patches when no .syx file is loaded.
//! The ROM1A bank contains 32 classic DX7 patches.

use crate::patch::DxVoice;

/// Get the name of a ROM1A preset by index.
pub fn rom1a_name(index: usize) -> &'static str {
    if index < 32 { crate::rom1a::ROM1A_VOICE_NAMES[index] } else { "??????????" }
}

/// Create the E.PIANO 1 preset — the iconic DX7 electric piano sound.
/// Algorithm 5, 3 carrier pairs creating the classic FM e-piano timbre.
pub fn e_piano_1() -> DxVoice {
    crate::rom1a::load_rom1a_voice(10).unwrap()
}

/// Create the BRASS 1 preset.
pub fn brass_1() -> DxVoice {
    crate::rom1a::load_rom1a_voice(0).unwrap()
}

/// Create the BASS 1 preset.
pub fn bass_1() -> DxVoice {
    crate::rom1a::load_rom1a_voice(29).unwrap()
}

/// Get a ROM1A preset by index (0-31).
/// Uses the complete packed voice data from the rom1a module for all 32 voices.
/// Returns a default INIT VOICE for out-of-range indices.
pub fn get_rom1a_preset(index: usize) -> DxVoice {
    crate::rom1a::load_rom1a_voice(index).unwrap_or_default()
}
