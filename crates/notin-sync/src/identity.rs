//! Who this device is. There are no accounts: a device is its Noise
//! keypair, and "the same user" is a set of devices that trust each
//! other.

use std::fmt;
use std::path::Path;

use blake2::{Blake2s256, Digest};
use subtle::ConstantTimeEq;

use crate::error::IdentityError;

const PROFILE_VERSION: u32 = 3;

/// Stable identifier of a device: the hash of its public key.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    #[must_use]
    pub fn of_public_key(public: &[u8; 32]) -> Self {
        Self(Blake2s256::digest(public).into())
    }

    /// Replica id handed to the document store so every edit carries
    /// the device that made it. Unique per device, stable across runs.
    #[must_use]
    pub fn replica_id(&self) -> u64 {
        let mut head = [0u8; 8];
        head.copy_from_slice(&self.0[..8]);
        u64::from_le_bytes(head)
    }

    /// Eight hex characters for labels and logs.
    #[must_use]
    pub fn short(&self) -> String {
        hex::encode(&self.0[..4])
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

/// The device's long-term keypair. Authenticates every connection
/// and names the device.
#[derive(Clone)]
pub struct DeviceKey {
    keypair: zudp::Keypair,
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceKey")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

impl DeviceKey {
    #[must_use]
    pub fn generate() -> Self {
        Self {
            keypair: zudp::Keypair::generate(),
        }
    }

    #[must_use]
    pub const fn public(&self) -> &[u8; 32] {
        &self.keypair.public
    }

    #[must_use]
    pub fn id(&self) -> DeviceId {
        DeviceId::of_public_key(self.public())
    }

    /// The transport keypair, for opening authenticated channels.
    #[must_use]
    pub const fn keypair(&self) -> &zudp::Keypair {
        &self.keypair
    }

    /// Signs what this device leaves where readers have no channel to
    /// tell who wrote it, such as a relay mailbox. The transport key
    /// only does Diffie-Hellman, so this one is derived from it under
    /// its own domain and needs no storage.
    #[must_use]
    pub fn signing(&self) -> ed25519_dalek::SigningKey {
        let seed: [u8; 32] = Blake2s256::new()
            .chain_update(b"notin-sign")
            .chain_update(self.keypair.private)
            .finalize()
            .into();
        ed25519_dalek::SigningKey::from_bytes(&seed)
    }

    /// X25519 with the device whose transport key is `public`: the same
    /// secret on both ends, known to nobody else.
    #[must_use]
    pub fn agree(&self, public: &[u8; 32]) -> [u8; 32] {
        curve25519_dalek::montgomery::MontgomeryPoint(*public)
            .mul_clamped(self.keypair.private)
            .to_bytes()
    }

    /// Public half of [`Self::signing`].
    #[must_use]
    pub fn sign_public(&self) -> [u8; 32] {
        self.signing().verifying_key().to_bytes()
    }

    const fn from_parts(public: [u8; 32], private: [u8; 32]) -> Self {
        Self {
            keypair: zudp::Keypair { public, private },
        }
    }
}

/// Another device this one treats as the same user.
#[derive(bincode::Encode, bincode::Decode, Clone, Debug, PartialEq, Eq)]
pub struct TrustedDevice {
    pub id: DeviceId,
    pub public: [u8; 32],
    pub name: String,
    /// Unix milliseconds when the pairing happened.
    pub since: u64,
}

impl TrustedDevice {
    #[must_use]
    pub fn new(public: [u8; 32], name: impl Into<String>, since: u64) -> Self {
        Self {
            id: DeviceId::of_public_key(&public),
            public,
            name: name.into(),
            since,
        }
    }
}

/// Six-digit secret shown next to a QR so a device on the same
/// network can pair by typing instead of scanning.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PairingCode(u32);

impl PairingCode {
    const MAX: u32 = 1_000_000;

    #[must_use]
    pub fn random() -> Self {
        Self(rand::random::<u32>() % Self::MAX)
    }

    /// Parse exactly six digits.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != 6 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok().map(Self)
    }

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }

    /// Whether `code` is this one, in constant time.
    #[must_use]
    pub fn matches(self, code: u32) -> bool {
        self.0.to_be_bytes().ct_eq(&code.to_be_bytes()).into()
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06}", self.0)
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(..)")
    }
}

/// Random secret carried only by a pairing QR or link, never typed.
/// Six digits are too few to show a relay (it could try them all), so
/// reaching the offering device through one takes this instead.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PairingSecret([u8; 16]);

