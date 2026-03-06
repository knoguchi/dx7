//! Compare Rust render_core output against Dexed's EngineMkI::render.
//! Feeds identical FmOpParams to both and compares sample-by-sample.
//!
//! Run: cargo test --test dexed_cmp -- --nocapture

use dx7_core::patch::DxVoice;
use dx7_core::tables;
use dx7_core::voice::Voice;
use std::io::Write;
use std::process::{Command, Stdio};

const N: usize = 64;

fn ensure_init() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        tables::init_tables(44100);
        dx7_core::lfo::init_lfo(44100);
        dx7_core::pitchenv::init_pitchenv(44100);
    });
}

/// Dump the current params state in the format expected by the C++ test program.
fn dump_params(voice: &Voice, num_blocks: usize) -> String {
    let mut s = format!("{} {} {} {}\n",
        voice.algorithm, voice.fb_shift, voice.fb_buf[0], voice.fb_buf[1]);
    for op in 0..6 {
        s += &format!("{} {} {} {}\n",
            voice.params[op].level_in,
            voice.params[op].gain_out,
            voice.params[op].freq,
            voice.params[op].phase);
    }
    s += &format!("{}\n", num_blocks);
    s
}

#[test]
fn test_algo17_flunk_bass_vs_dexed() {
    ensure_init();

    let v = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);
    let mut voice = Voice::new();
    voice.note_on(&v, 36, 100); // C2, vel=100

    eprintln!("\n=== DEXED vs RUST: Algorithm 17 Render Comparison ===");
    eprintln!("Algorithm: {} (idx {}), fb_shift: {}",
        voice.algorithm + 1, voice.algorithm, voice.fb_shift);

    // Render block 0 through the full render() path to advance envelopes
    let mut block0 = [0i32; N];
    voice.render(&mut block0);

    eprintln!("Block 0 (via render()): peak={}",
        block0.iter().map(|s| s.abs()).max().unwrap_or(0));
    eprintln!("  first8: [{}, {}, {}, {}, {}, {}, {}, {}]",
        block0[0], block0[1], block0[2], block0[3],
        block0[4], block0[5], block0[6], block0[7]);

    // Now capture the state AFTER block 0.
    // params have: level_in from block 0's env tick,
    //              gain_out set by render_core,
    //              freq from block 0's freq computation,
    //              phase advanced by render_core.
    // Feed this EXACT state to both Rust render_core and C++ render.
    let num_test_blocks = 5;
    let input_state = dump_params(&voice, num_test_blocks);

    eprintln!("\nInput state for render_core comparison:");
    eprintln!("{}", input_state.trim());

    // Run Rust render_core for num_test_blocks blocks
    let mut rust_outputs: Vec<[i32; N]> = Vec::new();
    for _block in 0..num_test_blocks {
        let mut output = [0i32; N];
        voice.render_core(&mut output);
        rust_outputs.push(output);
        // render_core advances phase and updates gain_out internally
    }

    for (i, out) in rust_outputs.iter().enumerate() {
        eprintln!("Rust block {}: peak={}, first4=[{}, {}, {}, {}]",
            i, out.iter().map(|s| s.abs()).max().unwrap_or(0),
            out[0], out[1], out[2], out[3]);
    }

    // Run C++ program
    let dexed_test = "/tmp/dexed_cmp/dexed_test";
    if !std::path::Path::new(dexed_test).exists() {
        eprintln!("\nWARNING: {} not found, skipping C++ comparison", dexed_test);
        eprintln!("Compile: cd /tmp/dexed_cmp && c++ -std=c++17 -O0 -o dexed_test dexed_algo17_test.cpp");
        return;
    }

    let mut child = Command::new(dexed_test)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn dexed_test");

    child.stdin.as_mut().unwrap().write_all(input_state.as_bytes()).unwrap();
    drop(child.stdin.take()); // close stdin

    let result = child.wait_with_output().expect("Failed to wait for dexed_test");

    if !result.status.success() {
        eprintln!("C++ stderr: {}", String::from_utf8_lossy(&result.stderr));
        panic!("C++ comparison program failed");
    }

    let stdout = String::from_utf8_lossy(&result.stdout);

    // Parse C++ output: multiple blocks
    let mut cpp_blocks: Vec<Vec<i32>> = Vec::new();
    let mut current_block: Vec<i32> = Vec::new();

    for line in stdout.lines() {
        if line.starts_with("BLOCK") {
            if !current_block.is_empty() {
                cpp_blocks.push(current_block);
                current_block = Vec::new();
            }
            continue;
        }
        if line.starts_with("PARAMS") || line.starts_with("FB") {
            continue;
        }
        if let Ok(val) = line.trim().parse::<i32>() {
            current_block.push(val);
        }
    }
    if !current_block.is_empty() {
        cpp_blocks.push(current_block);
    }

    assert_eq!(cpp_blocks.len(), num_test_blocks,
        "Expected {} blocks from C++, got {}", num_test_blocks, cpp_blocks.len());

    // Compare block by block, sample by sample
    let mut total_diffs = 0;
    let mut max_diff: i64 = 0;

    for (block, cpp_out) in cpp_blocks.iter().enumerate() {
        assert_eq!(cpp_out.len(), N,
            "Block {} has {} samples instead of {}", block, cpp_out.len(), N);

        let rust_out = &rust_outputs[block];
        let mut block_diffs = 0;
        let mut block_max_diff: i64 = 0;

        for i in 0..N {
            let diff = (rust_out[i] as i64 - cpp_out[i] as i64).abs();
            if diff > 0 {
                block_diffs += 1;
                total_diffs += 1;
                if diff > block_max_diff { block_max_diff = diff; }
                if diff > max_diff { max_diff = diff; }
                if block_diffs <= 3 {
                    eprintln!("  DIFF block {} sample {}: rust={} cpp={} diff={}",
                        block, i, rust_out[i], cpp_out[i], diff);
                }
            }
        }

        let cpp_peak = cpp_out.iter().map(|s| s.abs()).max().unwrap_or(0);
        if block_diffs > 0 {
            eprintln!("  Block {block}: {block_diffs}/64 differ, max_diff={block_max_diff}, cpp_peak={cpp_peak}");
        } else {
            eprintln!("  Block {block}: MATCH (all 64 identical), cpp_peak={cpp_peak}");
        }
    }

    eprintln!("\n=== RESULT ===");
    eprintln!("Total differing samples: {total_diffs}/{}", num_test_blocks * N);
    eprintln!("Max sample difference: {max_diff}");

    if total_diffs == 0 {
        eprintln!("PERFECT MATCH!");
    }

    assert_eq!(total_diffs, 0,
        "Rust and Dexed output must match. {} samples differ, max_diff={}",
        total_diffs, max_diff);
}

