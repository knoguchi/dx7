//! Render a short test note for each GM program to audit sound quality.
//! Output: gm_audit.wav — 128 programs, ~2.5 min total.
//!
//! Usage: cargo run --example gm_audit
//!   Then: afplay gm_audit.wav

use dx7_core::{Synth, DxVoice};
use dx7_core::operator::{OperatorParams, ScalingCurve};
use dx7_core::envelope::EnvParams;
use dx7_core::lfo::LfoParams;
use std::collections::HashMap;
use std::fs;

type GmEntry = (&'static str, usize);

const GM_MAP: [GmEntry; 128] = [
    ("factory/rom1a.syx", 7),   //  0: Acoustic Grand Piano
    ("factory/rom1a.syx", 8),   //  1: Bright Acoustic Piano
    ("factory/rom3b.syx", 1),   //  2: Electric Grand Piano
    ("factory/rom3b.syx", 3),   //  3: Honky-tonk Piano
    ("factory/rom1a.syx", 10),  //  4: Electric Piano 1
    ("factory/rom1b.syx", 2),   //  5: Electric Piano 2
    ("factory/rom1a.syx", 18),  //  6: Harpsichord
    ("factory/rom1a.syx", 19),  //  7: Clavinet
    ("factory/rom1b.syx", 6),   //  8: Celesta
    ("factory/rom2a.syx", 21),  //  9: Glockenspiel
    ("vrc/vrc112b.syx", 26),    // 10: Music Box
    ("factory/rom1a.syx", 20),  // 11: Vibraphone
    ("factory/rom1a.syx", 21),  // 12: Marimba
    ("factory/rom2a.syx", 23),  // 13: Xylophone
    ("factory/rom1a.syx", 25),  // 14: Tubular Bells
    ("vrc/vrc101b.syx", 15),    // 15: Dulcimer
    ("factory/rom1a.syx", 16),  // 16: Drawbar Organ
    ("factory/rom1b.syx", 12),  // 17: Percussive Organ
    ("factory/rom1b.syx", 13),  // 18: Rock Organ
    ("factory/rom1a.syx", 17),  // 19: Church Organ
    ("factory/rom1b.syx", 14),  // 20: Reed Organ
    ("factory/rom1b.syx", 20),  // 21: Accordion
    ("factory/rom2a.syx", 17),  // 22: Harmonica
    ("vrc/vrc102b.syx", 21),    // 23: Tango Accordion
    ("factory/rom3b.syx", 27),  // 24: Acoustic Guitar (nylon)
    ("vrc/vrc101a.syx", 19),    // 25: Acoustic Guitar (steel)
    ("factory/rom3a.syx", 14),  // 26: Jazz Electric Guitar
    ("vrc/vrc101a.syx", 24),    // 27: Clean Electric Guitar
    ("vrc/vrc101a.syx", 28),    // 28: Muted Guitar
    ("factory/rom3a.syx", 20),  // 29: Overdriven Guitar
    ("vrc/vrc109a.syx", 21),    // 30: Distortion Guitar
    ("factory/rom1b.syx", 24),  // 31: Guitar Harmonics
    ("vrc/vrc101b.syx", 8),     // 32: Acoustic Bass
    ("vrc/vrc101a.syx", 29),    // 33: Electric Bass (finger)
    ("factory/rom1b.syx", 30),  // 34: Electric Bass (pick)
    ("factory/rom3a.syx", 17),  // 35: Fretless Bass
    ("factory/rom3b.syx", 31),  // 36: Slap Bass 1
    ("factory/rom1a.syx", 14),  // 37: Slap Bass 2
    ("vrc/vrc101b.syx", 2),     // 38: Synth Bass 1
    ("vrc/vrc101b.syx", 3),     // 39: Synth Bass 2
    ("vrc/vrc103a.syx", 0),     // 40: Violin
    ("factory/rom4a.syx", 14),  // 41: Viola
    ("vrc/vrc103a.syx", 4),     // 42: Cello
    ("vrc/vrc103a.syx", 6),     // 43: Contrabass
    ("factory/rom2a.syx", 6),   // 44: Tremolo Strings
    ("factory/rom4a.syx", 17),  // 45: Pizzicato Strings
    ("factory/rom1b.syx", 28),  // 46: Orchestral Harp
    ("factory/rom1a.syx", 27),  // 47: Timpani
    ("factory/rom1a.syx", 3),   // 48: String Ensemble 1
    ("factory/rom1a.syx", 4),   // 49: String Ensemble 2
    ("vrc/vrc106a.syx", 0),     // 50: Synth Strings 1
    ("vrc/vrc106a.syx", 1),     // 51: Synth Strings 2
    ("factory/rom1a.syx", 29),  // 52: Choir Aahs
    ("factory/rom2a.syx", 19),  // 53: Voice Oohs
    ("factory/rom2b.syx", 12),  // 54: Synth Voice
    ("factory/rom1a.syx", 6),   // 55: Orchestra Hit
    ("vrc/vrc102a.syx", 26),    // 56: Trumpet
    ("vrc/vrc102b.syx", 5),     // 57: Trombone
    ("vrc/vrc102b.syx", 10),    // 58: Tuba
    ("vrc/vrc102b.syx", 2),     // 59: Muted Trumpet
    ("vrc/vrc102a.syx", 21),    // 60: French Horn
    ("factory/rom1a.syx", 0),   // 61: Brass Section
    ("factory/rom2b.syx", 8),   // 62: Synth Brass 1
    ("factory/rom2b.syx", 9),   // 63: Synth Brass 2
    ("vrc/vrc102a.syx", 17),    // 64: Soprano Sax
    ("vrc/vrc102a.syx", 18),    // 65: Alto Sax
    ("vrc/vrc102a.syx", 19),    // 66: Tenor Sax
    ("vrc/vrc102a.syx", 20),    // 67: Baritone Sax
    ("factory/rom2a.syx", 2),   // 68: Oboe
    ("vrc/vrc102a.syx", 9),     // 69: English Horn
    ("factory/rom2a.syx", 5),   // 70: Bassoon
    ("factory/rom2a.syx", 3),   // 71: Clarinet
    ("factory/rom2a.syx", 0),   // 72: Piccolo
    ("factory/rom1a.syx", 23),  // 73: Flute
    ("factory/rom2a.syx", 16),  // 74: Recorder
    ("factory/rom4a.syx", 5),   // 75: Pan Flute
    ("vrc/vrc102b.syx", 15),    // 76: Blown Bottle
    ("vrc/vrc109b.syx", 0),     // 77: Shakuhachi
    ("vrc/vrc103b.syx", 14),    // 78: Whistle
    ("vrc/vrc102b.syx", 13),    // 79: Ocarina
    ("factory/rom2b.syx", 0),   // 80: Square Lead
    ("factory/rom2b.syx", 1),   // 81: Sawtooth Lead
    ("factory/rom1b.syx", 19),  // 82: Calliope Lead
    ("factory/rom2b.syx", 2),   // 83: Chiff Lead
    ("factory/rom1a.syx", 13),  // 84: Charang Lead
    ("vrc/vrc106b.syx", 4),     // 85: Voice Lead
    ("factory/rom4a.syx", 19),  // 86: Fifths Lead
    ("factory/rom2b.syx", 3),   // 87: Bass + Lead
    ("factory/rom2b.syx", 22),  // 88: New Age
    ("vrc/vrc106a.syx", 2),     // 89: Warm Pad
    ("factory/rom2b.syx", 13),  // 90: Polysynth
    ("factory/rom2a.syx", 20),  // 91: Choir Pad
    ("factory/rom4a.syx", 18),  // 92: Bowed Pad
    ("factory/rom2b.syx", 20),  // 93: Metallic Pad
    ("factory/rom2b.syx", 24),  // 94: Halo Pad
    ("factory/rom2b.syx", 23),  // 95: Sweep Pad
    ("vrc/vrc105a.syx", 0),     // 96: Rain
    ("vrc/vrc105b.syx", 18),    // 97: Soundtrack
    ("vrc/vrc109a.syx", 11),    // 98: Crystal
    ("vrc/vrc105b.syx", 19),    // 99: Atmosphere
    ("factory/rom2b.syx", 17),  //100: Brightness
    ("factory/rom2b.syx", 27),  //101: Goblins
    ("factory/rom2b.syx", 16),  //102: Echoes
    ("factory/rom2b.syx", 25),  //103: Sci-fi
    ("factory/rom1b.syx", 21),  //104: Sitar
    ("factory/rom1b.syx", 27),  //105: Banjo
    ("vrc/vrc101b.syx", 22),    //106: Shamisen
    ("factory/rom1a.syx", 22),  //107: Koto
    ("vrc/vrc101b.syx", 15),    //108: Kalimba
    ("vrc/vrc102b.syx", 18),    //109: Bagpipe
    ("vrc/vrc103a.syx", 1),     //110: Fiddle
    ("vrc/vrc102b.syx", 14),    //111: Shanai
    ("factory/rom2a.syx", 27),  //112: Tinkle Bell
    ("vrc/vrc104b.syx", 11),    //113: Agogo
    ("factory/rom1a.syx", 26),  //114: Steel Drums
    ("factory/rom4a.syx", 27),  //115: Woodblock
    ("vrc/vrc104a.syx", 18),    //116: Taiko Drum
    ("vrc/vrc104a.syx", 3),     //117: Melodic Tom
    ("vrc/vrc104a.syx", 9),     //118: Synth Drum
    ("vrc/vrc104b.syx", 21),    //119: Reverse Cymbal
    ("factory/rom2a.syx", 30),  //120: Guitar Fret Noise
    ("vrc/vrc105a.syx", 4),     //121: Breath Noise
    ("vrc/vrc105a.syx", 6),     //122: Seashore
    ("vrc/vrc105b.syx", 5),     //123: Bird Tweet
    ("vrc/vrc105a.syx", 27),    //124: Telephone Ring
    ("vrc/vrc105a.syx", 24),    //125: Helicopter
    ("vrc/vrc105a.syx", 10),    //126: Applause
    ("vrc/vrc105a.syx", 11),    //127: Gunshot
];

fn make_bass_op(level: u8, coarse: u8, rates: [u8;4], levels: [u8;4], kvs: u8) -> OperatorParams {
    OperatorParams {
        eg: EnvParams { rates, levels },
        kbd_level_scaling_break_point: 39,
        kbd_level_scaling_left_depth: 0,
        kbd_level_scaling_right_depth: 0,
        kbd_level_scaling_left_curve: ScalingCurve::NegLin,
        kbd_level_scaling_right_curve: ScalingCurve::NegLin,
        kbd_rate_scaling: 0,
        amp_mod_sensitivity: 0,
        key_velocity_sensitivity: kvs,
        output_level: level,
        osc_mode: 0,
        osc_freq_coarse: coarse,
        osc_freq_fine: 0,
        osc_detune: 7,
    }
}

fn gm_bass_patch() -> DxVoice {
    let ops = [
        make_bass_op(90, 1, [99,80,70,55], [99,92,88,0], 2),
        make_bass_op(99, 1, [99,80,70,55], [99,92,88,0], 2),
        make_bass_op(88, 2, [99,80,70,55], [99,88,82,0], 2),
        make_bass_op(99, 1, [99,80,70,55], [99,92,88,0], 2),
        make_bass_op(78, 3, [99,80,70,55], [99,85,78,0], 0),
        make_bass_op(99, 1, [99,80,70,55], [99,92,88,0], 2),
    ];
    DxVoice {
        operators: ops,
        pitch_eg: EnvParams { rates: [99,99,99,99], levels: [50,50,50,50] },
        algorithm: 4,
        feedback: 6,
        osc_key_sync: true,
        lfo: LfoParams::default(),
        pitch_mod_sensitivity: 0,
        transpose: 24,
        name: *b"FM BASS   ",
    }
}

fn program_gain(program: u8) -> f32 {
    match program {
        32..=39 => 4.0,
        40..=43 => 1.5,
        44..=47 => 1.3,
        48..=51 => 1.2,
        64..=71 => 1.3,
        72..=79 => 1.3,
        _ => 1.0,
    }
}

fn main() {
    let sysex_dir = "sysex";
    let sample_rate = 44100u32;
    let sr = sample_rate as f64;
    let note_on_secs = 0.8;
    let release_secs = 0.4;
    let note_on_samples = (note_on_secs * sr) as usize;
    let release_samples = (release_secs * sr) as usize;
    let total_per_note = note_on_samples + release_samples;

    let gm_names = [
        "Acoustic Grand Piano", "Bright Acoustic Piano", "Electric Grand Piano", "Honky-tonk Piano",
        "Electric Piano 1", "Electric Piano 2", "Harpsichord", "Clavinet",
        "Celesta", "Glockenspiel", "Music Box", "Vibraphone",
        "Marimba", "Xylophone", "Tubular Bells", "Dulcimer",
        "Drawbar Organ", "Percussive Organ", "Rock Organ", "Church Organ",
        "Reed Organ", "Accordion", "Harmonica", "Tango Accordion",
        "Acoustic Guitar (nylon)", "Acoustic Guitar (steel)", "Jazz Electric Guitar", "Clean Electric Guitar",
        "Muted Guitar", "Overdriven Guitar", "Distortion Guitar", "Guitar Harmonics",
        "Acoustic Bass", "Electric Bass (finger)", "Electric Bass (pick)", "Fretless Bass",
        "Slap Bass 1", "Slap Bass 2", "Synth Bass 1", "Synth Bass 2",
        "Violin", "Viola", "Cello", "Contrabass",
        "Tremolo Strings", "Pizzicato Strings", "Orchestral Harp", "Timpani",
        "String Ensemble 1", "String Ensemble 2", "Synth Strings 1", "Synth Strings 2",
        "Choir Aahs", "Voice Oohs", "Synth Voice", "Orchestra Hit",
        "Trumpet", "Trombone", "Tuba", "Muted Trumpet",
        "French Horn", "Brass Section", "Synth Brass 1", "Synth Brass 2",
        "Soprano Sax", "Alto Sax", "Tenor Sax", "Baritone Sax",
        "Oboe", "English Horn", "Bassoon", "Clarinet",
        "Piccolo", "Flute", "Recorder", "Pan Flute",
        "Blown Bottle", "Shakuhachi", "Whistle", "Ocarina",
        "Square Lead", "Sawtooth Lead", "Calliope Lead", "Chiff Lead",
        "Charang Lead", "Voice Lead", "Fifths Lead", "Bass + Lead",
        "New Age", "Warm Pad", "Polysynth", "Choir Pad",
        "Bowed Pad", "Metallic Pad", "Halo Pad", "Sweep Pad",
        "Rain", "Soundtrack", "Crystal", "Atmosphere",
        "Brightness", "Goblins", "Echoes", "Sci-fi",
        "Sitar", "Banjo", "Shamisen", "Koto",
        "Kalimba", "Bagpipe", "Fiddle", "Shanai",
        "Tinkle Bell", "Agogo", "Steel Drums", "Woodblock",
        "Taiko Drum", "Melodic Tom", "Synth Drum", "Reverse Cymbal",
        "Guitar Fret Noise", "Breath Noise", "Seashore", "Bird Tweet",
        "Telephone Ring", "Helicopter", "Applause", "Gunshot",
    ];

    // Load patches from sysex (same as gen_gm_rom)
    let mut cache: HashMap<&str, Vec<DxVoice>> = HashMap::new();
    let mut patches: Vec<DxVoice> = Vec::with_capacity(128);

    for &(file, voice_idx) in GM_MAP.iter() {
        let bank = cache.entry(file).or_insert_with(|| {
            let path = format!("{}/{}", sysex_dir, file);
            let data = fs::read(&path).unwrap_or_else(|e| panic!("Cannot read {}: {}", path, e));
            DxVoice::parse_bulk_dump(&data).unwrap_or_else(|e| panic!("Cannot parse {}: {}", path, e))
        });
        patches.push(bank[voice_idx].clone());
    }

    // Override bass (32-39)
    let bass = gm_bass_patch();
    for prog in 32..=39usize {
        patches[prog] = bass.clone();
    }

    // Speed up string ensemble attacks (48-51)
    for prog in 48..=51usize {
        for op in patches[prog].operators.iter_mut() {
            if op.output_level > 0 {
                op.eg.rates[0] = op.eg.rates[0].max(85);
            }
        }
    }

    let mut all_samples: Vec<f32> = Vec::new();
    let mut synth = Synth::new(sample_rate);

    for prog in 0..128u8 {
        let patch = &patches[prog as usize];
        synth.load_patch(patch.clone());

        // Choose appropriate note for the instrument
        let note = match prog {
            32..=39 => 36, // Bass: C2
            43 => 36,      // Contrabass: C2
            40..=42 => 60, // Solo strings: C4
            44..=47 => 60, // Tremolo/pizz: C4
            48..=51 => 60, // String ensemble: C4
            56..=63 => 60, // Brass: C4
            64..=71 => 65, // Reeds: F4
            72..=79 => 72, // Pipes: C5
            _ => 60,       // Default: C4
        };

        // Render note-on
        let mut buf = vec![0.0f32; total_per_note];
        synth.note_on(note, 100);
        synth.render_mono(&mut buf[..note_on_samples]);

        // Note off + release tail
        synth.note_off(note);
        synth.render_mono(&mut buf[note_on_samples..]);

        // Apply gain compensation
        let gain = program_gain(prog);
        for s in buf.iter_mut() {
            *s *= gain;
        }

        // Peak normalize if too loud
        let peak = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        if peak > 0.9 {
            let scale = 0.9 / peak;
            for s in buf.iter_mut() {
                *s *= scale;
            }
        }

        let dx_name = std::str::from_utf8(&patch.name).unwrap_or("?").trim();
        eprintln!("{:>3}: {:30} [{}] peak={:.3}", prog, gm_names[prog as usize], dx_name, peak);

        all_samples.extend_from_slice(&buf);
    }

    // Write WAV
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create("gm_audit.wav", spec).unwrap();
    for &s in &all_samples {
        let sample = (s * 32767.0).clamp(-32767.0, 32767.0) as i16;
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();

    let total_secs = all_samples.len() as f64 / sr;
    eprintln!("\nWrote gm_audit.wav ({:.1}s, {} programs)", total_secs, 128);
    eprintln!("Each program: {:.1}s note-on + {:.1}s release", note_on_secs, release_secs);
}
