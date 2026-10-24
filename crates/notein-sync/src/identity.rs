//! Who this device is. There are no accounts: a device is its Noise
//! keypair, and "the same user" is a set of devices that trust each
//! other.

use std::fmt;
use std::path::Path;

use blake2::{Blake2s256, Digest};

use crate::error::IdentityError;

const PROFILE_VERSION: u32 = 2;

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

/// What the device persists: its key, its display name and the
/// devices it trusts.
#[derive(Clone, Debug)]
pub struct DeviceProfile {
    key: DeviceKey,
    name: String,
    trusted: Vec<TrustedDevice>,
    relay: Option<String>,
}

#[derive(bincode::Encode, bincode::Decode)]
struct ProfileFile {
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
            trusted: Vec::new(),
            relay: None,
        }
    }

    /// Read the profile at `path`, or create one named `default_name`
    /// and write it there.
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

    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.encode()?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, IdentityError> {
        let file = ProfileFile {
            version: PROFILE_VERSION,
            public: self.key.keypair.public,
            private: self.key.keypair.private,
            name: self.name.clone(),
            trusted: self.trusted.clone(),
            relay: self.relay.clone(),
        };
        bincode::encode_to_vec(file, bincode::config::standard())
            .map_err(|err| IdentityError::Codec(err.to_string()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, IdentityError> {
        let config = bincode::config::standard();
        let current: Option<ProfileFile> = bincode::decode_from_slice(bytes, config)
            .ok()
            .map(|(file, _): (ProfileFile, usize)| file)
            .filter(|file| file.version == PROFILE_VERSION);
        let file = if let Some(file) = current {
            file
        } else {
            let (old, _): (ProfileFileV1, usize) = bincode::decode_from_slice(bytes, config)
                .map_err(|err| IdentityError::Codec(err.to_string()))?;
            if old.version != 1 {
                return Err(IdentityError::Version(old.version));
            }
            ProfileFile {
                version: PROFILE_VERSION,
                public: old.public,
                private: old.private,
                name: old.name,
                trusted: old.trusted,
                relay: None,
            }
        };
        Ok(Self {
            key: DeviceKey::from_parts(file.public, file.private),
            name: file.name,
            trusted: file.trusted,
            relay: file.relay,
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
        let dir = std::env::temp_dir().join(format!("notein-sync-{}", rand::random::<u64>()));
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
            trusted: Vec::new(),
            relay: None,
        };
        let bytes = bincode::encode_to_vec(file, bincode::config::standard()).unwrap();
        assert!(matches!(
            DeviceProfile::decode(&bytes),
            Err(IdentityError::Version(99))
        ));
    }
}
