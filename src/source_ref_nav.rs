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

use crate::active_bible_sources;
use crate::active_boards;
use crate::active_pdf_sources;
use crate::app::CanvasMount;
use crate::bookmark::SourceRef;
use crate::canvas::lock;
use crate::ids::BookmarkId;
use crate::library::{ItemId, ItemKind};
use crate::pdf_viewer::PDF_PLUGIN_ID;

const BIBLE_SOURCE_PLUGIN_ID: &str = "io.notein.bible";
const BIBLE_VIEWER_PLUGIN_ID: &str = "io.notein.bible.viewer";

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
            let item_id = ItemId(*item);
            navigate_pdf(item_id, anchor.clone());
        }
        SourceRef::Plugin {
            plugin_id, anchor, ..
        } if plugin_id == BIBLE_SOURCE_PLUGIN_ID => {
            navigate_bible(anchor);
        }
        SourceRef::Plugin { plugin_id, .. } => {
            log::warn!("navigate: unsupported plugin ref {plugin_id}");
        }
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
    freya::prelude::spawn(async move {
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
    let router = freya::router::RouterContext::get();
    let _ = router.push(crate::route::Route::CanvasPdfView { id: item });
    freya::prelude::spawn(async move {
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

/// Navigate to a Bible chip's target verse. Decodes the plugin anchor,
/// finds the right Bible item in the library, queues a pending jump
/// keyed on the item id, and pushes the Bible route. The Bible viewer
/// polls [`active_bible_sources::take`] on mount and applies the jump.
///
/// When the viewer is already open against the right item the pending
/// jump is still queued — the viewer's reactive hook picks it up on
/// the next render.
fn navigate_bible(anchor_bytes: &[u8]) {
    let Some(decoded) = notein_bible::BibleAnchor::decode(anchor_bytes) else {
        log::warn!("navigate_bible: failed to decode anchor payload");
        return;
    };
    let jump = active_bible_sources::PendingJump {
        bible_id: decoded.bible_id.clone(),
        book_id: decoded.book_id.clone(),
        chapter: decoded.start_chapter,
        verse: decoded.start_verse,
    };
    // Capture the router at call time — the pointer-down handler runs
    // in a Freya scope, so `RouterContext::get()` resolves here.
    // The async spawn below cannot call it after `.await` because the
    // scope's thread-local disappears across executor steps.
    let router = freya::router::RouterContext::get();
    // Resolve the library item that hosts a Bible viewer. The chip
    // itself does not carry an `ItemId` — Bible chips come from
    // completions that originate at the plugin level, not from a
    // specific item. The viewer swaps translations in-place from the
    // user's installed Bibles, so any Bible item works as the mount
    // target; the jump's `bible_id` steers the final translation
    // once the viewer has rehydrated.
    freya::prelude::spawn(async move {
        let Ok(handle) = crate::home::open_library().await else {
            log::warn!("navigate_bible: library handle unavailable");
            return;
        };
        // Walk every item that hosts the Bible viewer — both the
        // standalone `PluginCanvas` form and the `DocLinked` bundle
        // (bible + canvas sibling). The item id + its route live
        // together so we can dispatch to the correct shell face
        // without conflating a split-view bundle with a flat viewer.
        let existing = {
            let lib = handle.lock().await;
            lib.index()
                .items
                .iter()
                .find_map(|item| match &item.kind {
                    ItemKind::PluginCanvas { plugin_id, .. }
                        if plugin_id == BIBLE_VIEWER_PLUGIN_ID =>
                    {
                        Some((item.id, crate::route::Route::BibleView { id: item.id }))
                    }
                    ItemKind::DocLinked { source_plugin }
                        if source_plugin == BIBLE_VIEWER_PLUGIN_ID =>
                    {
                        Some((
                            item.id,
                            crate::route::Route::DocLinkedView { id: item.id },
                        ))
                    }
                    _ => None,
                })
        };
        let (item_id, route) = match existing {
            Some(found) => found,
            None => {
                // No Bible item in the library yet. Mirrors
                // `home::flow_open_bible` — create a fresh
                // `PluginCanvas` under the root folder so chip taps
                // keep working for users who installed a translation
                // but never opened the viewer manually.
                let init_bytes = bincode::encode_to_vec(
                    &String::new(),
                    bincode::config::standard(),
                )
                .unwrap_or_default();
                let mut lib = handle.lock().await;
                match lib
                    .create_item(
                        crate::library::ROOT_FOLDER,
                        ItemKind::PluginCanvas {
                            plugin_id: BIBLE_VIEWER_PLUGIN_ID.into(),
                            init_bytes,
                        },
                        "Biblia",
                        None,
                        Vec::new(),
                        crate::library::BackgroundStyle::default(),
                    )
                    .await
                {
                    Ok(id) => (id, crate::route::Route::BibleView { id }),
                    Err(e) => {
                        log::warn!("navigate_bible: create_item failed: {e}");
                        return;
                    }
                }
            }
        };
        active_bible_sources::queue(item_id, jump);
        let _ = router.push(route);
    });
}

