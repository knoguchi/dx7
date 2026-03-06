//! DX7 FM Synthesizer Engine
//!
//! A faithful emulation of the Yamaha DX7 FM synthesizer.
//! This is a platform-independent library crate with no external dependencies.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

pub mod generated_tables;
pub mod tables;
pub mod envelope;
pub mod lfo;
pub mod pitchenv;
pub mod operator;
pub mod algorithm;
pub mod voice;
pub mod patch;
pub mod preset;
pub mod rom1a;

#[cfg(feature = "std")]
pub mod synth;
#[cfg(feature = "std")]
pub mod effects;

// Re-export main types for convenience
#[cfg(feature = "std")]
pub use synth::{Synth, SynthCommand};
#[cfg(feature = "std")]
pub use effects::{Reverb, Chorus};
pub use patch::DxVoice;
pub use preset::{get_rom1a_preset, e_piano_1};
pub use rom1a::{load_rom1a_voice, ROM1A_VOICE_DATA};
#[cfg(feature = "std")]
pub use rom1a::load_rom1a;
