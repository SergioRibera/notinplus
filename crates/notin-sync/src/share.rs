//! Sharing documents with other people.
//!
//! A share names a set of documents, who may touch them and how, and
//! the key that keeps its traffic private. The owner hands out an
//! invitation (link or QR) carrying the key; anyone holding it can
//! join, at once or once the owner lets them in.

use std::collections::HashSet;
use std::fmt;

use crate::cipher::ShareKey;
use crate::identity::{DeviceId, DeviceKey};
use crate::relay::RecordKind;
use crate::replica::{BlobKey, BlobKind, DocId, Scope};

const LINK_SCHEME: &str = "notein://join?";
/// Earlier keys a share keeps for what was sealed with them.
const PREVIOUS_KEYS: usize = 32;

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

/// What someone may do inside a share, lowest first. The owner
/// outranks everyone; a member is never `Owner`.
#[derive(
    bincode::Encode,
    bincode::Decode,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Debug,
    Default,
)]
pub enum Role {
    /// Reads and follows the others.
    Viewer,
    /// Writes, draws, annotates, renames the share.
    #[default]
    Editor,
    /// Also invites, lets people in, adds and removes documents, and
    /// changes the roles below its own.
    CoOrganizer,
    /// Created the share and is the only one who can end it.
    Owner,
}

impl Role {
    /// The highest role a member can hold.
    pub const MEMBER_MAX: Self = Self::CoOrganizer;

    /// Clamp to what a member may be.
    #[must_use]
    pub fn for_member(self) -> Self {
        self.min(Self::MEMBER_MAX)
    }

    #[must_use]
    pub fn can_edit(self) -> bool {
        self >= Self::Editor
    }

    #[must_use]
    pub fn can_manage(self) -> bool {
        self >= Self::CoOrganizer
    }

    const fn tag(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::CoOrganizer => "co-organizer",
            Self::Owner => "owner",
        }
    }

    fn parse(tag: &str) -> Option<Self> {
        Some(match tag {
            "viewer" => Self::Viewer,
            "editor" => Self::Editor,
            "co-organizer" => Self::CoOrganizer,
            "owner" => Self::Owner,
            _ => return None,
        })
    }
}

/// Someone inside a share.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct Member {
    pub device: DeviceId,
    pub public: [u8; 32],
    /// What its mailbox records are signed with, see
    /// [`crate::identity::DeviceKey::signing`].
    pub sign: [u8; 32],
    /// How the person wants to be called.
    pub name: String,
    /// The device they are on, to tell two of theirs apart.
    pub device_name: String,
    pub role: Role,
    pub joined_at: u64,
}

