//! Shell-side navigation for [`crate::bookmark::SourceRef`] chips.
//!
//! Bookmark cards + sidebar rows render each ref as a chip; tapping
//! the chip calls into here. For PDF plugin anchors we:
//!
//! 1. Resolve the backing [`crate::library::ItemId`] from the ref.
//! 2. Make sure a PDF plugin instance is open (see
//!    [`crate::active_pdf_sources::ensure_registered`]) so we have
//!    world-space geometry on hand.
//! 3. Scroll the PDF-face board to the anchor's content centre.
//! 4. Fire a 500 ms pulse highlight via
//!    [`crate::active_pdf_sources::pulse`] so the user visually tracks
//!    where the chip landed.
//!
//! Other `SourceRef` variants (Canvas, Item, External) are stubbed
//! until Phase 6 — chips still render; tapping logs a warning.

use crate::active_boards;
use crate::active_pdf_sources;
use crate::app::CanvasMount;
use crate::bookmark::SourceRef;
use crate::canvas::lock;
use crate::ids::BookmarkId;
use crate::library::ItemId;
use crate::pdf_viewer::PDF_PLUGIN_ID;

/// Human-readable one-liner for a chip's visible label. For PDF
/// anchors returns `"PDF p.N"` (or `"PDF p.N (range)"` when the
/// anchor carries a char range). Falls back to the plugin id for
/// unknown sources.
#[must_use]
pub fn chip_label(src: &SourceRef) -> String {
    match src {
        SourceRef::Plugin {
            plugin_id,
            anchor,
            selection,
            ..
        } if plugin_id == PDF_PLUGIN_ID => {
            use crate::pdf_viewer::decode_pdf_anchor;
            let page = decode_pdf_anchor(anchor).map(|a| a.page() + 1);
            match (page, selection.is_some()) {
                (Some(p), true) => format!("PDF p.{p} · fragmento"),
                (Some(p), false) => format!("PDF p.{p}"),
                _ => "PDF".to_owned(),
            }
        }
        SourceRef::Plugin { plugin_id, .. } => plugin_id.clone(),
        SourceRef::Canvas { .. } => "Bookmark".to_owned(),
        SourceRef::External { url } => url.clone(),
    }
}

/// Navigate to `src`. For PDF plugin anchors this scrolls the active
/// PDF face + triggers a pulse highlight; other variants currently
/// log a warning and no-op pending Phase 6 chip plumbing.
pub fn navigate(src: &SourceRef) {
    match src {
        SourceRef::Plugin {
            plugin_id,
            item,
            anchor,
            ..
        } if plugin_id == PDF_PLUGIN_ID => {
            // PDF stays on a shell-specific path: anchor resolves to
            // world coords via the plugin, which the shell then
            // centres + pulses in the mounted canvas. The generic
            // plugin dispatch below only models "find item + route +
            // queue payload" — not enough for the PDF pulse UX.
            let item_id = ItemId(*item);
            navigate_pdf(item_id, anchor.clone());
        }
        SourceRef::Plugin {
            plugin_id, anchor, ..
        } => navigate_plugin(plugin_id, anchor),
        SourceRef::Canvas { bookmark } => navigate_canvas_bookmark(*bookmark),
        SourceRef::External { url } => open_external_url(url),
    }
}

