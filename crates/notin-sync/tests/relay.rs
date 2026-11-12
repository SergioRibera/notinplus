//! Devices that never see each other on the LAN meet through a relay:
//! a guest joins with the invitation, trusted devices find each other,
//! a pairing link pairs from afar. Everything rides end-to-end
//! channels the relay carries but cannot read.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::MemReplica;
use notin_sync::driver::DriverEvent;
use notin_sync::{
    Action, DeviceKey, DeviceProfile, DocId, Invite, LanConfig, LanNode, NetEvent, PairingOffer,
    Path, RelayAddr, RelayConfig, RelayLimits, RelayServer, RelayStore, Share, ShareBook,
    ShareMode, SyncDriver, TrustedDevice,
};
/// A device: its transport plus its driver, pumped by hand.
struct Device {
    node: LanNode,
    driver: SyncDriver<MemReplica>,
    events: Vec<DriverEvent>,
}

impl Device {
    fn new(name: &str) -> Self {
        Self::start(DeviceProfile::generate(name), true)
    }

    /// A device whose channels through the relay never go direct.
    fn relay_only(name: &str) -> Self {
        Self::start(DeviceProfile::generate(name), false)
    }

    fn start(profile: DeviceProfile, punching: bool) -> Self {
        let node = LanNode::start(
            LanConfig::new(profile.key().clone(), profile.name())
                .port(0)
                .advertise(false)
                .scan(false)
                .punching(punching)
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
                Action::ConnectVia { relay, public } => {
                    self.node.connect_via(relay, public).unwrap();
                }
                Action::Punch { device } => self.node.punch(device).unwrap(),
                Action::Disconnect { device } => self.node.disconnect(device).unwrap(),
                Action::Bulk { device, allowed } => self.node.allow_bulk(device, allowed).unwrap(),
                Action::Event(event) => self.events.push(event),
            }
        }
    }

    /// Redial whoever it waits on, as the app does every second.
    fn supervise(&mut self) {
        let actions = self.driver.supervise(Instant::now());
        self.perform(actions);
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
        a.supervise();
        b.supervise();
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
            limits: RelayLimits::default(),
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
    share.approve_entry = false;
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

/// A temporary session between two devices that only share a relay:
/// nothing is kept there, so everything rides live links.
#[test]
fn session_runs_live_through_the_relay_only() {
    let (_server, relay_addr) = relay();
    let mut owner = Device::new("Sergio");
    let mut guest = Device::new("Ana");

    let actions = owner.driver.set_relay(Some(relay_addr));
    owner.perform(actions);
    owner.mark(D1, 1, 1);
    let mut share = Share::new(owner.driver.me(), "Clase", ShareMode::Session, 1);
    share.docs.push(D1);
    let id = share.id;
    let actions = owner.driver.create_share(share);
    owner.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(5), |o, _| {
        o.driver.relay_online()
    });
    let invite = owner.driver.invite_for(id).unwrap();
    let invite = Invite::parse_link(&invite.to_link()).unwrap();

    let actions = guest.driver.join(invite);
    guest.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(8), |_, g| {
        g.marks(D1) == vec![(1, 1)]
    });
    assert_eq!(
        guest.marks(D1),
        vec![(1, 1)],
        "content made before the guest arrived never reached it: {:?}",
        guest.events
    );

    for round in 2..5 {
        owner.mark(D1, 1, round);
        let actions = owner.driver.push_doc(D1).unwrap();
        owner.perform(actions);
        guest.mark(D1, 2, round);
        let actions = guest.driver.push_doc(D1).unwrap();
        guest.perform(actions);
    }
    let both = |d: &Device| d.marks(D1).len() == 7;
    settle(&mut owner, &mut guest, Duration::from_secs(8), |o, g| {
        both(o) && both(g)
    });
    assert_eq!(owner.marks(D1), guest.marks(D1));
    assert_eq!(owner.marks(D1).len(), 7);

    // Ephemera (ink under the pen, pointers) reach the other side too.
    let actions = owner.driver.live(D1, b"ink");
    owner.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(3), |_, g| {
        g.events
            .iter()
            .any(|e| matches!(e, DriverEvent::Live { bytes, .. } if bytes == b"ink"))
    });
    assert!(
        guest
            .events
            .iter()
            .any(|e| matches!(e, DriverEvent::Live { bytes, .. } if bytes == b"ink")),
        "live state never crossed the relay"
    );
}