impl Member {
    /// "Name · Device" when the device adds something, else the name.
    #[must_use]
    pub fn full_name(&self) -> String {
        if self.device_name.is_empty() || self.device_name == self.name {
            self.name.clone()
        } else {
            format!("{} · {}", self.name, self.device_name)
        }
    }
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
    /// The library folder this share mirrors, when it is a shared
    /// folder: whatever lands inside travels to everyone.
    pub folder: Option<[u8; 16]>,
    /// Shared as a whole desk: members get a desk with the documents,
    /// not loose documents.
    pub desk: bool,
    pub mode: ShareMode,
    /// The role someone gets by opening the invitation.
    pub entry_role: Role,
    /// Whether an organizer has to let each newcomer in.
    pub approve_entry: bool,
    /// Whether editors may add and remove documents, not only organizers.
    pub editors_add_docs: bool,
    pub members: Vec<Member>,
    /// People who asked in and wait for an organizer. Kept by the owner
    /// only.
    pub pending: Vec<Member>,
    /// Members the owner threw out, so one that was away hears it when
    /// it comes back. Kept by the owner only.
    pub expelled: Vec<DeviceId>,
    /// Members that walked away themselves: their old invitation still
    /// lets them back, though the key changed when they left. Kept by
    /// the owner only.
    pub departed: Vec<DeviceId>,
    pub key: ShareKey,
    pub created_at: u64,
    pub updated_at: u64,
    /// Last relay record applied here; the mailbox is read from the
    /// next one.
    pub relay_cursor: u64,
    /// The owner's relay, where members meet when no direct path
    /// exists: a [`crate::relay::RelayAddr`] as text.
    pub server: Option<String>,
    /// How many times the owner changed the key; grows with each.
    pub epoch: u32,
    /// Keys the share had before, newest first, to open what was sealed
    /// with them.
    pub previous: Vec<ShareKey>,
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
            owner: Member {
                role: Role::Owner,
                ..owner
            },
            title: title.into(),
            docs: Vec::new(),
            blobs: Vec::new(),
            folder: None,
            desk: false,
            mode,
            entry_role: Role::Editor,
            // A link that lasts for good is a link that gets forwarded.
            approve_entry: mode == ShareMode::Permanent,
            editors_add_docs: false,
            members: Vec::new(),
            pending: Vec::new(),
            expelled: Vec::new(),
            departed: Vec::new(),
            key: ShareKey::generate(),
            created_at: now,
            updated_at: now,
            relay_cursor: 0,
            server: None,
            epoch: 0,
            previous: Vec::new(),
        }
    }

    /// Open `sealed` with whichever of the share's keys sealed it.
    #[must_use]
    pub fn open_any(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        self.key_opening(sealed)?.open(sealed).ok()
    }

    /// The key of this share that sealed `sealed`.
    #[must_use]
    pub fn key_opening(&self, sealed: &[u8]) -> Option<&ShareKey> {
        std::iter::once(&self.key)
            .chain(&self.previous)
            .find(|key| key.open(sealed).is_ok())
    }

    /// The owner's new key: the current one joins the earlier ones.
    pub fn rotate(&mut self, key: ShareKey) {
        let old = std::mem::replace(&mut self.key, key);
        self.previous.insert(0, old);
        self.previous.truncate(PREVIOUS_KEYS);
        self.epoch = self.epoch.saturating_add(1);
    }

    /// Take the keys the owner handed over for `epoch`, newest first.
    /// Returns whether they were new here.
    pub fn adopt_keys(&mut self, epoch: u32, keys: Vec<ShareKey>) -> bool {
        if epoch <= self.epoch {
            return false;
        }
        let mut keys = keys.into_iter();
        let Some(newest) = keys.next() else {
            return false;
        };
        let mut known: Vec<ShareKey> = std::iter::once(std::mem::replace(&mut self.key, newest))
            .chain(std::mem::take(&mut self.previous))
            .chain(keys)
            .collect();
        let mut seen = Vec::new();
        known.retain(|k| {
            let fresh = *k != self.key && !seen.contains(k);
            seen.push(k.clone());
            fresh
        });
        known.truncate(PREVIOUS_KEYS);
        self.previous = known;
        self.epoch = epoch;
        true
    }

    /// Mark a change by the owner. `updated_at` only ever grows, so a
    /// member takes a view of the share only when it is newer than the
    /// one it holds and a replayed one is moot.
    pub fn touch(&mut self, now: u64) {
        self.updated_at = now.max(self.updated_at.saturating_add(1));
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

    #[must_use]
    pub fn member(&self, device: DeviceId) -> Option<&Member> {
        self.participants().find(|m| m.device == device)
    }

    /// What `device` may do here; `None` for someone outside.
    #[must_use]
    pub fn role_of(&self, device: DeviceId) -> Option<Role> {
        self.member(device).map(|m| m.role)
    }

    #[must_use]
    pub fn can_edit(&self, device: DeviceId) -> bool {
        self.role_of(device).is_some_and(Role::can_edit)
    }

    /// Linking documents to one another needs the same as editing them.
    #[must_use]
    pub fn can_link(&self, device: DeviceId) -> bool {
        self.can_edit(device)
    }

    /// Deleting a document for everyone is an organizer's call.
    #[must_use]
    pub fn can_delete(&self, device: DeviceId) -> bool {
        self.can_manage(device)
    }

    #[must_use]
    pub fn can_rename(&self, device: DeviceId) -> bool {
        self.can_edit(device)
    }

    /// Invite, let people in, expel, change roles below one's own.
    #[must_use]
    pub fn can_manage(&self, device: DeviceId) -> bool {
        self.role_of(device).is_some_and(Role::can_manage)
    }

    #[must_use]
    pub fn can_add_docs(&self, device: DeviceId) -> bool {
        self.can_manage(device) || (self.editors_add_docs && self.can_edit(device))
    }

    /// Whether `actor` may change or throw out `target`: organizers act
    /// on roles strictly below their own, never on the owner.
    #[must_use]
    pub fn outranks(&self, actor: DeviceId, target: DeviceId) -> bool {
        match (self.role_of(actor), self.role_of(target)) {
            (Some(a), Some(t)) => a.can_manage() && a > t,
            _ => false,
        }
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

    /// Add or refresh a member; returns whether it was new. A member
    /// never outranks [`Role::MEMBER_MAX`].
    pub fn admit(&mut self, member: Member) -> bool {
        if member.device == self.owner.device {
            return false;
        }
        let member = Member {
            role: member.role.for_member(),
            ..member
        };
        self.expelled.retain(|d| *d != member.device);
        self.departed.retain(|d| *d != member.device);
        self.pending.retain(|p| p.device != member.device);
        if let Some(existing) = self.members.iter_mut().find(|m| m.device == member.device) {
            *existing = member;
            false
        } else {
            self.members.push(member);
            true
        }
    }

    /// Someone asked in and waits for an organizer; returns whether
    /// they were not already waiting.
    pub fn knock(&mut self, member: Member) -> bool {
        if self.is_member(member.device) {
            return false;
        }
        if let Some(waiting) = self.pending.iter_mut().find(|p| p.device == member.device) {
            *waiting = member;
            false
        } else {
            self.pending.push(member);
            true
        }
    }

    /// Take someone off the waiting list.
    pub fn answer(&mut self, device: DeviceId) -> Option<Member> {
        let at = self.pending.iter().position(|p| p.device == device)?;
        Some(self.pending.remove(at))
    }

    /// Give a member another role; returns whether it changed.
    pub fn set_role(&mut self, device: DeviceId, role: Role) -> bool {
        let role = role.for_member();
        match self.members.iter_mut().find(|m| m.device == device) {
            Some(member) if member.role != role => {
                member.role = role;
                true
            }
            _ => false,
        }
    }

    /// A member is gone of its own accord; returns whether it was in.
    pub fn remove_member(&mut self, device: DeviceId) -> bool {
        let before = self.members.len();
        self.members.retain(|m| m.device != device);
        before != self.members.len()
    }

    /// Throw a member out, remembering it; returns whether it was in.
    pub fn expel(&mut self, device: DeviceId) -> bool {
        let removed = self.remove_member(device);
        if removed && !self.expelled.contains(&device) {
            self.expelled.push(device);
        }
        removed
    }

    /// A member walks away on its own; returns whether it was inside.
    pub fn depart(&mut self, device: DeviceId) -> bool {
        let removed = self.remove_member(device);
        if removed && !self.departed.contains(&device) {
            self.departed.push(device);
        }
        removed
    }

    #[must_use]
    pub fn was_expelled(&self, device: DeviceId) -> bool {
        self.expelled.contains(&device)
    }

    /// Bring documents in; returns whether anything was new.
    pub fn add_docs(&mut self, docs: &[DocId], blobs: &[BlobKey]) -> bool {
        let mut changed = false;
        for doc in docs {
            if !self.docs.contains(doc) {
                self.docs.push(*doc);
                changed = true;
            }
        }
        for blob in blobs {
            if !self.blobs.contains(blob) {
                self.blobs.push(*blob);
                changed = true;
            }
        }
        changed
    }

    /// Take documents out, with their own records; returns whether
    /// anything left.
    pub fn remove_docs(&mut self, docs: &[DocId]) -> bool {
        let before = (self.docs.len(), self.blobs.len());
        self.docs.retain(|d| !docs.contains(d));
        self.blobs
            .retain(|b| !(b.kind == BlobKind::Item && docs.iter().any(|d| d.0 == b.id)));
        before != (self.docs.len(), self.blobs.len())
    }

    /// The folders travelling with a shared folder, the root included.
    pub fn folders(&self) -> impl Iterator<Item = [u8; 16]> + '_ {
        self.blobs
            .iter()
            .filter(|b| b.kind == BlobKind::Folder)
            .map(|b| b.id)
    }

    /// Stop carrying folders; returns whether any left.
    pub fn remove_folders(&mut self, folders: &[[u8; 16]]) -> bool {
        let before = self.blobs.len();
        self.blobs
            .retain(|b| !(b.kind == BlobKind::Folder && folders.contains(&b.id)));
        before != self.blobs.len()
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
            role: self.entry_role,
            key: self.key.clone(),
            server: self.server.clone(),
        }
    }

    /// What a member sees after joining: the share minus the owner's
    /// bookkeeping. The key stays, since the member already holds it
    /// from the invitation.
    #[must_use]
    pub fn for_member(&self) -> Self {
        Self {
            relay_cursor: 0,
            expelled: Vec::new(),
            departed: Vec::new(),
            pending: Vec::new(),
            ..self.clone()
        }
    }

    /// What the owner keeps of this share once it is closed.
    #[must_use]
    pub fn ended(&self, now: u64) -> EndedShare {
        EndedShare {
            id: self.id,
            key: self.key.clone(),
            members: self.members.iter().map(|m| m.device).collect(),
            title: self.title.clone(),
            mode: self.mode,
            docs: self.docs.clone(),
            blobs: self.blobs.clone(),
            folder: self.folder,
            desk: self.desk,
            ended_at: now,
        }
    }

    /// What a member keeps after walking away, so it can come back.
    #[must_use]
    pub fn left(&self, server: Option<String>, now: u64) -> LeftShare {
        LeftShare {
            invite: Invite {
                server,
                ..self.invite()
            },
            title: self.title.clone(),
            docs: self.docs.clone(),
            left_at: now,
        }
    }
}