/// Scroll the pane that owns `bookmark` to its live world position.
///
/// Two paths:
///
/// 1. Fast — the bookmark lives in a currently-mounted board. Scan
///    [`active_boards::snapshot`] and centre on the first match.
/// 2. Slow — scan the library on disk: for each registered item,
///    deserialize its [`crate::doc::Doc`] and check if the bookmark
///    is there. On a hit, open the item in the active workspace's
///    focused pane (via [`crate::library::Library::add_pane_tab`])
///    and poll [`active_boards::get_item`] until the board mounts,
///    then centre.
///
/// Capped at a short poll window so a hung mount can't wedge the
/// task forever.
fn navigate_canvas_bookmark(bookmark: BookmarkId) {
    // Fast path.
    for (_, board) in active_boards::snapshot() {
        let mut guard = lock(&board);
        if guard.doc().bookmark(bookmark).is_some() {
            guard.center_on_bookmark(bookmark);
            return;
        }
    }
    // Slow path — spawn an async task so the UI stays responsive while
    // the library scan + tab-open sequence runs.
    freya::prelude::spawn_forever(async move {
        let Ok(handle) = crate::home::open_library().await else {
            log::warn!("navigate_canvas: library handle unavailable");
            return;
        };
        let (found, workspace, focused_pane) = {
            let lib = handle.lock().await;
            let mut hit = None;
            for item in &lib.index().items {
                if let Ok(doc) = lib.load_doc(item.id).await
                    && doc.bookmark(bookmark).is_some()
                {
                    hit = Some(item.id);
                    break;
                }
            }
            let workspace = lib.active_workspace();
            let focused_pane = workspace
                .and_then(|wid| lib.workspace(wid).and_then(|w| w.focused_pane));
            (hit, workspace, focused_pane)
        };
        let Some(item_id) = found else {
            log::warn!("navigate_canvas: bookmark {bookmark:?} not found in any item");
            return;
        };
        let (Some(workspace), Some(pane)) = (workspace, focused_pane) else {
            log::warn!(
                "navigate_canvas: bookmark lives in item {item_id:?} but no \
                 active workspace / focused pane to host the tab"
            );
            return;
        };
        {
            let mut lib = handle.lock().await;
            if let Err(err) = lib.add_pane_tab(workspace, pane, item_id).await {
                log::error!("navigate_canvas: add_pane_tab failed: {err}");
                return;
            }
        }
        // Poll for mount — the pane re-renders, the canvas registers
        // its board under `CanvasMount::Item(item_id)`, and then we
        // can centre. 3 s budget.
        for _ in 0..30 {
            async_io::Timer::after(std::time::Duration::from_millis(100)).await;
            if let Some(board) = active_boards::get_item(item_id) {
                lock(&board).center_on_bookmark(bookmark);
                return;
            }
        }
        log::warn!(
            "navigate_canvas: item {item_id:?} opened but board never mounted \
             within the poll window"
        );
    });
}

/// Open `url` with the host's default handler. Freya exposes
/// `UserEvent::OpenUrl` on desktop but reaching it from a render-scope
/// `Platform::get()` call turned out silent (dispatcher queue not
/// drained for our window?) during Phase 6 testing. We spawn the
/// platform-native opener directly instead — simpler and matches what
/// `open::that` would do inside the renderer anyway.
pub fn open_external_url(url: &str) {
    log::info!("navigate: opening external url {url}");
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        #[cfg(target_os = "linux")]
        let (cmd, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);
        #[cfg(target_os = "macos")]
        let (cmd, args): (&str, Vec<&str>) = ("open", vec![url]);
        #[cfg(target_os = "windows")]
        let (cmd, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);

        match std::process::Command::new(cmd)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => log::info!("navigate: spawned {cmd}"),
            Err(e) => log::warn!("navigate: failed to launch {cmd} for {url}: {e}"),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        log::warn!("navigate: external url nav unavailable on this platform ({url})");
    }
}

