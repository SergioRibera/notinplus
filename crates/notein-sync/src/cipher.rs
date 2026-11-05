//! Per-share symmetric key. Everything a share sends through a relay
//! is sealed with it, so the relay only ever stores ciphertext.

use std::fmt;

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::error::CipherError;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

/// 256-bit key shared by every member of a share. Travels in the
/// fragment of the invitation link, never through the relay.
#[derive(Clone, PartialEq, Eq)]
pub struct ShareKey([u8; KEY_LEN]);

impl fmt::Debug for ShareKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ShareKey(..)")
    }
}

impl From<[u8; KEY_LEN]> for ShareKey {
    fn from(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }
}

impl ShareKey {
    /// Fresh random key from the OS generator.
    #[must_use]
    pub fn generate() -> Self {
        Self(XChaCha20Poly1305::generate_key(&mut OsRng).into())
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Hex form for invitation links and QR codes.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse the form produced by [`Self::to_hex`].
    pub fn from_hex(text: &str) -> Result<Self, CipherError> {
        let bytes = hex::decode(text.trim()).map_err(|_| CipherError::BadKey)?;
        let key: [u8; KEY_LEN] = bytes.try_into().map_err(|_| CipherError::BadKey)?;
        Ok(Self(key))
    }

    /// Encrypt and authenticate `plain`. Output is `nonce || ciphertext || tag`.
    #[cfg_attr(feature = "sismo", sismo::function("sync::cipher::seal"))]
    pub fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, CipherError> {
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let body = cipher
            .encrypt(&nonce, plain)
            .map_err(|_| CipherError::Rejected)?;
        let mut out = Vec::with_capacity(NONCE_LEN + body.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Reverse of [`Self::seal`].
    #[cfg_attr(feature = "sismo", sismo::function("sync::cipher::open"))]
    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, CipherError> {
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return Err(CipherError::TooShort(sealed.len()));
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        cipher
            .decrypt(XNonce::from_slice(nonce), body)
            .map_err(|_| CipherError::Rejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_then_open_round_trips() {
        let key = ShareKey::generate();
        let sealed = key.seal(b"hola").unwrap();
        assert_ne!(&sealed[NONCE_LEN..], b"hola");
        assert_eq!(key.open(&sealed).unwrap(), b"hola");
    }

    #[test]
    fn other_key_is_rejected() {
        let sealed = ShareKey::generate().seal(b"x").unwrap();
        assert_eq!(
            ShareKey::generate().open(&sealed),
            Err(CipherError::Rejected)
        );
    }

    #[test]
    fn tampering_is_rejected() {
        let key = ShareKey::generate();
        let mut sealed = key.seal(b"payload").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert_eq!(key.open(&sealed), Err(CipherError::Rejected));
    }

    #[test]
    fn short_input_is_reported() {
        assert_eq!(
            ShareKey::generate().open(&[0; 10]),
            Err(CipherError::TooShort(10))
        );
    }

    #[test]
    fn hex_round_trips() {
        let key = ShareKey::generate();
        assert_eq!(ShareKey::from_hex(&key.to_hex()).unwrap(), key);
        assert_eq!(ShareKey::from_hex("zz"), Err(CipherError::BadKey));
    }
}