/// What travels in an invitation link or QR.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct Invite {
    pub share: ShareId,
    pub owner: Member,
    pub title: String,
    pub mode: ShareMode,
    /// The role the link grants on entry.
    pub role: Role,
    pub key: ShareKey,
    /// Relay to meet at when no direct path exists.
    pub server: Option<String>,
}

impl Invite {
    /// `notein://join?share=…&owner=…&name=…&sign=…&title=…&mode=…&role=…[&server=…]#<key>`.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::share::to_link"))]
    pub fn to_link(&self) -> String {
        let mode = match self.mode {
            ShareMode::Session => "session",
            ShareMode::Permanent => "permanent",
        };
        let mut link = format!(
            "{LINK_SCHEME}share={}&owner={}&name={}&sign={}&title={}&mode={mode}&role={}",
            hex::encode(self.share.0),
            hex::encode(self.owner.public),
            hex::encode(self.owner.name.as_bytes()),
            hex::encode(self.owner.sign),
            hex::encode(self.title.as_bytes()),
            self.role.tag(),
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
    #[cfg_attr(feature = "sismo", sismo::function("sync::share::parse_link"))]
    pub fn parse_link(link: &str) -> Option<Self> {
        let rest = link.trim().strip_prefix(LINK_SCHEME)?;
        let (query, fragment) = rest.split_once('#')?;
        let key = ShareKey::from_hex(fragment).ok()?;
        let mut share = None;
        let mut public = None;
        let mut name = None;
        let mut sign = None;
        let mut title = None;
        let mut mode = None;
        let mut role = Role::Editor;
        let mut server = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=')?;
            match k {
                "share" => share = Some(ShareId(<[u8; 16]>::try_from(hex::decode(v).ok()?).ok()?)),
                "owner" => public = Some(<[u8; 32]>::try_from(hex::decode(v).ok()?).ok()?),
                "name" => name = Some(String::from_utf8(hex::decode(v).ok()?).ok()?),
                "sign" => sign = Some(<[u8; 32]>::try_from(hex::decode(v).ok()?).ok()?),
                "title" => title = Some(String::from_utf8(hex::decode(v).ok()?).ok()?),
                "mode" => {
                    mode = Some(match v {
                        "session" => ShareMode::Session,
                        "permanent" => ShareMode::Permanent,
                        _ => return None,
                    });
                }
                "role" => role = Role::parse(v)?.for_member(),
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
                sign: sign?,
                name: name?,
                device_name: String::new(),
                role: Role::Owner,
                joined_at: 0,
            },
            title: title?,
            mode: mode?,
            role,
            key,
            server,
        })
    }