/// Scroll the PDF face for `item` to the anchor's content centre + pulse
/// highlight. Opens the plugin instance lazily when it's not already
/// registered. If no canvas pane is currently mounted for `item`, the
/// slow path adds a tab to the focused workspace pane and polls until
/// the board mounts, mirroring [`navigate_canvas_bookmark`].
fn navigate_pdf(item: ItemId, anchor: Vec<u8>) {
    log::info!("navigate_pdf: item={item:?}");
    let Some(handle) = active_pdf_sources::ensure_registered(item) else {
        log::warn!("navigate_pdf: failed to open PDF plugin for {item:?}");
        return;
    };
    let target = handle
        .plugin
        .anchor_scroll_target_world(handle.instance, &anchor);
    active_pdf_sources::pulse(item, anchor.clone());
    let Some((wx, wy)) = target else {
        return;
    };
    if let Some(board) = active_boards::get(CanvasMount::Item(item)) {
        lock(&board).center_on_world(wx, wy);
        return;
    }
    // Slow path — not mounted. Route to the item's PDF canvas view
    // and poll `active_boards` until the mount lands, then centre +
    // re-pulse so the pulse window isn't exhausted by the route /
    // asset-load latency.
    let router = crate::app::global_router();
    let _ = router.push(crate::route::Route::CanvasPdfView { id: item });
    freya::prelude::spawn_forever(async move {
        for _ in 0..30 {
            async_io::Timer::after(std::time::Duration::from_millis(100)).await;
            if let Some(board) = active_boards::get(CanvasMount::Item(item)) {
                lock(&board).center_on_world(wx, wy);
                active_pdf_sources::pulse(item, anchor);
                return;
            }
        }
        log::warn!(
            "navigate_pdf: item {item:?} routed but board never mounted \
             within the poll window"
        );
    });
}

/// Generic chip-tap dispatch for any `SourceRef::Plugin` whose plugin
/// opts into navigation via [`notein_plugin::Source::navigate_payload`].
///
/// 1. Ask the plugin for the payload it wants its viewer to receive.
///    `None` → chip is inert for this plugin; abort.
/// 2. Find any library item citing the plugin (via
///    [`crate::library::ItemKind::hosting_plugin`]). Covers
///    standalone `PluginCanvas` + `DocLinked` bundles in one scan.
/// 3. Queue the payload on the host under `(plugin_id, item_id)` and
///    broadcast so a live viewer on the same item re-applies.
/// 4. Route to the item's native face via
///    [`crate::route::route_for_kind`].
///
/// No fallback item-creation: when the plugin opts into chip nav but
/// no library item of its kind exists, we log + stop. Opening a
/// placeholder item is a dedicated flow the user should drive (FAB,
/// settings, bundle import) — not a side effect of tapping a chip.
fn navigate_plugin(plugin_id: &str, anchor: &[u8]) {
    let registry = crate::plugins::registry();
    let plugin = {
        let guard = crate::plugins::lock(&registry);
        guard.get_source(plugin_id).cloned()
    };
    let Some(plugin) = plugin else {
        log::warn!("navigate_plugin: no plugin registered for {plugin_id}");
        return;
    };
    let Some(payload) = plugin.navigate_payload(anchor) else {
        log::info!(
            "navigate_plugin: plugin {plugin_id} opted out of chip nav"
        );
        return;
    };
    // Target plugin id is the viewer side — plugins with a split
    // source / viewer design (e.g. Bible) override `viewer_kind_id`
    // so the library lookup + pending-anchor channel land on the
    // right id. For single-id plugins the default returns
    // `self.kind_id()`, keeping behaviour transparent.
    let viewer_id = plugin.viewer_kind_id().to_string();
    // Capture the launch-installed router — reaching through
    // `RouterContext::get()` would panic on chips rendered outside
    // the Router subtree (e.g. the host-level `BookmarkEditModal`
    // lives in `HostOverlay`, a sibling of Outlet).
    let router = crate::app::global_router();
    freya::prelude::spawn_forever(async move {
        let Ok(handle) = crate::home::open_library().await else {
            log::warn!("navigate_plugin: library handle unavailable");
            return;
        };
        let found = {
            let lib = handle.lock().await;
            lib.index().items.iter().find_map(|item| {
                item.kind
                    .hosting_plugin()
                    .filter(|id| *id == viewer_id)
                    .map(|_| (item.id, crate::route::route_for_kind(&item.kind, item.id)))
            })
        };
        let Some((item_id, route)) = found else {
            log::warn!(
                "navigate_plugin: no library item hosts {viewer_id}; chip \
                 target unreachable"
            );
            return;
        };
        crate::host_api::host_handle().queue_source_anchor(&viewer_id, item_id.0, payload);
        let _ = router.push(route);
    });
}