/// A relay keeping its mailboxes in `store`, telling `observer` what it
/// forwards between devices.
fn observed_relay(
    store: RelayStore,
    observer: notin_sync::RelayObserver,
) -> (RelayServer, RelayAddr) {
    let key = DeviceKey::generate();
    let public = *key.public();
    let server = RelayServer::start_observed(
        RelayConfig {
            key,
            name: "relay".into(),
            port: 0,
            limits: RelayLimits::default(),
        },
        store,
        observer,
    )
    .unwrap();
    let addr = RelayAddr {
        host: format!("127.0.0.1:{}", server.local_addr().port()),
        public,
    };
    (server, addr)
}

/// Both devices trust each other already.
fn trust_each_other(a: &mut Device, b: &mut Device) {
    let (pa, pb) = (
        *a.driver.profile().key().public(),
        *b.driver.profile().key().public(),
    );
    let (na, nb) = (
        a.driver.profile().name().to_owned(),
        b.driver.profile().name().to_owned(),
    );
    a.driver.profile_mut().trust(TrustedDevice::new(pb, nb, 1));
    b.driver.profile_mut().trust(TrustedDevice::new(pa, na, 1));
}

fn on_relay(device: &mut Device, addr: &RelayAddr) {
    let actions = device.driver.set_relay(Some(addr.clone()));
    device.perform(actions);
}

fn connected(a: &Device, b: &Device) -> bool {
    let (ia, ib) = (a.driver.profile().id(), b.driver.profile().id());
    a.driver.connected().any(|p| p.device == ib) && b.driver.connected().any(|p| p.device == ia)
}

fn got_live(device: &Device, bytes: &[u8]) -> bool {
    device
        .events
        .iter()
        .any(|e| matches!(e, DriverEvent::Live { bytes: b, .. } if b == bytes))
}

#[test]
fn trusted_devices_sync_and_share_live_state_only_through_the_relay() {
    let (_server, relay_addr) = relay();
    let mut a = Device::relay_only("Tablet");
    let mut b = Device::relay_only("PC");
    trust_each_other(&mut a, &mut b);
    a.mark(D1, 1, 1);
    b.mark(D1, 2, 1);
    on_relay(&mut a, &relay_addr);
    on_relay(&mut b, &relay_addr);
    settle(&mut a, &mut b, Duration::from_secs(10), connected);
    assert!(connected(&a, &b), "never met at the relay");
    let (ia, ib) = (a.driver.profile().id(), b.driver.profile().id());
    assert_eq!(a.driver.path_to(ib), Some(Path::Relay));
    assert_eq!(b.driver.path_to(ia), Some(Path::Relay));
    settle(&mut a, &mut b, Duration::from_secs(5), |a, b| {
        a.marks(D1).len() == 2 && b.marks(D1).len() == 2
    });
    assert_eq!(a.marks(D1), vec![(1, 1), (2, 1)]);
    assert_eq!(b.marks(D1), a.marks(D1));

    a.mark(D1, 1, 2);
    let actions = a.driver.push_doc(D1).unwrap();
    a.perform(actions);
    let actions = a.driver.live(D1, b"ink");
    a.perform(actions);
    settle(&mut a, &mut b, Duration::from_secs(5), |_, b| {
        b.marks(D1).contains(&(1, 2)) && got_live(b, b"ink")
    });
    assert!(b.marks(D1).contains(&(1, 2)), "the edit never crossed");
    assert!(got_live(&b, b"ink"), "live state never crossed");
    // Still through the relay: punching is off on both.
    assert_eq!(a.driver.path_to(ib), Some(Path::Relay));
}

#[test]
fn a_punched_channel_reports_a_direct_path() {
    let (_server, relay_addr) = relay();
    let mut a = Device::new("Tablet");
    let mut b = Device::new("PC");
    trust_each_other(&mut a, &mut b);
    on_relay(&mut a, &relay_addr);
    on_relay(&mut b, &relay_addr);
    let (ia, ib) = (a.driver.profile().id(), b.driver.profile().id());
    let direct = |a: &Device, b: &Device| {
        a.driver.path_to(ib) == Some(Path::Punched) && b.driver.path_to(ia) == Some(Path::Punched)
    };
    settle(&mut a, &mut b, Duration::from_secs(15), direct);
    assert!(
        direct(&a, &b),
        "never punched: {:?} / {:?}",
        a.driver.path_to(ib),
        b.driver.path_to(ia)
    );
    assert!(a.events.iter().any(|e| matches!(
        e,
        DriverEvent::PathChanged {
            path: Path::Punched,
            ..
        }
    )));
    // The same channel carries on.
    let actions = a.driver.live(D1, b"after punch");
    a.perform(actions);
    settle(&mut a, &mut b, Duration::from_secs(3), |_, b| {
        got_live(b, b"after punch")
    });
    assert!(got_live(&b, b"after punch"));
}

