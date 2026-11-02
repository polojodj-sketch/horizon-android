//! Simple Voice Chat protocol types.
//!
//! Wire formats follow the official `voicechat` plugin (compatibility version 20,
//! release 2.6.x): custom payload channels over the Minecraft connection, and
//! AES-GCM encrypted datagrams over UDP.

use std::net::SocketAddr;

pub const COMPATIBILITY_VERSION: i32 = 20;
pub const DEFAULT_UDP_PORT: i32 = 24454;
pub const MAX_OPUS_PAYLOAD_SIZE: usize = 1275;
pub const MAX_PACKET_SIZE: usize = 2048;
/// The output sample rate of the SVC Opus codec.
pub const SAMPLE_RATE: u32 = 48_000;
/// Opus frame length in milliseconds, as every SVC client uses.
pub const FRAME_MILLIS: u64 = 20;
/// Samples per Opus frame at 48 kHz.
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE as usize / 1000) * FRAME_MILLIS as usize;

pub const SECRET_CHANNEL: &str = "voicechat:secret";
pub const REQUEST_SECRET_CHANNEL: &str = "voicechat:request_secret";
pub const UPDATE_STATE_CHANNEL: &str = "voicechat:update_state";

/// What the `voicechat:secret` payload carries.
#[derive(Clone, Debug)]
pub struct SecretInfo {
    pub secret: [u8; 16],
    pub server_port: i32,
    pub player_uuid: u128,
    pub codec: u8,
    pub mtu_size: i32,
    pub distance: f64,
    pub keep_alive: i32,
    pub groups_enabled: bool,
    /// Empty: use the Minecraft server's host.
    pub voice_host: String,
    pub allow_recording: bool,
}

impl SecretInfo {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = Reader::new(data);
        let mut secret = [0u8; 16];
        secret.copy_from_slice(r.take(16)?);
        let server_port = r.i32()?;
        let player_uuid = r.uuid()?;
        let codec = r.u8()?;
        let mtu_size = r.i32()?;
        let distance = r.f64()?;
        let keep_alive = r.i32()?;
        let groups_enabled = r.u8()? != 0;
        let voice_host = r.string()?;
        let allow_recording = r.u8()? != 0;
        r.finished().then_some(Self {
            secret,
            server_port,
            player_uuid,
            codec,
            mtu_size,
            distance,
            keep_alive,
            groups_enabled,
            voice_host,
            allow_recording,
        })
    }

    /// The UDP endpoint the voice server listens on.
    pub fn udp_addr(&self, fallback_host: &str) -> Option<SocketAddr> {
        let host = if self.voice_host.is_empty() { fallback_host } else { &self.voice_host };
        let port = if self.server_port == -1 { DEFAULT_UDP_PORT } else { self.server_port };
        std::net::ToSocketAddrs::to_socket_addrs(&(host, port as u16))
            .ok()?
            .next()
    }
}

/// A decrypted UDP voice packet body (after the type byte).
#[derive(Clone, Debug)]
pub enum VoicePacket {
    Mic { data: Vec<u8>, sequence_number: i64, whispering: bool },
    PlayerSound(PlayerSound),
    GroupSound { sender: u128, data: Vec<u8>, sequence_number: i64 },
    LocationSound { sender: u128, location: [f64; 3], data: Vec<u8>, sequence_number: i64, distance: f32 },
    Authenticate,
    AuthenticateAck,
    Ping { id: u128, timestamp: i64 },
    KeepAlive,
}

#[derive(Clone, Debug)]
pub struct PlayerSound {
    pub channel_id: u128,
    pub sender: u128,
    pub data: Vec<u8>,
    pub sequence_number: i64,
    pub distance: f32,
    pub whispering: bool,
}

impl VoicePacket {
    pub fn type_id(&self) -> u8 {
        match self {
            Self::Mic { .. } => 0x1,
            Self::PlayerSound(_) => 0x2,
            Self::GroupSound { .. } => 0x3,
            Self::LocationSound { .. } => 0x4,
            Self::Authenticate => 0x5,
            Self::AuthenticateAck => 0x6,
            Self::Ping { .. } => 0x7,
            Self::KeepAlive => 0x8,
        }
    }

