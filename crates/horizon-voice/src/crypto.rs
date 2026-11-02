//! AES-128-GCM secret shared with the voice server, matching the official
//! client: 16 random bytes, used as both the AES key and the `Secret` UUID.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use rand::Rng;

/// Nonce (12) + tag (16).
pub const ENCRYPTION_OVERHEAD: usize = 28;

#[derive(Clone)]
pub struct Secret {
    cipher: Aes128Gcm,
}

impl Secret {
    pub fn new(bytes: [u8; 16]) -> Self {
        let key = aes_gcm::Key::<Aes128Gcm>::from(bytes);
        Self { cipher: Aes128Gcm::new(&key) }
    }

    /// nonce ++ ciphertext ++ tag, as the official protocol frames it.
    pub fn encrypt(&self, data: &[u8]) -> Vec<u8> {
        let mut iv = [0u8; 12];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut iv);
        let nonce = Nonce::from_slice(&iv);
        let payload = self
            .cipher
            .encrypt(nonce, data)
            .expect("AES-GCM encryption cannot fail with valid key and nonce");
        let mut out = Vec::with_capacity(12 + payload.len());
        out.extend_from_slice(&iv);
        out.extend_from_slice(&payload);
        out
    }

    pub fn decrypt(&self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 12 {
            return None;
        }
        let nonce = Nonce::from_slice(&payload[..12]);
        self.cipher.decrypt(nonce, &payload[12..]).ok()
    }
}

/// Frames a decrypted packet body the way the official client sends it:
/// `0xFF`, varint length of the encrypted blob, then nonce++ciphertext++tag.
pub fn datagram(secret: &Secret, body: &[u8]) -> Vec<u8> {
    let encrypted = secret.encrypt(body);
    let mut out = Vec::with_capacity(1 + 5 + encrypted.len());
    out.push(0xFF);
    let mut len = encrypted.len() as u32;
    loop {
        let b = (len & 0x7F) as u8;
        len >>= 7;
        if len == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
    out.extend_from_slice(&encrypted);
    out
}

/// Strips the `0xFF`/varint framing; returns the encrypted blob.
pub fn unframe(datagram: &[u8]) -> Option<&[u8]> {
    if datagram.first() != Some(&0xFF) {
        return None;
    }
    let mut num = 0u32;
    let mut shift = 0;
    for (i, b) in datagram[1..].iter().enumerate().take(5) {
        num |= u32::from(b & 0x7F) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            let start = 2 + i;
            let blob = datagram.get(start..)?;
            return (blob.len() == num as usize).then_some(blob);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_datagram_framing() {
        let secret = Secret::new([7; 16]);
        let body = vec![0x2, 0x11, 0x22];
        let framed = datagram(&secret, &body);
        let blob = unframe(&framed).expect("framed");
        assert_eq!(secret.decrypt(blob).expect("decrypts"), body);
    }

    #[test]
    fn rejects_tampered_framing() {
        let secret = Secret::new([1; 16]);
        let framed = datagram(&secret, &[1, 2, 3]);
        assert!(unframe(&framed).is_some());
        assert!(unframe(&[]).is_none());
        assert!(unframe(&[0x00, 3, 1, 2, 3]).is_none());
        let wrong = Secret::new([2; 16]);
        let blob = unframe(&framed).unwrap();
        assert!(wrong.decrypt(blob).is_none());
    }
}
