//! Two trusted devices on one network find each other again after one
//! of them leaves: away in the background, or restarted.

mod common;

use std::net::{Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

use common::MemReplica;
use notein_sync::{Action, DeviceProfile, DocId, LanConfig, LanNode, SyncDriver, TrustedDevice};

const D1: DocId = DocId([1; 16]);

fn free_udp_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A device on the loopback network: transport and driver, pumped by
/// hand the way the app's sync service does it.
struct Device {
    profile: DeviceProfile,
    port: u16,
    discovery_port: u16,
    node: LanNode,
    driver: SyncDriver<MemReplica>,
}

impl Device {
    fn start(profile: DeviceProfile, port: u16, discovery_port: u16) -> Self {
        let node = LanNode::start(
            LanConfig::new(profile.key().clone(), profile.name())
                .port(port)
                .discovery_port(discovery_port)
                .broadcast(Ipv4Addr::new(127, 255, 255, 255))
                .probe_interval(Duration::from_millis(200))
                .liveness(Duration::from_millis(200), Duration::from_secs(2))
                .connect_timeout(Duration::from_secs(2)),
        )
        .unwrap();
        Self {
            driver: SyncDriver::new(profile.clone(), MemReplica::default(), Vec::new()),
            profile,
            port,
            discovery_port,
            node,
        }
    }

    /// The same device after its app was closed and opened again.
    fn restart(self) -> Self {
        let Self {
            profile,
            port,
            discovery_port,
            node,
            driver,
        } = self;
        let docs = driver.with_replica(|r| std::mem::take(&mut r.docs));
        drop(node);
        let restarted = Self::start(profile, port, discovery_port);
        restarted.driver.with_replica(|r| r.docs = docs);
        restarted
    }

    fn perform(&self, actions: Vec<Action>) {
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
                Action::Event(_) => {}
            }
        }
    }

    fn pump(&mut self) {
        while let Ok(event) = self.node.events().try_recv() {
            let actions = self.driver.on_event(event).unwrap();
            self.perform(actions);
        }
        let actions = self.driver.supervise(Instant::now());
        self.perform(actions);
    }

    fn online_with(&self, other: &Self) -> bool {
        self.driver.online().any(|p| p.device == other.profile.id())
    }

    fn marks(&self) -> Vec<(u64, u64)> {
        self.driver
            .with_replica(|r| r.docs.get(&D1).map(|m| m.iter().copied().collect()))
            .unwrap_or_default()
    }

    fn mark(&self, author: u64, counter: u64) {
        self.driver
            .with_replica(|r| r.docs.entry(D1).or_default().insert((author, counter)));
    }
}

/// Two devices that trust each other: `(smaller id, larger id)`.
fn pair() -> (Device, Device) {
    let discovery_port = free_udp_port();
    let mut a = DeviceProfile::generate("Tablet");
    let mut b = DeviceProfile::generate("PC");
    a.trust(TrustedDevice::new(*b.key().public(), "PC", 1));
    b.trust(TrustedDevice::new(*a.key().public(), "Tablet", 1));
    if b.id() < a.id() {
        std::mem::swap(&mut a, &mut b);
    }
    (
        Device::start(a, free_udp_port(), discovery_port),
        Device::start(b, free_udp_port(), discovery_port),
    )
}

/// Pump both until `done` holds; whether it did within `wait`.
fn settle(
    a: &mut Device,
    b: &mut Device,
    wait: Duration,
    done: impl Fn(&Device, &Device) -> bool,
) -> bool {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        a.pump();
        b.pump();
        if done(a, b) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn both_online(a: &Device, b: &Device) -> bool {
    a.online_with(b) && b.online_with(a)
}

const WAIT: Duration = Duration::from_secs(15);

#[test]
fn a_device_back_from_the_background_reconnects_and_catches_up() {
    let (mut small, mut large) = pair();
    assert!(settle(&mut small, &mut large, WAIT, both_online));

    for (counter, small_leaves) in [(1, true), (2, false)] {
        if small_leaves {
            small.node.pause().unwrap();
        } else {
            large.node.pause().unwrap();
        }
        assert!(
            settle(&mut small, &mut large, WAIT, |a, b| !a.online_with(b)
                && !b.online_with(a)),
            "both see the device leave"
        );
        // Edited while apart.
        small.mark(1, counter);
        if small_leaves {
            small.node.resume().unwrap();
        } else {
            large.node.resume().unwrap();
        }
        assert!(
            settle(&mut small, &mut large, WAIT, |a, b| both_online(a, b)
                && a.marks() == b.marks()),
            "reconnected and in step after coming back"
        );
    }
}

#[test]
fn a_restarted_device_reconnects_whichever_id_it_has() {
    let (mut small, mut large) = pair();
    assert!(settle(&mut small, &mut large, WAIT, both_online));

    large = large.restart();
    large.mark(2, 1);
    assert!(
        settle(&mut small, &mut large, WAIT, |a, b| both_online(a, b)
            && a.marks() == vec![(2, 1)]),
        "the larger id came back"
    );

    small = small.restart();
    small.mark(1, 1);
    assert!(
        settle(&mut small, &mut large, WAIT, |a, b| both_online(a, b)
            && b.marks() == vec![(1, 1), (2, 1)]),
        "the smaller id came back"
    );
}
