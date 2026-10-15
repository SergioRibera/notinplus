//! Built-in PDF source plugin.
//!
//! Lifecycle + creation entry only — the legacy `freya-pdf` pipeline
//! keeps serving actual page rasterisation inside Modo A canvases. The
//! plugin exists so the home FAB `+` menu, the settings panel, and the
//! future `SourceRef::Plugin { plugin_id: "io.notein.source.pdf", .. }`
//! resolver all route through one declarative surface.
//!
//! `render_tile` / `hit_test` / `resolve_anchor` / `search_*` stay as
//! honest stubs — Phase 5 (Modo B) wires them to [`freya_pdf`] when the
//! source pane needs to render independently of the stroke canvas.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use freya::icons::lucide;
use freya_canvas_bg::Rect;
use freya_pdf::PdfDocument;
use notein_plugin::{
    AnchorView, ChipView, Completion, ConfigError, ConfigField, ConfigFieldKind, ConfigSchema,
    ConfigValue, CreationCategory, CreationEntry, CreationError, CreationKind,
    ItemCreationResult, SearchHit, SelectMode, Selection, Source, SourceError, SourceRef, Tile,
    TileRequest,
};

use crate::library::ItemId;
use crate::pdf_viewer::{PdfAnchor, PdfSelection, decode_pdf_anchor, encode_pdf_anchor};

/// Stable id this plugin registers under. Referenced by future
/// `SourceRef::Plugin` anchors.
pub const KIND_ID: &str = "io.notein.source.pdf";

/// Shell-wide creation entry id. Preserved from the old
/// `shell_builtins::IMPORT_PDF` constant so persisted FAB shortcuts
/// keep resolving after the migration.
pub const IMPORT_PDF: &str = "import-pdf";

/// Modo B counterpart to [`IMPORT_PDF`] — imports the PDF as a
/// standalone source item and spawns a sibling canvas item linked to
/// it. Handled by the shell's `flow_import_pdf_linked` flow.
pub const IMPORT_PDF_LINKED: &str = "import-pdf-linked";

/// Default render DPI. Matches the legacy `freya-pdf` setting.
const DEFAULT_RENDER_DPI: f64 = 150.0;
const MIN_RENDER_DPI: f64 = 72.0;
const MAX_RENDER_DPI: f64 = 600.0;

const CONFIG_KEY_RENDER_DPI: &str = "render_dpi";

/// Default [`Source`] implementation for PDF files.
#[derive(Debug, Default)]
pub struct PdfSourcePlugin {
    instances: Mutex<HashMap<u64, PdfInstance>>,
    next_instance: AtomicU64,
    config: Mutex<PdfConfig>,
}

#[derive(Debug)]
struct PdfInstance {
    doc: PdfDocument,
    /// Library item id this instance was opened for. Shell populates it
    /// via [`PdfSourcePlugin::attach_item`] right after `open` so that
    /// [`Source::current_selection`] can encode a complete
    /// [`PdfAnchor::Range`] without the shell having to know the
    /// anchor's byte shape.
    source: Option<ItemId>,
    /// Live selection, if the user is dragging or has a finalised range.
    /// `None` = no selection. Normalised on insert.
    selection: Option<PdfSelection>,
    /// Cached page-rect layout in world coords. Mirrors
    /// `freya-pdf::backend::compute_layout` (centered-column stack with
    /// a fixed gap) so the plugin can do pointer hit-testing without
    /// the shell having to pass layout state in every call.
    pages: Vec<PageLayoutEntry>,
    /// Partial-drag anchor: `start_page` + `start_char` captured by the
    /// most recent `pointer_begin_select`. The `end_*` endpoint moves
    /// with `pointer_extend_select`. Cleared on `pointer_end_select` —
    /// after that the finalised range lives only in [`Self::selection`].
    drag_anchor: Option<DragAnchor>,
}