impl PairingSecret {
    #[must_use]
    pub fn random() -> Self {
        Self(rand::random())
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// What a device holding the link shows to pair: the secret bound
    /// to its own key, so a relay that saw it cannot pair as another.
    #[must_use]
    pub fn proof(&self, requester: &[u8; 32]) -> [u8; 32] {
        Blake2s256::new()
            .chain_update(b"notin-pair-proof")
            .chain_update(self.0)
            .chain_update(requester)
            .finalize()
            .into()
    }

    /// Whether `proof` is [`Self::proof`] for `requester`, in constant
    /// time.
    #[must_use]
    pub fn proves(&self, requester: &[u8; 32], proof: &[u8; 32]) -> bool {
        self.proof(requester).ct_eq(proof).into()
    }
}

impl From<[u8; 16]> for PairingSecret {
    fn from(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for PairingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingSecret(..)")
    }
}

/// What the device persists: its key, its display name and the
/// devices it trusts.
#[derive(Clone, Debug)]
pub struct DeviceProfile {
    key: DeviceKey,
    name: String,
    /// How the person wants to be called in a share, when it is not
    /// the device's name.
    user_name: Option<String>,
    trusted: Vec<TrustedDevice>,
    relay: Option<String>,
}

#[derive(bincode::Encode, bincode::Decode)]
struct ProfileFile {
    version: u32,
    public: [u8; 32],
    private: [u8; 32],
    name: String,
    user_name: Option<String>,
    trusted: Vec<TrustedDevice>,
    relay: Option<String>,
}

/// The second layout, without a user name.
#[derive(bincode::Decode)]
struct ProfileFileV2 {
    version: u32,
    public: [u8; 32],
    private: [u8; 32],
    name: String,
    trusted: Vec<TrustedDevice>,
    relay: Option<String>,
}

/// The first layout, without a relay.
#[derive(bincode::Decode)]
struct ProfileFileV1 {
    version: u32,
    public: [u8; 32],
    private: [u8; 32],
    name: String,
    trusted: Vec<TrustedDevice>,
}

impl DeviceProfile {
    /// Brand-new identity with no trusted devices.
    #[must_use]
    pub fn generate(name: impl Into<String>) -> Self {
        Self {
            key: DeviceKey::generate(),
            name: name.into(),
            user_name: None,
            trusted: Vec::new(),
            relay: None,
        }
    }