/// Full-pipeline comparison: initialize Flunk Bass identically in both
/// Dexed C++ and our Rust code, run full render() for many blocks,
/// compare every sample.
#[test]
fn test_full_pipeline_flunk_bass_vs_dexed() {
    ensure_init();

    let v = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);
    let unpacked = v.to_unpacked();

    // MIDI note — sent directly to both engines (transpose is not applied)
    let midi_note = 36i32;
    let velocity = 100;
    let num_blocks = 3000; // ~4.4 seconds

    // Initialize Rust voice
    let mut voice = Voice::new();
    voice.note_on(&v, midi_note as u8, velocity as u8);

    // Print basepitch for each op
    for op in 0..6 {
        eprintln!("  Rust OP{}: basepitch={}", 6 - op, voice.basepitch[op]);
    }

    // Render all blocks in Rust
    let mut rust_samples: Vec<i32> = Vec::with_capacity(num_blocks * N);
    for block in 0..num_blocks {
        let mut buf = [0i32; N];
        voice.render(&mut buf);
        if block < 5 {
            eprint!("Rust Block {} level_in:", block);
            for op in 0..6 {
                eprint!(" {}", voice.params[op].level_in);
            }
            eprintln!();
        }
        rust_samples.extend_from_slice(&buf);
    }

    // Run C++ full pipeline
    let dexed_test = "/tmp/dexed_cmp/full_pipeline_test";
    if !std::path::Path::new(dexed_test).exists() {
        eprintln!("\nWARNING: {} not found, skipping full pipeline comparison", dexed_test);
        eprintln!("Compile: cd /tmp/dexed_cmp && c++ -std=c++17 -O0 -o full_pipeline_test full_pipeline_test.cpp -lm");
        return;
    }

    // Build input: 156 patch bytes + midinote + velocity + num_blocks
    let mut input = String::new();
    for b in &unpacked {
        input += &format!("{} ", b);
    }
    input += &format!("\n{} {} {}\n", midi_note, velocity, num_blocks);

    let mut child = Command::new(dexed_test)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn full_pipeline_test");

    child.stdin.as_mut().unwrap().write_all(input.as_bytes()).unwrap();
    drop(child.stdin.take());

    let result = child.wait_with_output().expect("Failed to wait for full_pipeline_test");

    let stderr_str = String::from_utf8_lossy(&result.stderr);
    eprintln!("C++ stderr:\n{}", stderr_str);

    if !result.status.success() {
        panic!("C++ full pipeline test failed");
    }

    let stdout = String::from_utf8_lossy(&result.stdout);

    // Parse C++ output: one i32 per line
    let cpp_samples: Vec<i32> = stdout.lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .collect();

    let expected_total = num_blocks * N;
    assert_eq!(cpp_samples.len(), expected_total,
        "Expected {} samples from C++, got {}", expected_total, cpp_samples.len());

    // Compare sample by sample
    let mut total_diffs = 0;
    let mut max_diff: i64 = 0;
    let mut first_diff_block = None;

    for block in 0..num_blocks {
        let mut block_diffs = 0;
        let mut block_max_diff: i64 = 0;

        for i in 0..N {
            let idx = block * N + i;
            let diff = (rust_samples[idx] as i64 - cpp_samples[idx] as i64).abs();
            if diff > 0 {
                block_diffs += 1;
                total_diffs += 1;
                if diff > block_max_diff { block_max_diff = diff; }
                if diff > max_diff { max_diff = diff; }
                if first_diff_block.is_none() {
                    first_diff_block = Some(block);
                }
                if total_diffs <= 10 {
                    eprintln!("  DIFF block {} sample {}: rust={} cpp={} diff={}",
                        block, i, rust_samples[idx], cpp_samples[idx], diff);
                }
            }
        }

        if block < 5 || block_diffs > 0 {
            let rust_peak = rust_samples[block*N..(block+1)*N].iter()
                .map(|s| s.abs()).max().unwrap_or(0);
            let cpp_peak = cpp_samples[block*N..(block+1)*N].iter()
                .map(|s| s.abs()).max().unwrap_or(0);

            if block_diffs > 0 {
                eprintln!("  Block {}: {}/{} differ, max_diff={}, rust_peak={}, cpp_peak={}",
                    block, block_diffs, N, block_max_diff, rust_peak, cpp_peak);
            } else if block < 5 {
                eprintln!("  Block {}: MATCH, rust_peak={}, cpp_peak={}",
                    block, rust_peak, cpp_peak);
            }
        }
    }

    eprintln!("\n=== FULL PIPELINE RESULT ===");
    eprintln!("Total samples: {}", expected_total);
    eprintln!("Differing samples: {}", total_diffs);
    eprintln!("Max difference: {}", max_diff);
    if let Some(fb) = first_diff_block {
        eprintln!("First diff at block: {}", fb);
    }

    if total_diffs == 0 {
        eprintln!("PERFECT MATCH across full pipeline!");
    }

    // Allow small differences due to floating-point in osc_freq detune
    // (our Rust uses f64, C++ uses float for the detune ratio)
    assert!(total_diffs == 0 || max_diff <= 1,
        "Full pipeline: {} samples differ, max_diff={}. First diff at block {:?}",
        total_diffs, max_diff, first_diff_block);

    // Write both raw voice outputs to WAV for listening comparison
    write_i32_wav("/tmp/flunk_bass_RUST_voice.wav", &rust_samples, 44100);
    write_i32_wav("/tmp/flunk_bass_DEXED_voice.wav", &cpp_samples, 44100);
    eprintln!("Written: /tmp/flunk_bass_RUST_voice.wav  (raw voice, normalized)");
    eprintln!("Written: /tmp/flunk_bass_DEXED_voice.wav (raw voice, normalized)");

    // Also render Dexed's full output chain (conversion + DC filter) to WAV
    // This is what Dexed's VST actually outputs (with default settings: no LP filter, unity gain)
    let mut child_wav = Command::new(dexed_test)
        .args(&["--wav", "/tmp/flunk_bass_DEXED_chain.wav"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn full_pipeline_test for WAV");

    child_wav.stdin.as_mut().unwrap().write_all(input.as_bytes()).unwrap();
    drop(child_wav.stdin.take());

    let wav_result = child_wav.wait_with_output().expect("Failed to wait for WAV render");
    let wav_stderr = String::from_utf8_lossy(&wav_result.stderr);
    eprintln!("Dexed chain WAV: {}", wav_stderr.trim());

    // Also render our Rust full output chain (Synth + DC blocker) to WAV
    {
        let v2 = DxVoice::from_packed(&DxVoice::FLUNK_BASS_PACKED);
        let mut synth = dx7_core::Synth::new(44100);
        synth.load_patch(v2);
        // Dexed default: no master volume scaling (it's applied per-sample as 1.0)
        synth.set_master_volume(1.0);

        let total = num_blocks * N;
        let mut buf = vec![0.0f32; total];
        synth.note_on(36, velocity as u8);
        synth.render_mono(&mut buf);

        // Apply DC blocker (matches render_wav path)
        let mut dc = dx7_core::effects::DcBlocker::new(44100.0);
        dc.process(&mut buf);

        // Normalize to -1 dB
        let peak = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        if peak > 0.0 {
            let gain = 0.891 / peak;
            for s in buf.iter_mut() { *s *= gain; }
        }
        eprintln!("Rust chain: peak before norm = {:.6}", peak);

        // Write WAV
        write_f32_wav("/tmp/flunk_bass_RUST_chain.wav", &buf, 44100);
        eprintln!("Written: /tmp/flunk_bass_RUST_chain.wav  (Synth + DC blocker, normalized)");
        eprintln!("Written: /tmp/flunk_bass_DEXED_chain.wav (Dexed conversion + DC filter, normalized)");
    }
}

/// Write f32 samples to a 16-bit mono WAV.
fn write_f32_wav(path: &str, samples: &[f32], sample_rate: u32) {
    use std::io::Write as _;
    let num_samples = samples.len() as u32;
    let data_size = num_samples * 2;
    let file_size = 36 + data_size;

    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&file_size.to_le_bytes()).unwrap();
    f.write_all(b"WAVE").unwrap();
    f.write_all(b"fmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&sample_rate.to_le_bytes()).unwrap();
    f.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_size.to_le_bytes()).unwrap();
    for &s in samples {
        let i16_val = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        f.write_all(&i16_val.to_le_bytes()).unwrap();
    }
}