#[derive(Debug, Clone, Copy)]
struct PageLayoutEntry {
    page: u32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

#[derive(Debug, Clone, Copy)]
struct DragAnchor {
    page: u32,
    char_idx: u32,
    /// Granularity of the drag — `Point` tracks the pointer char-by-char,
    /// `Word` snaps both endpoints to word boundaries while dragging.
    mode: SelectMode,
}

/// Vertical gap between stacked pages in world units. Matches
/// `freya-pdf::backend` default so hit-testing lines up with the painted
/// background 1:1.
const PAGE_GAP: f32 = 40.0;

#[derive(Debug, Clone, Copy)]
struct PdfConfig {
    render_dpi: f64,
}

impl Default for PdfConfig {
    fn default() -> Self {
        Self {
            render_dpi: DEFAULT_RENDER_DPI,
        }
    }
}

impl PdfSourcePlugin {
    fn alloc_id(&self) -> u64 {
        self.next_instance
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    fn lock_instances(&self) -> std::sync::MutexGuard<'_, HashMap<u64, PdfInstance>> {
        match self.instances.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn lock_config(&self) -> std::sync::MutexGuard<'_, PdfConfig> {
        match self.config.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// Bind a library [`ItemId`] to the given instance. The shell calls
    /// this right after `open` so [`Source::current_selection`] can
    /// encode a full [`PdfAnchor::Range`] back to the item.
    ///
    /// Also publishes the item into the process-wide completion
    /// catalog so the `[[` popup can list this PDF whether or not
    /// a viewer is currently open (static catalog pattern).
    ///
    /// Noop on unknown instance.
    pub fn attach_item(&self, instance: u64, item: ItemId) {
        let page_count = {
            let mut g = self.lock_instances();
            let Some(inst) = g.get_mut(&instance) else {
                return;
            };
            inst.source = Some(item);
            inst.doc.page_count()
        };
        let anchor = encode_pdf_anchor(PdfAnchor::Page {
            source: item,
            page: 0,
        });
        let short_id: String = item.to_string().chars().take(8).collect();
        crate::completion_catalog::register(crate::completion_catalog::CatalogEntry {
            plugin_id: KIND_ID.to_owned(),
            item_id: item,
            label: format!("PDF ({page_count} páginas)"),
            subtitle: Some(format!("PDF · {short_id}")),
            source: SourceRef::Plugin {
                plugin_id: KIND_ID.to_owned(),
                item: item.0,
                anchor,
                selection: None,
            },
        });
    }

    /// Store (or clear) the live selection for an instance. Automatically
    /// normalises the range so `start` precedes `end`.
    pub fn set_selection(&self, instance: u64, sel: Option<PdfSelection>) {
        if let Some(inst) = self.lock_instances().get_mut(&instance) {
            inst.selection = sel.map(PdfSelection::normalized);
        }
    }

    /// Current selection stored for the instance, if any.
    #[must_use]
    pub fn selection(&self, instance: u64) -> Option<PdfSelection> {
        self.lock_instances().get(&instance)?.selection
    }

    /// Union-merged bounding rects covering `sel` on `page`. Returns an
    /// empty vec when the selection doesn't touch `page`, is empty, or
    /// the instance is unknown.
    #[must_use]
    pub fn selection_rects(&self, instance: u64, page: u32, sel: PdfSelection) -> Vec<Rect> {
        let guard = self.lock_instances();
        let Some(inst) = guard.get(&instance) else {
            return Vec::new();
        };
        let sel = sel.normalized();
        if sel.is_empty() {
            return Vec::new();
        }
        let (start, end) = if page < sel.start_page || page > sel.end_page {
            return Vec::new();
        } else if sel.start_page == sel.end_page {
            (sel.start_char as usize, sel.end_char as usize)
        } else if page == sel.start_page {
            // Head page: from start_char to end of page's text.
            let Ok(text) = inst.doc.extract_page_text(page as usize) else {
                return Vec::new();
            };
            (sel.start_char as usize, text.chars().count())
        } else if page == sel.end_page {
            // Tail page: from 0 to end_char.
            (0, sel.end_char as usize)
        } else {
            // Middle page: whole text.
            let Ok(text) = inst.doc.extract_page_text(page as usize) else {
                return Vec::new();
            };
            (0, text.chars().count())
        };
        inst.doc
            .char_rects(page as usize, start, end)
            .unwrap_or_default()
    }

    /// World-space rects for an explicit `PdfAnchor` payload. Mirrors
    /// [`Self::selection_world_rects`] but reads the anchor given on the
    /// call (used by chip navigation) instead of the instance's live
    /// selection. Returns an empty vec on decode error / unknown page.
    #[must_use]
    pub fn anchor_world_rects(&self, instance: u64, anchor: &[u8]) -> Vec<Rect> {
        let Some(anchor) = decode_pdf_anchor(anchor) else {
            return Vec::new();
        };
        let guard = self.lock_instances();
        let Some(inst) = guard.get(&instance) else {
            return Vec::new();
        };
        let Some(entry) = inst.pages.iter().find(|e| e.page == anchor.page()) else {
            return Vec::new();
        };
        let (start, end) = match anchor.char_range() {
            Some((s, e)) => (s as usize, e as usize),
            None => {
                let Ok(text) = inst.doc.extract_page_text(entry.page as usize) else {
                    return Vec::new();
                };
                (0, text.chars().count())
            }
        };
        let Ok(rects) = inst.doc.char_rects(entry.page as usize, start, end) else {
            return Vec::new();
        };
        rects
            .into_iter()
            .map(|r| Rect {
                min_x: r.min_x + entry.min_x,
                min_y: r.min_y + entry.min_y,
                max_x: r.max_x + entry.min_x,
                max_y: r.max_y + entry.min_y,
            })
            .collect()
    }

    /// World-space centre of an `anchor`'s targeted content, suitable
    /// for `Board::center_on_world` so chip navigation scrolls the
    /// pane to the referenced page / range.
    ///
    /// Falls back to the page's layout origin when `anchor` points at
    /// a known page but no glyphs resolve (empty page, bad range).
    #[must_use]
    pub fn anchor_scroll_target_world(&self, instance: u64, anchor: &[u8]) -> Option<(f32, f32)> {
        let decoded = decode_pdf_anchor(anchor)?;
        let page_centre = {
            let guard = self.lock_instances();
            let inst = guard.get(&instance)?;
            let entry = inst.pages.iter().find(|e| e.page == decoded.page())?;
            ((entry.min_x + entry.max_x) * 0.5, (entry.min_y + entry.max_y) * 0.5)
        };
        let rects = self.anchor_world_rects(instance, anchor);
        if let Some(first) = rects.first() {
            return Some(((first.min_x + first.max_x) * 0.5, (first.min_y + first.max_y) * 0.5));
        }
        Some(page_centre)
    }

    /// Merge the stored selection's rects into world-space (shared
    /// coordinate system with the canvas viewport) across every page
    /// the selection touches. Returns an empty vec when there is no
    /// selection or the instance is unknown.
    ///
    /// Each returned rect translates a page-local `char_rects` output
    /// by the page's layout origin, so overlay paint can map world → screen
    /// via [`crate::canvas::Board::world_to_screen`] without touching
    /// per-page offsets.
    #[must_use]
    pub fn selection_world_rects(&self, instance: u64) -> Vec<Rect> {
        let guard = self.lock_instances();
        let Some(inst) = guard.get(&instance) else {
            return Vec::new();
        };
        let Some(sel) = inst.selection else {
            return Vec::new();
        };
        let sel = sel.normalized();
        if sel.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for entry in &inst.pages {
            if entry.page < sel.start_page || entry.page > sel.end_page {
                continue;
            }
            // Reuse the per-page selection logic to compute char-range
            // bounds + merged rects, then shift into world space.
            let (start, end) = if sel.start_page == sel.end_page {
                (sel.start_char as usize, sel.end_char as usize)
            } else if entry.page == sel.start_page {
                let Ok(text) = inst.doc.extract_page_text(entry.page as usize) else {
                    continue;
                };
                (sel.start_char as usize, text.chars().count())
            } else if entry.page == sel.end_page {
                (0, sel.end_char as usize)
            } else {
                let Ok(text) = inst.doc.extract_page_text(entry.page as usize) else {
                    continue;
                };
                (0, text.chars().count())
            };
            let Ok(rects) = inst.doc.char_rects(entry.page as usize, start, end) else {
                continue;
            };
            for r in rects {
                out.push(Rect {
                    min_x: r.min_x + entry.min_x,
                    min_y: r.min_y + entry.min_y,
                    max_x: r.max_x + entry.min_x,
                    max_y: r.max_y + entry.min_y,
                });
            }
        }
        out
    }

    /// UTF-8 snippet covering `sel`. Pages are joined with `\n`.
    /// Returns an empty string on error / empty selection.
    #[must_use]
    pub fn selection_snippet(&self, instance: u64, sel: PdfSelection) -> String {
        let guard = self.lock_instances();
        let Some(inst) = guard.get(&instance) else {
            return String::new();
        };
        let sel = sel.normalized();
        if sel.is_empty() {
            return String::new();
        }
        if sel.start_page == sel.end_page {
            return page_text_slice(
                &inst.doc,
                sel.start_page,
                sel.start_char as usize,
                sel.end_char as usize,
            )
            .unwrap_or_default();
        }
        let mut out = String::new();
        // Head.
        if let Some(s) = page_text_slice(
            &inst.doc,
            sel.start_page,
            sel.start_char as usize,
            usize::MAX,
        ) {
            out.push_str(&s);
        }
        for p in (sel.start_page + 1)..sel.end_page {
            out.push('\n');
            if let Some(s) = page_text_slice(&inst.doc, p, 0, usize::MAX) {
                out.push_str(&s);
            }
        }
        out.push('\n');
        if let Some(s) = page_text_slice(&inst.doc, sel.end_page, 0, sel.end_char as usize) {
            out.push_str(&s);
        }
        out
    }
}

/// Centered-column vertical stack of pages, mirroring
/// `freya-pdf::backend::compute_layout`. Called once at `open`.
fn compute_page_layout(doc: &PdfDocument) -> Vec<PageLayoutEntry> {
    let sizes = doc.page_sizes();
    let mut out = Vec::with_capacity(sizes.len());
    let mut y = 0.0_f32;
    for (idx, &(w, h)) in sizes.iter().enumerate() {
        let x = -(w * 0.5);
        out.push(PageLayoutEntry {
            page: u32::try_from(idx).unwrap_or(u32::MAX),
            min_x: x,
            min_y: y,
            max_x: x + w,
            max_y: y + h,
        });
        y += h + PAGE_GAP;
    }
    out
}

/// `(page, local_x, local_y)` for the world-space point, or `None` when
/// the point falls between pages or outside the laid-out column.
fn hit_page(pages: &[PageLayoutEntry], x: f32, y: f32) -> Option<(u32, f32, f32)> {
    for entry in pages {
        if y >= entry.min_y && y < entry.max_y {
            // Clamp x into the page's horizontal span so a drag outside
            // the right / left margin still extends to end / start of
            // line instead of silently failing.
            let local_x = x.clamp(entry.min_x, entry.max_x - 1.0) - entry.min_x;
            let local_y = y - entry.min_y;
            return Some((entry.page, local_x, local_y));
        }
    }
    None
}

/// Expand `anchor_char` outward over `text` (char-indexed by `starts`)
/// to the nearest whitespace boundaries. Returns `(start_char, end_char)`
/// (half-open). Falls back to `(anchor_char, anchor_char + 1)` when the
/// anchor char itself is whitespace.
fn word_bounds(text: &str, starts: &[usize], anchor_char: usize) -> (usize, usize) {
    let last = starts.len().saturating_sub(1);
    if anchor_char >= last {
        return (last, last);
    }
    let char_at = |i: usize| -> Option<char> {
        let s = *starts.get(i)?;
        text[s..].chars().next()
    };
    let is_word = |c: char| !c.is_whitespace();
    let Some(anchor_ch) = char_at(anchor_char) else {
        return (anchor_char, anchor_char + 1);
    };
    if !is_word(anchor_ch) {
        return (anchor_char, anchor_char + 1);
    }
    let mut start = anchor_char;
    while start > 0 {
        let Some(c) = char_at(start - 1) else {
            break;
        };
        if !is_word(c) {
            break;
        }
        start -= 1;
    }
    let mut end = anchor_char + 1;
    while end < last {
        let Some(c) = char_at(end) else {
            break;
        };
        if !is_word(c) {
            break;
        }
        end += 1;
    }
    (start, end)
}

/// UTF-8 slice of a page's extracted text by char index. Returns `None`
/// on page out of range. `end = usize::MAX` means "to end of page".
fn page_text_slice(doc: &PdfDocument, page: u32, start: usize, end: usize) -> Option<String> {
    let (text, starts) = doc.extract_page_text_indexed(page as usize).ok()?;
    let last_char = starts.len().saturating_sub(1);
    let end = end.min(last_char);
    let start = start.min(end);
    let start_byte = *starts.get(start)?;
    let end_byte = *starts.get(end)?;
    Some(text[start_byte..end_byte].to_owned())
}

impl Source for PdfSourcePlugin {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn kind_id(&self) -> &str {
        KIND_ID
    }

