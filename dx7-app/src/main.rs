//! DX7 FM Synthesizer — Desktop Application
//!
//! Usage:
//!   dx7-app                          # Interactive mode with keyboard/MIDI
//!   dx7-app --render output.wav      # Offline WAV rendering
//!   dx7-app --sysex sysex/rom1a.syx   # Load patches from SysEx file
//!   dx7-app --list-midi              # List MIDI ports

mod audio;
#[cfg(feature = "bluetooth")]
mod bluetooth;
mod gm;
mod gm_rom;
mod keyboard;
mod midi;
#[cfg(feature = "rtp-midi")]
mod rtpmidi;

use clap::Parser;
use crossterm::terminal;
use dx7_core::{get_rom1a_preset, DxVoice, SynthCommand};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "dx7", about = "DX7 FM Synthesizer Emulator")]
struct Args {
    /// Render to WAV file instead of real-time playback
    #[arg(long)]
    render: Option<String>,

    /// SysEx file to load patches from
    #[arg(long)]
    sysex: Option<String>,

    /// Patch index to load (0-31)
    #[arg(long, default_value = "3")]
    patch: usize,

    /// MIDI note for WAV rendering
    #[arg(long, default_value = "60")]
    note: u8,

    /// Velocity for WAV rendering
    #[arg(long, default_value = "100")]
    velocity: u8,

    /// Duration in seconds for WAV rendering
    #[arg(long, default_value = "3")]
    duration: f64,

    /// Sample rate
    #[arg(long, default_value = "44100")]
    sample_rate: u32,

    /// MIDI port name (partial match)
    #[arg(long)]
    midi_port: Option<String>,

    /// List available MIDI ports and exit
    #[arg(long)]
    list_midi: bool,

    /// Render a MIDI file to WAV
    #[arg(long)]
    midi_file: Option<String>,

    /// MIDI track index to render (default: all tracks)
    #[arg(long)]
    track: Option<Vec<usize>>,

    /// Map MIDI channels to specific DX7 patches: ch2=sysex/rom1a.syx:21
    /// Append @vol,reverb to mix (e.g. ch5=file:3@1.5,0.3 for 1.5x vol, 0.3 reverb send).
    /// Can be specified multiple times. Channel numbers are 1-based.
    #[arg(long = "ch", value_name = "CHn=FILE:VOICE[@VOL[,REV]]")]
    channel_map: Vec<String>,

    /// Disable effects chain (reverb, chorus, tremolo, exciter, widener, saturation)
    /// for dry output comparable to Dexed
    #[arg(long)]
    dry: bool,

    /// Enable General MIDI sound set (maps program changes to DX7 patches).
    /// Requires sysex/ directory with factory, vrc, and greymatter banks.
    #[arg(long)]
    gm: bool,

    /// Enable BLE MIDI peripheral mode (requires BlueZ on Linux)
    #[cfg(feature = "bluetooth")]
    #[arg(long)]
    bluetooth: bool,

    /// Enable RTP-MIDI listener (advertises as "DX7" via mDNS)
    #[cfg(feature = "rtp-midi")]
    #[arg(long)]
    rtp_midi: bool,

    /// RTP-MIDI control port (default: 5004, data port = control + 1)
    #[cfg(feature = "rtp-midi")]
    #[arg(long, default_value_t = 5004)]
    rtp_midi_port: u16,
}

fn main() {
    let args = Args::parse();

    // List MIDI ports
    if args.list_midi {
        let ports = midi::MidiHandler::list_ports();
        if ports.is_empty() {
            println!("No MIDI input ports found.");
        } else {
            println!("Available MIDI input ports:");
            for (i, name) in ports.iter().enumerate() {
                println!("  {}: {}", i, name);
            }
        }
        return;
    }

    // Load patches
    let patches = load_patches(&args);
    let patch_idx = args.patch.min(patches.len() - 1);
    let initial_patch = patches[patch_idx].clone();

    println!("DX7 FM Synthesizer");
    println!("Patch: {} ({})", patch_idx, initial_patch.name_str());

    // MIDI file rendering mode
    if let Some(ref midi_path) = args.midi_file {
        let output_path = args.render.as_deref().unwrap_or("output.wav");
        let patch_locked = args.sysex.is_some();
        render_midi_file(midi_path, output_path, &initial_patch, &patches, patch_locked, &args);
        return;
    }

    // WAV rendering mode (single note)
    if let Some(ref output_path) = args.render {
        render_wav(output_path, &initial_patch, &args);
        return;
    }

    // Interactive mode
    run_interactive(initial_patch, patches, &args);
}