#[test]
fn a_pairing_link_pairs_through_the_relay() {
    let (_server, relay_addr) = relay();
    let mut offering = Device::relay_only("Tablet");
    let mut pairing = Device::relay_only("PC");
    on_relay(&mut offering, &relay_addr);
    settle(
        &mut offering,
        &mut pairing,
        Duration::from_secs(5),
        |o, _| o.driver.relay_online(),
    );
    let (offer, actions) = offering.driver.begin_pairing();
    offering.perform(actions);
    let link = offer.to_link();
    let parsed = PairingOffer::parse_link(&link).unwrap();
    assert_eq!(parsed.relay.as_ref(), Some(&relay_addr));
    assert!(parsed.secret.is_some());

    // The pairing device has nothing but the link.
    let actions = pairing.driver.pair_with(&parsed);
    pairing.perform(actions);
    let (io, ip) = (
        offering.driver.profile().id(),
        pairing.driver.profile().id(),
    );
    let paired = |o: &Device, p: &Device| {
        o.driver.profile().is_trusted(ip) && p.driver.profile().is_trusted(io)
    };
    settle(&mut offering, &mut pairing, Duration::from_secs(10), paired);
    assert!(
        paired(&offering, &pairing),
        "never paired: {:?} / {:?}",
        offering.events,
        pairing.events
    );
    assert_eq!(pairing.driver.relay(), Some(&relay_addr));
}

#[test]
fn a_wrong_code_through_the_relay_pairs_nobody() {
    let (_server, relay_addr) = relay();
    let mut offering = Device::relay_only("Tablet");
    let mut pairing = Device::relay_only("PC");
    on_relay(&mut offering, &relay_addr);
    settle(
        &mut offering,
        &mut pairing,
        Duration::from_secs(5),
        |o, _| o.driver.relay_online(),
    );
    let (offer, actions) = offering.driver.begin_pairing();
    offering.perform(actions);
    let wrong = (offer.code.value() + 1) % 1_000_000;
    let offer = PairingOffer {
        code: notin_sync::PairingCode::parse(&format!("{wrong:06}")).unwrap(),
        ..offer
    };
    let actions = pairing.driver.pair_with(&offer);
    pairing.perform(actions);
    let ip = pairing.driver.profile().id();
    settle(
        &mut offering,
        &mut pairing,
        Duration::from_secs(5),
        |_, p| {
            p.events
                .iter()
                .any(|e| matches!(e, DriverEvent::PairFailed { .. }))
        },
    );
    assert!(!offering.driver.profile().is_trusted(ip));
    assert!(
        pairing
            .events
            .iter()
            .any(|e| matches!(e, DriverEvent::PairFailed { .. })),
        "{:?}",
        pairing.events
    );
}

#[test]
fn a_stranger_without_a_knock_cannot_reach_a_device() {
    let (_server, relay_addr) = relay();
    let quiet = |key: &DeviceKey, name: &str| {
        LanNode::start(
            LanConfig::new(key.clone(), name)
                .port(0)
                .advertise(false)
                .scan(false)
                .connect_timeout(Duration::from_secs(3)),
        )
        .unwrap()
    };
    let target_key = DeviceKey::generate();
    let stranger_key = DeviceKey::generate();
    let target = quiet(&target_key, "Tablet");
    let stranger = quiet(&stranger_key, "Stranger");
    let wait_connected = |node: &LanNode| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Ok(event) = node
            .events()
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if matches!(event, NetEvent::Connected(_)) {
                return;
            }
        }
        panic!("never reached the relay");
    };
    target
        .connect_host(relay_addr.host.clone(), relay_addr.public)
        .unwrap();
    wait_connected(&target);
    stranger
        .connect_host(relay_addr.host.clone(), relay_addr.public)
        .unwrap();
    wait_connected(&stranger);

    stranger
        .connect_via(relay_addr.device(), *target_key.public())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let failed = loop {
        match stranger
            .events()
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(NetEvent::ConnectFailed { reason, .. }) => break Some(reason),
            Ok(NetEvent::Connected(peer)) => panic!("reached {peer:?}"),
            Ok(_) => {}
            Err(_) => break None,
        }
    };
    let reason = failed.expect("the dial has to fail, and fast");
    assert!(reason.contains("cannot reach"), "{reason}");
    stranger
        .send(target_key.id(), notin_sync::Lane::Data, b"hola".to_vec())
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    while let Ok(event) = target.events().try_recv() {
        assert!(
            !matches!(event, NetEvent::Connected(_) | NetEvent::Message { .. }),
            "the stranger got through: {event:?}"
        );
    }
}

