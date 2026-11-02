//! Simple Voice Chat client for the Horizon engine.
//!
//! One `VoiceEngine` per client connection. The game layer feeds it the
//! `voicechat:secret` payload, ticks it once per frame, and pulls decoded
//! voice frames to place at speaking players' positions.

pub mod crypto;
#[cfg(feature = "mic")]
pub mod mic;
pub mod net;
pub mod opus;
pub mod protocol;


use crypto::Secret;
use net::{ToNet, VoiceNet};
use protocol::{SecretInfo, VoicePacket, FRAME_SAMPLES};

/// A decoded voice frame, ready to place in the world.
pub struct VoiceFrame {
    /// The speaking player's UUID.
    pub sender: u128,
    /// 48 kHz mono f32.
    pub pcm: Vec<f32>,
    pub whispering: bool,
    /// Group or location-based sound: no entity position to take.
    pub ambient: bool,
    pub distance: f32,
}

pub struct VoiceEngine {
    state: State,
    net: Option<VoiceNet>,
    decoder: Option<opus::Decoder>,
    /// Whether the microphone is on.
    mic_enabled: bool,
    /// The gate's threshold on frame RMS: below it, nothing is sent.
    mic_threshold: f32,
    #[cfg(feature = "mic")]
    mic: Option<mic::MicCapture>,
}

enum State {
    Idle,
    Connected { info: SecretInfo },
    Failed,
}

impl Default for VoiceEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceEngine {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            net: None,
            decoder: opus::Decoder::new(),
            mic_enabled: true,
            mic_threshold: 0.01,
            #[cfg(feature = "mic")]
            mic: None,
        }
    }

    pub fn connected(&self) -> bool {
        matches!(self.state, State::Connected { .. })
    }

    pub fn secret_info(&self) -> Option<&SecretInfo> {
        match &self.state {
            State::Connected { info } => Some(info),
            _ => None,
        }
    }

    pub fn mic_enabled(&self) -> bool {
        self.mic_enabled
    }

    /// Starts the voice session from a `voicechat:secret` payload.
    pub fn start(&mut self, info: SecretInfo, fallback_host: &str) {
        if self.connected() {
            return;
        }
        let Some(addr) = info.udp_addr(fallback_host) else {
            log::warn!("voice: could not resolve voice server address");
            self.state = State::Failed;
            return;
        };
        let secret = Secret::new(info.secret);
        let keep_alive = info.keep_alive.max(1) as u32;
        let uuid = info.player_uuid;
        match VoiceNet::spawn(addr, secret, uuid, keep_alive, self.mic_threshold) {
            Ok(net) => {
                // Authenticate immediately: the server answers with an ack.
                let _ = net.tx.send(ToNet::Send(VoicePacket::Authenticate));
                if self.mic_enabled {
                    let tx = net.tx.clone();
                    self.start_mic(&tx);
                }
                log::info!("voice: connecting to {addr}");
                self.net = Some(net);
                self.state = State::Connected { info };
            }
            Err(e) => {
                log::warn!("voice: UDP socket failed: {e}");
                self.state = State::Failed;
            }
        }
    }

    fn start_mic(&mut self, net_tx: &std::sync::mpsc::Sender<ToNet>) {
        #[cfg(feature = "mic")]
        {
            if self.mic.is_some() {
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
            // Bridge frames from the capture callback into the net thread.
            let net_tx = net_tx.clone();
            std::thread::Builder::new()
                .name("voice-mic".into())
                .spawn(move || {
                    while let Ok(frame) = rx.recv() {
                        if net_tx.send(ToNet::MicFrame(frame)).is_err() {
                            return;
                        }
                    }
                })
                .ok();
            match mic::MicCapture::start(tx) {
                Some(capture) => self.mic = Some(capture),
                None => log::warn!("voice: no microphone available, receive-only"),
            }
        }
        #[cfg(not(feature = "mic"))]
        {
            let _ = net_tx;
        }
    }

    /// Toggles the microphone while connected.
    pub fn set_mic(&mut self, on: bool) {
        self.mic_enabled = on;
        if !self.connected() {
            return;
        }
        if on {
            if let Some(net) = &self.net {
                let tx = net.tx.clone();
                self.start_mic(&tx);
            }
        } else {
            #[cfg(feature = "mic")]
            {
                self.mic = None;
            }
        }
    }

    /// Shuts everything down; called when leaving the server.
    pub fn stop(&mut self) {
        self.net = None;
        #[cfg(feature = "mic")]
        {
            self.mic = None;
        }
        self.state = State::Idle;
    }

    /// Drains incoming packets; returns decoded frames for the game to place.
    pub fn tick(&mut self) -> Vec<VoiceFrame> {
        let mut frames = Vec::new();
        let mut messages = Vec::new();
        if let Some(net) = &self.net {
            while let Ok(msg) = net.rx.try_recv() {
                messages.push(msg);
                if messages.len() > 128 {
                    break;
                }
            }
        }
        for msg in messages {
            match msg {
                net::FromNet::Disconnected => {
                    self.stop();
                    return frames;
                }
                net::FromNet::Packet(packet) => match packet {
                    VoicePacket::PlayerSound(p) => {
                        if let Some(pcm) = self.decode(&p.data) {
                            frames.push(VoiceFrame {
                                sender: p.sender,
                                pcm,
                                whispering: p.whispering,
                                ambient: false,
                                distance: p.distance,
                            });
                        }
                    }
                    VoicePacket::GroupSound { sender, data, .. } => {
                        if let Some(pcm) = self.decode(&data) {
                            frames.push(VoiceFrame {
                                sender,
                                pcm,
                                whispering: true,
                                ambient: true,
                                distance: 16.0,
                            });
                        }
                    }
                    VoicePacket::LocationSound { sender, data, distance, .. } => {
                        if let Some(pcm) = self.decode(&data) {
                            frames.push(VoiceFrame {
                                sender,
                                pcm,
                                whispering: false,
                                ambient: true,
                                distance,
                            });
                        }
                    }
                    VoicePacket::AuthenticateAck => log::info!("voice: authenticated with server"),
                    VoicePacket::KeepAlive | VoicePacket::Authenticate | VoicePacket::Ping { .. } => {}
                    VoicePacket::Mic { .. } => {}
                },
            }
        }
        frames
    }

    fn decode(&mut self, data: &[u8]) -> Option<Vec<f32>> {
        let pcm = self.decoder.as_mut()?.decode(data)?;
        if pcm.len() > FRAME_SAMPLES * 4 {
            return None;
        }
        Some(pcm)
    }
}