    fn kind_label(&self) -> &str {
        "PDF"
    }

    fn file_extensions(&self) -> &[&'static str] {
        const EXTS: &[&str] = &["pdf"];
        EXTS
    }

    fn open(&self, item_bytes: &[u8]) -> Result<u64, SourceError> {
        if item_bytes.is_empty() {
            return Err(SourceError::OpenFailed("empty pdf bytes".to_owned()));
        }
        let doc = PdfDocument::open_bytes(item_bytes.to_vec())
            .map_err(|e| SourceError::OpenFailed(e.to_string()))?;
        let pages = compute_page_layout(&doc);
        let id = self.alloc_id();
        self.lock_instances().insert(
            id,
            PdfInstance {
                doc,
                source: None,
                selection: None,
                pages,
                drag_anchor: None,
            },
        );
        Ok(id)
    }

    fn close(&self, instance: u64) {
        self.lock_instances().remove(&instance);
    }

    fn title(&self, _instance: u64) -> String {
        // Pdfium exposes a document title metadata field but Modo A
        // owns the display name on the Item itself. Return a stable
        // placeholder; the shell renders the Item title instead.
        "PDF".to_owned()
    }

    fn content_size(&self, instance: u64) -> (f32, f32) {
        let guard = self.lock_instances();
        let Some(inst) = guard.get(&instance) else {
            return (0.0, 0.0);
        };
        if inst.doc.page_count() == 0 {
            return (0.0, 0.0);
        }
        inst.doc.page_size(0).unwrap_or((0.0, 0.0))
    }