#[test]
fn a_joiner_on_another_relay_joins_at_the_shares() {
    let (_ours, owner_relay) = relay();
    let (_theirs, guest_relay) = relay();
    let mut owner = Device::relay_only("Sergio");
    let mut guest = Device::relay_only("Ana");
    on_relay(&mut owner, &owner_relay);
    on_relay(&mut guest, &guest_relay);
    owner.mark(D1, 1, 1);
    let mut share = Share::new(owner.driver.me(), "Clase", ShareMode::Session, 1);
    share.docs.push(D1);
    let id = share.id;
    let actions = owner.driver.create_share(share);
    owner.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(5), |o, g| {
        o.driver.relay_online() && g.driver.relay_online()
    });
    let invite = owner.driver.invite_for(id).unwrap();
    assert_eq!(invite.server, Some(owner_relay.to_string()));

    let actions = guest.driver.join(invite);
    guest.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(10), |_, g| {
        g.marks(D1) == vec![(1, 1)]
    });
    assert!(
        guest.driver.share(id).is_some(),
        "never got in: {:?}",
        guest.events
    );
    assert_eq!(guest.marks(D1), vec![(1, 1)]);
    // Its own relay stays its own.
    assert_eq!(guest.driver.relay(), Some(&guest_relay));
    assert_eq!(
        guest.driver.path_to(owner.driver.profile().id()),
        Some(Path::Relay)
    );
}

#[test]
fn the_relay_never_sees_plaintext() {
    const TITLE: &str = "title-marker-7f3a";
    const INK: &[u8] = b"ink-marker-91c2";
    const NAME: &str = "name-marker-55d0";
    let dir = std::env::temp_dir().join(format!("notin-relay-plain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let observer = {
        let seen = Arc::clone(&seen);
        Arc::new(move |_: &[u8; 32], _: &[u8; 32], inner: &[u8]| {
            seen.lock().unwrap().push(inner.to_vec());
        })
    };
    let (_server, relay_addr) = observed_relay(RelayStore::on_disk(dir.clone()).unwrap(), observer);
    let mut owner = Device::relay_only(NAME);
    let mut guest = Device::relay_only("Ana");
    on_relay(&mut owner, &relay_addr);
    owner.mark(D1, 1, 1);
    let mut share = Share::new(owner.driver.me(), TITLE, ShareMode::Permanent, 1);
    share.approve_entry = false;
    share.docs.push(D1);
    let id = share.id;
    let actions = owner.driver.create_share(share);
    owner.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(5), |o, _| {
        o.driver.relay_online()
    });
    let invite = owner.driver.invite_for(id).unwrap();
    let actions = guest.driver.join(invite);
    guest.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(10), |_, g| {
        g.marks(D1) == vec![(1, 1)]
    });
    assert!(guest.driver.share(id).is_some_and(|s| s.title == TITLE));
    let actions = owner.driver.live(D1, INK);
    owner.perform(actions);
    settle(&mut owner, &mut guest, Duration::from_secs(3), |_, g| {
        got_live(g, INK)
    });
    assert!(got_live(&guest, INK));
    let actions = owner.driver.announce().unwrap();
    owner.perform(actions);
    settle(
        &mut owner,
        &mut guest,
        Duration::from_millis(500),
        |_, _| false,
    );

    let contains =
        |haystack: &[u8], needle: &[u8]| haystack.windows(needle.len()).any(|w| w == needle);
    let forwarded = seen.lock().unwrap().clone();
    assert!(!forwarded.is_empty(), "nothing went through the relay");
    for frame in &forwarded {
        for marker in [TITLE.as_bytes(), INK, NAME.as_bytes()] {
            assert!(!contains(frame, marker), "the relay read {marker:?}");
        }
    }
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        files += 1;
        for marker in [TITLE.as_bytes(), INK, NAME.as_bytes()] {
            assert!(!contains(&bytes, marker), "the relay stored {marker:?}");
        }
    }
    assert!(files > 0, "the permanent share kept no mailbox");
    std::fs::remove_dir_all(dir).unwrap();
}
