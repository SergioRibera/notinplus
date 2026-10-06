//! Device + user identity for CRDT op authorship.
//!
//! `DeviceId` is per-install (first start allocates, persists to
//! `~/.config/notein/device.toml`). `UserId` is per-account — until a
//! real account system lands, it is also allocated on first start and
//! persisted next to the device id. `ActorId` pairs them: every CRDT
//! op carries its `ActorId` so Lamport desembate is deterministic
//! across devices and users.
//!
//! Persistence uses a hand-rolled two-line TOML format (`key = "uuid"`).
//! No `toml` / `serde` dep — the schema is two UUID strings; a real
//! parser would be dead weight.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use uuid::{Bytes, Uuid};

/// Per-install device identifier. Persists in `device.toml`.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct DeviceId(pub Bytes);

impl DeviceId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Uuid::from_bytes(self.0).fmt(f)
    }
}

/// Per-user identifier. Shared across every device a user signs in to.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct UserId(pub Bytes);

impl UserId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Uuid::from_bytes(self.0).fmt(f)
    }
}

/// `(user, device)` pair. Written as the authorship key on every op;
/// used to break ties when two ops share a Lamport timestamp.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct ActorId {
    pub user: UserId,
    pub device: DeviceId,
}

impl Ord for ActorId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.user
            .0
            .cmp(&other.user.0)
            .then_with(|| self.device.0.cmp(&other.device.0))
    }
}

impl PartialOrd for ActorId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

static GLOBAL_ACTOR: OnceLock<ActorId> = OnceLock::new();

/// Load the persisted actor, or allocate + persist a fresh one on
/// first run. Cached in an `OnceLock` — the on-disk I/O happens once
/// per process. Falls back to an ephemeral in-memory actor if the
/// config directory can't be created or written (so the app still
/// boots on read-only filesystems; the id just won't survive restart).
pub fn global_actor() -> ActorId {
    *GLOBAL_ACTOR.get_or_init(|| match load_or_create() {
        Ok(actor) => actor,
        Err(err) => {
            log::warn!(
                "notein identity: failed to persist device.toml ({err}); falling back to ephemeral actor"
            );
            ActorId {
                user: UserId::new_v4(),
                device: DeviceId::new_v4(),
            }
        }
    })
}

fn load_or_create() -> io::Result<ActorId> {
    let path = config_path()?;
    if path.exists() {
        if let Some(actor) = parse(&fs::read_to_string(&path)?) {
            return Ok(actor);
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let actor = ActorId {
        user: UserId::new_v4(),
        device: DeviceId::new_v4(),
    };
    fs::write(&path, serialize(actor))?;
    Ok(actor)
}

fn config_path() -> io::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME nor XDG_CONFIG_HOME"))?;
    Ok(base.join("notein").join("device.toml"))
}

fn serialize(actor: ActorId) -> String {
    format!(
        "device_id = \"{}\"\nuser_id = \"{}\"\n",
        Uuid::from_bytes(actor.device.0),
        Uuid::from_bytes(actor.user.0),
    )
}

fn parse(text: &str) -> Option<ActorId> {
    let mut device = None;
    let mut user = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rhs) = line.split_once('=')?;
        let key = key.trim();
        let value = rhs.trim().trim_matches('"');
        let bytes = *Uuid::parse_str(value).ok()?.as_bytes();
        match key {
            "device_id" => device = Some(DeviceId(bytes)),
            "user_id" => user = Some(UserId(bytes)),
            _ => {}
        }
    }
    Some(ActorId {
        device: device?,
        user: user?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_parse() {
        let a = ActorId {
            user: UserId::new_v4(),
            device: DeviceId::new_v4(),
        };
        let text = serialize(a);
        let b = parse(&text).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse("device_id = \"not-a-uuid\"\nuser_id = \"also-nope\"\n").is_none());
    }

    #[test]
    fn tolerates_comments_blank_lines() {
        let a = ActorId {
            user: UserId::new_v4(),
            device: DeviceId::new_v4(),
        };
        let text = format!(
            "# notein identity\n\n{}\n# trailing\n",
            serialize(a).trim()
        );
        let b = parse(&text).unwrap();
        assert_eq!(a, b);
    }
}