    fn render_tile(&self, _instance: u64, _req: TileRequest) -> Result<Tile, SourceError> {
        // Phase 2.7 keeps the legacy `freya-pdf` background renderer in
        // charge of the pane. Phase 5 (Modo B) wires this to
        // `freya_pdf::RenderPool` when the source lives in its own pane.
        Err(SourceError::RenderFailed(
            "pdf render_tile stub; served by legacy pipeline".to_owned(),
        ))
    }

    fn hit_test(&self, _instance: u64, _x: f32, _y: f32) -> Option<Vec<u8>> {
        None
    }

    fn resolve_anchor(&self, instance: u64, anchor: &[u8]) -> Option<AnchorView> {
        let anchor = decode_pdf_anchor(anchor)?;
        let guard = self.lock_instances();
        let inst = guard.get(&instance)?;
        let page = anchor.page();
        let title = format!("Página {}", page.saturating_add(1));
        let snippet = match anchor.char_range() {
            Some((start, end)) => {
                page_text_slice(&inst.doc, page, start as usize, end as usize).unwrap_or_default()
            }
            None => String::new(),
        };
        Some(AnchorView { title, snippet })
    }

    // `plugin_completions` is intentionally not overridden now —
    // the shell reads from `completion_catalog` which is populated
    // by `attach_item` whenever the PDF opens. Keeps completions
    // visible even after the instance closes.

