//! RTP-MIDI (AppleMIDI) session listener.
//!
//! Advertises as "DX7" via mDNS (`_apple-midi._udp`), accepts incoming
//! sessions from macOS Network MIDI, iPads, rtpMIDI on Windows, etc.
//! Parses RTP MIDI packets and feeds commands into the synth engine.

use crate::midi;
use dx7_core::SynthCommand;
use ringbuf::traits::Producer;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Default control port (data port = control + 1).
const DEFAULT_PORT: u16 = 5004;

/// AppleMIDI signature prefix for all session packets.
const SIGNATURE: [u8; 2] = [0xFF, 0xFF];

/// AppleMIDI command codes.
const CMD_IN: [u8; 2] = *b"IN"; // Invitation
const CMD_OK: [u8; 2] = *b"OK"; // Invitation accepted
const _CMD_NO: [u8; 2] = *b"NO"; // Invitation rejected
const CMD_BY: [u8; 2] = *b"BY"; // End session
const CMD_CK: [u8; 2] = *b"CK"; // Clock sync

/// Our SSRC (arbitrary but fixed per session).
const OUR_SSRC: u32 = 0xDEAD_D707;

/// Handle to the running RTP-MIDI listener.
/// Dropping this stops the listener thread and kills the mDNS advertisement.
pub struct RtpMidiHandler {
    _thread: std::thread::JoinHandle<()>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    _mdns_child: Option<std::process::Child>,
}

impl Drop for RtpMidiHandler {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(ref mut child) = self._mdns_child {
            let _ = child.kill();
        }
    }
}

impl RtpMidiHandler {
    /// Start the RTP-MIDI listener on the given port (or 5004 by default).
    pub fn start(
        port: Option<u16>,
        command_tx: Arc<Mutex<ringbuf::HeapProd<SynthCommand>>>,
    ) -> Result<Self, String> {
        let control_port = port.unwrap_or(DEFAULT_PORT);
        let data_port = control_port + 1;
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown2 = shutdown.clone();

        // Bind both UDP sockets before spawning (fail early)
        // Bind to IPv6 dual-stack (accepts both IPv4 and IPv6 on Linux)
        let ctrl_sock = UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, control_port))
            .or_else(|_| UdpSocket::bind(("0.0.0.0", control_port)))
            .map_err(|e| format!("Failed to bind control port {control_port}: {e}"))?;
        let data_sock = UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, data_port))
            .or_else(|_| UdpSocket::bind(("0.0.0.0", data_port)))
            .map_err(|e| format!("Failed to bind data port {data_port}: {e}"))?;

        // Non-blocking: we poll both sockets in a tight loop
        ctrl_sock.set_nonblocking(true).ok();
        data_sock.set_nonblocking(true).ok();

        // Register mDNS service via OS tool (avahi-publish on Linux, dns-sd on macOS).
        // This avoids conflicts with the system mDNS daemon.
        let mdns_child = register_mdns("DX7", control_port);
        if mdns_child.is_none() {
            eprintln!("RTP-MIDI: mDNS registration failed (install avahi-utils on Linux)");
        }

        let handle = std::thread::Builder::new()
            .name("rtp-midi".into())
            .spawn(move || {
                run_listener(ctrl_sock, data_sock, command_tx, &shutdown2);
            })
            .map_err(|e| format!("Failed to spawn RTP-MIDI thread: {e}"))?;

        Ok(RtpMidiHandler {
            _thread: handle,
            shutdown,
            _mdns_child: mdns_child,
        })
    }
}

/// Register the RTP-MIDI service via the OS mDNS daemon.
/// Returns a child process handle that must be kept alive.
fn register_mdns(name: &str, port: u16) -> Option<std::process::Child> {
    use std::process::{Command, Stdio};
    let port_str = port.to_string();

    // Linux: avahi-publish-service
    #[cfg(target_os = "linux")]
    {
        Command::new("avahi-publish-service")
            .args([name, "_apple-midi._udp", &port_str])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }

    // macOS: dns-sd -R
    #[cfg(target_os = "macos")]
    {
        Command::new("dns-sd")
            .args(["-R", name, "_apple-midi._udp", "local", &port_str])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (name, port_str);
        None
    }
}