    pub fn encode(&self, buf: &mut Vec<u8>) {
        buf.push(self.type_id());
        match self {
            Self::Mic { data, sequence_number, whispering } => {
                put_bytes(buf, data);
                buf.extend_from_slice(&sequence_number.to_be_bytes());
                buf.push(u8::from(*whispering));
            }
            Self::PlayerSound(p) => {
                put_uuid(buf, p.channel_id);
                put_uuid(buf, p.sender);
                put_bytes(buf, &p.data);
                buf.extend_from_slice(&p.sequence_number.to_be_bytes());
                buf.extend_from_slice(&p.distance.to_be_bytes());
                buf.push(u8::from(p.whispering));
            }
            Self::GroupSound { sender, data, sequence_number } => {
                put_uuid(buf, *sender);
                put_uuid(buf, *sender);
                put_bytes(buf, data);
                buf.extend_from_slice(&sequence_number.to_be_bytes());
                buf.push(0);
            }
            Self::LocationSound { sender, location, data, sequence_number, distance } => {
                put_uuid(buf, *sender);
                put_uuid(buf, *sender);
                for v in location {
                    buf.extend_from_slice(&v.to_be_bytes());
                }
                put_bytes(buf, data);
                buf.extend_from_slice(&sequence_number.to_be_bytes());
                buf.extend_from_slice(&distance.to_be_bytes());
                buf.push(0);
            }
            Self::Authenticate { .. } | Self::AuthenticateAck | Self::KeepAlive => {}
            Self::Ping { id, timestamp } => {
                put_uuid(buf, *id);
                buf.extend_from_slice(&timestamp.to_be_bytes());
            }
        }
    }

    /// Parses a decrypted datagram body (type byte included).
    pub fn decode(data: &[u8]) -> Option<Self> {
        let mut r = Reader::new(data);
        let ty = r.u8()?;
        let packet = match ty {
            0x2 => {
                let channel_id = r.uuid()?;
                let sender = r.uuid()?;
                let data = r.byte_array()?;
                let sequence_number = r.i64()?;
                let distance = r.f32()?;
                let flags = r.u8()?;
                let whispering = flags & 1 != 0;
                let has_category = flags & 2 != 0;
                if has_category {
                    r.string()?;
                }
                Self::PlayerSound(PlayerSound { channel_id, sender, data, sequence_number, distance, whispering })
            }
            0x3 => {
                let _channel_id = r.uuid()?;
                let sender = r.uuid()?;
                let data = r.byte_array()?;
                let sequence_number = r.i64()?;
                let flags = r.u8()?;
                if flags & 2 != 0 {
                    r.string()?;
                }
                Self::GroupSound { sender, data, sequence_number }
            }
            0x4 => {
                let _channel_id = r.uuid()?;
                let sender = r.uuid()?;
                let mut location = [0f64; 3];
                for v in &mut location {
                    *v = r.f64()?;
                }
                let data = r.byte_array()?;
                let sequence_number = r.i64()?;
                let distance = r.f32()?;
                let flags = r.u8()?;
                if flags & 2 != 0 {
                    r.string()?;
                }
                Self::LocationSound { sender, location, data, sequence_number, distance }
            }
            0x5 => {
                let _player = r.uuid()?;
                let _secret = r.take(16)?;
                Self::Authenticate
            }
            0x6 => Self::AuthenticateAck,
            0x7 => {
                let id = r.uuid()?;
                let timestamp = r.i64()?;
                Self::Ping { id, timestamp }
            }
            0x8 => Self::KeepAlive,
            _ => return None,
        };
        r.finished().then_some(packet)
    }
}

/// `voicechat:request_secret` payload: compatibility version.
pub fn request_secret_payload() -> Vec<u8> {
    COMPATIBILITY_VERSION.to_be_bytes().to_vec()
}

/// `voicechat:update_state` payload.
pub fn update_state_payload(disabled: bool) -> Vec<u8> {
    vec![u8::from(disabled)]
}

