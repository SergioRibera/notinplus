//! Two devices that never see each other on the LAN meet through a
//! relay: the guest joins with the invitation, both publish, both
//! receive.

mod common;

use std::time::{Duration, Instant};

use common::MemReplica;
use notein_sync::driver::DriverEvent;
use notein_sync::{
    Action, DeviceKey, DeviceProfile, DocId, Invite, LanConfig, LanNode, RelayAddr, RelayConfig,
    RelayServer, RelayStore, Share, ShareBook, ShareMode, SyncDriver,
};
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
            driver: SyncDriver::new(profile, MemReplica::default(), ShareBook::default()),
            events: Vec::new(),
        }
    }

    fn perform(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send {
                    device,
                    lane,
                    bytes,
                } => self.node.send(device, lane, bytes).unwrap(),
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

    fn mark(&self, doc: DocId, author: u64, counter: u64) {
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
    let book = guest.driver.book();
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
        driver: SyncDriver::new(profile, MemReplica::default(), book),
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