    /// Proof that the device holding `joiner` (its public key) holds the
    /// share key: the share id and that key sealed with it. Bound to the
    /// joiner so whoever sees it on the way cannot knock with it.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::share::proof"))]
    pub fn proof(&self, joiner: &[u8; 32]) -> Vec<u8> {
        self.key
            .seal(&self.share.proof_plain(joiner))
            .unwrap_or_default()
    }
}

impl ShareId {
    fn proof_plain(self, joiner: &[u8; 32]) -> Vec<u8> {
        [self.0.as_slice(), joiner.as_slice()].concat()
    }
}

/// Check a joiner's proof against the share's key and the public key of
/// the device that sent it. One that left on its own may prove it with
/// the key of its old invitation.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("sync::share::proof_is_valid"))]
pub fn proof_is_valid(share: &Share, proof: &[u8], joiner: &[u8; 32]) -> bool {
    proof_key(share, proof, joiner).is_some()
}

/// The key a valid proof was made with, which the answer is sealed with
/// so the joiner can read it.
#[must_use]
pub fn proof_key<'a>(share: &'a Share, proof: &[u8], joiner: &[u8; 32]) -> Option<&'a ShareKey> {
    let departed = share.departed.contains(&DeviceId::of_public_key(joiner));
    let expected = share.id.proof_plain(joiner);
    std::iter::once(&share.key)
        .chain(share.previous.iter().filter(|_| departed))
        .find(|key| {
            key.open(proof).is_ok_and(|plain| {
                subtle::ConstantTimeEq::ct_eq(plain.as_slice(), expected.as_slice()).into()
            })
        })
}