/// Main listener loop: handles both control and data sockets.
fn run_listener(
    ctrl_sock: UdpSocket,
    data_sock: UdpSocket,
    command_tx: Arc<Mutex<ringbuf::HeapProd<SynthCommand>>>,
    shutdown: &std::sync::atomic::AtomicBool,
) {
    let mut ctrl_buf = [0u8; 512];
    let mut data_buf = [0u8; 512];
    let epoch = Instant::now();

    while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
        let mut got_data = false;

        // Poll control socket (invitations, sync, bye)
        if let Ok((n, addr)) = ctrl_sock.recv_from(&mut ctrl_buf) {
            got_data = true;
            if n >= 4 && ctrl_buf[..2] == SIGNATURE {
                handle_session_packet(&ctrl_buf[2..n], &ctrl_sock, addr, epoch);
            }
        }

        // Poll data socket (invitations on data port + RTP MIDI)
        if let Ok((n, addr)) = data_sock.recv_from(&mut data_buf) {
            got_data = true;
            if n >= 4 && data_buf[..2] == SIGNATURE {
                // Session packet on data port (invitation step 2, sync, etc.)
                handle_session_packet(&data_buf[2..n], &data_sock, addr, epoch);
            } else if n >= 12 && (data_buf[0] >> 6) == 2 {
                // RTP packet (version 2)
                if let Some(commands) = parse_rtp_midi(&data_buf[..n]) {
                    if let Ok(mut tx) = command_tx.lock() {
                        for cmd in commands {
                            let _ = tx.try_push(cmd);
                        }
                    }
                }
            }
        }

        // Sleep only when idle to avoid burning CPU
        if !got_data {
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
    }
}

/// Handle an AppleMIDI session management packet (after 0xFFFF prefix stripped).
fn handle_session_packet(
    data: &[u8],
    sock: &UdpSocket,
    addr: std::net::SocketAddr,
    epoch: Instant,
) {
    if data.len() < 2 {
        return;
    }

    let cmd = [data[0], data[1]];


    match cmd {
        CMD_IN => handle_invitation(data, sock, addr),
        CMD_CK => handle_sync(data, sock, addr, epoch),
        CMD_BY => {
            eprintln!("RTP-MIDI: session ended by remote");
        }
        _ => {}
    }
}

/// Accept an invitation: reply with OK using the same initiator token.
fn handle_invitation(data: &[u8], sock: &UdpSocket, addr: std::net::SocketAddr) {
    // data: [IN(2)] [version(4)] [token(4)] [ssrc(4)] [name...]
    if data.len() < 14 {
        return;
    }

    let token = &data[6..10];
    let remote_name = if data.len() > 14 {
        std::str::from_utf8(&data[14..])
            .unwrap_or("?")
            .trim_end_matches('\0')
    } else {
        "?"
    };

    eprintln!("RTP-MIDI: invitation from '{remote_name}' at {addr}");

    // Build OK response
    let name = b"DX7\0";
    let mut resp = Vec::with_capacity(16 + name.len());
    resp.extend_from_slice(&SIGNATURE);
    resp.extend_from_slice(&CMD_OK);
    resp.extend_from_slice(&[0, 0, 0, 2]); // protocol version 2
    resp.extend_from_slice(token); // echo back initiator token
    resp.extend_from_slice(&OUR_SSRC.to_be_bytes());
    resp.extend_from_slice(name);

    let _ = sock.send_to(&resp, addr);
}

/// Respond to a clock sync packet (3-way timestamp exchange).
fn handle_sync(
    data: &[u8],
    sock: &UdpSocket,
    addr: std::net::SocketAddr,
    epoch: Instant,
) {
    // data: [CK(2)] [ssrc(4)] [count(1)] [padding(3)] [ts1(8)] [ts2(8)] [ts3(8)]
    // Total: 34 bytes (ts3 may be zero-filled for count=0)
    if data.len() < 34 {
        return;
    }

    let count = data[6];
    let now = epoch.elapsed().as_micros() as u64 / 100; // 100µs units

    match count {
        0 => {
            // Peer sent ts1, we reply with ts1 + our ts2
            let mut resp = Vec::with_capacity(40);
            resp.extend_from_slice(&SIGNATURE);
            resp.extend_from_slice(&CMD_CK);
            resp.extend_from_slice(&OUR_SSRC.to_be_bytes());
            resp.push(1); // count = 1
            resp.extend_from_slice(&[0, 0, 0]); // padding
            resp.extend_from_slice(&data[10..18]); // ts1 (echo back)
            resp.extend_from_slice(&now.to_be_bytes()); // ts2 (our time)
            resp.extend_from_slice(&[0u8; 8]); // ts3 (unused)
            let _ = sock.send_to(&resp, addr);
        }
        2 => {
            // Final step — peer completed the exchange, nothing to do
        }
        _ => {}
    }
}

