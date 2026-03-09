//! Real-time audio output via cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};
use dx7_core::SynthCommand;
use ringbuf::traits::{Consumer, Producer, Split};
use std::sync::{Arc, Mutex};

/// Audio engine that owns the output stream.
/// Commands are sent via a shared ring buffer producer.
pub struct AudioEngine {
    _stream: Stream,
    /// Shared command producer — clone this for MIDI thread
    pub command_tx: Arc<Mutex<ringbuf::HeapProd<SynthCommand>>>,
    pub sample_rate: u32,
}

impl AudioEngine {
    /// Create and start the audio output.
    pub fn start(initial_patch: dx7_core::DxVoice) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No audio output device found")?;

        let (config, sample_format) = find_config(&device)?;
        let sample_rate = config.sample_rate.0;

        // Synth runs at 48000 or 44100; upsample if device needs higher rate
        let synth_rate = if sample_rate % 48000 == 0 {
            48000
        } else if sample_rate % 44100 == 0 {
            44100
        } else {
            sample_rate // fallback, may not work perfectly
        };

        // Create command ring buffer (512 commands for headroom)
        let ring = ringbuf::HeapRb::<SynthCommand>::new(512);
        let (command_tx, mut command_rx) = ring.split();

        let command_tx = Arc::new(Mutex::new(command_tx));

        // Create synth on the audio thread side
        let mut synth = dx7_core::Synth::new(synth_rate);
        synth.load_patch(initial_patch);
        if sample_rate != synth_rate {
            eprintln!("Audio device: {}Hz, synth: {}Hz ({}x upsample)", sample_rate, synth_rate, sample_rate / synth_rate);
        }

        let channels = config.channels as usize;
        let upsample_ratio = sample_rate / synth_rate;

        let render_f32 = move |data: &mut [f32], command_rx: &mut ringbuf::HeapCons<SynthCommand>, synth: &mut dx7_core::Synth| {
            while let Some(cmd) = command_rx.try_pop() {
                synth.process_command(cmd);
            }
            let out_frames = data.len() / channels;
            // Round up so we render enough synth frames to cover all output frames
            let synth_frames = (out_frames + upsample_ratio as usize - 1) / upsample_ratio as usize;
            if channels == 2 && upsample_ratio == 1 {
                synth.render(data);
            } else {
                let mut stereo_buf = vec![0.0f32; synth_frames * 2];
                synth.render(&mut stereo_buf);
                for i in 0..out_frames {
                    let src = (i / upsample_ratio as usize).min(synth_frames - 1);
                    let l = stereo_buf[src * 2];
                    let r = stereo_buf[src * 2 + 1];
                    if channels >= 2 {
                        data[i * channels] = l;
                        data[i * channels + 1] = r;
                    } else {
                        data[i * channels] = l;
                    }
                    for ch in 2..channels {
                        data[i * channels + ch] = 0.0;
                    }
                }
            }
        };

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    render_f32(data, &mut command_rx, &mut synth);
                },
                |err| eprintln!("Audio stream error: {err}"),
                None,
            )},
            cpal::SampleFormat::I16 => {
                // Pre-allocate buffer for synth frames
                let mut float_buf = vec![0.0f32; 8192 * 2];
                device.build_output_stream(
                &config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let out_frames = data.len() / channels;
                    let synth_frames = (out_frames + upsample_ratio as usize - 1) / upsample_ratio as usize;
                    while let Some(cmd) = command_rx.try_pop() {
                        synth.process_command(cmd);
                    }
                    let buf = &mut float_buf[..synth_frames * 2];
                    buf.fill(0.0);
                    synth.render(buf);
                    for i in 0..out_frames {
                        let src = (i / upsample_ratio as usize).min(synth_frames - 1);
                        for ch in 0..channels {
                            let s = buf[src * 2 + ch.min(1)];
                            data[i * channels + ch] = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {err}"),
                None,
            )},
            _ => return Err(format!("Unsupported sample format: {:?}", sample_format)),
        }
        .map_err(|e| format!("Failed to build output stream: {e}"))?;

        stream.play().map_err(|e| format!("Failed to play stream: {e}"))?;

        Ok(AudioEngine {
            _stream: stream,
            command_tx,
            sample_rate: synth_rate,
        })
    }

    /// Send a command to the synth on the audio thread.
    pub fn send_command(&self, cmd: SynthCommand) {
        if let Ok(mut tx) = self.command_tx.lock() {
            let _ = tx.try_push(cmd);
        }
    }

    /// Get a clone of the command producer for another thread (e.g., MIDI).
    pub fn command_sender(&self) -> Arc<Mutex<ringbuf::HeapProd<SynthCommand>>> {
        Arc::clone(&self.command_tx)
    }
}

/// Find a suitable output configuration (prefer 44100 or 48000 Hz stereo).
fn find_config(device: &Device) -> Result<(StreamConfig, cpal::SampleFormat), String> {
    let supported = device
        .supported_output_configs()
        .map_err(|e| format!("Failed to query audio configs: {e}"))?;

    let mut best: Option<cpal::SupportedStreamConfigRange> = None;

    // Prefer F32, then I16, then any format
    let preferred = [cpal::SampleFormat::F32, cpal::SampleFormat::I16];
    for &fmt in &preferred {
        for config in device
            .supported_output_configs()
            .map_err(|e| format!("Failed to query audio configs: {e}"))?
        {
            if config.sample_format() == fmt && (best.is_none() || config.channels() == 2) {
                best = Some(config);
            }
        }
        if best.is_some() {
            break;
        }
    }
    // Fallback: any format
    if best.is_none() {
        for config in supported {
            if best.is_none() || config.channels() == 2 {
                best = Some(config);
            }
        }
    }

    let range = best.ok_or("No suitable audio output format found")?;
    let sample_format = range.sample_format();

    // Prefer native 48000/44100; accept integer multiples (96000, 88200, etc.)
    let min = range.min_sample_rate().0;
    let max = range.max_sample_rate().0;
    let preferred = [48000, 44100, 96000, 88200, 192000, 176400];
    let sample_rate = preferred.iter()
        .find(|&&r| r >= min && r <= max)
        .map(|&r| SampleRate(r))
        .ok_or_else(|| format!(
            "No supported sample rate (need 44100/48000 or multiple, device supports {}-{})",
            min, max
        ))?;

    Ok((range.with_sample_rate(sample_rate).config(), sample_format))
}
