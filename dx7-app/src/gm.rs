//! General MIDI sound set — re-exports from dx7-midi and provides
//! the std-only `GmSoundSet` wrapper.

use dx7_core::DxVoice;

pub use dx7_midi::gm::{gm_voice, program_gain, is_bass_program};

/// Preloaded GM sound set — 128 DxVoice patches from compiled ROM.
pub struct GmSoundSet {
    patches: [DxVoice; 128],
}

impl GmSoundSet {
    /// Load all 128 GM patches from the compiled-in ROM data.
    /// No sysex files needed at runtime.
    pub fn load(_sysex_dir: &str) -> Self {
        let patches: [DxVoice; 128] = std::array::from_fn(|i| {
            gm_voice(i as u8)
        });
        Self { patches }
    }

    /// Get the DX7 patch for a GM program number (0-127).
    pub fn get(&self, program: u8) -> Option<&DxVoice> {
        Some(&self.patches[program as usize])
    }
}