    /// Read the profile at `path`, or create one named `default_name`
    /// and write it there.
    #[cfg_attr(feature = "sismo", sismo::function("sync::profile::load_or_create"))]
    pub fn load_or_create(path: &Path, default_name: &str) -> Result<Self, IdentityError> {
        match std::fs::read(path) {
            Ok(bytes) => Self::decode(&bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let profile = Self::generate(default_name);
                profile.save(path)?;
                Ok(profile)
            }
            Err(err) => Err(err.into()),
        }
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::profile::save"))]
    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.encode()?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::profile::encode"))]
    pub fn encode(&self) -> Result<Vec<u8>, IdentityError> {
        let file = ProfileFile {
            version: PROFILE_VERSION,
            public: self.key.keypair.public,
            private: self.key.keypair.private,
            name: self.name.clone(),
            user_name: self.user_name.clone(),
            trusted: self.trusted.clone(),
            relay: self.relay.clone(),
        };
        bincode::encode_to_vec(file, bincode::config::standard())
            .map_err(|err| IdentityError::Codec(err.to_string()))
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::profile::decode"))]
    pub fn decode(bytes: &[u8]) -> Result<Self, IdentityError> {
        let config = bincode::config::standard();
        let current: Option<ProfileFile> = bincode::decode_from_slice(bytes, config)
            .ok()
            .map(|(file, _): (ProfileFile, usize)| file)
            .filter(|file| file.version == PROFILE_VERSION);
        let file = if let Some(file) = current {
            file
        } else {
            Self::decode_older(bytes)?
        };
        Ok(Self {
            key: DeviceKey::from_parts(file.public, file.private),
            name: file.name,
            user_name: file.user_name,
            trusted: file.trusted,
            relay: file.relay,
        })
    }

    /// Earlier layouts, read into the current one.
    fn decode_older(bytes: &[u8]) -> Result<ProfileFile, IdentityError> {
        let config = bincode::config::standard();
        let v2: Option<ProfileFileV2> = bincode::decode_from_slice(bytes, config)
            .ok()
            .map(|(file, _): (ProfileFileV2, usize)| file)
            .filter(|file| file.version == 2);
        if let Some(old) = v2 {
            return Ok(ProfileFile {
                version: PROFILE_VERSION,
                public: old.public,
                private: old.private,
                name: old.name,
                user_name: None,
                trusted: old.trusted,
                relay: old.relay,
            });
        }
        let (old, _): (ProfileFileV1, usize) = bincode::decode_from_slice(bytes, config)
            .map_err(|err| IdentityError::Codec(err.to_string()))?;
        if old.version != 1 {
            return Err(IdentityError::Version(old.version));
        }
        Ok(ProfileFile {
            version: PROFILE_VERSION,
            public: old.public,
            private: old.private,
            name: old.name,
            user_name: None,
            trusted: old.trusted,
            relay: None,
        })
    }

    /// Relay this device meets others at, as `host:port/<key>`.
    #[must_use]
    pub fn relay(&self) -> Option<&str> {
        self.relay.as_deref()
    }

    pub fn set_relay(&mut self, relay: Option<String>) {
        self.relay = relay.filter(|s| !s.trim().is_empty());
    }

    #[must_use]
    pub const fn key(&self) -> &DeviceKey {
        &self.key
    }

    #[must_use]
    pub fn id(&self) -> DeviceId {
        self.key.id()
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// The name chosen for shares, if any.
    #[must_use]
    pub fn user_name(&self) -> Option<&str> {
        self.user_name.as_deref()
    }

    /// What other people see: the chosen name, else the device's.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.user_name.as_deref().unwrap_or(&self.name)
    }

    pub fn set_user_name(&mut self, name: Option<String>) {
        self.user_name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
    }

    #[must_use]
    pub fn trusted(&self) -> &[TrustedDevice] {
        &self.trusted
    }

    #[must_use]
    pub fn is_trusted(&self, id: DeviceId) -> bool {
        self.trusted.iter().any(|d| d.id == id)
    }

    /// Add or refresh a trusted device. Re-pairing the same device
    /// updates its name and keeps a single entry.
    pub fn trust(&mut self, device: TrustedDevice) {
        match self.trusted.iter_mut().find(|d| d.id == device.id) {
            Some(existing) => *existing = device,
            None => self.trusted.push(device),
        }
    }

    /// Returns whether the device was trusted before.
    pub fn revoke(&mut self, id: DeviceId) -> bool {
        let before = self.trusted.len();
        self.trusted.retain(|d| d.id != id);
        before != self.trusted.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_checked_whole() {
        let code = PairingCode::random();
        assert!(code.matches(code.value()));
        assert!(!code.matches(code.value() ^ 1));
        let secret = PairingSecret::random();
        let proof = secret.proof(&[1; 32]);
        assert!(secret.proves(&[1; 32], &proof));
        assert!(!secret.proves(&[2; 32], &proof));
        let mut almost = proof;
        almost[31] ^= 1;
        assert!(!secret.proves(&[1; 32], &almost));
    }

    #[test]
    fn id_is_stable_for_a_key() {
        let key = DeviceKey::generate();
        assert_eq!(key.id(), DeviceId::of_public_key(key.public()));
        assert_ne!(key.id(), DeviceKey::generate().id());
    }

    #[test]
    fn profile_round_trips_through_bytes() {
        let mut profile = DeviceProfile::generate("Tablet");
        profile.trust(TrustedDevice::new([7; 32], "PC", 1));
        let back = DeviceProfile::decode(&profile.encode().unwrap()).unwrap();
        assert_eq!(back.id(), profile.id());
        assert_eq!(back.name(), "Tablet");
        assert_eq!(back.trusted(), profile.trusted());
        assert_eq!(
            back.key().keypair().private,
            profile.key().keypair().private
        );
    }

    #[test]
    fn load_or_create_persists_the_first_identity() {
        let dir = std::env::temp_dir().join(format!("notin-sync-{}", rand::random::<u64>()));
        let path = dir.join("device.bin");
        let first = DeviceProfile::load_or_create(&path, "Nuevo").unwrap();
        let again = DeviceProfile::load_or_create(&path, "Otro").unwrap();
        assert_eq!(first.id(), again.id());
        assert_eq!(again.name(), "Nuevo");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn trust_dedupes_and_revoke_removes() {
        let mut profile = DeviceProfile::generate("A");
        profile.trust(TrustedDevice::new([1; 32], "B", 1));
        profile.trust(TrustedDevice::new([1; 32], "B renamed", 2));
        assert_eq!(profile.trusted().len(), 1);
        assert_eq!(profile.trusted()[0].name, "B renamed");
        let id = profile.trusted()[0].id;
        assert!(profile.is_trusted(id));
        assert!(profile.revoke(id));
        assert!(!profile.revoke(id));
        assert!(!profile.is_trusted(id));
    }

    #[test]
    fn pairing_code_is_six_digits() {
        let code = PairingCode::random();
        let text = code.to_string();
        assert_eq!(text.len(), 6);
        assert_eq!(PairingCode::parse(&text), Some(code));
        assert_eq!(PairingCode::parse("12345"), None);
        assert_eq!(PairingCode::parse("12345a"), None);
    }

    #[test]
    fn unknown_profile_version_is_refused() {
        let file = ProfileFile {
            version: 99,
            public: [0; 32],
            private: [0; 32],
            name: String::new(),
            user_name: None,
            trusted: Vec::new(),
            relay: None,
        };
        let bytes = bincode::encode_to_vec(file, bincode::config::standard()).unwrap();
        assert!(matches!(
            DeviceProfile::decode(&bytes),
            Err(IdentityError::Version(99))
        ));
    }

    #[test]
    fn user_name_round_trips_and_falls_back_to_the_device() {
        let mut profile = DeviceProfile::generate("Tablet");
        assert_eq!(profile.display_name(), "Tablet");
        profile.set_user_name(Some("  ".into()));
        assert_eq!(profile.user_name(), None);
        profile.set_user_name(Some(" Sergio ".into()));
        assert_eq!(profile.display_name(), "Sergio");
        let back = DeviceProfile::decode(&profile.encode().unwrap()).unwrap();
        assert_eq!(back.user_name(), Some("Sergio"));
        assert_eq!(back.name(), "Tablet");
    }
}