fn load_patches(args: &Args) -> Vec<DxVoice> {
    if let Some(ref syx_path) = args.sysex {
        match std::fs::read(syx_path) {
            Ok(data) => match DxVoice::parse_bulk_dump(&data) {
                Ok(voices) => {
                    println!("Loaded {} patches from {}", voices.len(), syx_path);
                    for (i, v) in voices.iter().enumerate() {
                        println!("  {}: {}", i, v.name_str());
                    }
                    return voices;
                }
                Err(e) => {
                    eprintln!("Failed to parse SysEx file: {e}");
                }
            },
            Err(e) => {
                eprintln!("Failed to read SysEx file: {e}");
            }
        }
    }

    // Fall back to built-in presets
    (0..32).map(get_rom1a_preset).collect()
}

fn render_wav(output_path: &str, patch: &DxVoice, args: &Args) {
    let sample_rate = args.sample_rate;
    let mut synth = dx7_core::Synth::new(sample_rate);
    synth.load_patch(patch.clone());

    let note_samples = (args.duration * sample_rate as f64) as usize;
    let tail_samples = (2.0 * sample_rate as f64) as usize;
    let total_samples = note_samples + tail_samples;

    println!(
        "Rendering: note={}, vel={}, duration={}s, tail=2s",
        args.note, args.velocity, args.duration
    );

    synth.note_on(args.note, args.velocity);

    let mut all_samples = vec![0.0f32; total_samples];

    synth.render_mono(&mut all_samples[..note_samples]);
    synth.note_off(args.note);
    synth.render_mono(&mut all_samples[note_samples..]);

    // DC-blocking filter (simulates DX7 analog output coupling caps)
    let mut dc = dx7_core::effects::DcBlocker::new(sample_rate as f64);
    dc.process(&mut all_samples);

    // Normalize to -1 dB peak (matches professional headroom)
    let peak = all_samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if peak > 0.0 {
        let target = 0.891; // -1 dB
        let gain = target / peak;
        for s in all_samples.iter_mut() {
            *s *= gain;
        }
        println!("Normalized: peak {:.4} → {:.4} (gain {:.1}x)", peak, target, gain);
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer =
        hound::WavWriter::create(output_path, spec).expect("Failed to create WAV file");

    for &sample in &all_samples {
        let i16_sample = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer.write_sample(i16_sample).unwrap();
    }

    writer.finalize().unwrap();
    println!("Written {} samples to {}", total_samples, output_path);
}

/// A MIDI event parsed from a Standard MIDI File.
#[derive(Debug, Clone)]
struct MidiEvent {
    tick: u64,
    channel: u8,
    kind: MidiEventKind,
}

#[derive(Debug, Clone)]
enum MidiEventKind {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    ControlChange { cc: u8, value: u8 },
    PitchBend { value: i16 },
    ProgramChange { program: u8 },
    Tempo(u32), // microseconds per beat
}

/// Parse a Standard MIDI File, returning events sorted by tick.
fn parse_midi_file(data: &[u8], track_filter: &Option<Vec<usize>>) -> (u16, Vec<MidiEvent>) {
    let smf = midly::Smf::parse(data).expect("Failed to parse MIDI file");

    let tpb = match smf.header.timing {
        midly::Timing::Metrical(tpb) => tpb.as_int(),
        midly::Timing::Timecode(..) => panic!("SMPTE timecode not supported"),
    };

    let mut all_events = Vec::new();

    for (track_idx, track) in smf.tracks.iter().enumerate() {
        // Always parse track 0 (tempo map), plus selected tracks
        let include = track_idx == 0
            || match track_filter {
                Some(tracks) => tracks.contains(&track_idx),
                None => true,
            };
        if !include {
            continue;
        }

        let mut tick: u64 = 0;
        for event in track {
            tick += event.delta.as_int() as u64;
            match event.kind {
                midly::TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    let kind = match message {
                        midly::MidiMessage::NoteOn { key, vel } => {
                            if vel.as_int() > 0 {
                                MidiEventKind::NoteOn {
                                    note: key.as_int(),
                                    velocity: vel.as_int(),
                                }
                            } else {
                                MidiEventKind::NoteOff { note: key.as_int() }
                            }
                        }
                        midly::MidiMessage::NoteOff { key, .. } => {
                            MidiEventKind::NoteOff { note: key.as_int() }
                        }
                        midly::MidiMessage::Controller { controller, value } => {
                            MidiEventKind::ControlChange {
                                cc: controller.as_int(),
                                value: value.as_int(),
                            }
                        }
                        midly::MidiMessage::PitchBend { bend } => {
                            MidiEventKind::PitchBend {
                                value: bend.as_int(),
                            }
                        }
                        midly::MidiMessage::ProgramChange { program } => {
                            MidiEventKind::ProgramChange {
                                program: program.as_int(),
                            }
                        }
                        _ => continue, // Aftertouch, etc.
                    };
                    all_events.push(MidiEvent { tick, channel: ch, kind });
                }
                midly::TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) => {
                    all_events.push(MidiEvent {
                        tick,
                        channel: 0,
                        kind: MidiEventKind::Tempo(t.as_int()),
                    });
                }
                _ => {} // SysEx, other meta — skip
            }
        }
    }

    // Sort by tick (stable sort preserves order for same-tick events)
    all_events.sort_by_key(|e| e.tick);

    (tpb, all_events)
}

