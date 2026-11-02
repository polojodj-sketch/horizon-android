//! The UDP half: one thread sending (mic frames, keepalives) and one thread
//! receiving, both talking to the voice server over AES-GCM datagrams.

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use crate::crypto::{datagram, unframe, Secret};
use crate::protocol::{VoicePacket, MAX_PACKET_SIZE};

/// Commands for the network threads.
pub enum ToNet {
    /// A 20 ms microphone frame at 48 kHz mono.
    MicFrame(Vec<f32>),
    /// A control packet to encrypt and send as-is.
    Send(VoicePacket),
}

/// What the receiver hands the game.
#[derive(Debug)]
pub enum FromNet {
    Packet(VoicePacket),
    /// The socket died or the server went away.
    Disconnected,
}

pub struct VoiceNet {
    pub tx: Sender<ToNet>,
    pub rx: Receiver<FromNet>,
    _handle: std::thread::JoinHandle<()>,
}

impl VoiceNet {
    pub fn spawn(
        addr: SocketAddr,
        secret: Secret,
        self_uuid: u128,
        #[cfg_attr(not(feature = "mic"), allow(unused_variables))]
        keep_alive_secs: u32,
        #[cfg_attr(not(feature = "mic"), allow(unused_variables))]
        mic_threshold: f32,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.connect(addr)?;
        socket.set_read_timeout(Some(Duration::from_millis(250)))?;
        let (to_tx, to_rx) = std::sync::mpsc::channel::<ToNet>();
        let (from_tx, from_rx) = std::sync::mpsc::channel::<FromNet>();

        // Receiver: decrypt and decode datagrams until the socket dies.
        let recv_sock = socket.try_clone()?;
        let recv_secret = secret.clone();
        std::thread::Builder::new()
            .name("voice-recv".into())
            .spawn(move || {
                let mut buf = [0u8; MAX_PACKET_SIZE + 128];
                loop {
                    match recv_sock.recv(&mut buf) {
                        Ok(len) => {
                            let Some(blob) = unframe(&buf[..len]) else { continue };
                            let Some(plain) = recv_secret.decrypt(blob) else { continue };
                            if let Some(packet) = VoicePacket::decode(&plain) {
                                if from_tx.send(FromNet::Packet(packet)).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                        Err(_) => {
                            let _ = from_tx.send(FromNet::Disconnected);
                            return;
                        }
                    }
                }
            })?;

        // Sender: mic frames encoded with Opus, control packets, keepalives.
        let send_sock = socket;
        let handle = std::thread::Builder::new()
            .name("voice-send".into())
            .spawn(move || {
                #[cfg(feature = "mic")]
                let mut encoder = match crate::opus::Encoder::new() {
                    Some(e) => e,
                    None => {
                        log::warn!("voice: opus encoder unavailable, mic disabled");
                        None
                    }
                };
                #[cfg(not(feature = "mic"))]
                #[allow(unused_variables)]
                let encoder: Option<()> = None;
                #[cfg_attr(not(feature = "mic"), allow(unused_mut, unused_variables))]
                let mut sequence: i64 = 0;
                let mut last_keepalive = Instant::now();
                let interval = Duration::from_secs(keep_alive_secs.max(1) as u64);
                loop {
                    let item = if let Ok(item) = to_rx.recv_timeout(interval) {
                        item
                    } else {
                        ToNet::Send(VoicePacket::KeepAlive)
                    };
                    match item {
                        ToNet::Send(packet) => {
                            let mut body = Vec::with_capacity(64);
                            packet.encode(&mut body);
                            let _ = send_sock.send(&datagram(&secret, &body));
                            if matches!(packet, VoicePacket::AuthenticateAck) {
                                last_keepalive = Instant::now();
                            }
                        }
                        #[cfg(feature = "mic")]
                        ToNet::MicFrame(frame) => {
                            // Silence gate: skip frames quieter than the threshold.
                            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
                            if rms < mic_threshold {
                                continue;
                            }
                            if let Some(enc) = encoder.as_mut() {
                                if let Some(bytes) = enc.encode(&frame) {
                                    sequence = sequence.wrapping_add(1);
                                    let packet = VoicePacket::Mic {
                                        data: bytes,
                                        sequence_number: sequence,
                                        whispering: false,
                                    };
                                    let mut body = Vec::with_capacity(64 + FRAME_BYTES_CAP);
                                    packet.encode(&mut body);
                                    let _ = send_sock.send(&datagram(&secret, &body));
                                }
                            }
                        }
                        #[cfg(not(feature = "mic"))]
                        ToNet::MicFrame(_) => {}
                    }
                    if last_keepalive.elapsed() >= interval {
                        let mut body = Vec::with_capacity(8);
                        VoicePacket::KeepAlive.encode(&mut body);
                        let _ = send_sock.send(&datagram(&secret, &body));
                        last_keepalive = Instant::now();
                    }
                }
            })?;
        let _ = self_uuid;
        Ok(Self { tx: to_tx, rx: from_rx, _handle: handle })
    }
}

#[cfg_attr(not(feature = "mic"), allow(dead_code))]
const FRAME_BYTES_CAP: usize = 1280;
