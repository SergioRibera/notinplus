//! Sharing documents with other people.
//!
//! A share names a set of documents, who may touch them and how, and
//! the key that keeps its traffic private. The owner hands out an
//! invitation (link or QR) carrying the key; anyone holding it can
//! join.

use std::collections::HashSet;
use std::fmt;

use crate::cipher::ShareKey;
use crate::identity::DeviceId;
use crate::replica::{BlobKey, DocId, Scope};

const LINK_SCHEME: &str = "notein://join?";

/// Identifies a share everywhere it travels.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShareId(pub [u8; 16]);

impl ShareId {
    #[must_use]
    pub fn random() -> Self {
        Self(rand::random())
    }
}

impl fmt::Debug for ShareId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShareId({})", hex::encode(&self.0[..4]))
    }
}

/// How long a share lives.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShareMode {
    /// Live while the owner is around; nothing is kept afterwards.
    Session,
    /// Every member keeps a replica and edits land whenever any path
    /// between members exists.
    Permanent,
}

/// Who may do something inside a share.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Who {
    Owner,
    Everyone,
    Nobody,
}

/// What members may do. The owner always may.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, Debug)]
pub struct SharePolicy {
    pub edit_content: Who,
    pub link_docs: Who,
    pub create_docs: Who,
    pub delete_docs: Who,
    pub invite: Who,
}

impl Default for SharePolicy {
    fn default() -> Self {
        Self {
            edit_content: Who::Everyone,
            link_docs: Who::Owner,
            create_docs: Who::Owner,
            delete_docs: Who::Owner,
            invite: Who::Owner,
        }
    }
}

impl SharePolicy {
    /// Members only look.
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            edit_content: Who::Nobody,
            link_docs: Who::Owner,
            create_docs: Who::Owner,
            delete_docs: Who::Owner,
            invite: Who::Owner,
        }
    }
}

/// Someone inside a share.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct Member {
    pub device: DeviceId,
    pub public: [u8; 32],
    pub name: String,
    pub joined_at: u64,
}

/// A share as every member holds it. The key never leaves the device
/// except inside an invitation.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct Share {
    pub id: ShareId,
    pub owner: Member,
    pub title: String,
    pub docs: Vec<DocId>,
    /// Records that travel with the documents: their item entries and
    /// the links whose both ends are inside.
    pub blobs: Vec<BlobKey>,
    pub mode: ShareMode,
    pub policy: SharePolicy,
    pub members: Vec<Member>,
    pub key: ShareKey,
    pub created_at: u64,
    pub updated_at: u64,
    /// Last relay record applied here; the mailbox is read from the
    /// next one.
    pub relay_cursor: u64,
}

impl bincode::Encode for ShareKey {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        self.as_bytes().encode(encoder)
    }
}

impl<C> bincode::Decode<C> for ShareKey {
    fn decode<D: bincode::de::Decoder<Context = C>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        <[u8; 32]>::decode(decoder).map(Self::from)
    }
}

bincode::impl_borrow_decode!(ShareKey);

impl Share {
    #[must_use]
    pub fn new(owner: Member, title: impl Into<String>, mode: ShareMode, now: u64) -> Self {
        Self {
            id: ShareId::random(),
            owner,
            title: title.into(),
            docs: Vec::new(),
            blobs: Vec::new(),
            mode,
            policy: SharePolicy::default(),
            members: Vec::new(),
            key: ShareKey::generate(),
            created_at: now,
            updated_at: now,
            relay_cursor: 0,
        }
    }

    #[must_use]
    pub fn is_owner(&self, device: DeviceId) -> bool {
        self.owner.device == device
    }

    #[must_use]
    pub fn is_member(&self, device: DeviceId) -> bool {
        self.is_owner(device) || self.members.iter().any(|m| m.device == device)
    }

    /// Every device inside, owner first.
    pub fn participants(&self) -> impl Iterator<Item = &Member> {
        std::iter::once(&self.owner).chain(self.members.iter())
    }

    fn allows(&self, who: Who, device: DeviceId) -> bool {
        match who {
            Who::Owner => self.is_owner(device),
            Who::Everyone => self.is_member(device),
            Who::Nobody => self.is_owner(device),
        }
    }

    #[must_use]
    pub fn can_edit(&self, device: DeviceId) -> bool {
        self.allows(self.policy.edit_content, device)
    }

    #[must_use]
    pub fn can_link(&self, device: DeviceId) -> bool {
        self.allows(self.policy.link_docs, device)
    }

    #[must_use]
    pub fn can_delete(&self, device: DeviceId) -> bool {
        self.allows(self.policy.delete_docs, device)
    }