/// A share its owner closed, kept so a member that was away is told
/// when it comes back instead of editing into the void, and so the
/// owner can open it again.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct EndedShare {
    pub id: ShareId,
    pub key: ShareKey,
    /// Who was inside when it closed.
    pub members: Vec<DeviceId>,
    pub title: String,
    pub mode: ShareMode,
    pub docs: Vec<DocId>,
    pub blobs: Vec<BlobKey>,
    pub folder: Option<[u8; 16]>,
    pub desk: bool,
    pub ended_at: u64,
}

/// A share this device walked away from, kept so it can ask back in
/// with the same invitation while the owner keeps the share open.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct LeftShare {
    pub invite: Invite,
    pub title: String,
    pub docs: Vec<DocId>,
    pub left_at: u64,
}

/// Every share a device takes part in, as persisted.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug, Default)]
pub struct ShareBook {
    pub shares: Vec<Share>,
    /// Shares this device owned and closed.
    pub ended: Vec<EndedShare>,
    /// Shares this device walked away from.
    pub left: Vec<LeftShare>,
    /// Invitations accepted whose owner has not let us in yet.
    pub joins: Vec<Invite>,
}

impl ShareBook {
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::share_book::encode"))]
    pub fn encode(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::share_book::decode"))]
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

/// A change to a share that someone other than the owner asks for, or
/// the owner applies. The owner checks the asker's role before applying.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum ShareEdit {
    Rename(String),
    AddDocs {
        docs: Vec<DocId>,
        blobs: Vec<BlobKey>,
    },
    RemoveDocs(Vec<DocId>),
    /// Stop carrying subfolders of a shared folder.
    RemoveFolders(Vec<[u8; 16]>),
    SetRole {
        device: DeviceId,
        role: Role,
    },
    Expel(DeviceId),
    /// Let someone waiting in, with this role.
    Approve {
        device: DeviceId,
        role: Role,
    },
    /// Turn someone waiting away.
    Refuse(DeviceId),
    SetEntryRole(Role),
    SetApproveEntry(bool),
    SetEditorsAddDocs(bool),
}

/// What members exchange inside a share, always sealed with its key.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum ShareMessage {
    /// An exchange message for the share's documents.
    Sync(Vec<u8>),
    /// The owner's current view of the share (roles, members, scope).
    Meta(Box<Share>),
    /// The owner closed the share.
    Ended,
    /// A member walked away.
    Left,
    /// Ephemeral state of one of the share's documents, see
    /// `SyncDriver::live`.
    Live { doc: DocId, bytes: Vec<u8> },
    /// The owner threw this member out.
    Expelled,
    /// A member asks the owner for a change.
    Edit(ShareEdit),
}

impl ShareMessage {
    /// Seal for the wire.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::share::seal"))]
    pub fn seal(&self, key: &ShareKey) -> Vec<u8> {
        let plain = bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default();
        key.seal(&plain).unwrap_or_default()
    }

    /// Open from the wire.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::share::open"))]
    pub fn open(key: &ShareKey, sealed: &[u8]) -> Option<Self> {
        let plain = key.open(sealed).ok()?;
        bincode::decode_from_slice(&plain, bincode::config::standard())
            .ok()
            .map(|(message, _)| message)
    }

    /// [`Self::open`] with whichever of the share's keys sealed it.
    #[must_use]
    pub fn open_in(share: &Share, sealed: &[u8]) -> Option<Self> {
        Self::open(share.key_opening(sealed)?, sealed)
    }

    /// Seal for a relay mailbox of `share`, signed by `author`. Anyone
    /// holding the share key can seal, and the relay says who sent a
    /// record; only the signature tells who wrote it.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::share::seal_signed"))]
    pub fn seal_signed(&self, share: &Share, kind: RecordKind, author: &DeviceKey) -> Vec<u8> {
        use ed25519_dalek::Signer;
        let message = bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default();
        let signed = signed_bytes(share.id, kind, author.public(), &message);
        let record = SignedRecord {
            author: *author.public(),
            signature: author.signing().sign(&signed).to_bytes(),
            message,
        };
        let plain =
            bincode::encode_to_vec(&record, bincode::config::standard()).unwrap_or_default();
        share.key.seal(&plain).unwrap_or_default()
    }

