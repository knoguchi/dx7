//! Polyphonic DX7 synthesizer engine.

use crate::patch::DxVoice;
use crate::tables::{self, N};
use crate::voice::{Voice, VoiceState};

const DEFAULT_MAX_VOICES: usize = 32;

/// Commands that can be sent to the synth from other threads.
#[derive(Clone, Debug)]
pub enum SynthCommand {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    PitchBend { value: i16 },
    ControlChange { cc: u8, value: u8 },
    LoadPatch(Box<DxVoice>),
}

/// The main polyphonic synthesizer.
pub struct Synth {
    voices: Vec<Voice>,
    current_patch: DxVoice,
    _sample_rate: u32,
    sustain: bool,
    master_volume: f32,
    expression: f32,
    /// Pitch bend in the internal log-frequency domain (Q24 octaves).
    pitch_bend_logfreq: i32,
    /// Last raw MIDI pitch bend value (-8192..8191), kept so a range change
    /// re-applies to the current wheel position.
    pitch_bend_raw: i16,
    /// Pitch bend range in semitones for full wheel deflection. DX7 function
    /// mode default is 2; RPN 0 (pitch bend sensitivity) also sets it.
    pitch_bend_range: u8,
    /// Registered parameter number selected via CC 101/100 (MSB, LSB).
    /// 0x7F/0x7F means "null" (no RPN selected).
    rpn: (u8, u8),
    /// Mod wheel value (0–127). Adds LFO pitch/amp modulation on top of the
    /// patch's own depth (see Voice::render).
    mod_wheel: i32,
    /// Notes waiting for a stolen voice to finish its one-block fade-out:
    /// (voice index, note, velocity). Applied at the end of the next block.
    pending_notes: Vec<(usize, u8, u8)>,
    /// Internal scratch buffer for mixing (one N-sample block).
    mix_buffer: [i32; N],
    /// Residue buffer for sub-block alignment in render (stereo).
    stereo_residue: [f32; N * 2],
    stereo_residue_len: usize, // in frames (not samples)
    /// Residue buffer for sub-block alignment in render_mono.
    /// Voices always advance by N samples per block; when the caller
    /// requests a non-N-aligned number of frames, we buffer the
    /// leftover samples here so no audio is lost.
    mono_residue: [f32; N],
    mono_residue_len: usize,
}

impl Synth {
    pub fn new(sample_rate: u32) -> Self {
        Self::with_max_voices(sample_rate, DEFAULT_MAX_VOICES)
    }

    pub fn with_max_voices(sample_rate: u32, max_voices: usize) -> Self {
        // Initialize all lookup tables (must be done once before any rendering)
        tables::init_tables(sample_rate);
        crate::lfo::init_lfo(sample_rate);
        crate::pitchenv::init_pitchenv(sample_rate);

        let mut voices = Vec::with_capacity(max_voices);
        for _ in 0..max_voices {
            voices.push(Voice::new());
        }

        Self {
            voices,
            current_patch: DxVoice::init_voice(),
            _sample_rate: sample_rate,
            sustain: false,
            master_volume: 0.5,
            expression: 1.0,
            pitch_bend_logfreq: 0,
            pitch_bend_raw: 0,
            pitch_bend_range: 2,
            rpn: (0x7F, 0x7F),
            mod_wheel: 0,
            pending_notes: Vec::new(),
            mix_buffer: [0i32; N],
            stereo_residue: [0.0; N * 2],
            stereo_residue_len: 0,
            mono_residue: [0.0; N],
            mono_residue_len: 0,
        }
    }

    /// Load a new patch.
    pub fn load_patch(&mut self, patch: DxVoice) {
        self.current_patch = patch;
    }

    /// Set master volume (0.0 to 1.0).
    pub fn set_master_volume(&mut self, vol: f32) {
        self.master_volume = vol;
    }

    /// Get the current patch name.
    pub fn patch_name(&self) -> &str {
        self.current_patch.name_str()
    }