    /// Whether a member only looks.
    #[must_use]
    pub fn is_read_only_for(&self, device: DeviceId) -> bool {
        !self.can_edit(device)
    }

    #[must_use]
    pub fn scope(&self) -> Scope {
        Scope::Subset {
            docs: self.docs.iter().copied().collect::<HashSet<_>>(),
            blobs: self.blobs.iter().copied().collect::<HashSet<_>>(),
        }
    }

    /// Add or refresh a member; returns whether it was new.
    pub fn admit(&mut self, member: Member) -> bool {
        if member.device == self.owner.device {
            return false;
        }
        match self.members.iter_mut().find(|m| m.device == member.device) {
            Some(existing) => {
                *existing = member;
                false
            }
            None => {
                self.members.push(member);
                true
            }
        }
    }

    pub fn expel(&mut self, device: DeviceId) -> bool {
        let before = self.members.len();
        self.members.retain(|m| m.device != device);
        before != self.members.len()
    }

    /// The invitation for this share.
    #[must_use]
    pub fn invite(&self) -> Invite {
        Invite {
            share: self.id,
            owner: Member {
                joined_at: 0,
                ..self.owner.clone()
            },
            title: self.title.clone(),
            mode: self.mode,
            key: self.key.clone(),
            server: None,
        }
    }

    /// What a member sees after joining: the share minus nothing, since
    /// the member already holds the key from the invitation.
    #[must_use]
    pub fn for_member(&self) -> Self {
        Self {
            relay_cursor: 0,
            ..self.clone()
        }
    }
}

/// What travels in an invitation link or QR.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Invite {
    pub share: ShareId,
    pub owner: Member,
    pub title: String,
    pub mode: ShareMode,
    pub key: ShareKey,
    /// Relay to meet at when no direct path exists.
    pub server: Option<String>,
}

impl Invite {
    /// `notein://join?share=…&owner=…&name=…&title=…&mode=…[&server=…]#<key>`.
    #[must_use]
    pub fn to_link(&self) -> String {
        let mode = match self.mode {
            ShareMode::Session => "session",
            ShareMode::Permanent => "permanent",
        };
        let mut link = format!(
            "{LINK_SCHEME}share={}&owner={}&name={}&title={}&mode={mode}",
            hex::encode(self.share.0),
            hex::encode(self.owner.public),
            hex::encode(self.owner.name.as_bytes()),
            hex::encode(self.title.as_bytes()),
        );
        if let Some(server) = &self.server {
            link.push_str("&server=");
            link.push_str(&hex::encode(server.as_bytes()));
        }
        link.push('#');
        link.push_str(&self.key.to_hex());
        link
    }

    /// Reverse of [`Self::to_link`].
    #[must_use]
    pub fn parse_link(link: &str) -> Option<Self> {
        let rest = link.trim().strip_prefix(LINK_SCHEME)?;
        let (query, fragment) = rest.split_once('#')?;
        let key = ShareKey::from_hex(fragment).ok()?;
        let mut share = None;
        let mut public = None;
        let mut name = None;
        let mut title = None;
        let mut mode = None;
        let mut server = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=')?;
            match k {
                "share" => share = Some(ShareId(<[u8; 16]>::try_from(hex::decode(v).ok()?).ok()?)),
                "owner" => public = Some(<[u8; 32]>::try_from(hex::decode(v).ok()?).ok()?),
                "name" => name = Some(String::from_utf8(hex::decode(v).ok()?).ok()?),
                "title" => title = Some(String::from_utf8(hex::decode(v).ok()?).ok()?),
                "mode" => {
                    mode = Some(match v {
                        "session" => ShareMode::Session,
                        "permanent" => ShareMode::Permanent,
                        _ => return None,
                    });
                }
                "server" => server = Some(String::from_utf8(hex::decode(v).ok()?).ok()?),
                _ => {}
            }
        }
        let public = public?;
        Some(Self {
            share: share?,
            owner: Member {
                device: DeviceId::of_public_key(&public),
                public,
                name: name?,
                joined_at: 0,
            },
            title: title?,
            mode: mode?,
            key,
            server,
        })
    }

    /// Proof that the joiner holds the key: the share id sealed with it.
    #[must_use]
    pub fn proof(&self) -> Vec<u8> {
        self.key.seal(&self.share.0).unwrap_or_default()
    }
}

/// Check a joiner's proof against the share's key.
#[must_use]
pub fn proof_is_valid(share: &Share, proof: &[u8]) -> bool {
    share.key.open(proof).is_ok_and(|plain| plain == share.id.0)
}