    /// Reverse of [`Self::seal_signed`]: the message and the device that
    /// wrote it, when the author takes part in `share` and the signature
    /// holds for this share and `kind`.
    #[cfg_attr(feature = "sismo", sismo::function("sync::share::open_signed"))]
    pub fn open_signed(
        share: &Share,
        sealed: &[u8],
        kind: RecordKind,
    ) -> Result<(DeviceId, Self), Unopened> {
        // A key not held here yet may be one the owner has still to hand over.
        let plain = share.open_any(sealed).ok_or(Unopened::Stranger)?;
        let (record, _): (SignedRecord, usize) =
            bincode::decode_from_slice(&plain, bincode::config::standard())
                .map_err(|_| Unopened::Invalid)?;
        let author = share
            .participants()
            .find(|m| m.public == record.author)
            .ok_or(Unopened::Stranger)?;
        let verifying =
            ed25519_dalek::VerifyingKey::from_bytes(&author.sign).map_err(|_| Unopened::Invalid)?;
        let signature = ed25519_dalek::Signature::from_bytes(&record.signature);
        let signed = signed_bytes(share.id, kind, &record.author, &record.message);
        verifying
            .verify_strict(&signed, &signature)
            .map_err(|_| Unopened::Invalid)?;
        let (message, _) = bincode::decode_from_slice(&record.message, bincode::config::standard())
            .map_err(|_| Unopened::Invalid)?;
        Ok((author.device, message))
    }
}

/// Why a mailbox record did not open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unopened {
    /// Sealed for the share but by someone this view of it does not list
    /// yet: a newer view may.
    Stranger,
    /// Not sealed for the share, not signed, or signed for something else.
    Invalid,
}

/// The share's keys from `epoch`, newest first, for one member alone.
///
/// Sealed for the member whose transport key is `member` under what only
/// the owner and that member can work out, so the relay and everyone
/// else, an expelled member included, see nothing. `None` when `member`
/// is not a key one can agree a secret with.
#[must_use]
pub fn seal_keys(
    owner: &DeviceKey,
    member: &[u8; 32],
    share: ShareId,
    epoch: u32,
    keys: &[ShareKey],
) -> Option<Vec<u8>> {
    let plain = bincode::encode_to_vec(keys, bincode::config::standard()).ok()?;
    let boxed = key_box(&owner.agree(member)?, share, epoch)
        .seal(&plain)
        .ok()?;
    Some([epoch.to_be_bytes().as_slice(), &boxed].concat())
}

/// Reverse of [`seal_keys`] on the member's side: the epoch and keys.
#[must_use]
pub fn open_keys(
    me: &DeviceKey,
    owner: &[u8; 32],
    share: ShareId,
    sealed: &[u8],
) -> Option<(u32, Vec<ShareKey>)> {
    let (epoch, boxed) = sealed.split_first_chunk::<4>()?;
    let epoch = u32::from_be_bytes(*epoch);
    let plain = key_box(&me.agree(owner)?, share, epoch).open(boxed).ok()?;
    let (keys, _): (Vec<ShareKey>, usize) =
        bincode::decode_from_slice(&plain, bincode::config::standard()).ok()?;
    Some((epoch, keys))
}

fn key_box(shared: &[u8; 32], share: ShareId, epoch: u32) -> ShareKey {
    use blake2::{Blake2s256, Digest};
    let key: [u8; 32] = Blake2s256::new()
        .chain_update(b"notin-share-keys")
        .chain_update(shared)
        .chain_update(share.0)
        .chain_update(epoch.to_be_bytes())
        .finalize()
        .into();
    ShareKey::from(key)
}

/// A [`ShareMessage`] with the signature of the member that wrote it.
#[derive(bincode::Encode, bincode::Decode)]
struct SignedRecord {
    author: [u8; 32],
    signature: [u8; 64],
    message: Vec<u8>,
}