/// Parse an RTP MIDI packet and extract MIDI commands.
fn parse_rtp_midi(packet: &[u8]) -> Option<Vec<SynthCommand>> {
    // RTP header: 12 bytes minimum
    if packet.len() < 13 {
        return None;
    }

    // Skip RTP header (12 bytes) to reach MIDI command section
    let payload = &packet[12..];

    // MIDI command section header
    let b_flag = payload[0] & 0x80 != 0;
    let _j_flag = payload[0] & 0x40 != 0;
    let _z_flag = payload[0] & 0x20 != 0;
    let _p_flag = payload[0] & 0x10 != 0;

    let (midi_len, midi_start) = if b_flag {
        // Long header: 12-bit length
        if payload.len() < 2 {
            return None;
        }
        let len = ((payload[0] as usize & 0x0F) << 8) | payload[1] as usize;
        (len, 2)
    } else {
        // Short header: 4-bit length
        let len = (payload[0] as usize) & 0x0F;
        (len, 1)
    };

    if midi_len == 0 || midi_start + midi_len > payload.len() {
        return None;
    }

    let midi_data = &payload[midi_start..midi_start + midi_len];
    let mut commands = Vec::new();
    let mut pos = 0;
    let mut running_status: u8 = 0;

    while pos < midi_data.len() {
        let b = midi_data[pos];

        // Skip delta time: variable-length quantity where continuation bytes
        // have bit 7 set but are NOT valid MIDI status bytes (0xF0+).
        // The final byte of the delta has bit 7 clear.
        if b & 0x80 != 0 && !is_midi_status(b) {
            // System range byte used as delta continuation (0xF0-0xFF)
            pos += 1;
            continue;
        }
        if b & 0x80 == 0 && !is_midi_status(b) && running_status == 0 {
            // Low byte before any status — must be delta time final byte
            pos += 1;
            continue;
        }

        // Status byte
        if is_midi_status(b) {
            running_status = b;
            pos += 1;
            if pos >= midi_data.len() {
                break;
            }
        }

        if running_status == 0 {
            pos += 1;
            continue;
        }

        // Data bytes for the current status
        let data_len = midi_data_length(running_status);
        if pos + data_len > midi_data.len() {
            break;
        }

        let mut msg = vec![running_status];
        for i in 0..data_len {
            msg.push(midi_data[pos + i]);
        }
        pos += data_len;

        if let Some(cmd) = midi::parse_midi_message(&msg) {
            commands.push(cmd);
        }
    }

    if commands.is_empty() {
        None
    } else {
        Some(commands)
    }
}

fn is_midi_status(byte: u8) -> bool {
    byte >= 0x80 && byte <= 0xEF
}

fn midi_data_length(status: u8) -> usize {
    match status & 0xF0 {
        0x80 => 2,
        0x90 => 2,
        0xA0 => 2,
        0xB0 => 2,
        0xC0 => 1,
        0xD0 => 1,
        0xE0 => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rtp_midi_note_on() {
        // RTP header (12 bytes) + short MIDI command section
        let mut packet = vec![
            0x80, 0x61, 0x00, 0x01, // V=2, PT=0x61, seq=1
            0x00, 0x00, 0x00, 0x00, // timestamp
            0x00, 0x00, 0x00, 0x01, // SSRC
        ];
        // MIDI command section: short header (B=0, J=0, Z=0, P=0, LEN=3)
        // + Note On C4 vel=100
        packet.extend_from_slice(&[0x03, 0x90, 60, 100]);

        let cmds = parse_rtp_midi(&packet).unwrap();
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::NoteOn { note, velocity } => {
                assert_eq!(*note, 60);
                assert_eq!(*velocity, 100);
            }
            other => panic!("Expected NoteOn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_rtp_midi_note_off() {
        let mut packet = vec![
            0x80, 0x61, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01,
        ];
        packet.extend_from_slice(&[0x03, 0x80, 60, 0]);

        let cmds = parse_rtp_midi(&packet).unwrap();
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::NoteOff { note } => assert_eq!(*note, 60),
            other => panic!("Expected NoteOff, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_rtp_midi_empty_payload() {
        let packet = vec![
            0x80, 0x61, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01,
            0x00, // LEN=0
        ];
        assert!(parse_rtp_midi(&packet).is_none());
    }

    #[test]
    fn test_parse_rtp_midi_pitch_bend() {
        let mut packet = vec![
            0x80, 0x61, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01,
        ];
        // Pitch bend center: 0xE0 0x00 0x40 = value 8192 - 8192 = 0
        packet.extend_from_slice(&[0x03, 0xE0, 0x00, 0x40]);

        let cmds = parse_rtp_midi(&packet).unwrap();
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::PitchBend { value } => assert_eq!(*value, 0),
            other => panic!("Expected PitchBend, got {:?}", other),
        }
    }
}