    fn render_chip(&self, anchor: &[u8]) -> Option<ChipView> {
        let decoded = decode_pdf_anchor(anchor)?;
        let page = decoded.page().saturating_add(1);
        let has_range = decoded.char_range().is_some();
        let label = if has_range {
            format!("PDF p.{page} · fragmento")
        } else {
            format!("PDF p.{page}")
        };
        Some(ChipView {
            label,
            icon: Some("\u{1F4D6}".to_owned()), // 📖
            tint: None,
            tooltip: Some("Referencia a PDF".to_owned()),
        })
    }

    fn pointer_begin_select(&self, instance: u64, x: f32, y: f32, mode: SelectMode) -> bool {
        let mut guard = self.lock_instances();
        let Some(inst) = guard.get_mut(&instance) else {
            return false;
        };
        let Some((page, local_x, local_y)) = hit_page(&inst.pages, x, y) else {
            return false;
        };
        let Ok(Some(char_idx)) = inst.doc.hit_char_at(page as usize, local_x, local_y) else {
            return false;
        };
        let (start_char, end_char) = match mode {
            SelectMode::Point => (char_idx as u32, char_idx as u32),
            SelectMode::Word => {
                if let Ok((text, starts)) = inst.doc.extract_page_text_indexed(page as usize) {
                    let (s, e) = word_bounds(&text, &starts, char_idx);
                    (s as u32, e as u32)
                } else {
                    (char_idx as u32, char_idx as u32)
                }
            }
        };
        inst.drag_anchor = Some(DragAnchor {
            page,
            char_idx: start_char,
            mode,
        });
        inst.selection = Some(PdfSelection {
            start_page: page,
            start_char,
            end_page: page,
            end_char,
        });
        true
    }

