//! RTP-MIDI (AppleMIDI) session listener.
//!
//! Advertises as "DX7" via mDNS (`_apple-midi._udp`), accepts incoming
//! sessions from macOS Network MIDI, iPads, rtpMIDI on Windows, etc.
//! Parses RTP MIDI packets (RFC 6295) and feeds commands into the synth engine.

use crate::midi::{self, CommandSink, MidiMessage};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Instant;

/// Default control port (data port = control + 1).
const DEFAULT_PORT: u16 = 5004;

/// AppleMIDI signature prefix for all session packets.
const SIGNATURE: [u8; 2] = [0xFF, 0xFF];

/// AppleMIDI command codes.
const CMD_IN: [u8; 2] = *b"IN"; // Invitation
const CMD_OK: [u8; 2] = *b"OK"; // Invitation accepted
const CMD_NO: [u8; 2] = *b"NO"; // Invitation rejected
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
    ///
    /// `allow` restricts which peers may open a session; when empty, any
    /// host may connect (classic AppleMIDI behaviour).
    pub fn start(port: Option<u16>, allow: Vec<IpAddr>, sink: Arc<CommandSink>) -> Result<Self, String> {
        let control_port = port.unwrap_or(DEFAULT_PORT);
        let data_port = control_port + 1;
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown2 = shutdown.clone();

        // Bind both UDP sockets before spawning (fail early)
        let ctrl_socks = bind_all(control_port).map_err(|e| format!("Failed to bind control port {control_port}: {e}"))?;
        let data_socks = bind_all(data_port).map_err(|e| format!("Failed to bind data port {data_port}: {e}"))?;

        // Register mDNS service via OS tool (avahi-publish on Linux, dns-sd on macOS).
        // This avoids conflicts with the system mDNS daemon.
        let mdns_child = register_mdns("DX7", control_port);
        if mdns_child.is_none() {
            eprintln!("RTP-MIDI: mDNS registration failed (install avahi-utils on Linux)");
        }

        if !allow.is_empty() {
            let list: Vec<String> = allow.iter().map(|a| a.to_string()).collect();
            println!("RTP-MIDI: accepting sessions only from {}", list.join(", "));
        } else {
            eprintln!("RTP-MIDI: warning: any host on the network may connect (use --rtp-midi-allow to restrict)");
        }

        let handle = std::thread::Builder::new()
            .name("rtp-midi".into())
            .spawn(move || {
                let mut listener = Listener::new(allow, sink);
                listener.run(&ctrl_socks, &data_socks, &shutdown2);
            })
            .map_err(|e| format!("Failed to spawn RTP-MIDI thread: {e}"))?;

        Ok(RtpMidiHandler {
            _thread: handle,
            shutdown,
            _mdns_child: mdns_child,
        })
    }
}

