//! Real-time audio output via cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};
use dx7_core::SynthCommand;
use ringbuf::traits::{Consumer, Split};
use std::sync::{Arc, Mutex};

/// Synth frames rendered per chunk inside the audio callback. The device
/// buffer is processed in chunks of this size so any buffer length works
/// without allocating.
const CHUNK_FRAMES: usize = 1024;

/// Audio engine that owns the output stream.
/// Commands are sent via a shared ring buffer producer.
pub struct AudioEngine {
    _stream: Stream,
    /// Shared command producer — clone this for MIDI thread
    pub command_tx: Arc<Mutex<ringbuf::HeapProd<SynthCommand>>>,
    pub sample_rate: u32,
}

/// Everything the audio callback owns: the synth, the command queue and a
/// preallocated scratch buffer plus the resampler state needed to upsample
/// the synth's 44.1/48 kHz output to a higher device rate.
struct Renderer {
    synth: dx7_core::Synth,
    command_rx: ringbuf::HeapCons<SynthCommand>,
    /// Device channel count.
    channels: usize,
    /// Integer device_rate / synth_rate (1 = no resampling).
    ratio: usize,
    inv_ratio: f32,
    /// Interleaved stereo scratch: CHUNK_FRAMES synth frames.
    buf: Vec<f32>,
    /// Read cursor / fill level of `buf`, in frames.
    buf_pos: usize,
    buf_len: usize,
    /// Synth frames still to be rendered during the current callback.
    need: usize,
    /// Linear-interpolation state: the two synth frames bracketing the
    /// current output position, and the sub-frame phase in [0, ratio).
    prev: [f32; 2],
    cur: [f32; 2],
    phase: usize,
}

impl Renderer {
    fn new(synth: dx7_core::Synth, command_rx: ringbuf::HeapCons<SynthCommand>, channels: usize, ratio: usize) -> Self {
        let ratio = ratio.max(1);
        Renderer {
            synth,
            command_rx,
            channels,
            ratio,
            inv_ratio: 1.0 / ratio as f32,
            buf: vec![0.0f32; CHUNK_FRAMES * 2],
            buf_pos: 0,
            buf_len: 0,
            need: 0,
            prev: [0.0; 2],
            cur: [0.0; 2],
            phase: 0,
        }
    }

    /// Render one synth chunk into `buf` (at most CHUNK_FRAMES, at most `need`).
    fn refill(&mut self) {
        let n = self.need.min(CHUNK_FRAMES);
        if n > 0 {
            self.synth.render(&mut self.buf[..n * 2]);
            self.need -= n;
        }
        self.buf_pos = 0;
        self.buf_len = n;
    }

    /// Pull the next synth frame, rendering a new chunk when the scratch
    /// buffer is exhausted. Returns silence if nothing is left to render
    /// (cannot happen when `need` was computed correctly).
    #[inline]
    fn pull(&mut self) -> [f32; 2] {
        if self.buf_pos == self.buf_len {
            self.refill();
            if self.buf_len == 0 {
                return [0.0, 0.0];
            }
        }
        let i = self.buf_pos * 2;
        self.buf_pos += 1;
        [self.buf[i], self.buf[i + 1]]
    }

    /// Fill a device buffer, converting each f32 sample with `convert`.
    /// Never allocates; works for any buffer length and channel count.
    fn render<S>(&mut self, data: &mut [S], convert: impl Fn(f32) -> S) {
        while let Some(cmd) = self.command_rx.try_pop() {
            self.synth.process_command(cmd);
        }

        let channels = self.channels;
        let out_frames = data.len() / channels;

        if self.ratio == 1 {
            // No resampling: render synth frames in chunks straight into the
            // device buffer (the synth is dual-mono, so L == R).
            let mut done = 0;
            while done < out_frames {
                let n = (out_frames - done).min(CHUNK_FRAMES);
                self.synth.render(&mut self.buf[..n * 2]);
                for f in 0..n {
                    let l = self.buf[f * 2];
                    let r = self.buf[f * 2 + 1];
                    write_frame(data, (done + f) * channels, channels, l, r, &convert);
                }
                done += n;
            }
            return;
        }

        // Linear-interpolation upsample by an integer ratio. Output frame k
        // sits at phase k/ratio between `prev` and `cur`; whenever the phase
        // wraps we shift and pull the next synth frame. The number of pulls
        // in this callback is exactly (phase + out_frames) / ratio, so we
        // render exactly that many synth frames — no drift across callbacks.
        self.need = (self.phase + out_frames) / self.ratio;
        for k in 0..out_frames {
            let t = self.phase as f32 * self.inv_ratio;
            let l = self.prev[0] + (self.cur[0] - self.prev[0]) * t;
            let r = self.prev[1] + (self.cur[1] - self.prev[1]) * t;
            write_frame(data, k * channels, channels, l, r, &convert);

            self.phase += 1;
            if self.phase == self.ratio {
                self.phase = 0;
                self.prev = self.cur;
                self.cur = self.pull();
            }
        }
        // Any frames left in `buf` (none, by construction) carry over.
        self.need = 0;
    }
}