    fn pointer_extend_select(&self, instance: u64, x: f32, y: f32) {
        let mut guard = self.lock_instances();
        let Some(inst) = guard.get_mut(&instance) else {
            return;
        };
        let Some(anchor) = inst.drag_anchor else {
            return;
        };
        let Some((page, local_x, local_y)) = hit_page(&inst.pages, x, y) else {
            return;
        };
        let Ok(Some(char_idx)) = inst.doc.hit_char_at(page as usize, local_x, local_y) else {
            return;
        };
        // Word mode: snap both the live end and the anchor-side end
        // outward so a drag that starts mid-word still highlights whole
        // words through the pointer path.
        let (start_page, start_char, end_page, end_char) = match anchor.mode {
            SelectMode::Point => (anchor.page, anchor.char_idx, page, char_idx as u32),
            SelectMode::Word => {
                let anchor_bounds = inst
                    .doc
                    .extract_page_text_indexed(anchor.page as usize)
                    .ok()
                    .map(|(t, s)| word_bounds(&t, &s, anchor.char_idx as usize));
                let live_bounds = inst
                    .doc
                    .extract_page_text_indexed(page as usize)
                    .ok()
                    .map(|(t, s)| word_bounds(&t, &s, char_idx));
                let (as_s, as_e) = anchor_bounds
                    .unwrap_or((anchor.char_idx as usize, anchor.char_idx as usize + 1));
                let (ls_s, ls_e) = live_bounds.unwrap_or((char_idx, char_idx + 1));
                // Direction of the drag relative to the anchor word.
                let going_right = (page, char_idx as u32) >= (anchor.page, anchor.char_idx);
                if going_right {
                    (anchor.page, as_s as u32, page, ls_e as u32)
                } else {
                    (page, ls_s as u32, anchor.page, as_e as u32)
                }
            }
        };
        inst.selection = Some(PdfSelection {
            start_page,
            start_char,
            end_page,
            end_char,
        });
    }

    fn pointer_end_select(&self, instance: u64) {
        if let Some(inst) = self.lock_instances().get_mut(&instance) {
            inst.drag_anchor = None;
        }
    }

    fn clear_selection(&self, instance: u64) {
        if let Some(inst) = self.lock_instances().get_mut(&instance) {
            inst.drag_anchor = None;
            inst.selection = None;
        }
    }

    fn current_selection(&self, instance: u64) -> Option<Selection> {
        let guard = self.lock_instances();
        let inst = guard.get(&instance)?;
        let sel = inst.selection?;
        let source = inst.source?;
        if sel.is_empty() {
            return None;
        }
        // Single-page selections become proper Range anchors; cross-page
        // selections fall back to the start page so chip resolve still
        // navigates somewhere useful (anchor shape doesn't model page
        // spans yet).
        let (page, start_char, end_char) = if sel.start_page == sel.end_page {
            (sel.start_page, sel.start_char, sel.end_char)
        } else {
            (sel.start_page, sel.start_char, u32::MAX)
        };
        let snippet = {
            // Drop the guard before re-locking inside selection_snippet
            // (it would deadlock otherwise since std::sync::Mutex isn't
            // reentrant).
            drop(guard);
            self.selection_snippet(instance, sel)
        };
        let anchor = encode_pdf_anchor(PdfAnchor::Range {
            source,
            page,
            start_char,
            end_char,
        });
        Some(Selection { anchor, snippet })
    }

    fn search_begin(&self, _instance: u64, _query: &str) -> u64 {
        0
    }

    fn search_poll(&self, _stream_id: u64) -> Vec<SearchHit> {
        Vec::new()
    }

    fn search_end(&self, _stream_id: u64) {}

    fn config_schema(&self) -> ConfigSchema {
        ConfigSchema {
            version: 1,
            fields: vec![ConfigField {
                key: CONFIG_KEY_RENDER_DPI.to_owned(),
                label: "Resolución de render".to_owned(),
                help: "DPI de rasterizado de páginas PDF (72–600).".to_owned(),
                kind: ConfigFieldKind::Float {
                    min: MIN_RENDER_DPI,
                    max: MAX_RENDER_DPI,
                    step: 1.0,
                },
                default: ConfigValue::Float(DEFAULT_RENDER_DPI),
                aliases: Vec::new(),
            }],
        }
    }