/// Write i32 voice samples to a normalized 16-bit mono WAV.
/// Uses the same scaling for both so they can be A/B compared.
fn write_i32_wav(path: &str, samples: &[i32], sample_rate: u32) {
    use std::io::Write as _;
    // Find peak to normalize
    let peak = samples.iter().map(|s| s.abs() as f64).fold(0.0f64, f64::max);
    let gain = if peak > 0.0 { 0.9 * (i16::MAX as f64) / peak } else { 1.0 };

    let num_samples = samples.len() as u32;
    let data_size = num_samples * 2;
    let file_size = 36 + data_size;

    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&file_size.to_le_bytes()).unwrap();
    f.write_all(b"WAVE").unwrap();
    f.write_all(b"fmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    f.write_all(&1u16.to_le_bytes()).unwrap(); // mono
    f.write_all(&sample_rate.to_le_bytes()).unwrap();
    f.write_all(&(sample_rate * 2).to_le_bytes()).unwrap(); // byte rate
    f.write_all(&2u16.to_le_bytes()).unwrap(); // block align
    f.write_all(&16u16.to_le_bytes()).unwrap(); // bits per sample
    f.write_all(b"data").unwrap();
    f.write_all(&data_size.to_le_bytes()).unwrap();
    for &s in samples {
        let scaled = (s as f64 * gain).clamp(-32768.0, 32767.0) as i16;
        f.write_all(&scaled.to_le_bytes()).unwrap();
    }
}
