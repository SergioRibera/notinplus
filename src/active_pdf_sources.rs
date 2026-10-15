//! Lookup table for the live `PdfSourcePlugin` instance id of each
//! mounted PDF face.
//!
//! Phase 5.3 selection UX (pointer drag → highlight → create bookmark
//! from source anchor) needs the shell to drive plugin-side selection
//! state. The plugin's `pointer_begin_select` / `pointer_extend_select`
//! / `pointer_end_select` / `selection_rects` / `current_selection` all
//! live behind an opaque instance handle; this registry publishes that
//! handle keyed on the library [`ItemId`] so overlays, chrome buttons
//! and context menus can reach it without threading state through
//! Freya's parent-child context.
//!
//! The mount site opens the plugin via `Source::open` + attaches the
//! item id, stores the handle here, and clears on drop (see
//! [`register`] / [`unregister`]). Readers get [`None`] when nothing
//! is mounted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::builtin::pdf_source::PdfSourcePlugin;
use crate::library::ItemId;

/// Live PDF plugin instance for one mounted item. Carries the plugin
/// `Arc` + the opaque instance id so callers can invoke the plugin's
/// pointer / selection API without going through the global registry.
#[derive(Clone)]
pub struct PdfSourceHandle {
    pub plugin: Arc<PdfSourcePlugin>,
    pub instance: u64,
}

impl std::fmt::Debug for PdfSourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfSourceHandle")
            .field("instance", &self.instance)
            .finish_non_exhaustive()
    }
}

type Registry = Mutex<HashMap<ItemId, PdfSourceHandle>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> MutexGuard<'static, HashMap<ItemId, PdfSourceHandle>> {
    match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Publish `handle` as the live PDF instance for `item`. A second
/// register for the same id wins — the previous instance should be
/// closed by its own owner before replacing.
pub fn register(item: ItemId, handle: PdfSourceHandle) {
    lock().insert(item, handle);
}

/// Drop the registration + close the plugin instance. No-op when `item`
/// has no live registration.
pub fn unregister(item: ItemId) {
    let mut g = lock();
    if let Some(handle) = g.remove(&item) {
        use notein_plugin::Source;
        handle.plugin.close(handle.instance);
    }
}

/// Current PDF source handle for `item`, if any.
#[must_use]
pub fn get(item: ItemId) -> Option<PdfSourceHandle> {
    lock().get(&item).cloned()
}

/// Pulse highlight the overlay paints on top of the selection colour
/// to signal chip navigation. Lives in a side registry (not on the
/// plugin instance) because the pulse is a shell UX concern.
#[derive(Clone)]
pub struct PulseState {
    pub anchor: Vec<u8>,
    pub until: Instant,
}

/// Duration of the chip-tap highlight pulse. Matches the "500 ms"
/// callout in `SOURCES_PLAN` §7.
pub const PULSE_DURATION: Duration = Duration::from_millis(500);

type PulseRegistry = Mutex<HashMap<ItemId, PulseState>>;

fn pulses() -> &'static PulseRegistry {
    static P: OnceLock<PulseRegistry> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pulses() -> MutexGuard<'static, HashMap<ItemId, PulseState>> {
    match pulses().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Trigger a pulse highlight over `anchor` for the current PDF face of
/// `item`. Lasts [`PULSE_DURATION`]; the overlay paint queries this on
/// every frame and renders translucent rects while within the window.
pub fn pulse(item: ItemId, anchor: Vec<u8>) {
    lock_pulses().insert(
        item,
        PulseState {
            anchor,
            until: Instant::now() + PULSE_DURATION,
        },
    );
}

/// Current pulse for `item`, or `None` when expired / absent.
/// Expired pulses are reaped on query so stale state doesn't grow
/// across sessions.
#[must_use]
pub fn active_pulse(item: ItemId) -> Option<PulseState> {
    let mut g = lock_pulses();
    let keep = g.get(&item).is_some_and(|p| Instant::now() < p.until);
    if !keep {
        g.remove(&item);
        return None;
    }
    g.get(&item).cloned()
}

/// Selection-side [`PdfSourcePlugin`] singleton. Decoupled from the
/// [`crate::plugins`] registry's instance so we can hold a concrete-type
/// `Arc` (the registry only exposes `Arc<dyn Source>` which cannot be
/// downcast back to a typed `Arc` on stable Rust).
///
/// Divergence with the registry's PdfSourcePlugin is intentional and
/// safe: config (`render_dpi`) only affects raster render, which the
/// registry plugin owns; selection state (instances / drag anchor /
/// page layout) is per-instance and lives entirely in whichever plugin
/// opened the item. Opening a given `ItemId` twice (once per plugin)
/// is cheap — `PdfDocument` is `Arc`-shared internally.
fn selection_plugin() -> &'static Arc<PdfSourcePlugin> {
    static P: OnceLock<Arc<PdfSourcePlugin>> = OnceLock::new();
    P.get_or_init(|| Arc::new(PdfSourcePlugin::default()))
}

/// Lazily open the PDF source plugin for `item` if no live instance is
/// registered. Reads the on-disk PDF bytes via [`crate::library::bodies`],
/// opens them through a selection-side plugin singleton, attaches the
/// [`ItemId`] so [`notein_plugin::Source::current_selection`] can
/// produce complete anchors, and publishes the handle.
///
/// Idempotent — returns the already-registered handle on subsequent
/// calls. Returns `None` when the PDF body is missing or the plugin
/// refuses to open the bytes.
///
/// Used as a lazy hook from UI call sites (selection overlay, context
/// menu, chip resolver) so the shell can skip wiring open+attach into
/// every PDF mount path — the first UI consumer forces the open.
pub fn ensure_registered(item: ItemId) -> Option<PdfSourceHandle> {
    if let Some(handle) = get(item) {
        return Some(handle);
    }
    let bytes = crate::library::bodies::read_pdf(item).ok()?;
    let plugin = Arc::clone(selection_plugin());
    use notein_plugin::Source as _;
    let instance = plugin.open(&bytes).ok()?;
    plugin.attach_item(instance, item);
    let handle = PdfSourceHandle { plugin, instance };
    register(item, handle.clone());
    Some(handle)
}