    fn get_config(&self, key: &str) -> Option<ConfigValue> {
        match key {
            CONFIG_KEY_RENDER_DPI => Some(ConfigValue::Float(self.lock_config().render_dpi)),
            _ => None,
        }
    }

    fn set_config(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
        match (key, value) {
            (CONFIG_KEY_RENDER_DPI, ConfigValue::Float(v)) => {
                if !v.is_finite() || !(MIN_RENDER_DPI..=MAX_RENDER_DPI).contains(&v) {
                    return Err(ConfigError::OutOfRange);
                }
                self.lock_config().render_dpi = v;
                Ok(())
            }
            (CONFIG_KEY_RENDER_DPI, _) => Err(ConfigError::TypeMismatch),
            (other, _) => Err(ConfigError::UnknownKey(other.to_owned())),
        }
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            CreationEntry {
                id: IMPORT_PDF.to_owned(),
                label: "Importar PDF".to_owned(),
                icon: lucide::file_text(),
                category: CreationCategory::Source,
                kind: CreationKind::Import {
                    dialog_title: "Elegí un PDF".to_owned(),
                    extensions: vec!["pdf".to_owned()],
                    mime_types: vec!["application/pdf".to_owned()],
                },
            },
            CreationEntry {
                id: IMPORT_PDF_LINKED.to_owned(),
                label: "Importar PDF ligado".to_owned(),
                icon: lucide::file_symlink(),
                category: CreationCategory::Source,
                kind: CreationKind::Import {
                    dialog_title: "Elegí un PDF para ligar a un lienzo".to_owned(),
                    extensions: vec!["pdf".to_owned()],
                    mime_types: vec!["application/pdf".to_owned()],
                },
            },
        ]
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        import_bytes: Option<&[u8]>,
        name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        if entry_id != IMPORT_PDF && entry_id != IMPORT_PDF_LINKED {
            return Err(CreationError::UnknownEntry(entry_id.to_owned()));
        }
        let bytes = import_bytes.ok_or(CreationError::MissingImportBytes)?;
        if bytes.is_empty() {
            return Err(CreationError::Invalid("empty pdf bytes".to_owned()));
        }
        // Validate the file parses before handing it back to the shell.
        PdfDocument::open_bytes(bytes.to_vec())
            .map_err(|e| CreationError::BackendFailed(e.to_string()))?;
        let suggested_name = name_hint.map_or_else(|| "Documento PDF".to_owned(), str::to_owned);
        Ok(ItemCreationResult {
            item_bytes: bytes.to_vec(),
            suggested_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_entries_expose_import_pdf() {
        let plugin = PdfSourcePlugin::default();
        let ids: Vec<String> = plugin
            .creation_entries()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert!(ids.iter().any(|id| id == IMPORT_PDF));
    }

    #[test]
    fn open_rejects_empty_bytes() {
        let plugin = PdfSourcePlugin::default();
        assert!(matches!(plugin.open(&[]), Err(SourceError::OpenFailed(_))));
    }

    #[test]
    fn execute_creation_unknown_entry_errors() {
        let plugin = PdfSourcePlugin::default();
        let err = plugin
            .execute_creation("not-import", None, None)
            .expect_err("should fail");
        assert!(matches!(err, CreationError::UnknownEntry(_)));
    }

    #[test]
    fn execute_creation_requires_bytes() {
        let plugin = PdfSourcePlugin::default();
        let err = plugin
            .execute_creation(IMPORT_PDF, None, None)
            .expect_err("should fail");
        assert!(matches!(err, CreationError::MissingImportBytes));
    }

    #[test]
    fn file_extensions_lists_pdf() {
        let plugin = PdfSourcePlugin::default();
        assert_eq!(plugin.file_extensions(), &["pdf"]);
    }

    #[test]
    fn config_round_trip_render_dpi() {
        let plugin = PdfSourcePlugin::default();
        plugin
            .set_config(CONFIG_KEY_RENDER_DPI, ConfigValue::Float(200.0))
            .expect("set");
        match plugin.get_config(CONFIG_KEY_RENDER_DPI) {
            Some(ConfigValue::Float(v)) => assert!((v - 200.0).abs() < f64::EPSILON),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn config_rejects_out_of_range() {
        let plugin = PdfSourcePlugin::default();
        let err = plugin
            .set_config(CONFIG_KEY_RENDER_DPI, ConfigValue::Float(10.0))
            .expect_err("too low");
        assert!(matches!(err, ConfigError::OutOfRange));
    }
}
