//! Two devices that never see each other on the LAN meet through a
//! relay: the guest joins with the invitation, both publish, both
//! receive.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::time::{Duration, Instant};

use notein_sync::driver::DriverEvent;
use notein_sync::{
    Action, Blob, BlobHead, BlobKey, DeviceKey, DeviceProfile, DocId, Invite, LanConfig, LanNode,
    NetEvent, RelayAddr, RelayConfig, RelayServer, RelayStore, Replica, Share, ShareMode,
    SyncDriver, Version,
};

/// Toy mergeable document: a set of `(author, counter)` marks.
#[derive(Default, Debug)]
struct MemReplica {
    docs: BTreeMap<DocId, BTreeSet<(u64, u64)>>,
    blobs: BTreeMap<BlobKey, Blob>,
}

fn encode<T: bincode::Encode>(value: T) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).unwrap()
}

fn decode<T: bincode::Decode<()>>(bytes: &[u8]) -> T {
    bincode::decode_from_slice(bytes, bincode::config::standard())
        .unwrap()
        .0
}

impl Replica for MemReplica {
    type Error = Infallible;

    fn docs(&self) -> Vec<(DocId, Version)> {
        self.docs
            .keys()
            .map(|doc| (*doc, self.doc_version(*doc).unwrap()))
            .collect()
    }

    fn doc_version(&self, doc: DocId) -> Option<Version> {
        self.docs
            .get(&doc)
            .map(|marks| Version(encode(marks.iter().copied().collect::<Vec<_>>())))
    }

    fn doc_updates_since(&self, doc: DocId, since: &Version) -> Result<Vec<u8>, Infallible> {
        let seen: BTreeSet<(u64, u64)> = decode::<Vec<(u64, u64)>>(&since.0).into_iter().collect();
        let missing: Vec<(u64, u64)> = self.docs[&doc].difference(&seen).copied().collect();
        Ok(if missing.is_empty() {
            Vec::new()
        } else {
            encode(missing)
        })
    }

    fn doc_snapshot(&self, doc: DocId) -> Result<Vec<u8>, Infallible> {
        Ok(encode(self.docs[&doc].iter().copied().collect::<Vec<_>>()))
    }

    fn doc_import(&mut self, doc: DocId, bytes: &[u8]) -> Result<(), Infallible> {
        let marks: Vec<(u64, u64)> = decode(bytes);
        self.docs.entry(doc).or_default().extend(marks);
        Ok(())
    }

    fn blob_heads(&self) -> Vec<BlobHead> {
        self.blobs.values().map(|b| b.head).collect()
    }

    fn blobs(&self, keys: &[BlobKey]) -> Vec<Blob> {
        keys.iter()
            .filter_map(|k| self.blobs.get(k).cloned())
            .collect()
    }

    fn blob_merge(&mut self, blob: Blob) -> Result<bool, Infallible> {
        let key = blob.head.key();
        let newer = self
            .blobs
            .get(&key)
            .is_none_or(|held| held.head.updated_at < blob.head.updated_at);
        if newer {
            self.blobs.insert(key, blob);
        }
        Ok(newer)
    }
}

/// A device: its transport plus its driver, pumped by hand.
struct Device {
    node: LanNode,
    driver: SyncDriver<MemReplica>,
    events: Vec<DriverEvent>,
}

impl Device {
    fn new(name: &str) -> Self {
        let profile = DeviceProfile::generate(name);
        let node = LanNode::start(
            LanConfig::new(profile.key().clone(), name)
                .port(0)
                .advertise(false)
                .scan(false)
                .liveness(Duration::from_millis(200), Duration::from_secs(3))
                .connect_timeout(Duration::from_secs(3)),
        )
        .unwrap();
        Self {
            node,
            driver: SyncDriver::new(profile, MemReplica::default(), Vec::new()),
            events: Vec::new(),
        }
    }

