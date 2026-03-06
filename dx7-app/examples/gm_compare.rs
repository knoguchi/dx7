//! Compare candidate patches for GM programs that need improvement.
//! Renders each candidate as a short note, separated by silence.
//! Output: gm_compare.wav
//!
//! Usage: cargo run --example gm_compare && afplay gm_compare.wav

use dx7_core::{Synth, DxVoice};
use std::collections::HashMap;
use std::fs;

struct Candidate {
    gm_name: &'static str,
    gm_prog: u8,
    note: u8,
    patches: Vec<(&'static str, usize, &'static str)>, // (sysex_file, voice_idx, label)
}

fn main() {
    let sample_rate = 44100u32;
    let sr = sample_rate as f64;
    let note_on_secs = 1.0;
    let release_secs = 0.5;
    let gap_secs = 0.3;
    let note_on_samples = (note_on_secs * sr) as usize;
    let release_samples = (release_secs * sr) as usize;
    let gap_samples = (gap_secs * sr) as usize;

    let candidates = vec![
        Candidate {
            gm_name: "Acoustic Guitar (steel)",
            gm_prog: 25,
            note: 60,
            patches: vec![
                ("factory/rom3b.syx", 24, "FOLK GUIT (current)"),
                ("vrc/vrc101a.syx", 19, "AC.GUITAR1"),
                ("vrc/vrc101a.syx", 20, "AC.GUITAR2"),
                ("vrc/vrc101a.syx", 21, "AC.GUITAR3"),
                ("vrc/vrc101a.syx", 23, "12S.GUITAR"),
            ],
        },
        Candidate {
            gm_name: "Muted Guitar",
            gm_prog: 28,
            note: 60,
            patches: vec![
                ("vrc/vrc101a.syx", 27, "E.GUITAR 4 (current)"),
                ("vrc/vrc101a.syx", 28, "E.GUITAR 5"),
                ("vrc/vrc101a.syx", 25, "E.GUITAR 2"),
                ("vrc/vrc101a.syx", 26, "E.GUITAR 3"),
            ],
        },
        Candidate {
            gm_name: "String Ensemble 1",
            gm_prog: 48,
            note: 60,
            patches: vec![
                ("factory/rom1a.syx", 3, "STRINGS 1 (current)"),
                ("vrc/vrc103a.syx", 7, "STRINGS  1 (VRC)"),
                ("vrc/vrc103a.syx", 8, "STRINGS  2 (VRC)"),
                ("vrc/vrc103a.syx", 9, "STRINGS  3 (VRC)"),
                ("vrc/vrc103a.syx", 10, "STRINGS  4 (VRC)"),
            ],
        },
        Candidate {
            gm_name: "String Ensemble 2",
            gm_prog: 49,
            note: 60,
            patches: vec![
                ("factory/rom1a.syx", 4, "STRINGS 2 (current)"),
                ("vrc/vrc103a.syx", 11, "STRINGS  5 (VRC)"),
                ("vrc/vrc103a.syx", 12, "STRINGS  6 (VRC)"),
                ("factory/rom4a.syx", 13, "STRG ENS 2"),
                ("factory/rom3a.syx", 2, "STRG ENS 1"),
            ],
        },
        Candidate {
            gm_name: "Choir Aahs",
            gm_prog: 52,
            note: 60,
            patches: vec![
                ("factory/rom1a.syx", 29, "VOICE 1 (current)"),
                ("vrc/vrc103a.syx", 22, "M. VOICE 1"),
                ("vrc/vrc103a.syx", 28, "F. VOICE 1"),
                ("vrc/vrc103b.syx", 7, "CHORUS   1"),
                ("vrc/vrc103b.syx", 0, "M.CHORUS 1"),
            ],
        },
        Candidate {
            gm_name: "Violin",
            gm_prog: 40,
            note: 67, // G4
            patches: vec![
                ("vrc/vrc103a.syx", 0, "VIOLIN 1 (current)"),
                ("vrc/vrc103a.syx", 1, "VIOLIN 2"),
                ("vrc/vrc103a.syx", 2, "VIOLIN 3"),
                ("vrc/vrc103a.syx", 3, "VIOLIN 4"),
            ],
        },
        Candidate {
            gm_name: "Bright Acoustic Piano",
            gm_prog: 1,
            note: 60,
            patches: vec![
                ("factory/rom1a.syx", 8, "PIANO 2 (current)"),
                ("factory/rom1a.syx", 9, "PIANO 3"),
                ("vrc/vrc101a.syx", 0, "PIANO 1 (VRC)"),
                ("vrc/vrc101a.syx", 1, "PIANO 2 (VRC)"),
            ],
        },
        Candidate {
            gm_name: "Overdriven Guitar",
            gm_prog: 29,
            note: 60,
            patches: vec![
                ("factory/rom3a.syx", 20, "HEAVYMETAL (current)"),
                ("vrc/vrc109a.syx", 22, "8VER-GUIT"),
            ],
        },
    ];

    let sysex_dir = "sysex";
    let mut cache: HashMap<&str, Vec<DxVoice>> = HashMap::new();
    let mut synth = Synth::new(sample_rate);
    let mut all_samples: Vec<f32> = Vec::new();

    for cand in &candidates {
        eprintln!("\n=== GM {:>3}: {} ===", cand.gm_prog, cand.gm_name);

        for &(file, voice_idx, label) in &cand.patches {
            let bank = cache.entry(file).or_insert_with(|| {
                let path = format!("{}/{}", sysex_dir, file);
                let data = fs::read(&path).unwrap_or_else(|e| panic!("Cannot read {}: {}", path, e));
                DxVoice::parse_bulk_dump(&data).unwrap_or_else(|e| panic!("Cannot parse {}: {}", path, e))
            });
            let patch = &bank[voice_idx];
            synth.load_patch(patch.clone());

            let mut buf = vec![0.0f32; note_on_samples + release_samples];
            synth.note_on(cand.note, 100);
            synth.render_mono(&mut buf[..note_on_samples]);
            synth.note_off(cand.note);
            synth.render_mono(&mut buf[note_on_samples..]);

            let peak = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            let dx_name = std::str::from_utf8(&patch.name).unwrap_or("?").trim();
            eprintln!("  {} [{}] peak={:.3}", label, dx_name, peak);

            all_samples.extend_from_slice(&buf);
            // Gap of silence between candidates
            all_samples.extend(std::iter::repeat(0.0f32).take(gap_samples));
        }

        // Longer gap between GM programs
        all_samples.extend(std::iter::repeat(0.0f32).take(gap_samples * 2));
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create("gm_compare.wav", spec).unwrap();
    for &s in &all_samples {
        let sample = (s * 32767.0).clamp(-32767.0, 32767.0) as i16;
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();

    let total_secs = all_samples.len() as f64 / sr;
    eprintln!("\nWrote gm_compare.wav ({:.1}s)", total_secs);
    eprintln!("Listen to compare patches for each GM program group.");
}