// ── Synthesized Drum Machine (GM channel 9) ──────────────────────────

struct DrumHit {
    age: f64,
    velocity: f32,
    tone_freq: f64,
    tone_freq_end: f64,
    tone_decay: f64,
    noise_level: f32,
    noise_decay: f64,
    noise_hp_alpha: f64,
    noise_hp_prev: f64,
    phase: f64,
}

struct DrumMachine {
    sample_rate: f64,
    hits: Vec<DrumHit>,
    rng_state: u32,
}

/// Synthesis parameters for a GM drum sound.
struct DrumParams {
    tone_freq: f64,
    tone_freq_end: f64,
    tone_decay: f64,
    noise_level: f32,
    noise_decay: f64,
    noise_hp_alpha: f64, // 0.0 = no HP filter, >0 = high-pass
}

impl DrumMachine {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            hits: Vec::new(),
            rng_state: 0xDEAD_BEEF,
        }
    }

    /// Map a GM drum note to synthesis parameters.
    fn drum_params(&self, note: u8) -> Option<DrumParams> {
        // High-pass alpha for hi-hats / cymbals at different cutoffs
        let hp_high = 1.0 - (-2.0 * std::f64::consts::PI * 8000.0 / self.sample_rate).exp();
        let hp_mid = 1.0 - (-2.0 * std::f64::consts::PI * 4000.0 / self.sample_rate).exp();

        match note {
            // Kick drum
            35 | 36 => Some(DrumParams {
                tone_freq: 150.0, tone_freq_end: 45.0, tone_decay: 0.15,
                noise_level: 0.15, noise_decay: 0.04, noise_hp_alpha: 0.0,
            }),
            // Side Stick
            37 => Some(DrumParams {
                tone_freq: 400.0, tone_freq_end: 400.0, tone_decay: 0.02,
                noise_level: 0.5, noise_decay: 0.02, noise_hp_alpha: 0.0,
            }),
            // Snare
            38 | 40 => Some(DrumParams {
                tone_freq: 180.0, tone_freq_end: 150.0, tone_decay: 0.08,
                noise_level: 0.7, noise_decay: 0.15, noise_hp_alpha: 0.0,
            }),
            // Clap
            39 => Some(DrumParams {
                tone_freq: 0.0, tone_freq_end: 0.0, tone_decay: 0.0,
                noise_level: 1.0, noise_decay: 0.12, noise_hp_alpha: 0.0,
            }),
            // Closed Hi-Hat
            42 | 44 => Some(DrumParams {
                tone_freq: 0.0, tone_freq_end: 0.0, tone_decay: 0.0,
                noise_level: 0.6, noise_decay: 0.04, noise_hp_alpha: hp_high,
            }),
            // Open Hi-Hat
            46 => Some(DrumParams {
                tone_freq: 0.0, tone_freq_end: 0.0, tone_decay: 0.0,
                noise_level: 0.6, noise_decay: 0.18, noise_hp_alpha: hp_high,
            }),
            // Toms — wide pitch spread so low vs high is distinct
            41 => Some(DrumParams {
                tone_freq: 80.0, tone_freq_end: 55.0, tone_decay: 0.22,
                noise_level: 0.2, noise_decay: 0.06, noise_hp_alpha: 0.0,
            }),
            43 => Some(DrumParams {
                tone_freq: 110.0, tone_freq_end: 80.0, tone_decay: 0.19,
                noise_level: 0.2, noise_decay: 0.06, noise_hp_alpha: 0.0,
            }),
            45 => Some(DrumParams {
                tone_freq: 150.0, tone_freq_end: 110.0, tone_decay: 0.16,
                noise_level: 0.2, noise_decay: 0.06, noise_hp_alpha: 0.0,
            }),
            47 => Some(DrumParams {
                tone_freq: 200.0, tone_freq_end: 155.0, tone_decay: 0.14,
                noise_level: 0.2, noise_decay: 0.06, noise_hp_alpha: 0.0,
            }),
            48 => Some(DrumParams {
                tone_freq: 260.0, tone_freq_end: 210.0, tone_decay: 0.12,
                noise_level: 0.2, noise_decay: 0.06, noise_hp_alpha: 0.0,
            }),
            50 => Some(DrumParams {
                tone_freq: 320.0, tone_freq_end: 270.0, tone_decay: 0.10,
                noise_level: 0.2, noise_decay: 0.06, noise_hp_alpha: 0.0,
            }),
            // Crash cymbals
            49 | 52 | 55 | 57 => Some(DrumParams {
                tone_freq: 0.0, tone_freq_end: 0.0, tone_decay: 0.0,
                noise_level: 0.5, noise_decay: 0.6, noise_hp_alpha: hp_mid,
            }),
            // Ride cymbals
            51 | 53 | 59 => Some(DrumParams {
                tone_freq: 0.0, tone_freq_end: 0.0, tone_decay: 0.0,
                noise_level: 0.4, noise_decay: 0.35, noise_hp_alpha: hp_mid,
            }),
            _ => None,
        }
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if let Some(p) = self.drum_params(note) {
            self.hits.push(DrumHit {
                age: 0.0,
                velocity: velocity as f32 / 127.0,
                tone_freq: p.tone_freq,
                tone_freq_end: p.tone_freq_end,
                tone_decay: p.tone_decay,
                noise_level: p.noise_level,
                noise_decay: p.noise_decay,
                noise_hp_alpha: p.noise_hp_alpha,
                noise_hp_prev: 0.0,
                phase: 0.0,
            });
        }
    }

    fn render(&mut self, output: &mut [f32]) {
        let dt = 1.0 / self.sample_rate;
        let level = 0.02f32; // drum mix level (below synth 0.05)

        for sample in output.iter_mut() {
            let mut sum = 0.0f32;

            for hit in self.hits.iter_mut() {
                let vel = hit.velocity;

                // Tone component (sine with pitch envelope)
                let mut tone_out = 0.0f32;
                if hit.tone_decay > 0.0 {
                    let pitch_t = (hit.age / hit.tone_decay).min(1.0);
                    let freq = hit.tone_freq + (hit.tone_freq_end - hit.tone_freq) * pitch_t;
                    hit.phase += freq * dt;
                    if hit.phase > 1.0 { hit.phase -= 1.0; }
                    let tone_env = (-hit.age / hit.tone_decay).exp() as f32;
                    tone_out = (hit.phase * 2.0 * std::f64::consts::PI).sin() as f32
                        * vel * tone_env;
                }

                // Noise component
                let mut noise_out = 0.0f32;
                if hit.noise_decay > 0.0 && hit.noise_level > 0.0 {
                    // xorshift32 PRNG
                    self.rng_state ^= self.rng_state << 13;
                    self.rng_state ^= self.rng_state >> 17;
                    self.rng_state ^= self.rng_state << 5;
                    let noise_raw = (self.rng_state as f64 / u32::MAX as f64) * 2.0 - 1.0;

                    let noise_env = (-hit.age / hit.noise_decay).exp() as f32;
                    noise_out = noise_raw as f32 * vel * hit.noise_level * noise_env;

                    // Optional high-pass filter
                    if hit.noise_hp_alpha > 0.0 {
                        let filtered = noise_out as f64 - hit.noise_hp_prev;
                        hit.noise_hp_prev += hit.noise_hp_alpha * filtered;
                        noise_out = filtered as f32;
                    }
                }

                sum += (tone_out + noise_out) * level;
                hit.age += dt;
            }

            *sample += sum;
        }

        // Prune dead hits (both envelopes decayed below threshold)
        self.hits.retain(|h| {
            let tone_alive = h.tone_decay > 0.0 && (-h.age / h.tone_decay).exp() > 0.001;
            let noise_alive = h.noise_decay > 0.0
                && h.noise_level > 0.0
                && (-h.age / h.noise_decay).exp() > 0.001;
            tone_alive || noise_alive
        });
    }
}

