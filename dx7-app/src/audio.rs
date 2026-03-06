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

        let config = find_config(&device)?;
        let sample_rate = config.sample_rate.0;

        // Create command ring buffer (512 commands for headroom)
        let ring = ringbuf::HeapRb::<SynthCommand>::new(512);
        let (command_tx, mut command_rx) = ring.split();

        let command_tx = Arc::new(Mutex::new(command_tx));

        // Create synth on the audio thread side
        let mut synth = dx7_core::Synth::new(sample_rate);
        synth.load_patch(initial_patch);

        let channels = config.channels as usize;

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // Drain command queue
                    while let Some(cmd) = command_rx.try_pop() {
                        synth.process_command(cmd);
                    }

                    if channels == 2 {
                        synth.render(data);
                    } else {
                        let frames = data.len() / channels;
                        let mut stereo_buf = vec![0.0f32; frames * 2];
                        synth.render(&mut stereo_buf);
                        for i in 0..frames {
                            let sample = stereo_buf[i * 2];
                            for ch in 0..channels {
                                data[i * channels + ch] = sample;
                            }
                        }
                    }
                },
                |err| {
                    eprintln!("Audio stream error: {err}");
                },
                None,
            )
            .map_err(|e| format!("Failed to build output stream: {e}"))?;

        stream.play().map_err(|e| format!("Failed to play stream: {e}"))?;

        Ok(AudioEngine {
            _stream: stream,
            command_tx,
            sample_rate,
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
fn find_config(device: &Device) -> Result<StreamConfig, String> {
    let supported = device
        .supported_output_configs()
        .map_err(|e| format!("Failed to query audio configs: {e}"))?;

    let mut best: Option<cpal::SupportedStreamConfigRange> = None;

    for config in supported {
        if config.sample_format() == cpal::SampleFormat::F32 {
            if best.is_none() || config.channels() == 2 {
                best = Some(config);
            }
        }
    }

    let range = best.ok_or("No suitable audio output format found")?;

    // Prefer 44100, then 48000
    let sample_rate = if range.min_sample_rate().0 <= 44100 && range.max_sample_rate().0 >= 44100 {
        SampleRate(44100)
    } else if range.min_sample_rate().0 <= 48000 && range.max_sample_rate().0 >= 48000 {
        SampleRate(48000)
    } else {
        range.min_sample_rate()
    };

    Ok(range.with_sample_rate(sample_rate).config())
}
