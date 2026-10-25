//! MIDI input via midir, plus the shared MIDI → synth dispatch path used by
//! every live input (USB/midir, BLE, RTP-MIDI and the computer keyboard).

use dx7_core::{DxVoice, SynthCommand};
use midir::{MidiInput, MidiInputConnection};
use ringbuf::traits::Producer;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// A channel-voice MIDI message decoded from a live input.
///
/// The channel is preserved so callers can route or filter on it even
/// though the interactive synth currently drives a single instrument.
#[derive(Debug, Clone)]
pub struct MidiMessage {
    /// MIDI channel, 0-15.
    pub channel: u8,
    pub kind: MidiMessageKind,
}

/// Payload of a [`MidiMessage`].
#[derive(Debug, Clone)]
pub enum MidiMessageKind {
    /// Maps directly onto a synth-engine command.
    Synth(SynthCommand),
    /// Program Change (0xCn): select a patch from the loaded bank.
    ProgramChange(u8),
}

/// Shared sink that turns [`MidiMessage`]s into [`SynthCommand`]s and pushes
/// them to the audio thread.
///
/// Program Change is resolved here against the loaded patch bank — the same
/// path the computer keyboard's 1-0 keys use — so hardware controllers can
/// switch patches live. All channels currently drive the one synth.
pub struct CommandSink {
    tx: Arc<Mutex<ringbuf::HeapProd<SynthCommand>>>,
    patches: Vec<DxVoice>,
}

impl CommandSink {
    pub fn new(tx: Arc<Mutex<ringbuf::HeapProd<SynthCommand>>>, patches: Vec<DxVoice>) -> Self {
        Self { tx, patches }
    }

    /// Push a raw synth command to the audio thread.
    pub fn send(&self, cmd: SynthCommand) {
        if let Ok(mut tx) = self.tx.lock() {
            let _ = tx.try_push(cmd);
        }
    }

    /// Load patch `idx` from the bank. Returns its name, or `None` if the
    /// index is out of range (nothing is sent in that case).
    pub fn select_patch(&self, idx: usize) -> Option<&str> {
        let patch = self.patches.get(idx)?;
        self.send(SynthCommand::LoadPatch(Box::new(patch.clone())));
        Some(patch.name_str())
    }

    /// Dispatch one decoded MIDI message.
    pub fn dispatch(&self, msg: MidiMessage) {
        match msg.kind {
            MidiMessageKind::Synth(cmd) => self.send(cmd),
            MidiMessageKind::ProgramChange(program) => {
                // Interactive mode runs in raw mode: \r, not \n.
                match self.select_patch(program as usize) {
                    Some(name) => print!(
                        "\r\x1b[K  Patch {}: {} (program change, ch{})\r",
                        program,
                        name,
                        msg.channel + 1
                    ),
                    None => print!(
                        "\r\x1b[K  Program {} ignored: bank has {} patches\r",
                        program,
                        self.patches.len()
                    ),
                }
                let _ = std::io::stdout().flush();
            }
        }
    }

    /// Dispatch a batch of messages (e.g. one BLE or RTP packet).
    #[cfg_attr(not(any(feature = "bluetooth", feature = "rtp-midi")), allow(dead_code))]
    pub fn dispatch_all<I: IntoIterator<Item = MidiMessage>>(&self, msgs: I) {
        for msg in msgs {
            self.dispatch(msg);
        }
    }
}

/// MIDI input handler.
pub struct MidiHandler {
    _connection: MidiInputConnection<()>,
    pub port_name: String,
}

