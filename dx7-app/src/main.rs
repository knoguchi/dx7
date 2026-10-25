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
mod keyboard;
mod midi;
#[cfg(feature = "rtp-midi")]
mod rtpmidi;

use clap::Parser;
use dx7_core::{get_rom1a_preset, DxVoice, SynthCommand};
use std::io::Write;
use std::sync::Arc;
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
    /// for dry output without effects
    #[arg(long)]
    dry: bool,

    /// Skip peak normalization of rendered WAV output. By default output is
    /// scaled to -1 dBFS peak, which makes --velocity loudness-invariant.
    #[arg(long)]
    no_normalize: bool,

    /// Enable General MIDI sound set (maps program changes to DX7 patches).
    /// Uses the compiled-in GM ROM; no sysex files are needed.
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

    /// Only accept RTP-MIDI sessions from this IP address (repeatable).
    /// Without it any host on the network may connect and play notes.
    #[cfg(feature = "rtp-midi")]
    #[arg(long, value_name = "IP")]
    rtp_midi_allow: Vec<std::net::IpAddr>,
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

    // Load patches (never empty: falls back to the ROM1A presets)
    let patches = load_patches(&args);
    let patch_idx = args.patch.min(patches.len().saturating_sub(1));
    let initial_patch = patches[patch_idx].clone();

    println!("DX7 FM Synthesizer");
    println!("Patch: {} ({})", patch_idx, initial_patch.name_str());

    // MIDI file rendering mode
    if let Some(ref midi_path) = args.midi_file {
        let output_path = args.render.as_deref().unwrap_or("output.wav");
        let patch_locked = args.sysex.is_some();
        if let Err(e) = render_midi_file(midi_path, output_path, &initial_patch, &patches, patch_locked, &args) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // WAV rendering mode (single note)
    if let Some(ref output_path) = args.render {
        if let Err(e) = render_wav(output_path, &initial_patch, &args) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // Interactive mode
    run_interactive(initial_patch, patches, &args);
}

fn load_patches(args: &Args) -> Vec<DxVoice> {
    if let Some(ref syx_path) = args.sysex {
        match std::fs::read(syx_path) {
            Ok(data) => match DxVoice::parse_bulk_dump(&data) {
                Ok(voices) if voices.is_empty() => {
                    eprintln!("SysEx file {} contains no voices; using ROM1A presets", syx_path);
                }
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

fn render_wav(output_path: &str, patch: &DxVoice, args: &Args) -> Result<(), String> {
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

    // Normalize to -1 dB peak (matches professional headroom) unless the
    // caller wants the engine's natural level (e.g. to compare velocities).
    if !args.no_normalize {
        normalize_peak(&mut [&mut all_samples]);
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(output_path, spec)
        .map_err(|e| format!("Failed to create WAV file {output_path}: {e}"))?;

    for &sample in &all_samples {
        let i16_sample = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer.write_sample(i16_sample).map_err(|e| format!("Failed to write WAV: {e}"))?;
    }

    writer.finalize().map_err(|e| format!("Failed to finalize WAV: {e}"))?;
    println!("Written {} samples to {}", total_samples, output_path);
    Ok(())
}

/// Scale all buffers by one common gain so the loudest sample hits -1 dBFS.
fn normalize_peak(buffers: &mut [&mut Vec<f32>]) {
    let peak = buffers
        .iter()
        .flat_map(|b| b.iter())
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    if peak > 0.0001 {
        let target = 0.891; // -1 dB headroom
        let scale = target / peak;
        for b in buffers.iter_mut() {
            for s in b.iter_mut() {
                *s *= scale;
            }
        }
        println!("Normalized: peak {:.4} -> {:.3} (scale {:.3})", peak, target, scale);
    }
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

impl MidiEventKind {
    /// Ordering among events that share a tick: tempo changes first so the
    /// following events are timed correctly, NoteOffs before NoteOns so a
    /// retriggered note is released before it is struck again (and a later
    /// track's NoteOff can't cut an earlier track's new NoteOn), and
    /// controllers/program changes before the notes they affect.
    fn rank(&self) -> u8 {
        match self {
            MidiEventKind::Tempo(_) => 0,
            MidiEventKind::NoteOff { .. } => 1,
            MidiEventKind::ControlChange { .. }
            | MidiEventKind::PitchBend { .. }
            | MidiEventKind::ProgramChange { .. } => 2,
            MidiEventKind::NoteOn { .. } => 3,
        }
    }
}

/// Parse a Standard MIDI File, returning (ticks per beat, events sorted by
/// (tick, rank)). Tempo events are collected from every track regardless of
/// `track_filter`, since a tempo map may live in any track; channel events
/// are taken only from the selected tracks (all tracks when `None`).
fn parse_midi_file(data: &[u8], track_filter: &Option<Vec<usize>>) -> Result<(u16, Vec<MidiEvent>), String> {
    let smf = midly::Smf::parse(data).map_err(|e| format!("Failed to parse MIDI file: {e}"))?;

    let tpb = match smf.header.timing {
        midly::Timing::Metrical(tpb) => tpb.as_int(),
        midly::Timing::Timecode(..) => return Err("SMPTE timecode MIDI files are not supported".into()),
    };

    let mut all_events = Vec::new();

    for (track_idx, track) in smf.tracks.iter().enumerate() {
        let include = match track_filter {
            Some(tracks) => tracks.contains(&track_idx),
            None => true,
        };

        let mut tick: u64 = 0;
        for event in track {
            tick += event.delta.as_int() as u64;
            match event.kind {
                midly::TrackEventKind::Midi { channel, message } => {
                    if !include {
                        continue;
                    }
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

    // Stable sort by (tick, rank): same-tick events keep file order within a
    // rank, but Tempo < NoteOff < CC/bend/program < NoteOn across ranks.
    all_events.sort_by_key(|e| (e.tick, e.kind.rank()));

    Ok((tpb, all_events))
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

/// Default per-channel reverb send; restored when a channel leaves a bass program.
const DEFAULT_REVERB_SEND: f32 = 0.15;

/// Mix every synth channel for `len` samples into the stereo bus and the
/// reverb send, applying per-channel DC blocking, volume and constant-power
/// panning (L = cos θ, R = sin θ, θ = pan·π/2: -3 dB at centre, 0 dB hard).
#[allow(clippy::too_many_arguments)]
fn mix_synth_channels(
    synths: &mut [dx7_core::Synth],
    dc_blockers: &mut [dx7_core::effects::DcBlocker],
    channel_volume: &[f32; 16],
    channel_gm_gain: &[f32; 16],
    channel_pan: &[f32; 16],
    channel_reverb: &[f32; 16],
    temp: &mut [f32],
    left: &mut [f32],
    right: &mut [f32],
    reverb_send: &mut [f32],
) {
    let len = temp.len();
    for (ch_idx, synth) in synths.iter_mut().enumerate() {
        temp.fill(0.0);
        synth.render_mono(temp);
        dc_blockers[ch_idx].process(temp);
        let vol = channel_volume[ch_idx] * channel_gm_gain[ch_idx];
        let theta = std::f32::consts::FRAC_PI_2 * channel_pan[ch_idx].clamp(0.0, 1.0);
        let l_gain = theta.cos() * vol;
        let r_gain = theta.sin() * vol;
        let rev_gain = vol * channel_reverb[ch_idx];
        for i in 0..len {
            left[i] += temp[i] * l_gain;
            right[i] += temp[i] * r_gain;
            reverb_send[i] += temp[i] * rev_gain;
        }
    }
}

fn render_midi_file(
    midi_path: &str,
    output_path: &str,
    patch: &DxVoice,
    patches: &[DxVoice],
    patch_locked: bool,
    args: &Args,
) -> Result<(), String> {
    let data = std::fs::read(midi_path).map_err(|e| format!("Failed to read MIDI file {midi_path}: {e}"))?;
    let (division, events) = parse_midi_file(&data, &args.track)?;

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
    let mut channel_reverb = [DEFAULT_REVERB_SEND; 16]; // default reverb send per channel
    channel_reverb[9] = 0.0; // drums: dry by default
    let mut channel_gm_gain = [1.0f32; 16]; // per-program gain compensation (GM mode)
    // DC-blocking filters — one per channel.
    // FM synthesis produces DC offset from asymmetric modulation;
    // the real DX7's coupling capacitors removed this.
    let mut dc_blockers: Vec<dx7_core::effects::DcBlocker> = (0..16)
        .map(|_| dx7_core::effects::DcBlocker::new(sample_rate as f64))
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
        let gm = gm::GmSoundSet::load();
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

    // Render by walking through events, converting tick→sample position.
    // The exact position is kept in f64 and only rounded when deciding how
    // many samples to emit, so per-event truncation can't accumulate drift.
    let mut tempo: u32 = 500_000; // default 120 BPM (standard MIDI default)
    let mut current_tick: u64 = 0;
    let mut sample_pos: f64 = 0.0;
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

    let mut temp: Vec<f32> = Vec::new();

    for event in &events {
        if event.tick > current_tick {
            // Advance the exact sample position, then render up to it.
            let delta_ticks = event.tick - current_tick;
            sample_pos += (delta_ticks as f64 * tempo as f64 * sample_rate as f64)
                / (division as f64 * 1_000_000.0);
            current_tick = event.tick;

            let start = left_samples.len();
            let target = sample_pos.round() as usize;
            if target > start {
                let len = target - start;
                left_samples.resize(target, 0.0);
                right_samples.resize(target, 0.0);
                drum_samples.resize(target, 0.0);
                reverb_send.resize(target, 0.0);

                temp.resize(len, 0.0);
                mix_synth_channels(
                    &mut synths,
                    &mut dc_blockers,
                    &channel_volume,
                    &channel_gm_gain,
                    &channel_pan,
                    &channel_reverb,
                    &mut temp[..len],
                    &mut left_samples[start..],
                    &mut right_samples[start..],
                    &mut reverb_send[start..],
                );

                // Mix drums separately (center)
                drums.render(&mut drum_samples[start..]);
            }
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
                            // Bass instruments: dry (no reverb) for tight low
                            // end; anything else gets the default send back.
                            channel_reverb[ch] = if gm::is_bass_program(*program) {
                                0.0
                            } else {
                                DEFAULT_REVERB_SEND
                            };
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
    temp.resize(tail, 0.0);
    mix_synth_channels(
        &mut synths,
        &mut dc_blockers,
        &channel_volume,
        &channel_gm_gain,
        &channel_pan,
        &channel_reverb,
        &mut temp[..tail],
        &mut left_samples[start..],
        &mut right_samples[start..],
        &mut reverb_send[start..],
    );
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

    // Normalize stereo output to -1 dB headroom (unless --no-normalize)
    if !args.no_normalize {
        normalize_peak(&mut [&mut left, &mut right]);
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
    let mut writer = hound::WavWriter::create(output_path, spec)
        .map_err(|e| format!("Failed to create WAV file {output_path}: {e}"))?;
    for i in 0..num {
        let l = (left[i] * 32767.0).clamp(-32768.0, 32767.0) as i16;
        let r = (right[i] * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer
            .write_sample(l)
            .and_then(|_| writer.write_sample(r))
            .map_err(|e| format!("Failed to write WAV: {e}"))?;
    }
    writer.finalize().map_err(|e| format!("Failed to finalize WAV: {e}"))?;
    Ok(())
}

fn run_interactive(initial_patch: DxVoice, patches: Vec<DxVoice>, args: &Args) {
    // Flags that only affect MIDI-file rendering are silently useless here.
    if args.gm {
        eprintln!("Warning: --gm only applies to --midi-file rendering; ignored in interactive mode");
    }
    if !args.channel_map.is_empty() {
        eprintln!("Warning: --ch only applies to --midi-file rendering; ignored in interactive mode");
    }
    if args.track.is_some() {
        eprintln!("Warning: --track only applies to --midi-file rendering; ignored in interactive mode");
    }

    let engine = match audio::AudioEngine::start(initial_patch) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Failed to start audio: {err}");
            return;
        }
    };

    println!("Audio: {} Hz", engine.sample_rate);

    // Shared dispatch path for every live input (MIDI, BLE, RTP, keyboard).
    // Program Change messages select from `patches`, like the 1-0 keys.
    let sink = Arc::new(midi::CommandSink::new(engine.command_sender(), patches));

    // Try to connect MIDI using the shared command channel
    let midi_handler = match midi::MidiHandler::connect(args.midi_port.as_deref(), Arc::clone(&sink)) {
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
        match bluetooth::BleHandler::start(Arc::clone(&sink)) {
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
        match rtpmidi::RtpMidiHandler::start(
            Some(args.rtp_midi_port),
            args.rtp_midi_allow.clone(),
            Arc::clone(&sink),
        ) {
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
    println!("  Q/Esc/^C   = quit");
    println!();

    // RAII guard: restores cooked mode (and pops keyboard-enhancement
    // flags) on drop, including during a panic.
    let raw_guard = match keyboard::RawModeGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Failed to enable raw mode: {e}");
            return;
        }
    };
    if !raw_guard.release_supported() {
        print!(
            "  Note: this terminal does not report key release events;\r\n\
             notes sustain until the same key is pressed again.\r\n"
        );
        let _ = std::io::stdout().flush();
    }

    let mut kbd = keyboard::KeyboardHandler::new();

    loop {
        let events = kbd.poll(Duration::from_millis(10));

        for event in &events {
            match event {
                keyboard::KeyboardEvent::NoteOn(note, vel) => {
                    sink.send(SynthCommand::NoteOn {
                        note: *note,
                        velocity: *vel,
                    });
                }
                keyboard::KeyboardEvent::NoteOff(note) => {
                    sink.send(SynthCommand::NoteOff { note: *note });
                }
                keyboard::KeyboardEvent::OctaveChange(oct) => {
                    print!("\r\x1b[K  Octave: {:+}\r", oct);
                }
                keyboard::KeyboardEvent::PatchChange(idx) => {
                    let idx = *idx as usize;
                    if let Some(name) = sink.select_patch(idx) {
                        print!("\r\x1b[K  Patch {}: {}\r", idx, name);
                        let _ = std::io::stdout().flush();
                    }
                }
                keyboard::KeyboardEvent::Quit => {
                    drop(raw_guard);
                    println!("\nBye!");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod midi_file_tests {
    use super::*;
    use midly::num::{u15, u24, u28, u4, u7};
    use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

    fn note(delta: u32, on: bool, key: u8) -> TrackEvent<'static> {
        let message = if on {
            MidiMessage::NoteOn { key: u7::new(key), vel: u7::new(100) }
        } else {
            MidiMessage::NoteOff { key: u7::new(key), vel: u7::new(0) }
        };
        TrackEvent {
            delta: u28::new(delta),
            kind: TrackEventKind::Midi { channel: u4::new(0), message },
        }
    }

    fn tempo(delta: u32, us_per_beat: u32) -> TrackEvent<'static> {
        TrackEvent {
            delta: u28::new(delta),
            kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us_per_beat))),
        }
    }

    /// Three-track file: track 0 holds the initial tempo, track 1 plays C4 for
    /// a beat, track 2 changes tempo and restrikes C4 exactly where track 1
    /// releases it.
    fn test_smf() -> Vec<u8> {
        let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(480))));
        smf.tracks.push(vec![tempo(0, 500_000)]);
        smf.tracks.push(vec![note(0, true, 60), note(480, false, 60)]);
        smf.tracks.push(vec![tempo(480, 250_000), note(0, true, 60)]);
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn tempo_is_collected_from_every_track_despite_filter() {
        let (tpb, events) = parse_midi_file(&test_smf(), &Some(vec![1])).unwrap();
        assert_eq!(tpb, 480);
        let tempos: Vec<u32> = events
            .iter()
            .filter_map(|e| match e.kind {
                MidiEventKind::Tempo(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tempos, vec![500_000, 250_000]);
        // Track 2's NoteOn was filtered out; track 1's two note events remain.
        assert_eq!(events.iter().filter(|e| !matches!(e.kind, MidiEventKind::Tempo(_))).count(), 2);
    }

    #[test]
    fn same_tick_events_order_tempo_noteoff_noteon() {
        let (_, events) = parse_midi_file(&test_smf(), &None).unwrap();
        let at_480: Vec<u8> = events.iter().filter(|e| e.tick == 480).map(|e| e.kind.rank()).collect();
        assert_eq!(at_480, vec![0, 1, 3], "{events:?}");
        assert!(matches!(events.iter().find(|e| e.tick == 480).unwrap().kind, MidiEventKind::Tempo(250_000)));
    }

    #[test]
    fn smpte_timing_is_an_error_not_a_panic() {
        let smf = Smf::new(Header::new(
            Format::SingleTrack,
            Timing::Timecode(midly::Fps::Fps25, 40),
        ));
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let err = parse_midi_file(&bytes, &None).unwrap_err();
        assert!(err.contains("SMPTE"), "{err}");
        assert!(parse_midi_file(b"not a midi file", &None).is_err());
    }
}