    /// Trigger a note on.
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }

        let voice_idx = self.allocate_voice();
        let voice = &mut self.voices[voice_idx];
        if voice.state == VoiceState::Inactive {
            voice.note_on(&self.current_patch, note, velocity);
        } else {
            // Stealing a sounding voice: fade it out over the next block and
            // start the new note after that, instead of cutting it (click).
            voice.begin_fade();
            if let Some(p) = self.pending_notes.iter_mut().find(|p| p.0 == voice_idx) {
                p.1 = note;
                p.2 = velocity;
            } else {
                self.pending_notes.push((voice_idx, note, velocity));
            }
        }
    }

    /// Start any notes whose stolen voice has finished fading. Called once
    /// per rendered block, after the fade block.
    fn start_pending_notes(&mut self) {
        if self.pending_notes.is_empty() {
            return;
        }
        let mut i = 0;
        while i < self.pending_notes.len() {
            let (idx, note, vel) = self.pending_notes[i];
            if self.voices[idx].state == VoiceState::Inactive {
                self.voices[idx].note_on(&self.current_patch, note, vel);
                self.pending_notes.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Trigger a note off. With the sustain pedal down the voice keeps
    /// sounding and is flagged `sustain_held`; the pedal release then
    /// releases exactly those voices, never a key that is still held.
    pub fn note_off(&mut self, note: u8) {
        // A note released before its stolen voice finished fading never
        // sounds at all (it was under 1.5 ms long).
        self.pending_notes.retain(|p| p.1 != note);
        for voice in &mut self.voices {
            if voice.state == VoiceState::Active && voice.note == note && !voice.sustain_held {
                if self.sustain {
                    voice.sustain_held = true;
                } else {
                    voice.note_off();
                }
            }
        }
    }

    /// Process a raw MIDI message.
    pub fn process_midi(&mut self, msg: &[u8]) {
        if msg.is_empty() {
            return;
        }

        let status = msg[0] & 0xF0;
        match status {
            0x90 => {
                if msg.len() >= 3 {
                    self.note_on(msg[1], msg[2]);
                }
            }
            0x80 => {
                if msg.len() >= 3 {
                    self.note_off(msg[1]);
                }
            }
            0xB0 => {
                if msg.len() >= 3 {
                    self.control_change(msg[1], msg[2]);
                }
            }
            0xE0
                if msg.len() >= 3 => {
                    let bend = ((msg[2] as i16) << 7 | msg[1] as i16) - 8192;
                    self.pitch_bend(bend);
                }
            _ => {}
        }
    }

    /// Process a synth command.
    pub fn process_command(&mut self, cmd: SynthCommand) {
        match cmd {
            SynthCommand::NoteOn { note, velocity } => self.note_on(note, velocity),
            SynthCommand::NoteOff { note } => self.note_off(note),
            SynthCommand::PitchBend { value } => self.pitch_bend(value),
            SynthCommand::ControlChange { cc, value } => self.control_change(cc, value),
            SynthCommand::LoadPatch(patch) => self.load_patch(*patch),
        }
    }

    pub fn control_change(&mut self, cc: u8, value: u8) {
        match cc {
            1 => {
                // Mod wheel — scales LFO pitch/amp modulation depth
                self.mod_wheel = value as i32;
            }
            7 => {
                self.master_volume = value as f32 / 127.0;
            }
            11 => {
                self.expression = value as f32 / 127.0;
            }
            64 => {
                self.sustain = value >= 64; // standard MIDI sustain pedal threshold
                if !self.sustain {
                    for voice in &mut self.voices {
                        if voice.sustain_held {
                            voice.sustain_held = false;
                            voice.note_off();
                        }
                    }
                }
            }
            // RPN select (MSB/LSB) and data entry. Only RPN 0 (pitch bend
            // sensitivity, semitones in the data MSB) is implemented.
            101 => self.rpn.0 = value & 0x7F,
            100 => self.rpn.1 = value & 0x7F,
            6 if self.rpn == (0, 0) => self.set_pitch_bend_range(value),
            _ => {}
        }
    }

    /// Set the pitch bend range in semitones (0..=24) for full wheel
    /// deflection. Re-applies the current wheel position immediately.
    pub fn set_pitch_bend_range(&mut self, semitones: u8) {
        self.pitch_bend_range = semitones.min(24);
        self.pitch_bend(self.pitch_bend_raw);
    }

    /// Current pitch bend range in semitones.
    pub fn pitch_bend_range(&self) -> u8 {
        self.pitch_bend_range
    }

    /// Apply a raw MIDI pitch bend value (-8192..=8191).
    pub fn pitch_bend(&mut self, value: i16) {
        self.pitch_bend_raw = value;
        // One octave is 1<<24 in the logfreq domain, so full deflection of
        // `range` semitones is range * (1<<24) / 12, spread over 8192 steps:
        // value * range * 2048 / 12.
        let lf = value as i64 * self.pitch_bend_range as i64 * 2048 / 12;
        self.pitch_bend_logfreq = lf as i32;
    }

    /// Render audio into the output buffer (interleaved stereo f32, -1.0..1.0).
    /// Handles non-N-aligned buffer sizes by buffering residue samples
    /// so voices always advance in exact N-sample blocks.
    pub fn render(&mut self, output: &mut [f32]) {
        let channels = 2;
        let total_frames = output.len() / channels;
        let mut frame_offset = 0;

        // First, drain any residue from a previous partial block
        if self.stereo_residue_len > 0 {
            let to_copy = self.stereo_residue_len.min(total_frames);
            let src = &self.stereo_residue[..to_copy * channels];
            output[..to_copy * channels].copy_from_slice(src);
            if to_copy < self.stereo_residue_len {
                let rem = self.stereo_residue_len - to_copy;
                self.stereo_residue.copy_within(to_copy * channels..self.stereo_residue_len * channels, 0);
                self.stereo_residue_len = rem;
                return;
            }
            self.stereo_residue_len = 0;
            frame_offset = to_copy;
        }

        let volume = self.master_volume * self.expression;

        while frame_offset < total_frames {
            // Always render a full N-sample block
            self.mix_buffer = [0i32; N];

            for voice in &mut self.voices {
                if voice.state != VoiceState::Inactive {
                    voice.pitch_bend = self.pitch_bend_logfreq;
                    voice.mod_wheel = self.mod_wheel;
                    let mut voice_buf = [0i32; N];
                    voice.render(&mut voice_buf);
                    for i in 0..N {
                        self.mix_buffer[i] += voice_buf[i] >> 4;
                    }
                }
            }
            self.start_pending_notes();

            // Convert full block to f32 stereo
            let mut block_stereo = [0.0f32; N * 2];
            for i in 0..N {
                let val = self.mix_buffer[i];
                let clip_val: i32 = if val < -(1 << 24) {
                    -0x8000
                } else if val >= (1 << 24) {
                    0x7FFF
                } else {
                    val >> 9
                };
                let sample_f32 = (clip_val as f32 / 0x8000 as f32) * volume;
                let clamped = sample_f32.clamp(-1.0, 1.0);
                block_stereo[i * 2] = clamped;
                block_stereo[i * 2 + 1] = clamped;
            }

            let remaining = total_frames - frame_offset;
            if remaining >= N {
                let out_start = frame_offset * channels;
                output[out_start..out_start + N * channels]
                    .copy_from_slice(&block_stereo);
                frame_offset += N;
            } else {
                let out_start = frame_offset * channels;
                output[out_start..].copy_from_slice(&block_stereo[..remaining * channels]);
                let residue_frames = N - remaining;
                self.stereo_residue[..residue_frames * channels]
                    .copy_from_slice(&block_stereo[remaining * channels..]);
                self.stereo_residue_len = residue_frames;
                frame_offset = total_frames;
            }
        }
    }

    /// Render mono f32 samples (for WAV rendering).
    /// Handles non-N-aligned buffer sizes by buffering residue samples
    /// so voices always advance in exact N-sample blocks.
    pub fn render_mono(&mut self, output: &mut [f32]) {
        let num_frames = output.len();
        let mut frame_offset = 0;

        // First, drain any residue from a previous partial block
        if self.mono_residue_len > 0 {
            let to_copy = self.mono_residue_len.min(num_frames);
            output[..to_copy].copy_from_slice(&self.mono_residue[..to_copy]);
            if to_copy < self.mono_residue_len {
                // Still more residue than output can hold — shift remainder
                self.mono_residue.copy_within(to_copy..self.mono_residue_len, 0);
                self.mono_residue_len -= to_copy;
                return;
            }
            self.mono_residue_len = 0;
            frame_offset = to_copy;
        }

        let volume = self.master_volume * self.expression;

        while frame_offset < num_frames {
            // Always render a full N-sample block
            self.mix_buffer = [0i32; N];

            for voice in &mut self.voices {
                if voice.state != VoiceState::Inactive {
                    voice.pitch_bend = self.pitch_bend_logfreq;
                    voice.mod_wheel = self.mod_wheel;
                    let mut voice_buf = [0i32; N];
                    voice.render(&mut voice_buf);
                    for i in 0..N {
                        self.mix_buffer[i] += voice_buf[i] >> 4;
                    }
                }
            }
            self.start_pending_notes();

            // Convert full block to f32: clip to 24-bit signed, shift >> 9, normalize
            // val >> 4 already done above; now clip to 24-bit, shift >> 9, normalize to float
            let mut block_f32 = [0.0f32; N];
            for i in 0..N {
                let val = self.mix_buffer[i];
                let clip_val: i32 = if val < -(1 << 24) {
                    -0x8000
                } else if val >= (1 << 24) {
                    0x7FFF
                } else {
                    val >> 9
                };
                block_f32[i] = (clip_val as f32 / 0x8000 as f32) * volume;
            }

            let remaining = num_frames - frame_offset;
            if remaining >= N {
                // Full block fits
                output[frame_offset..frame_offset + N].copy_from_slice(&block_f32);
                frame_offset += N;
            } else {
                // Partial block — copy what fits, buffer the rest
                output[frame_offset..].copy_from_slice(&block_f32[..remaining]);
                let residue_count = N - remaining;
                self.mono_residue[..residue_count].copy_from_slice(&block_f32[remaining..]);
                self.mono_residue_len = residue_count;
                frame_offset = num_frames;
            }
        }
    }

    /// Allocate a voice slot using priority: inactive > oldest released > oldest active.
    fn allocate_voice(&mut self) -> usize {
        // 1. Find an inactive voice
        for (i, voice) in self.voices.iter().enumerate() {
            if voice.state == VoiceState::Inactive {
                return i;
            }
        }

        // 2. Find the oldest released voice that is not already being stolen
        let mut oldest_released: Option<(usize, u32)> = None;
        for (i, voice) in self.voices.iter().enumerate() {
            if voice.state == VoiceState::Released
                && !voice.fading
                && (oldest_released.is_none() || voice.age > oldest_released.unwrap().1)
            {
                oldest_released = Some((i, voice.age));
            }
        }
        if let Some((idx, _)) = oldest_released {
            return idx;
        }

        // 3. Steal the oldest active voice (again preferring one not already
        //    fading; if every voice is fading, take the oldest anyway and the
        //    pending note for it is replaced).
        let mut oldest: (usize, u32) = (0, 0);
        let mut oldest_not_fading: Option<(usize, u32)> = None;
        for (i, voice) in self.voices.iter().enumerate() {
            if voice.age >= oldest.1 {
                oldest = (i, voice.age);
            }
            if !voice.fading && oldest_not_fading.is_none_or(|(_, a)| voice.age > a) {
                oldest_not_fading = Some((i, voice.age));
            }
        }
        oldest_not_fading.unwrap_or(oldest).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail_peak(synth: &mut Synth, seconds: f32) -> f32 {
        let mut out = vec![0.0f32; (44100.0 * seconds) as usize * 2];
        synth.render(&mut out);
        out[out.len() - 4096..]
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()))
    }

    /// Issue #4: releasing the pedal must not cut a key that is still held.
    #[test]
    fn test_pedal_release_keeps_held_key() {
        let mut s = Synth::new(44100);
        s.load_patch(DxVoice::init_voice());
        s.control_change(64, 127); // pedal down
        s.note_on(60, 100);
        s.note_off(60); // key up, pedal holds
        s.note_on(60, 100); // key down again, still physically held
        s.control_change(64, 0); // pedal up
        let peak = tail_peak(&mut s, 1.0);
        assert!(peak > 0.01, "held key was cut by pedal release, peak={peak}");
        assert_eq!(
            s.voices.iter().filter(|v| v.state == VoiceState::Active).count(),
            1,
            "only the re-pressed voice should still be active"
        );
    }

    /// Issue #4: pedal release does release notes whose keys are up.
    #[test]
    fn test_pedal_release_releases_sustained_notes() {
        let mut s = Synth::new(44100);
        s.load_patch(DxVoice::init_voice());
        s.control_change(64, 127);
        s.note_on(60, 100);
        s.note_on(64, 100);
        s.note_off(60);
        s.note_off(64);
        assert!(s.voices.iter().filter(|v| v.sustain_held).count() == 2);
        s.control_change(64, 0);
        assert!(s.voices.iter().all(|v| !v.sustain_held));
        let peak = tail_peak(&mut s, 1.0);
        assert!(peak < 0.001, "sustained notes should have released, peak={peak}");
    }

    /// Issue #4: a note released with the pedal down and then released
    /// again (duplicate NoteOff) must not accumulate state.
    #[test]
    fn test_duplicate_note_off_under_sustain_is_harmless() {
        let mut s = Synth::new(44100);
        s.load_patch(DxVoice::init_voice());
        s.control_change(64, 127);
        s.note_on(60, 100);
        for _ in 0..1000 {
            s.note_off(60);
        }
        assert_eq!(s.voices.iter().filter(|v| v.sustain_held).count(), 1);
    }

    /// Upward zero-crossing frequency estimate over a stereo buffer.
    fn est_freq(out: &[f32]) -> f64 {
        let mono: Vec<f32> = out.iter().step_by(2).copied().collect();
        let n = mono.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        n as f64 / (mono.len() as f64 / 44100.0)
    }

    /// Issue #8: default bend range is 2 semitones; RPN 0 and
    /// set_pitch_bend_range change it.
    #[test]
    fn test_pitch_bend_range_default_and_rpn() {
        let mut s = Synth::new(44100);
        s.load_patch(DxVoice::init_voice());
        s.note_on(69, 100);
        let mut warm = vec![0.0f32; 44100];
        s.render(&mut warm);

        let mut base = vec![0.0f32; 88200];
        s.render(&mut base);
        let f0 = est_freq(&base);

        s.pitch_bend(8191);
        let mut bent = vec![0.0f32; 88200];
        s.render(&mut bent);
        let f2 = est_freq(&bent);
        let ratio = f2 / f0;
        assert!(
            (ratio - 1.1225).abs() < 0.02,
            "default range 2 st: expected ratio ~1.1225, got {ratio:.4} ({f0:.1} -> {f2:.1})"
        );

        // RPN 0 (CC101=0, CC100=0, CC6=12) -> 12 semitones; wheel still up.
        s.control_change(101, 0);
        s.control_change(100, 0);
        s.control_change(6, 12);
        assert_eq!(s.pitch_bend_range(), 12);
        let mut bent12 = vec![0.0f32; 88200];
        s.render(&mut bent12);
        let ratio12 = est_freq(&bent12) / f0;
        assert!(
            (ratio12 - 2.0).abs() < 0.05,
            "range 12 st: expected ratio ~2.0, got {ratio12:.4}"
        );

        // Data entry with a different RPN selected must not touch the range.
        s.control_change(100, 1);
        s.control_change(6, 3);
        assert_eq!(s.pitch_bend_range(), 12);
    }

    /// Issue #9: stealing a voice must not produce a discontinuity larger
    /// than the natural per-sample slope of the signal.
    #[test]
    fn test_voice_steal_has_no_click() {
        let mut s = Synth::with_max_voices(44100, 1);
        s.load_patch(DxVoice::init_voice());
        s.note_on(69, 127);
        let mut warm = vec![0.0f32; 44100];
        s.render(&mut warm);

        // Steady-state slope reference.
        let mut steady = vec![0.0f32; 8192];
        s.render(&mut steady);
        let max_step = |buf: &[f32]| {
            buf.iter()
                .step_by(2)
                .collect::<Vec<_>>()
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let steady_step = max_step(&steady);
        let steady_peak = steady.iter().fold(0.0f32, |a, &b| a.max(b.abs()));

        // Steal: a second note on a 1-voice synth. Inspect only the fade
        // block itself (first N frames): the new note's own fast attack that
        // follows is normal DX7 behaviour, not a steal artefact.
        s.note_on(45, 127);
        let mut steal = vec![0.0f32; 8192];
        s.render(&mut steal);
        let steal_step = max_step(&steal[..N * 2]);
        assert!(
            steal_step < steady_step * 2.0,
            "steal should fade, not cut: max step {steal_step:.4} vs steady {steady_step:.4} (peak {steady_peak:.3})"
        );
        // End of the fade block must be (near) silent before the new note starts.
        let last = steal[N * 2 - 2].abs();
        assert!(last < steady_peak * 0.05, "fade should end near zero, got {last:.4}");

        // The new note is actually playing afterwards.
        let mut after = vec![0.0f32; 88200];
        s.render(&mut after);
        let f = est_freq(&after);
        assert!((f - 110.0).abs() < 5.0, "stolen voice should now play A2 (110 Hz), got {f:.1}");
        assert!(s.pending_notes.is_empty());
    }

    /// Issue #9: a NoteOff that arrives while the stolen voice is still
    /// fading cancels the pending note instead of leaving it hanging.
    #[test]
    fn test_pending_note_cancelled_by_note_off() {
        let mut s = Synth::with_max_voices(44100, 1);
        s.load_patch(DxVoice::init_voice());
        s.note_on(69, 100);
        let mut buf = vec![0.0f32; 4096];
        s.render(&mut buf);
        s.note_on(45, 100);
        assert_eq!(s.pending_notes.len(), 1);
        s.note_off(45);
        assert!(s.pending_notes.is_empty());
        let mut tail = vec![0.0f32; 44100];
        s.render(&mut tail);
        let peak = tail[tail.len() - 2048..].iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(peak < 0.001, "nothing should be sounding, peak={peak}");
    }
}