/// Write one output frame: L/R to the first two channels, silence beyond.
#[inline]
fn write_frame<S>(data: &mut [S], base: usize, channels: usize, l: f32, r: f32, convert: &impl Fn(f32) -> S) {
    data[base] = convert(l);
    if channels >= 2 {
        data[base + 1] = convert(r);
    }
    for ch in 2..channels {
        data[base + ch] = convert(0.0);
    }
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
        let (command_tx, command_rx) = ring.split();

        let command_tx = Arc::new(Mutex::new(command_tx));

        // Create synth on the audio thread side
        let mut synth = dx7_core::Synth::new(synth_rate);
        synth.load_patch(initial_patch);
        if sample_rate != synth_rate {
            eprintln!(
                "Audio device: {}Hz, synth: {}Hz ({}x linear-interpolation upsample)",
                sample_rate,
                synth_rate,
                sample_rate / synth_rate
            );
        }

        let channels = config.channels as usize;
        let upsample_ratio = (sample_rate / synth_rate) as usize;
        let mut renderer = Renderer::new(synth, command_rx, channels, upsample_ratio);

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    renderer.render(data, |s| s);
                },
                |err| eprintln!("Audio stream error: {err}"),
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    renderer.render(data, |s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16);
                },
                |err| eprintln!("Audio stream error: {err}"),
                None,
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::Producer;

    fn make(channels: usize, ratio: usize) -> (Renderer, ringbuf::HeapProd<SynthCommand>) {
        let (tx, rx) = ringbuf::HeapRb::<SynthCommand>::new(16).split();
        let mut synth = dx7_core::Synth::new(48000);
        synth.load_patch(dx7_core::get_rom1a_preset(0));
        (Renderer::new(synth, rx, channels, ratio), tx)
    }

    #[test]
    fn large_buffers_do_not_panic() {
        // Well above the old fixed 8192-frame scratch buffer.
        for &(channels, ratio) in &[(2usize, 1usize), (2, 2), (1, 4), (6, 2)] {
            let (mut r, _tx) = make(channels, ratio);
            let mut data = vec![0.0f32; 20_000 * channels];
            r.render(&mut data, |s| s);
            let mut data16 = vec![0i16; 12_345 * channels];
            r.render(&mut data16, |s| (s * 32767.0) as i16);
        }
    }

    #[test]
    fn odd_buffer_sizes_consume_exactly_out_frames_over_ratio() {
        // With ratio 4 and 1001-frame buffers, 4 callbacks must pull
        // exactly 1001 synth frames (no drift, no buffer carry-over).
        let (mut r, _tx) = make(2, 4);
        let mut data = vec![0.0f32; 1001 * 2];
        for _ in 0..4 {
            r.render(&mut data, |s| s);
            assert_eq!(r.buf_pos, r.buf_len, "no unconsumed synth frames");
        }
        assert_eq!(r.phase, 0);
    }

    #[test]
    fn upsampled_output_is_continuous() {
        // Play a note, upsample 4x; consecutive samples must not jump by
        // more than a plausible per-sample slope (zero-order hold would
        // produce flat runs then jumps; linear should be smooth).
        let (mut r, mut tx) = make(2, 4);
        tx.try_push(SynthCommand::NoteOn { note: 60, velocity: 100 }).unwrap();
        let mut data = vec![0.0f32; 4096 * 2];
        r.render(&mut data, |s| s);
        r.render(&mut data, |s| s);
        let left: Vec<f32> = data.chunks(2).map(|f| f[0]).collect();
        let peak = left.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.01, "expected audible output, peak {peak}");
        // No four-in-a-row identical values followed by a jump: check that
        // every 4-sample group is monotonic (linear ramps between frames).
        let mut non_monotonic = 0;
        let mut i = 0;
        while i + 4 <= left.len() {
            let g = &left[i..i + 4];
            let up = g[0] <= g[1] && g[1] <= g[2] && g[2] <= g[3];
            let down = g[0] >= g[1] && g[1] >= g[2] && g[2] >= g[3];
            if !(up || down) {
                non_monotonic += 1;
            }
            i += 4;
        }
        assert_eq!(non_monotonic, 0);
    }
}