pub fn put_uuid(buf: &mut Vec<u8>, id: u128) {
    buf.extend_from_slice(&id.to_be_bytes());
}

pub fn put_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    put_varint(buf, data.len() as u32);
    buf.extend_from_slice(data);
}

pub fn put_varint(buf: &mut Vec<u8>, value: u32) {
    let mut n = value;
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            buf.push(b);
            return;
        }
        buf.push(b | 0x80);
    }
}

/// Big-endian reader over a packet body, with the varint shapes the voice
/// protocol uses (byte arrays and strings).
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn varint(&mut self) -> Option<u32> {
        let mut num = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            num |= u32::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                return Some(num);
            }
        }
        None
    }

    pub fn u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    pub fn i32(&mut self) -> Option<i32> {
        let raw = self.take(4)?;
        Some(i32::from_be_bytes(raw.try_into().ok()?))
    }

    pub fn f32(&mut self) -> Option<f32> {
        let raw = self.take(4)?;
        Some(f32::from_be_bytes(raw.try_into().ok()?))
    }

    pub fn f64(&mut self) -> Option<f64> {
        let raw = self.take(8)?;
        Some(f64::from_be_bytes(raw.try_into().ok()?))
    }

    pub fn i64(&mut self) -> Option<i64> {
        let raw = self.take(8)?;
        Some(i64::from_be_bytes(raw.try_into().ok()?))
    }

    pub fn uuid(&mut self) -> Option<u128> {
        let raw = self.take(16)?;
        Some(u128::from_be_bytes(raw.try_into().ok()?))
    }

    pub fn byte_array(&mut self) -> Option<Vec<u8>> {
        let len = self.varint()? as usize;
        self.take(len.min(MAX_PACKET_SIZE)).map(|s| s.to_vec())
    }

    pub fn string(&mut self) -> Option<String> {
        let bytes = self.byte_array()?;
        String::from_utf8(bytes).ok()
    }

    pub fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    pub fn finished(&self) -> bool {
        self.pos == self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_info_parses_the_official_layout() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x7a; 16]);
        buf.extend_from_slice(&24454i32.to_be_bytes());
        buf.extend_from_slice(&0x11u128.to_be_bytes());
        buf.push(0);
        buf.extend_from_slice(&1024i32.to_be_bytes());
        buf.extend_from_slice(&48.0f64.to_be_bytes());
        buf.extend_from_slice(&10i32.to_be_bytes());
        buf.push(1);
        buf.push(4);
        buf.extend_from_slice(b"host");
        buf.push(0);
        let info = SecretInfo::parse(&buf).expect("parses");
        assert_eq!(info.secret, [0x7a; 16]);
        assert_eq!(info.player_uuid, 0x11);
        assert_eq!(info.voice_host, "host");
        assert!(!info.allow_recording);
    }

    #[test]
    fn mic_packet_encodes_the_official_layout() {
        // Clients send Mic, never receive it: check the wire shape directly.
        let mut buf = Vec::new();
        VoicePacket::Mic { data: vec![1, 2, 3], sequence_number: 7, whispering: true }.encode(&mut buf);
        let mut expected = vec![0x1, 3, 1, 2, 3];
        expected.extend_from_slice(&7i64.to_be_bytes());
        expected.push(1);
        assert_eq!(buf, expected);
    }

    #[test]
    fn player_sound_parses_flags_and_optional_category() {
        let mut body = Vec::new();
        body.push(0x2);
        put_uuid(&mut body, 1);
        put_uuid(&mut body, 2);
        put_bytes(&mut body, &[9, 9]);
        body.extend_from_slice(&42i64.to_be_bytes());
        body.extend_from_slice(&48.0f32.to_be_bytes());
        body.push(0b11);
        body.push(4);
        body.extend_from_slice(b"abcd");
        match VoicePacket::decode(&body).expect("decodes") {
            VoicePacket::PlayerSound(p) => {
                assert_eq!(p.sender, 2);
                assert!(p.whispering);
                assert_eq!(p.data, vec![9, 9]);
            }
            _ => panic!("wrong packet"),
        }
    }
}