/// Parse --ch entries like "ch2=file:21" or "ch5=file:15@2.0,0.3"
/// into (channel_index, DxVoice, volume, reverb_send).
fn parse_channel_map(entries: &[String]) -> Vec<(usize, DxVoice, f32, f32)> {
    let mut result = Vec::new();
    let mut cache: std::collections::HashMap<String, Vec<DxVoice>> = std::collections::HashMap::new();

    for entry in entries {
        let parts: Vec<&str> = entry.splitn(2, '=').collect();
        if parts.len() != 2 || !parts[0].starts_with("ch") {
            eprintln!("Warning: invalid --ch entry '{}', expected ch<N>=<file>:<voice>[@vol[,rev]]", entry);
            continue;
        }
        let ch_num: usize = match parts[0][2..].parse::<usize>() {
            Ok(n) if n >= 1 && n <= 16 => n - 1,
            _ => {
                eprintln!("Warning: invalid channel in '{}' (must be 1-16)", entry);
                continue;
            }
        };

        // Split off optional @vol,rev suffix
        let (rhs_str, volume, reverb_send) = if let Some(at_pos) = parts[1].rfind('@') {
            let params_str = &parts[1][at_pos + 1..];
            let param_parts: Vec<&str> = params_str.split(',').collect();
            let vol: f32 = param_parts[0].parse().unwrap_or(1.0);
            let rev: f32 = if param_parts.len() > 1 {
                param_parts[1].parse().unwrap_or(0.15)
            } else {
                0.15 // default reverb send
            };
            (&parts[1][..at_pos], vol, rev)
        } else {
            (parts[1], 1.0f32, 0.15f32) // default: 1.0 vol, 0.15 reverb
        };

        let rhs: Vec<&str> = rhs_str.rsplitn(2, ':').collect();
        if rhs.len() != 2 {
            eprintln!("Warning: invalid --ch entry '{}', expected <file>:<voice>", entry);
            continue;
        }
        let voice_idx: usize = match rhs[0].parse() {
            Ok(n) => n,
            _ => {
                eprintln!("Warning: invalid voice index in '{}'", entry);
                continue;
            }
        };
        let file_path = rhs[1];

        let bank = cache.entry(file_path.to_string()).or_insert_with(|| {
            let data = std::fs::read(file_path).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {}", file_path, e);
                Vec::new()
            });
            DxVoice::parse_bulk_dump(&data).unwrap_or_else(|e| {
                eprintln!("Failed to parse {}: {}", file_path, e);
                Vec::new()
            })
        });

        if voice_idx >= bank.len() {
            eprintln!("Warning: voice index {} out of range for {} ({} voices)", voice_idx, file_path, bank.len());
            continue;
        }

        let mut mix_str = String::new();
        if (volume - 1.0).abs() > 0.001 || (reverb_send - 0.15).abs() > 0.001 {
            mix_str = format!(" @{:.1}x rev={:.0}%", volume, reverb_send * 100.0);
        }
        println!("  ch{}: {} [{}:{}]{}", ch_num + 1, bank[voice_idx].name_str(), file_path, voice_idx, mix_str);
        result.push((ch_num, bank[voice_idx].clone(), volume, reverb_send));
    }
    result
}