impl MidiHandler {
    /// Try to connect to a MIDI input port, forwarding messages to `sink`.
    pub fn connect(port_name: Option<&str>, sink: Arc<CommandSink>) -> Result<Self, String> {
        let midi_in = MidiInput::new("dx7-app")
            .map_err(|e| format!("Failed to create MIDI input: {e}"))?;

        let ports = midi_in.ports();
        if ports.is_empty() {
            return Err("No MIDI input ports available".into());
        }

        // Find the requested port or use the first one
        let (port_idx, name) = if let Some(requested) = port_name {
            let found = ports.iter().enumerate().find(|(_, p)| {
                midi_in
                    .port_name(p)
                    .map(|n| n.contains(requested))
                    .unwrap_or(false)
            });
            match found {
                Some((idx, p)) => (idx, midi_in.port_name(p).unwrap_or_default()),
                None => return Err(format!("MIDI port '{}' not found", requested)),
            }
        } else {
            (0, midi_in.port_name(&ports[0]).unwrap_or_default())
        };

        let port = &ports[port_idx];
        let port_name_str = name.clone();

        let connection = midi_in
            .connect(
                port,
                "dx7-input",
                move |_stamp, message, _| {
                    if let Some(msg) = parse_midi_message(message) {
                        sink.dispatch(msg);
                    }
                },
                (),
            )
            .map_err(|e| format!("Failed to connect to MIDI port: {e}"))?;

        Ok(MidiHandler {
            _connection: connection,
            port_name: port_name_str,
        })
    }

    /// List available MIDI input ports.
    pub fn list_ports() -> Vec<String> {
        let midi_in = match MidiInput::new("dx7-list") {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };

        midi_in
            .ports()
            .iter()
            .filter_map(|p| midi_in.port_name(p).ok())
            .collect()
    }
}

/// Parse a complete channel-voice MIDI message (status byte + data bytes).
///
/// Returns `None` for system messages, unsupported voice messages
/// (aftertouch) and truncated input.
pub fn parse_midi_message(msg: &[u8]) -> Option<MidiMessage> {
    if msg.is_empty() {
        return None;
    }

    let status = msg[0] & 0xF0;
    let channel = msg[0] & 0x0F;
    let kind = match status {
        0x90 if msg.len() >= 3 => {
            if msg[2] == 0 {
                MidiMessageKind::Synth(SynthCommand::NoteOff { note: msg[1] })
            } else {
                MidiMessageKind::Synth(SynthCommand::NoteOn {
                    note: msg[1],
                    velocity: msg[2],
                })
            }
        }
        0x80 if msg.len() >= 3 => MidiMessageKind::Synth(SynthCommand::NoteOff { note: msg[1] }),
        0xB0 if msg.len() >= 3 => MidiMessageKind::Synth(SynthCommand::ControlChange {
            cc: msg[1],
            value: msg[2],
        }),
        0xC0 if msg.len() >= 2 => MidiMessageKind::ProgramChange(msg[1]),
        0xE0 if msg.len() >= 3 => {
            let bend = (((msg[2] as i16) << 7) | msg[1] as i16) - 8192;
            MidiMessageKind::Synth(SynthCommand::PitchBend { value: bend })
        }
        _ => return None,
    };
    Some(MidiMessage { channel, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_change_keeps_channel() {
        let msg = parse_midi_message(&[0xC3, 17]).expect("program change");
        assert_eq!(msg.channel, 3);
        assert!(matches!(msg.kind, MidiMessageKind::ProgramChange(17)));
    }

    #[test]
    fn note_on_keeps_channel() {
        let msg = parse_midi_message(&[0x9F, 60, 100]).expect("note on");
        assert_eq!(msg.channel, 15);
        assert!(matches!(
            msg.kind,
            MidiMessageKind::Synth(SynthCommand::NoteOn { note: 60, velocity: 100 })
        ));
    }

    #[test]
    fn truncated_program_change_is_ignored() {
        assert!(parse_midi_message(&[0xC0]).is_none());
    }

    #[test]
    fn system_messages_are_ignored() {
        assert!(parse_midi_message(&[0xF8]).is_none());
        assert!(parse_midi_message(&[0xF0, 0x43, 0xF7]).is_none());
    }
}
