//! Dialog headers: the session sheet, its title renamed in place, and
//! the other sheets and modals that share the header.

use std::time::Duration;

use freya::prelude::*;
use freya_testing::prelude::*;
use notin_sync::{DeviceId, Member, Role, ShareId, ShareMode};

use super::{out, render, sandbox, shell};
use crate::sync::{MemberView, ShareView, SyncSnapshot};

const SHARE: ShareId = ShareId([7; 16]);

fn member(n: u8, name: &str, role: Role) -> Member {
    Member {
        device: DeviceId([n; 32]),
        public: [n; 32],
        sign: [n; 32],
        name: name.to_owned(),
        device_name: format!("Tablet de {name}"),
        role,
        joined_at: 0,
    }
}

/// A session this device organizes, with two people in it.
fn demo_sync() -> SyncSnapshot {
    let me = member(1, "Sergio", Role::Owner);
    let view = ShareView {
        id: SHARE,
        title: "Estudio de Romanos — grupo de los jueves por la noche".to_owned(),
        owner: me.clone(),
        is_owner: true,
        my_role: Role::Owner,
        mode: ShareMode::Session,
        folder: None,
        folders: Vec::new(),
        desk: false,
        entry_role: Role::Editor,
        approve_entry: true,
        editors_add_docs: false,
        docs: Vec::new(),
        members: vec![
            MemberView {
                member: member(2, "Ana", Role::Editor),
                online: true,
                path: Some(notin_sync::Path::Direct),
            },
            MemberView {
                member: member(3, "Luis", Role::Viewer),
                online: false,
                path: None,
            },
        ],
        pending: Vec::new(),
        owner_online: true,
        owner_path: None,
        waiting_for_owner: false,
        invite_link: "notein://join/demo".to_owned(),
        invite_relayed: true,
        relay_online: true,
        can_edit: true,
        can_manage: true,
        can_add_docs: true,
        can_rename: true,
    };
    SyncSnapshot {
        device: Some(me.device),
        name: "Portátil".to_owned(),
        shares: vec![view],
        ..SyncSnapshot::default()
    }
}

fn session() -> Element {
    use_hook(|| {
        crate::sync::publish_preview(demo_sync());
        spawn(async { crate::screens::collab::session(SHARE) });
    });
    crate::screens::home::Home.into_element()
}

#[test]
fn session_sheet() {
    render("session-desktop", (1280.0, 860.0), session);
    render("session-phone", (390.0, 844.0), session);
}

/// The centre of the first node whose accessible name is `name`.
fn center_of(runner: &TestingRunner, name: &str) -> Option<(f64, f64)> {
    runner.find(|node, element| {
        let a11y = element.accessibility();
        let label = a11y.builder.label()?;
        (label == name).then(|| {
            let area = node.layout().area;
            (f64::from(area.center().x), f64::from(area.center().y))
        })
    })
}

fn settle(runner: &mut TestingRunner) {
    runner.poll_n(Duration::from_millis(40), 8);
    runner.sync_and_update();
}

/// The pencil opens the title as a field in the header row, with ✓ and
/// ✕ in place of the × ; Escape drops the edit and leaves the sheet open.
#[test]
fn session_title_renames_in_place() {
    sandbox();
    let size = (1280.0, 860.0);
    let app = shell(session, size, notin_ui_kit::Gutters::default());
    let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
    settle(&mut runner);
    settle(&mut runner);
    let pencil = center_of(&runner, "Renombrar")
        .or_else(|| center_of(&runner, "Rename"))
        .expect("the session title has a pencil");
    runner.click_cursor(pencil);
    settle(&mut runner);
    runner.render_to_file(out("session-renaming-desktop"));
    assert!(
        center_of(&runner, "Guardar")
            .or_else(|| center_of(&runner, "Save"))
            .is_some(),
        "editing shows ✓"
    );
    runner.press_key(Key::Named(NamedKey::Escape));
    settle(&mut runner);
    assert_eq!(
        runner.run_in(|| notin_ui_kit::ModalController::get().depth()),
        1,
        "Escape ends the edit, not the sheet"
    );
    assert!(
        center_of(&runner, "Renombrar")
            .or_else(|| center_of(&runner, "Rename"))
            .is_some(),
        "the title reads again"
    );
}

fn settings_devices() -> Element {
    use_hook(|| {
        crate::sync::publish_preview(demo_sync());
        spawn(async {
            crate::screens::settings::open_at(crate::screens::settings::Section::Devices);
        });
    });
    crate::screens::home::Home.into_element()
}

/// A row renamed in place keeps its label above the field, so what is
/// being changed stays in sight.
#[test]
fn settings_row_renames_in_place() {
    for (name, size) in [
        ("settings-devices-renaming-desktop", (1280.0, 860.0)),
        ("settings-devices-renaming-phone", (390.0, 844.0)),
    ] {
        sandbox();
        let app = shell(settings_devices, size, notin_ui_kit::Gutters::default());
        let (mut runner, ()) = TestingRunner::new(app, size.into(), |_| (), 1.0);
        settle(&mut runner);
        settle(&mut runner);
        // Phones open Settings on the list; go into the section first.
        if let Some(row) =
            center_of(&runner, "Dispositivos").or_else(|| center_of(&runner, "Devices"))
            && size.0 < 600.0
        {
            runner.click_cursor(row);
            settle(&mut runner);
        }
        let pencil = center_of(&runner, "Renombrar")
            .or_else(|| center_of(&runner, "Rename"))
            .expect("this device's name has a pencil");
        runner.click_cursor(pencil);
        settle(&mut runner);
        runner.render_to_file(out(name));
    }
}

fn pairing() -> Element {
    use_hook(|| {
        crate::sync::publish_preview(demo_sync());
        spawn(async { crate::screens::settings::open_pairing() });
    });
    crate::screens::home::Home.into_element()
}

#[test]
fn pairing_sheet() {
    render("pairing-desktop", (1280.0, 860.0), pairing);
    render("pairing-phone", (390.0, 844.0), pairing);
}