fn render_midi_file(midi_path: &str, output_path: &str, patch: &DxVoice, patches: &[DxVoice], patch_locked: bool, args: &Args) {
    let data = std::fs::read(midi_path).expect("Failed to read MIDI file");
    let (division, events) = parse_midi_file(&data, &args.track);

    println!(
        "MIDI: {} events, {} ticks/beat",
        events.len(),
        division
    );

    let sample_rate = args.sample_rate;

    // Create 16 independent synth instances (one per MIDI channel)
    // Use 64 voices per synth for offline rendering (concert piano with
    // sustain pedal can exceed the DX7's original 16-voice limit).
    let mut synths: Vec<dx7_core::Synth> = (0..16)
        .map(|_| {
            let mut s = dx7_core::Synth::with_max_voices(sample_rate, 64);
            s.load_patch(patch.clone());
            s.set_master_volume(0.05);
            s
        })
        .collect();

    // Apply channel map overrides (--ch entries)
    // Channels with overrides ignore ProgramChange events from the MIDI file.
    let mut mapped_channels = [false; 16];
    let mut channel_volume = [1.0f32; 16];
    let mut channel_reverb = [0.15f32; 16]; // default reverb send per channel
    channel_reverb[9] = 0.0; // drums: dry by default
    let mut channel_gm_gain = [1.0f32; 16]; // per-program gain compensation (GM mode)
    // DC-blocking filters — one per channel.
    // FM synthesis produces DC offset from asymmetric modulation;
    // the real DX7's coupling capacitors removed this.
    let mut dc_blockers: Vec<dx7_core::effects::DcBlocker> = (0..16)
        .map(|_| dx7_core::effects::DcBlocker::new(sample_rate as f64))
        .collect();
    let mut bass_lpf: Vec<Option<dx7_core::effects::LowPassFilter>> = (0..16)
        .map(|_| None)
        .collect();
    if !args.channel_map.is_empty() {
        println!("Channel map:");
        let map = parse_channel_map(&args.channel_map);
        for (ch, voice, vol, rev) in map {
            synths[ch].load_patch(voice);
            mapped_channels[ch] = true;
            channel_volume[ch] = vol;
            channel_reverb[ch] = rev;
        }
    }

    // Load GM sound set if --gm flag is set
    let gm_set = if args.gm {
        let gm = gm::GmSoundSet::load("sysex");
        println!("GM: loaded 128-program sound set");
        // Set initial patch on all non-mapped channels to GM program 0 (Acoustic Grand Piano)
        for (ch, synth) in synths.iter_mut().enumerate() {
            if !mapped_channels[ch] && ch != 9 {
                if let Some(p) = gm.get(0) {
                    synth.load_patch(p.clone());
                }
            }
        }
        Some(gm)
    } else {
        None
    };

    // Drum machine for GM channel 10 (index 9)
    let mut drums = DrumMachine::new(sample_rate as f64);

    // Render by walking through events, converting tick→sample position
    let mut tempo: u32 = 500_000; // default 120 BPM (standard MIDI default)
    let mut current_tick: u64 = 0;
    let mut _current_sample: u64 = 0;
    // Per-channel stereo panning (0.0 = hard left, 0.5 = center, 1.0 = hard right)
    // Default spread gives each active channel a slightly different position.
    let mut channel_pan = [0.5f32; 16];
    // Spread synth channels across the stereo field
    let pan_positions = [
        0.5,  // ch1: center
        0.5,  // ch2: center (bass)
        0.35, // ch3: slightly left
        0.65, // ch4: slightly right
        0.25, // ch5: left
        0.75, // ch6: right
        0.4,  // ch7: slightly left
        0.6,  // ch8: slightly right
        0.5,  // ch9: center
        0.5,  // ch10: center (drums)
        0.3,  // ch11-16: spread
        0.7, 0.35, 0.65, 0.45, 0.55,
    ];
    for (i, &p) in pan_positions.iter().enumerate() {
        channel_pan[i] = p;
    }

    let mut left_samples: Vec<f32> = Vec::new();
    let mut right_samples: Vec<f32> = Vec::new();
    let mut drum_samples: Vec<f32> = Vec::new();
    let mut reverb_send: Vec<f32> = Vec::new(); // per-channel reverb send (mono)

    for event in &events {
        if event.tick > current_tick {
            // Render samples between current position and this event
            let delta_ticks = event.tick - current_tick;
            let delta_samples = (delta_ticks as f64 * tempo as f64 * sample_rate as f64)
                / (division as f64 * 1_000_000.0);
            let delta_samples = delta_samples as u64;

            if delta_samples > 0 {
                let len = delta_samples as usize;
                let start = left_samples.len();
                left_samples.resize(start + len, 0.0);
                right_samples.resize(start + len, 0.0);
                drum_samples.resize(start + len, 0.0);
                reverb_send.resize(start + len, 0.0);

                // Mix each synth channel with panning
                let mut temp = vec![0.0f32; len];
                for (ch_idx, synth) in synths.iter_mut().enumerate() {
                    temp.iter_mut().for_each(|s| *s = 0.0);
                    synth.render_mono(&mut temp);
                    dc_blockers[ch_idx].process(&mut temp);
                    if let Some(ref mut lpf) = bass_lpf[ch_idx] {
                        lpf.process(&mut temp);
                    }
                    let vol = channel_volume[ch_idx] * channel_gm_gain[ch_idx];
                    let pan = channel_pan[ch_idx];
                    // Equal-power panning
                    let l_gain = (std::f32::consts::FRAC_PI_2 * (1.0 - pan)).cos().max(0.0).sqrt() * vol;
                    let r_gain = (std::f32::consts::FRAC_PI_2 * pan).cos().max(0.0).sqrt() * vol;
                    let rev = channel_reverb[ch_idx];
                    for i in 0..len {
                        left_samples[start + i] += temp[i] * l_gain;
                        right_samples[start + i] += temp[i] * r_gain;
                        reverb_send[start + i] += temp[i] * vol * rev;
                    }
                }

                // Mix drums separately (center)
                drums.render(&mut drum_samples[start..]);

            }
            current_tick = event.tick;
            _current_sample += delta_samples;
        }

        let ch = event.channel as usize;

        // Route drum channel (GM channel 10 = index 9) to drum machine
        if ch == 9 {
            if let MidiEventKind::NoteOn { note, velocity } = &event.kind {
                drums.note_on(*note, *velocity);
            }
            // Handle tempo changes even on ch9
            if let MidiEventKind::Tempo(t) = &event.kind {
                tempo = *t;
            }
            continue;
        }

        match &event.kind {
            MidiEventKind::NoteOn { note, velocity } => {
                synths[ch].note_on(*note, *velocity);
            }
            MidiEventKind::NoteOff { note } => {
                synths[ch].note_off(*note);
            }
            MidiEventKind::ControlChange { cc, value } => {
                synths[ch].control_change(*cc, *value);
            }
            MidiEventKind::PitchBend { value } => {
                // GM default pitch bend range is ±2 semitones.
                // DX7 native range is ±12 semitones.
                // Scale: 2/12 = 1/6.
                let scaled = (*value as i32 / 6) as i16;
                synths[ch].pitch_bend(scaled);
            }
            MidiEventKind::ProgramChange { program } => {
                // Skip ProgramChange for channels with --ch overrides
                // or when --sysex locks the patch selection
                if !mapped_channels[ch] && !patch_locked {
                    if let Some(ref gm) = gm_set {
                        // GM mode: map program number to curated DX7 patch
                        if let Some(p) = gm.get(*program) {
                            synths[ch].load_patch(p.clone());
                            channel_gm_gain[ch] = gm::program_gain(*program);
                            // Bass instruments: dry (no reverb) for tight low end
                            if gm::is_bass_program(*program) {
                                channel_reverb[ch] = 0.0;
                            }
                        }
                    } else if let Some(p) = patches.get(*program as usize) {
                        synths[ch].load_patch(p.clone());
                    }
                }
            }
            MidiEventKind::Tempo(t) => {
                tempo = *t;
            }
        }
    }

    // Render 4s tail for reverb decay
    let tail = (4.0 * sample_rate as f64) as usize;
    let start = left_samples.len();
    left_samples.resize(start + tail, 0.0);
    right_samples.resize(start + tail, 0.0);
    drum_samples.resize(start + tail, 0.0);
    reverb_send.resize(start + tail, 0.0);
    let mut temp = vec![0.0f32; tail];
    for (ch_idx, synth) in synths.iter_mut().enumerate() {
        temp.iter_mut().for_each(|s| *s = 0.0);
        synth.render_mono(&mut temp);
        dc_blockers[ch_idx].process(&mut temp);
        if let Some(ref mut lpf) = bass_lpf[ch_idx] {
            lpf.process(&mut temp);
        }
        let vol = channel_volume[ch_idx] * channel_gm_gain[ch_idx];
        let pan = channel_pan[ch_idx];
        let l_gain = (std::f32::consts::FRAC_PI_2 * (1.0 - pan)).cos().max(0.0).sqrt() * vol;
        let r_gain = (std::f32::consts::FRAC_PI_2 * pan).cos().max(0.0).sqrt() * vol;
        let rev = channel_reverb[ch_idx];
        for i in 0..tail {
            left_samples[start + i] += temp[i] * l_gain;
            right_samples[start + i] += temp[i] * r_gain;
            reverb_send[start + i] += temp[i] * vol * rev;
        }
    }
    drums.render(&mut drum_samples[start..]);

    let num = left_samples.len();

    let mut left = left_samples;
    let mut right = right_samples;

    if !args.dry {
        // Apply soft saturation for analog warmth
        for s in left.iter_mut() {
            *s = dx7_core::effects::soft_saturate(*s);
        }
        for s in right.iter_mut() {
            *s = dx7_core::effects::soft_saturate(*s);
        }

        // Short ambient reverb — adds space without echo
        // Small room, high damping = early reflections only
        let mut reverb = dx7_core::Reverb::new(sample_rate as f32);
        // Lexicon-style hall — large room, very low damping for bright shimmering tail
        reverb.set_params(0.85, 0.45, 0.7);
        let mut rev_l = vec![0.0f32; num];
        let mut rev_r = vec![0.0f32; num];
        reverb.process_mono_to_stereo(&reverb_send, &mut rev_l, &mut rev_r);

        for i in 0..num {
            left[i] += rev_l[i] * 0.15;
            right[i] += rev_r[i] * 0.15;
        }
    }

    // Mix dry drums into center
    for i in 0..num {
        left[i] += drum_samples[i];
        right[i] += drum_samples[i];
    }

    if !args.dry {
        // Stereo chorus — widens the mix
        let mut chorus = dx7_core::effects::Chorus::new(
            sample_rate as f64,
            0.6,   // LFO rate Hz
            7.0,   // center delay ms
            0.8,   // depth ms — subtle widening only, no detuning
            0.45,  // wet mix — stronger stereo spread
        );
        chorus.process_stereo_inplace(&mut left, &mut right);

        // Stereo tremolo — AM modulation for the "wow wow" movement
        let mut tremolo = dx7_core::effects::StereoTremolo::new(
            sample_rate as f64,
            3.5,   // rate Hz — moderate pulse
            0.12,  // depth — gentle for full mix
        );
        tremolo.process_stereo(&mut left, &mut right);

        // Exciter — adds harmonic brightness
        let mut exciter = dx7_core::effects::Exciter::new(
            sample_rate as f64,
            3500.0, // HP cutoff for harmonics
            1.5,    // drive
            0.08,   // mix — subtle sparkle
        );
        exciter.process_stereo(&mut left, &mut right);

        // Stereo widener — boost the side (L-R) component for wider image
        let widener = dx7_core::effects::StereoWidener::new(1.8);
        widener.process_stereo(&mut left, &mut right);
    }

    // Normalize stereo output to -1 dB headroom
    let peak = left
        .iter()
        .chain(right.iter())
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    if peak > 0.0001 {
        let target = 0.891; // -1 dB headroom
        let scale = target / peak;
        for s in left.iter_mut() {
            *s *= scale;
        }
        for s in right.iter_mut() {
            *s *= scale;
        }
        eprintln!(
            "Normalized: peak {:.4} -> {:.3} (scale {:.3})",
            peak, target, scale
        );
    }

    let duration_secs = num as f64 / sample_rate as f64;
    println!(
        "Rendered {:.1}s ({} samples, stereo) to {}",
        duration_secs, num, output_path
    );

    // Write stereo WAV
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(output_path, spec).expect("Failed to create WAV file");
    for i in 0..num {
        let l = (left[i] * 32767.0).clamp(-32768.0, 32767.0) as i16;
        let r = (right[i] * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer.write_sample(l).unwrap();
        writer.write_sample(r).unwrap();
    }
    writer.finalize().unwrap();
}

fn run_interactive(initial_patch: DxVoice, patches: Vec<DxVoice>, args: &Args) {
    let engine = match audio::AudioEngine::start(initial_patch) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Failed to start audio: {err}");
            return;
        }
    };

    println!("Audio: {} Hz", engine.sample_rate);


    // Try to connect MIDI using the shared command channel
    let midi_handler = match midi::MidiHandler::connect(
        args.midi_port.as_deref(),
        engine.command_sender(),
    ) {
        Ok(h) => {
            println!("MIDI: {}", h.port_name);
            Some(h)
        }
        Err(e) => {
            println!("MIDI: not connected ({e})");
            None
        }
    };
    let _ = midi_handler; // Keep connection alive

    #[cfg(feature = "bluetooth")]
    let _ble_handler = if args.bluetooth {
        match bluetooth::BleHandler::start(engine.command_sender()) {
            Ok(h) => {
                println!("BLE MIDI: Advertising as \"DX7\"");
                Some(h)
            }
            Err(e) => {
                eprintln!("BLE MIDI: Failed to start ({e})");
                None
            }
        }
    } else {
        None
    };

    #[cfg(feature = "rtp-midi")]
    let _rtp_handler = if args.rtp_midi {
        match rtpmidi::RtpMidiHandler::start(Some(args.rtp_midi_port), engine.command_sender()) {
            Ok(h) => {
                println!(
                    "RTP-MIDI: Listening on port {} (mDNS: \"DX7\")",
                    args.rtp_midi_port
                );
                Some(h)
            }
            Err(e) => {
                eprintln!("RTP-MIDI: Failed to start ({e})");
                None
            }
        }
    } else {
        None
    };

    println!();
    println!("Controls:");
    println!("  ASDFGHJKL; = white keys (C4-E5)");
    println!("  WETYUOP    = black keys");
    println!("  Z/X        = octave down/up");
    println!("  1-9,0      = select patch");
    println!("  Q/Esc      = quit");
    println!();

    terminal::enable_raw_mode().expect("Failed to enable raw mode");

    let mut kbd = keyboard::KeyboardHandler::new();

    loop {
        let events = kbd.poll(Duration::from_millis(10));

        for event in &events {
            match event {
                keyboard::KeyboardEvent::NoteOn(note, vel) => {
                    engine.send_command(SynthCommand::NoteOn {
                        note: *note,
                        velocity: *vel,
                    });
                }
                keyboard::KeyboardEvent::NoteOff(note) => {
                    engine.send_command(SynthCommand::NoteOff { note: *note });
                }
                keyboard::KeyboardEvent::OctaveChange(oct) => {
                    print!("\r\x1b[K  Octave: {:+}\r", oct);
                }
                keyboard::KeyboardEvent::PatchChange(idx) => {
                    let idx = *idx as usize;
                    if idx < patches.len() {
                        let patch = patches[idx].clone();
                        let name = patch.name_str().to_string();
                        engine.send_command(SynthCommand::LoadPatch(Box::new(patch)));
                        print!("\r\x1b[K  Patch {}: {}\r", idx, name);
                    }
                }
                keyboard::KeyboardEvent::Quit => {
                    terminal::disable_raw_mode().ok();
                    println!("\r\nBye!");
                    return;
                }
            }
        }
    }
}