/// Every share a device takes part in, as persisted.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug, Default)]
pub struct ShareBook {
    pub shares: Vec<Share>,
}

impl ShareBook {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(book, _): (Self, usize)| book)
    }

    #[must_use]
    pub fn get(&self, id: ShareId) -> Option<&Share> {
        self.shares.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: ShareId) -> Option<&mut Share> {
        self.shares.iter_mut().find(|s| s.id == id)
    }

    /// Insert or replace.
    pub fn put(&mut self, share: Share) {
        match self.get_mut(share.id) {
            Some(slot) => *slot = share,
            None => self.shares.push(share),
        }
    }

    pub fn remove(&mut self, id: ShareId) -> Option<Share> {
        let pos = self.shares.iter().position(|s| s.id == id)?;
        Some(self.shares.remove(pos))
    }

    /// Shares holding `doc`.
    pub fn holding(&self, doc: DocId) -> impl Iterator<Item = &Share> {
        self.shares.iter().filter(move |s| s.docs.contains(&doc))
    }
}

/// What members exchange inside a share, always sealed with its key.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum ShareMessage {
    /// An exchange message for the share's documents.
    Sync(Vec<u8>),
    /// The owner's current view of the share (policy, members, scope).
    Meta(Share),
    /// The owner closed the share.
    Ended,
    /// A member walked away.
    Left,
}

impl ShareMessage {
    /// Seal for the wire.
    #[must_use]
    pub fn seal(&self, key: &ShareKey) -> Vec<u8> {
        let plain = bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default();
        key.seal(&plain).unwrap_or_default()
    }

    /// Open from the wire.
    #[must_use]
    pub fn open(key: &ShareKey, sealed: &[u8]) -> Option<Self> {
        let plain = key.open(sealed).ok()?;
        bincode::decode_from_slice(&plain, bincode::config::standard())
            .ok()
            .map(|(message, _)| message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::DeviceKey;

    fn owner() -> Member {
        let key = DeviceKey::generate();
        Member {
            device: key.id(),
            public: *key.public(),
            name: "Sergio · Tablet".into(),
            joined_at: 1,
        }
    }

    #[test]
    fn invite_link_round_trips() {
        let mut share = Share::new(owner(), "Apuntes de física", ShareMode::Permanent, 7);
        share.docs.push(DocId([3; 16]));
        let invite = share.invite();
        let parsed = Invite::parse_link(&invite.to_link()).unwrap();
        assert_eq!(parsed, invite);
        assert!(proof_is_valid(&share, &parsed.proof()));
        assert!(!proof_is_valid(
            &Share::new(owner(), "otro", ShareMode::Session, 1),
            &parsed.proof()
        ));
    }

    #[test]
    fn policy_defaults_follow_the_owner() {
        let mut share = Share::new(owner(), "x", ShareMode::Session, 1);
        let guest = DeviceKey::generate();
        let member = Member {
            device: guest.id(),
            public: *guest.public(),
            name: "Ana".into(),
            joined_at: 2,
        };
        assert!(!share.can_edit(guest.id()));
        assert!(share.admit(member.clone()));
        assert!(!share.admit(member));
        assert!(share.can_edit(guest.id()));
        assert!(!share.can_link(guest.id()));
        assert!(share.can_link(share.owner.device));
        share.policy = SharePolicy::read_only();
        assert!(share.is_read_only_for(guest.id()));
        assert!(share.can_edit(share.owner.device));
        assert!(share.expel(guest.id()));
        assert!(!share.is_member(guest.id()));
    }

    #[test]
    fn share_messages_only_open_with_the_right_key() {
        let share = Share::new(owner(), "x", ShareMode::Session, 1);
        let sealed = ShareMessage::Ended.seal(&share.key);
        assert_eq!(
            ShareMessage::open(&share.key, &sealed),
            Some(ShareMessage::Ended)
        );
        assert_eq!(ShareMessage::open(&ShareKey::generate(), &sealed), None);
    }

    #[test]
    fn book_round_trips_and_finds_by_doc() {
        let mut book = ShareBook::default();
        let mut share = Share::new(owner(), "x", ShareMode::Permanent, 1);
        share.docs.push(DocId([9; 16]));
        book.put(share.clone());
        book.put(share.clone());
        assert_eq!(book.shares.len(), 1);
        let back = ShareBook::decode(&book.encode()).unwrap();
        assert_eq!(back, book);
        assert_eq!(back.holding(DocId([9; 16])).count(), 1);
        assert_eq!(back.holding(DocId([8; 16])).count(), 0);
    }
}
