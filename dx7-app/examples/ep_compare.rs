//! Compare all E.Piano variants — renders a short chord with each.
//! Usage: cargo run --example ep_compare && afplay ep_compare.wav

use dx7_core::{Synth, DxVoice};
use std::collections::HashMap;
use std::fs;

fn main() {
    let sample_rate = 44100u32;
    let sr = sample_rate as f64;
    let note_on = (1.5 * sr) as usize;
    let release = (0.8 * sr) as usize;
    let gap = (0.4 * sr) as usize;

    let patches: Vec<(&str, usize, &str)> = vec![
        ("factory/rom1a.syx", 10, "E.PIANO 1 (rom1a) - sharp tine"),
        ("factory/rom1b.syx", 2,  "E.PIANO 2 (rom1b) - hammer noise"),
        ("factory/rom1b.syx", 3,  "E.PIANO 3 (rom1b)"),
        ("factory/rom1b.syx", 4,  "E.PIANO 4 (rom1b)"),
        ("factory/rom3b.syx", 4,  "E.PIANO 2 (rom3b)"),
        ("factory/rom3b.syx", 5,  "E.PIANO 3 (rom3b)"),
        ("factory/rom3b.syx", 6,  "E.PIANO 4 (rom3b)"),
        ("vrc/vrc101a.syx", 6,    "E.PIANO 1 (VRC)"),
        ("vrc/vrc101a.syx", 7,    "E.PIANO 2 (VRC)"),
        ("vrc/vrc101a.syx", 8,    "E.PIANO 3 (VRC)"),
        ("vrc/vrc101a.syx", 9,    "E.PIANO 4 (VRC)"),
    ];

    let mut cache: HashMap<&str, Vec<DxVoice>> = HashMap::new();
    let mut synth = Synth::new(sample_rate);
    let mut all: Vec<f32> = Vec::new();

    for &(file, idx, label) in &patches {
        let bank = cache.entry(file).or_insert_with(|| {
            let path = format!("sysex/{}", file);
            let data = fs::read(&path).unwrap();
            DxVoice::parse_bulk_dump(&data).unwrap()
        });
        let patch = &bank[idx];
        synth.load_patch(patch.clone());

        // Play a Cmaj7 chord (C4 E4 G4 B4)
        let mut buf = vec![0.0f32; note_on + release];
        synth.note_on(60, 85);
        synth.note_on(64, 80);
        synth.note_on(67, 80);
        synth.note_on(71, 75);
        synth.render_mono(&mut buf[..note_on]);
        synth.note_off(60);
        synth.note_off(64);
        synth.note_off(67);
        synth.note_off(71);
        synth.render_mono(&mut buf[note_on..]);

        let peak = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let dx_name = std::str::from_utf8(&patch.name).unwrap_or("?").trim();
        eprintln!("{:>2}: {} [{}] peak={:.3}", all.len() / (note_on + release + gap), label, dx_name, peak);

        all.extend_from_slice(&buf);
        all.extend(std::iter::repeat(0.0f32).take(gap));
    }

    let spec = hound::WavSpec {
        channels: 1, sample_rate: 44100,
        bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create("ep_compare.wav", spec).unwrap();
    for &s in &all { w.write_sample((s * 32767.0).clamp(-32767.0, 32767.0) as i16).unwrap(); }
    w.finalize().unwrap();
    eprintln!("\nWrote ep_compare.wav ({:.1}s)", all.len() as f64 / sr);
}