/// What a mailbox signature covers: the share and the record kind too,
/// so a relay can neither move a record to another share nor pass it
/// off as another kind.
fn signed_bytes(share: ShareId, kind: RecordKind, author: &[u8; 32], message: &[u8]) -> Vec<u8> {
    let kind = bincode::encode_to_vec(kind, bincode::config::standard()).unwrap_or_default();
    [
        b"notin-record".as_slice(),
        share.0.as_slice(),
        &kind,
        author.as_slice(),
        message,
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::DeviceKey;

    fn person(name: &str, role: Role) -> Member {
        let key = DeviceKey::generate();
        Member {
            device: key.id(),
            public: *key.public(),
            sign: key.sign_public(),
            name: name.into(),
            device_name: "Tablet".into(),
            role,
            joined_at: 1,
        }
    }

    fn owner() -> Member {
        person("Sergio", Role::Owner)
    }

    #[test]
    fn invite_link_round_trips() {
        let mut share = Share::new(owner(), "Apuntes de física", ShareMode::Permanent, 7);
        share.docs.push(DocId([3; 16]));
        share.entry_role = Role::Viewer;
        let invite = share.invite();
        let parsed = Invite::parse_link(&invite.to_link()).unwrap();
        assert_eq!(parsed.role, Role::Viewer);
        assert_eq!(
            parsed,
            Invite {
                owner: Member {
                    device_name: String::new(),
                    ..invite.owner.clone()
                },
                ..invite
            }
        );
        let joiner = [9; 32];
        assert!(proof_is_valid(&share, &parsed.proof(&joiner), &joiner));
        assert!(!proof_is_valid(
            &Share::new(owner(), "otro", ShareMode::Session, 1),
            &parsed.proof(&joiner),
            &joiner
        ));
        // A proof seen on the way does not let another device in.
        assert!(!proof_is_valid(&share, &parsed.proof(&joiner), &[8; 32]));
    }

    #[test]
    fn roles_decide_what_members_may_do() {
        let mut share = Share::new(owner(), "x", ShareMode::Session, 1);
        let me = share.owner.device;
        let guest = person("Ana", Role::Editor);
        let viewer = person("Juan", Role::Viewer);
        let co = person("Lucía", Role::Owner);
        assert!(!share.can_edit(guest.device));
        assert!(share.admit(guest.clone()));
        assert!(!share.admit(guest.clone()));
        assert!(share.admit(viewer.clone()));
        assert!(share.admit(co.clone()));
        assert_eq!(share.role_of(co.device), Some(Role::CoOrganizer), "clamped");
        assert!(share.can_edit(guest.device));
        assert!(share.can_rename(guest.device));
        assert!(!share.can_add_docs(guest.device));
        share.editors_add_docs = true;
        assert!(share.can_add_docs(guest.device));
        assert!(!share.can_delete(guest.device));
        assert!(share.is_read_only_for(viewer.device));
        assert!(share.can_manage(co.device));
        assert!(share.outranks(co.device, guest.device));
        assert!(!share.outranks(guest.device, viewer.device));
        assert!(!share.outranks(co.device, me));
        assert!(share.outranks(me, co.device));
        assert!(share.set_role(viewer.device, Role::Editor));
        assert!(!share.set_role(viewer.device, Role::Editor));
        assert!(share.can_edit(viewer.device));
        assert!(share.expel(guest.device));
        assert!(!share.is_member(guest.device));
        assert!(share.was_expelled(guest.device));
    }

    #[test]
    fn knocking_waits_until_answered() {
        let mut share = Share::new(owner(), "x", ShareMode::Permanent, 1);
        assert!(share.approve_entry, "permanent shares ask first");
        let ana = person("Ana", Role::Editor);
        assert!(share.knock(ana.clone()));
        assert!(!share.knock(ana.clone()));
        assert!(!share.is_member(ana.device));
        let waiting = share.answer(ana.device).unwrap();
        assert_eq!(waiting.device, ana.device);
        assert!(share.answer(ana.device).is_none());
        assert!(share.admit(ana.clone()));
        assert!(!share.knock(ana), "members never wait");
        assert!(share.for_member().pending.is_empty());
    }

    #[test]
    fn documents_come_and_go_with_their_records() {
        let mut share = Share::new(owner(), "x", ShareMode::Session, 1);
        let doc = DocId([4; 16]);
        let item = BlobKey {
            kind: BlobKind::Item,
            id: [4; 16],
        };
        assert!(share.add_docs(&[doc], &[item]));
        assert!(!share.add_docs(&[doc], &[item]));
        assert!(share.remove_docs(&[doc]));
        assert!(share.docs.is_empty() && share.blobs.is_empty());
        assert!(!share.remove_docs(&[doc]));
        let sub = BlobKey {
            kind: BlobKind::Folder,
            id: [5; 16],
        };
        assert!(share.add_docs(&[], &[sub]));
        assert_eq!(share.folders().collect::<Vec<_>>(), vec![[5; 16]]);
        assert!(share.remove_folders(&[[5; 16]]));
        assert!(share.folders().next().is_none());
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
        book.left.push(share.left(Some("relay:1/k".into()), 5));
        book.ended.push(share.ended(6));
        assert_eq!(book.shares.len(), 1);
        let back = ShareBook::decode(&book.encode()).unwrap();
        assert_eq!(back, book);
        assert_eq!(back.holding(DocId([9; 16])).count(), 1);
        assert_eq!(back.holding(DocId([8; 16])).count(), 0);
        assert_eq!(back.left[0].invite.server.as_deref(), Some("relay:1/k"));
        assert_eq!(back.ended[0].title, "x");
    }

    fn member_of(key: &DeviceKey, role: Role) -> Member {
        Member {
            device: key.id(),
            public: *key.public(),
            sign: key.sign_public(),
            name: "x".into(),
            device_name: String::new(),
            role,
            joined_at: 1,
        }
    }

    #[test]
    fn mailbox_records_name_their_real_author() {
        let owner_key = DeviceKey::generate();
        let member_key = DeviceKey::generate();
        let mut share = Share::new(
            member_of(&owner_key, Role::Owner),
            "x",
            ShareMode::Permanent,
            1,
        );
        share.admit(member_of(&member_key, Role::Editor));
        let kind = RecordKind::Meta;
        let sealed = ShareMessage::Ended.seal_signed(&share, kind, &member_key);
        let (author, message) = ShareMessage::open_signed(&share, &sealed, kind).unwrap();
        assert_eq!(author, member_key.id());
        assert_eq!(message, ShareMessage::Ended);
        // Not passed off as another kind, nor moved to another share.
        assert!(
            ShareMessage::open_signed(
                &share,
                &sealed,
                RecordKind::Update {
                    doc: DocId([1; 16])
                }
            )
            .is_err()
        );
        let other = Share {
            id: ShareId([9; 16]),
            ..share.clone()
        };
        assert!(ShareMessage::open_signed(&other, &sealed, kind).is_err());
        // Sealing with the key alone, unsigned, or by an outsider, counts for nothing.
        assert!(
            ShareMessage::open_signed(&share, &ShareMessage::Ended.seal(&share.key), kind).is_err()
        );
        let outsider = DeviceKey::generate();
        let sealed = ShareMessage::Ended.seal_signed(&share, kind, &outsider);
        assert!(matches!(
            ShareMessage::open_signed(&share, &sealed, kind),
            Err(Unopened::Stranger)
        ));
    }

    #[test]
    fn new_keys_open_for_their_member_alone() {
        let owner = DeviceKey::generate();
        let member = DeviceKey::generate();
        let other = DeviceKey::generate();
        let share = ShareId([1; 16]);
        let keys = vec![ShareKey::generate(), ShareKey::generate()];
        let sealed = seal_keys(&owner, member.public(), share, 3, &keys).unwrap();
        assert_eq!(
            open_keys(&member, owner.public(), share, &sealed),
            Some((3, keys))
        );
        assert!(open_keys(&other, owner.public(), share, &sealed).is_none());
        assert!(open_keys(&member, other.public(), share, &sealed).is_none());
        assert!(open_keys(&member, owner.public(), ShareId([2; 16]), &sealed).is_none());
    }

    #[test]
    fn a_share_opens_what_any_of_its_keys_sealed() {
        let key = DeviceKey::generate();
        let mut share = Share::new(member_of(&key, Role::Owner), "x", ShareMode::Permanent, 1);
        let before = ShareMessage::Ended.seal(&share.key);
        share.rotate(ShareKey::generate());
        let after = ShareMessage::Ended.seal(&share.key);
        assert_eq!(share.epoch, 1);
        assert_eq!(
            ShareMessage::open_in(&share, &before),
            Some(ShareMessage::Ended)
        );
        assert_eq!(
            ShareMessage::open_in(&share, &after),
            Some(ShareMessage::Ended)
        );
        // A member holding only the old key opens the old, never the new.
        let mut old = share.clone();
        old.key = share.previous[0].clone();
        old.previous.clear();
        old.epoch = 0;
        assert!(ShareMessage::open_in(&old, &after).is_none());
        assert!(
            !old.adopt_keys(0, vec![share.key.clone()]),
            "an older epoch is ignored"
        );
        assert!(old.adopt_keys(1, vec![share.key.clone()]));
        assert_eq!(
            ShareMessage::open_in(&old, &after),
            Some(ShareMessage::Ended)
        );
        assert_eq!(
            ShareMessage::open_in(&old, &before),
            Some(ShareMessage::Ended)
        );
    }
}