    fn perform(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send { device, bytes } => self.node.send(device, bytes).unwrap(),
                Action::Connect { addr, public } => self.node.connect(addr, public).unwrap(),
                Action::ConnectHost { host, public } => {
                    self.node.connect_host(host, public).unwrap();
                }
                Action::Event(event) => self.events.push(event),
            }
        }
    }

    /// Feed pending transport events to the driver.
    fn pump(&mut self) {
        while let Ok(event) = self.node.events().try_recv() {
            let actions = self.driver.on_event(event).unwrap();
            self.perform(actions);
        }
    }

    fn marks(&self, doc: DocId) -> Vec<(u64, u64)> {
        self.driver
            .with_replica(|r| r.docs.get(&doc).map(|m| m.iter().copied().collect()))
            .unwrap_or_default()
    }

    fn mark(&mut self, doc: DocId, author: u64, counter: u64) {
        self.driver
            .with_replica(|r| r.docs.entry(doc).or_default().insert((author, counter)));
    }
}

/// Pump both devices until `done` holds or `wait` elapses.
fn settle(a: &mut Device, b: &mut Device, wait: Duration, done: impl Fn(&Device, &Device) -> bool) {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        a.pump();
        b.pump();
        if done(a, b) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

const D1: DocId = DocId([1; 16]);

fn relay() -> (RelayServer, RelayAddr) {
    let key = DeviceKey::generate();
    let public = *key.public();
    let server = RelayServer::start(
        RelayConfig {
            key,
            name: "relay".into(),
            port: 0,
        },
        RelayStore::memory(),
    )
    .unwrap();
    let addr = RelayAddr {
        host: format!("127.0.0.1:{}", server.local_addr().port()),
        public,
    };
    (server, addr)
}

#[test]
fn guest_joins_and_syncs_through_the_relay_only() {
    let (_server, relay_addr) = relay();
    let mut owner = Device::new("Sergio");
    let mut guest = Device::new("Ana");

    // Owner meets the relay and shares a document there.
    let actions = owner.driver.set_relay(Some(relay_addr));
    owner.perform(actions);
    owner.mark(D1, 1, 1);
    let mut share = Share::new(owner.driver.me(), "Física", ShareMode::Permanent, 1);
    share.docs.push(D1);
    let id = share.id;
    let actions = owner.driver.create_share(share);
    owner.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(5), |o, _| {
        o.driver.relay_online()
    });
    assert!(owner.driver.relay_online(), "owner never reached the relay");
    let invite = owner.driver.invite_for(id).unwrap();
    assert!(invite.server.is_some());
    let invite = Invite::parse_link(&invite.to_link()).unwrap();

    // The guest only has the invitation; it adopts the relay from it.
    let actions = guest.driver.join(invite);
    guest.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(8), |_, g| {
        g.driver.share(id).is_some() && g.marks(D1) == vec![(1, 1)]
    });
    assert!(
        guest.driver.share(id).is_some(),
        "guest never got in: {:?}",
        guest.events
    );
    assert_eq!(guest.marks(D1), vec![(1, 1)]);
    assert_eq!(owner.driver.share(id).unwrap().members.len(), 1);

    // Guest edits travel back through the mailbox.
    guest.mark(D1, 2, 1);
    let actions = guest.driver.announce().unwrap();
    guest.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(5), |o, _| {
        o.marks(D1) == vec![(1, 1), (2, 1)]
    });
    assert_eq!(owner.marks(D1), vec![(1, 1), (2, 1)]);

    // Owner edits while the guest is away land when the guest returns.
    owner.mark(D1, 1, 2);
    let actions = owner.driver.announce().unwrap();
    owner.perform(actions);
    std::thread::sleep(Duration::from_millis(300));
    owner.pump();
    let cursor_before = guest.driver.share(id).unwrap().relay_cursor;
    let shares: Vec<Share> = guest.driver.shares().cloned().collect();
    let profile = guest.driver.profile().clone();
    let relay_addr = guest.driver.relay().cloned();
    drop(guest);
    let mut returning = Device {
        node: LanNode::start(
            LanConfig::new(profile.key().clone(), "Ana")
                .port(0)
                .advertise(false)
                .scan(false),
        )
        .unwrap(),
        driver: SyncDriver::new(profile, MemReplica::default(), shares),
        events: Vec::new(),
    };
    let actions = returning.driver.set_relay(relay_addr);
    returning.perform(actions);
    settle(
        &mut owner,
        &mut returning,
        Duration::from_secs(8),
        |_, g| g.marks(D1).contains(&(1, 2)),
    );
    assert!(
        returning.marks(D1).contains(&(1, 2)),
        "backlog never arrived: {:?}",
        returning.events
    );
    assert!(returning.driver.share(id).unwrap().relay_cursor > cursor_before);
}