/// Bind `port` on every address family we can.
///
/// On Linux/macOS the IPv6 wildcard socket is dual-stack, so the extra IPv4
/// bind fails with "address in use" and is skipped. On Windows `[::]` is
/// v6-only, so the IPv4 socket is what actually receives IPv4 peers.
/// Returns an error only if no socket could be bound at all.
fn bind_all(port: u16) -> std::io::Result<Vec<UdpSocket>> {
    let mut socks = Vec::new();
    let mut last_err = None;
    for addr in [
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
    ] {
        match UdpSocket::bind(addr) {
            Ok(s) => {
                // Non-blocking: we poll all sockets in one loop
                s.set_nonblocking(true)?;
                socks.push(s);
            }
            Err(e) => last_err = Some(e),
        }
    }
    if socks.is_empty() {
        Err(last_err.unwrap_or_else(|| std::io::Error::other("no address family available")))
    } else {
        Ok(socks)
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

/// Per-peer RTP state, keyed by the peer's address.
#[derive(Debug, Default)]
struct PeerState {
    /// Highest RTP sequence number accepted so far.
    last_seq: Option<u16>,
    /// MIDI running status carried across packets (used when P=1).
    running_status: u8,
    /// Set once we've logged that this peer sends recovery journals.
    journal_logged: bool,
    /// Duplicate / stale packets dropped (for a rate-limited log).
    dropped: u32,
}

/// Why a packet's sequence number was rejected.
#[derive(Debug, PartialEq)]
enum SeqVerdict {
    /// In order (possibly with `lost` packets missing before it).
    Accept { lost: u16 },
    /// Same sequence number as one already processed.
    Duplicate,
    /// Older than the last accepted packet (reordered / replayed).
    Stale,
}

impl PeerState {
    /// Validate and record an RTP sequence number (RFC 3550 §A.1 style,
    /// with 16-bit wraparound).
    fn check_sequence(&mut self, seq: u16) -> SeqVerdict {
        let verdict = match self.last_seq {
            None => SeqVerdict::Accept { lost: 0 },
            Some(last) => {
                let delta = seq.wrapping_sub(last) as i16;
                if delta == 0 {
                    SeqVerdict::Duplicate
                } else if delta < 0 {
                    SeqVerdict::Stale
                } else {
                    SeqVerdict::Accept { lost: (delta - 1) as u16 }
                }
            }
        };
        if matches!(verdict, SeqVerdict::Accept { .. }) {
            self.last_seq = Some(seq);
        }
        verdict
    }
}

/// Listener state shared across the control and data sockets.
struct Listener {
    allow: Vec<IpAddr>,
    sink: Arc<CommandSink>,
    /// Peers with an accepted invitation. Only these may send RTP data.
    sessions: HashMap<IpAddr, PeerState>,
    /// Addresses we've already warned about, to avoid log spam.
    warned: std::collections::HashSet<IpAddr>,
    epoch: Instant,
}

impl Listener {
    fn new(allow: Vec<IpAddr>, sink: Arc<CommandSink>) -> Self {
        Listener {
            allow: allow.into_iter().map(|a| a.to_canonical()).collect(),
            sink,
            sessions: HashMap::new(),
            warned: std::collections::HashSet::new(),
            epoch: Instant::now(),
        }
    }

    fn is_allowed(&self, ip: IpAddr) -> bool {
        self.allow.is_empty() || self.allow.contains(&ip)
    }

    /// Main loop: poll every control and data socket until shutdown.
    fn run(&mut self, ctrl_socks: &[UdpSocket], data_socks: &[UdpSocket], shutdown: &std::sync::atomic::AtomicBool) {
        let mut buf = [0u8; 1500];

        while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            let mut got_data = false;

            // Control sockets: invitations, sync, bye
            for sock in ctrl_socks {
                if let Ok((n, addr)) = sock.recv_from(&mut buf) {
                    got_data = true;
                    if n >= 4 && buf[..2] == SIGNATURE {
                        self.handle_session_packet(&buf[2..n], sock, addr);
                    }
                }
            }

            // Data sockets: invitation step 2 / sync, plus RTP MIDI
            for sock in data_socks {
                if let Ok((n, addr)) = sock.recv_from(&mut buf) {
                    got_data = true;
                    if n >= 4 && buf[..2] == SIGNATURE {
                        self.handle_session_packet(&buf[2..n], sock, addr);
                    } else if n >= 12 && (buf[0] >> 6) == 2 {
                        self.handle_rtp_packet(&buf[..n], addr);
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
    fn handle_session_packet(&mut self, data: &[u8], sock: &UdpSocket, addr: SocketAddr) {
        if data.len() < 2 {
            return;
        }
        let ip = addr.ip().to_canonical();

        match [data[0], data[1]] {
            CMD_IN => self.handle_invitation(data, sock, addr, ip),
            CMD_CK => {
                if self.sessions.contains_key(&ip) {
                    handle_sync(data, sock, addr, self.epoch);
                }
            }
            CMD_BY if self.sessions.remove(&ip).is_some() => {
                eprintln!("RTP-MIDI: session ended by {addr}");
            }
            _ => {}
        }
    }

    /// Accept (or reject) an invitation, echoing the initiator token.
    fn handle_invitation(&mut self, data: &[u8], sock: &UdpSocket, addr: SocketAddr, ip: IpAddr) {
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

        let accept = self.is_allowed(ip);
        if accept {
            eprintln!("RTP-MIDI: invitation from '{remote_name}' at {addr}");
            self.sessions.entry(ip).or_default();
        } else if self.warned.insert(ip) {
            eprintln!("RTP-MIDI: rejected invitation from '{remote_name}' at {addr} (not in allow list)");
        }

        let name = b"DX7\0";
        let mut resp = Vec::with_capacity(16 + name.len());
        resp.extend_from_slice(&SIGNATURE);
        resp.extend_from_slice(if accept { &CMD_OK } else { &CMD_NO });
        resp.extend_from_slice(&[0, 0, 0, 2]); // protocol version 2
        resp.extend_from_slice(token); // echo back initiator token
        resp.extend_from_slice(&OUR_SSRC.to_be_bytes());
        resp.extend_from_slice(name);

        let _ = sock.send_to(&resp, addr);
    }

    /// Validate an RTP packet's origin and sequence, then dispatch its MIDI.
    fn handle_rtp_packet(&mut self, packet: &[u8], addr: SocketAddr) {
        let ip = addr.ip().to_canonical();
        let Some(peer) = self.sessions.get_mut(&ip) else {
            if self.warned.insert(ip) {
                eprintln!("RTP-MIDI: ignoring RTP data from {addr} without a session");
            }
            return;
        };

        let Some(header) = parse_rtp_header(packet) else { return };

        match peer.check_sequence(header.seq) {
            SeqVerdict::Accept { lost } if lost > 0 => {
                eprintln!("RTP-MIDI: {lost} packet(s) lost before seq {}", header.seq);
            }
            SeqVerdict::Accept { .. } => {}
            SeqVerdict::Duplicate | SeqVerdict::Stale => {
                peer.dropped += 1;
                if peer.dropped.is_power_of_two() {
                    eprintln!(
                        "RTP-MIDI: dropped duplicate/out-of-order packet seq {} ({} so far)",
                        header.seq, peer.dropped
                    );
                }
                return;
            }
        }

        if let Some(section) = parse_command_section(&packet[header.payload_offset..], peer) {
            if section.has_journal && !peer.journal_logged {
                peer.journal_logged = true;
                eprintln!("RTP-MIDI: peer sends a recovery journal; it is ignored (not implemented)");
            }
            self.sink.dispatch_all(section.messages);
        }
    }
}

/// Respond to a clock sync packet (3-way timestamp exchange).
fn handle_sync(data: &[u8], sock: &UdpSocket, addr: SocketAddr, epoch: Instant) {
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

// ── RTP / RFC 6295 parsing ───────────────────────────────────────────

/// The parts of the RTP fixed header we care about.
#[derive(Debug, PartialEq)]
struct RtpHeader {
    seq: u16,
    #[allow(dead_code)]
    ssrc: u32,
    /// Offset of the MIDI command section (after CSRCs and any extension).
    payload_offset: usize,
}

/// Parse the RTP fixed header (RFC 3550 §5.1), skipping CSRC entries and
/// a header extension if present.
fn parse_rtp_header(packet: &[u8]) -> Option<RtpHeader> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let has_ext = packet[0] & 0x10 != 0;
    let csrc_count = (packet[0] & 0x0F) as usize;
    let seq = u16::from_be_bytes([packet[2], packet[3]]);
    let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);

    let mut offset = 12 + csrc_count * 4;
    if has_ext {
        if packet.len() < offset + 4 {
            return None;
        }
        let ext_words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        offset += 4 + ext_words * 4;
    }
    if offset > packet.len() {
        return None;
    }
    Some(RtpHeader { seq, ssrc, payload_offset: offset })
}

/// Result of parsing one MIDI command section.
#[derive(Debug)]
struct CommandSection {
    messages: Vec<MidiMessage>,
    has_journal: bool,
}

/// Parse the RTP-MIDI payload: command section header, MIDI list, and
/// (ignored) recovery journal, per RFC 6295 §3.
///
/// MIDI list grammar: `[delta]? cmd (delta cmd)*` — a delta time precedes
/// every command except the first, which has one only when Z=1. A command
/// is an optional status octet (running status when omitted) followed by
/// its data; P=1 means the first command's status is the running status
/// left over from the previous packet.
fn parse_command_section(payload: &[u8], peer: &mut PeerState) -> Option<CommandSection> {
    let flags = *payload.first()?;
    let b_flag = flags & 0x80 != 0;
    let j_flag = flags & 0x40 != 0;
    let z_flag = flags & 0x20 != 0;
    let p_flag = flags & 0x10 != 0;

    let (midi_len, midi_start) = if b_flag {
        // Long header: 12-bit length
        let len = (((flags & 0x0F) as usize) << 8) | *payload.get(1)? as usize;
        (len, 2)
    } else {
        ((flags & 0x0F) as usize, 1)
    };

    if midi_start + midi_len > payload.len() {
        return None;
    }
    let list = &payload[midi_start..midi_start + midi_len];

    let mut messages = Vec::new();
    let mut pos = 0;
    let mut first = true;
    // Running status is per RTP stream; P=1 relies on the previous packet's.
    let mut running_status = if p_flag { peer.running_status } else { 0 };

    while pos < list.len() {
        // Delta time: before every command except (when Z=0) the first.
        if !first || z_flag {
            let Some(n) = skip_vlq(&list[pos..]) else { break };
            pos += n;
            if pos >= list.len() {
                break;
            }
        }
        first = false;

        let b = list[pos];
        let status = if b & 0x80 != 0 {
            pos += 1;
            b
        } else {
            if running_status == 0 {
                // Data with no status to attach to; the list is unusable.
                break;
            }
            running_status
        };

        if status >= 0xF0 {
            pos = skip_system_command(status, list, pos, &mut running_status);
            continue;
        }
        running_status = status;

        let data_len = channel_data_length(status);
        if pos + data_len > list.len() {
            break;
        }
        let data = &list[pos..pos + data_len];
        if data.iter().any(|d| d & 0x80 != 0) {
            // Malformed: a status byte where data was expected. Give up on
            // the rest of the list rather than invent notes.
            break;
        }
        let mut msg = [status, 0, 0];
        msg[1..1 + data_len].copy_from_slice(data);
        pos += data_len;

        if let Some(m) = midi::parse_midi_message(&msg[..1 + data_len]) {
            messages.push(m);
        }
    }

    peer.running_status = running_status;

    Some(CommandSection {
        messages,
        has_journal: j_flag,
    })
}

/// Length in bytes of a variable-length delta time (1-4 octets; all but the
/// last have bit 7 set). `None` if truncated or longer than 4 octets.
fn skip_vlq(bytes: &[u8]) -> Option<usize> {
    for (i, b) in bytes.iter().take(4).enumerate() {
        if b & 0x80 == 0 {
            return Some(i + 1);
        }
    }
    None
}

/// Skip a System message whose status is at `pos - 1`; return the new
/// position. SysEx (F0 ... F7, or RFC 6295 §3.2 segments ending in F0/F7/F4)
/// and System Common clear running status; real-time leaves it alone.
fn skip_system_command(status: u8, list: &[u8], mut pos: usize, running_status: &mut u8) -> usize {
    match status {
        0xF0 | 0xF7 => {
            *running_status = 0;
            // Data octets until a terminator (F7 end, F0 "more to come", F4 cancel).
            while pos < list.len() && list[pos] & 0x80 == 0 {
                pos += 1;
            }
            if pos < list.len() && matches!(list[pos], 0xF7 | 0xF0 | 0xF4) {
                pos += 1;
            }
            pos
        }
        0xF8..=0xFF => pos,
        0xF1 | 0xF3 => {
            *running_status = 0;
            pos + 1
        }
        0xF2 => {
            *running_status = 0;
            pos + 2
        }
        _ => {
            *running_status = 0;
            pos
        }
    }
}

fn channel_data_length(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiMessageKind;
    use dx7_core::SynthCommand;
    use ringbuf::traits::Split;

    /// RTP header: V=2, PT=0x61, seq, ts=0, SSRC=1.
    fn rtp_header(seq: u16) -> Vec<u8> {
        let mut h = vec![0x80, 0x61];
        h.extend_from_slice(&seq.to_be_bytes());
        h.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        h
    }

    fn packet(seq: u16, section: &[u8]) -> Vec<u8> {
        let mut p = rtp_header(seq);
        p.extend_from_slice(section);
        p
    }

    /// Short-header command section with the given flags and MIDI list.
    fn section(flags: u8, list: &[u8]) -> Vec<u8> {
        assert!(list.len() < 16);
        let mut s = vec![flags | list.len() as u8];
        s.extend_from_slice(list);
        s
    }

    fn parse(packet: &[u8]) -> Vec<MidiMessage> {
        let mut peer = PeerState::default();
        let h = parse_rtp_header(packet).expect("rtp header");
        parse_command_section(&packet[h.payload_offset..], &mut peer)
            .map(|s| s.messages)
            .unwrap_or_default()
    }

    fn synth_cmds(msgs: &[MidiMessage]) -> Vec<SynthCommand> {
        msgs.iter()
            .map(|m| match &m.kind {
                MidiMessageKind::Synth(c) => c.clone(),
                other => panic!("expected synth command, got {:?}", other),
            })
            .collect()
    }

    fn notes(msgs: &[MidiMessage]) -> Vec<(u8, u8)> {
        msgs.iter()
            .filter_map(|m| match &m.kind {
                MidiMessageKind::Synth(SynthCommand::NoteOn { note, velocity }) => Some((*note, *velocity)),
                MidiMessageKind::Synth(SynthCommand::NoteOff { note }) => Some((*note, 0)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_parse_rtp_midi_note_on() {
        let cmds = synth_cmds(&parse(&packet(1, &section(0, &[0x90, 60, 100]))));
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
        let cmds = synth_cmds(&parse(&packet(1, &section(0, &[0x80, 60, 0]))));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::NoteOff { note } => assert_eq!(*note, 60),
            other => panic!("Expected NoteOff, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_rtp_midi_empty_payload() {
        assert!(parse(&packet(1, &[0x00])).is_empty());
    }

    #[test]
    fn test_parse_rtp_midi_pitch_bend() {
        // Pitch bend center: 0xE0 0x00 0x40 = value 8192 - 8192 = 0
        let cmds = synth_cmds(&parse(&packet(1, &section(0, &[0xE0, 0x00, 0x40]))));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            SynthCommand::PitchBend { value } => assert_eq!(*value, 0),
            other => panic!("Expected PitchBend, got {:?}", other),
        }
    }

    // ── RFC 6295 §3 command list ─────────────────────────────────────

    #[test]
    fn multi_command_with_delta_between() {
        // From issue #22: [0x90,60,100][delta 0x00][0x90,62,100]
        let msgs = parse(&packet(1, &section(0, &[0x90, 60, 100, 0x00, 0x90, 62, 100])));
        assert_eq!(notes(&msgs), vec![(60, 100), (62, 100)], "{msgs:?}");
    }

    #[test]
    fn running_status_after_delta() {
        // Second command omits its status byte.
        let msgs = parse(&packet(1, &section(0, &[0x90, 60, 100, 0x00, 62, 100, 0x05, 64, 90])));
        assert_eq!(notes(&msgs), vec![(60, 100), (62, 100), (64, 90)]);
    }

    #[test]
    fn z_flag_means_delta_before_first_command() {
        // Z=1 (0x20): first command is preceded by a delta time.
        let msgs = parse(&packet(1, &section(0x20, &[0x10, 0x90, 60, 100, 0x10, 0x80, 60, 64])));
        assert_eq!(notes(&msgs), vec![(60, 100), (60, 0)]);
    }

    #[test]
    fn multi_byte_delta_is_not_a_status_byte() {
        // Delta 0x81 0x00 (=128) sits in the channel-status range.
        let msgs = parse(&packet(1, &section(0, &[0x90, 60, 100, 0x81, 0x00, 0x90, 62, 100])));
        assert_eq!(notes(&msgs), vec![(60, 100), (62, 100)]);
        // Four-byte delta with running status.
        let msgs = parse(&packet(1, &section(0, &[0x90, 60, 100, 0xFF, 0xFF, 0xFF, 0x7F, 62, 100])));
        assert_eq!(notes(&msgs), vec![(60, 100), (62, 100)]);
    }

    #[test]
    fn long_header_length() {
        // B=1: 12-bit length in the next byte.
        let list = [0x90, 60, 100, 0x00, 0xB0, 1, 64];
        let mut sect = vec![0x80, list.len() as u8];
        sect.extend_from_slice(&list);
        let msgs = parse(&packet(1, &sect));
        assert_eq!(msgs.len(), 2);
        assert!(matches!(msgs[1].kind, MidiMessageKind::Synth(SynthCommand::ControlChange { cc: 1, value: 64 })));
    }

    #[test]
    fn journal_bytes_are_not_parsed_as_midi() {
        // J=1 (0x40): bytes after LEN belong to the journal, not the MIDI list.
        let mut sect = section(0x40, &[0x90, 60, 100]);
        sect.extend_from_slice(&[0x90, 61, 100, 0x90, 62, 100]); // "journal"
        let mut peer = PeerState::default();
        let p = packet(1, &sect);
        let h = parse_rtp_header(&p).unwrap();
        let s = parse_command_section(&p[h.payload_offset..], &mut peer).unwrap();
        assert!(s.has_journal);
        assert_eq!(notes(&s.messages), vec![(60, 100)]);
    }

    #[test]
    fn p_flag_uses_running_status_from_previous_packet() {
        let mut peer = PeerState::default();
        let p1 = packet(1, &section(0, &[0x90, 60, 100]));
        let h = parse_rtp_header(&p1).unwrap();
        parse_command_section(&p1[h.payload_offset..], &mut peer).unwrap();
        assert_eq!(peer.running_status, 0x90);

        // P=1 (0x10): first command has no status byte.
        let p2 = packet(2, &section(0x10, &[62, 100]));
        let h = parse_rtp_header(&p2).unwrap();
        let s = parse_command_section(&p2[h.payload_offset..], &mut peer).unwrap();
        assert_eq!(notes(&s.messages), vec![(62, 100)]);
    }

    #[test]
    fn sysex_is_skipped_and_clears_running_status() {
        let msgs = parse(&packet(1, &section(0, &[0x90, 60, 100, 0x00, 0xF0, 0x43, 0x10, 0xF7, 0x00, 62, 100])));
        // The trailing [62,100] has no status (cleared by SysEx) and is dropped.
        assert_eq!(notes(&msgs), vec![(60, 100)]);
        let msgs = parse(&packet(1, &section(0, &[0xF0, 0x43, 0xF7, 0x00, 0x90, 60, 100])));
        assert_eq!(notes(&msgs), vec![(60, 100)]);
    }

    #[test]
    fn realtime_preserves_running_status() {
        let msgs = parse(&packet(1, &section(0, &[0x90, 60, 100, 0x00, 0xF8, 0x00, 62, 100])));
        assert_eq!(notes(&msgs), vec![(60, 100), (62, 100)]);
    }

    #[test]
    fn program_change_keeps_channel() {
        let msgs = parse(&packet(1, &section(0, &[0xC5, 7, 0x00, 0x95, 60, 100])));
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].channel, 5);
        assert!(matches!(msgs[0].kind, MidiMessageKind::ProgramChange(7)));
        assert_eq!(msgs[1].channel, 5);
    }

    #[test]
    fn truncated_command_is_dropped() {
        let msgs = parse(&packet(1, &section(0, &[0x90, 60, 100, 0x00, 0x90, 62])));
        assert_eq!(notes(&msgs), vec![(60, 100)]);
    }

    #[test]
    fn rtp_header_skips_csrc_and_extension() {
        // CC=1, X=1: one CSRC word, extension of 1 word.
        let mut p = vec![0x80 | 0x10 | 0x01, 0x61, 0x00, 0x07, 0, 0, 0, 0, 0, 0, 0, 1];
        p.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // CSRC
        p.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 1, 2, 3, 4]); // ext header + 1 word
        p.extend_from_slice(&section(0, &[0x90, 60, 100]));
        let h = parse_rtp_header(&p).unwrap();
        assert_eq!(h.seq, 7);
        assert_eq!(h.payload_offset, 24);
        assert_eq!(notes(&parse(&p)), vec![(60, 100)]);
    }

    // ── Sequence numbers ─────────────────────────────────────────────

    #[test]
    fn sequence_dedupe_and_stale_drop() {
        let mut peer = PeerState::default();
        assert_eq!(peer.check_sequence(10), SeqVerdict::Accept { lost: 0 });
        assert_eq!(peer.check_sequence(10), SeqVerdict::Duplicate);
        assert_eq!(peer.check_sequence(11), SeqVerdict::Accept { lost: 0 });
        assert_eq!(peer.check_sequence(9), SeqVerdict::Stale);
        assert_eq!(peer.check_sequence(14), SeqVerdict::Accept { lost: 2 });
        assert_eq!(peer.check_sequence(12), SeqVerdict::Stale);
    }

    #[test]
    fn sequence_wraps_around() {
        let mut peer = PeerState::default();
        assert_eq!(peer.check_sequence(0xFFFF), SeqVerdict::Accept { lost: 0 });
        assert_eq!(peer.check_sequence(0x0000), SeqVerdict::Accept { lost: 0 });
        assert_eq!(peer.check_sequence(0xFFFF), SeqVerdict::Stale);
        assert_eq!(peer.check_sequence(0x0001), SeqVerdict::Accept { lost: 0 });
    }

    #[test]
    fn allow_list_matches_canonical_addresses() {
        let (tx, _rx) = ringbuf::HeapRb::<SynthCommand>::new(4).split();
        let sink = Arc::new(CommandSink::new(
            Arc::new(std::sync::Mutex::new(tx)),
            Vec::new(),
        ));
        let l = Listener::new(vec!["192.168.1.10".parse().unwrap()], sink.clone());
        assert!(l.is_allowed("192.168.1.10".parse().unwrap()));
        // IPv4-mapped IPv6 (what a dual-stack socket reports) must match too.
        assert!(l.is_allowed("::ffff:192.168.1.10".parse::<IpAddr>().unwrap().to_canonical()));
        assert!(!l.is_allowed("192.168.1.11".parse().unwrap()));
        let open = Listener::new(Vec::new(), sink);
        assert!(open.is_allowed("10.0.0.1".parse().unwrap()));
    }
}
