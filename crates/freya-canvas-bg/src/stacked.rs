//! Vertical stack of [`PageProvider`]s — the classic paginated-document
//! background.
//!
//! Pages sit stacked top-to-bottom in world coordinates with a
//! configurable gap between them and are centred horizontally on
//! `x = 0` so mixed-width pages stay aligned. The composite culls
//! per-page before delegating paint so a 5000-page document only walks
//! the layout table for the visible slice.

use std::sync::Arc;

use crate::{
    AttributionMode, BgPaintCtx, CanvasBackground, Color, PageAttachment, PageId, PageLayout,
    Rect,
    page::PageProvider,
};

/// Vertical stack composite background. Cheap to reconfigure at
/// runtime — mutating the provider list recomputes the cached layout
/// in O(N) once instead of per-frame.
#[derive(Debug)]
pub struct StackedPagesBackground {
    providers: Vec<Arc<dyn PageProvider>>,
    gap: f32,
    backdrop: Color,
    attribution: AttributionMode,
    layout: Vec<PageLayout>,
    bounds: Rect,
}

impl StackedPagesBackground {
    /// Empty stack. Push providers via [`Self::push`] / [`Self::extend`].
    #[must_use]
    pub fn new(gap: f32, backdrop: Color) -> Self {
        Self {
            providers: Vec::new(),
            gap,
            backdrop,
            attribution: AttributionMode::PerPage,
            layout: Vec::new(),
            bounds: EMPTY_RECT,
        }
    }

    /// Build directly from a provider list.
    #[must_use]
    pub fn from_providers(
        providers: Vec<Arc<dyn PageProvider>>,
        gap: f32,
        backdrop: Color,
    ) -> Self {
        let mut this = Self::new(gap, backdrop);
        this.providers = providers;
        this.recompute();
        this
    }

    /// Override the stroke attribution mode. Default is
    /// [`AttributionMode::PerPage`] — appropriate for document
    /// backgrounds. Switch to [`AttributionMode::World`] for
    /// scrapbook-style backgrounds where strokes should not follow
    /// page reorders.
    #[must_use]
    pub const fn with_attribution(mut self, mode: AttributionMode) -> Self {
        self.attribution = mode;
        self
    }

    /// Append one page provider to the end of the stack.
    pub fn push(&mut self, provider: Arc<dyn PageProvider>) {
        self.providers.push(provider);
        self.recompute();
    }

    /// Append multiple page providers to the end of the stack.
    pub fn extend<I>(&mut self, providers: I)
    where
        I: IntoIterator<Item = Arc<dyn PageProvider>>,
    {
        self.providers.extend(providers);
        self.recompute();
    }

    /// Remove every provider. `content_bounds` reverts to `None`.
    pub fn clear(&mut self) {
        self.providers.clear();
        self.recompute();
    }

    /// Number of pages currently in the stack.
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// `true` when there are no page providers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    fn recompute(&mut self) {
        self.layout.clear();
        if self.providers.is_empty() {
            self.bounds = EMPTY_RECT;
            return;
        }
        let max_w = self
            .providers
            .iter()
            .map(|p| p.natural_size().0)
            .fold(0.0_f32, f32::max);
        let mut y = 0.0_f32;
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        for (idx, provider) in self.providers.iter().enumerate() {
            let (w, h) = provider.natural_size();
            let x = -(w * 0.5);
            let rect = Rect {
                min_x: x,
                min_y: y,
                max_x: x + w,
                max_y: y + h,
            };
            min_x = min_x.min(rect.min_x);
            max_x = max_x.max(rect.max_x);
            self.layout.push(PageLayout {
                #[allow(clippy::cast_possible_truncation)]
                id: PageId(idx as u64),
                rect,
                natural_size: (w, h),
            });
            y += h + self.gap;
        }
        let bottom = y - self.gap;
        let widest_half = max_w * 0.5;
        self.bounds = Rect {
            min_x: min_x.min(-widest_half),
            min_y: 0.0,
            max_x: max_x.max(widest_half),
            max_y: bottom,
        };
    }
}

impl CanvasBackground for StackedPagesBackground {
    fn content_bounds(&self) -> Option<Rect> {
        if self.layout.is_empty() { None } else { Some(self.bounds) }
    }

    fn pages(&self) -> &[PageLayout] {
        &self.layout
    }

    fn viewport_backdrop(&self) -> Option<Color> {
        Some(self.backdrop)
    }

    fn attribution_mode(&self) -> AttributionMode {
        self.attribution
    }

    fn locate(&self, x: f32, y: f32) -> Option<PageAttachment> {
        self.layout.iter().find_map(|pl| {
            if pl.rect.contains(x, y) {
                Some(PageAttachment {
                    page: pl.id,
                    local: (x - pl.rect.min_x, y - pl.rect.min_y),
                })
            } else {
                None
            }
        })
    }

    fn paint(&self, cx: &mut BgPaintCtx<'_>) {
        for (i, pl) in self.layout.iter().enumerate() {
            if !pl.rect.intersects(&cx.visible) {
                continue;
            }
            self.providers[i].paint(cx.canvas, pl.rect, cx.scale);
        }
    }
}

const EMPTY_RECT: Rect = Rect {
    min_x: 0.0,
    min_y: 0.0,
    max_x: 0.0,
    max_y: 0.0,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{SolidPageProvider, paper};

    fn provider(size: (f32, f32)) -> Arc<dyn PageProvider> {
        Arc::new(SolidPageProvider::new(size, Color::from_rgb(255, 255, 255)))
    }

    #[test]
    fn empty_stack_has_no_bounds() {
        let bg = StackedPagesBackground::new(10.0, Color::from_rgb(90, 90, 96));
        assert!(bg.content_bounds().is_none());
        assert!(bg.pages().is_empty());
    }

    #[test]
    fn stacks_pages_vertically_with_gap() {
        let mut bg = StackedPagesBackground::new(10.0, Color::from_rgb(90, 90, 96));
        bg.push(provider((100.0, 200.0)));
        bg.push(provider((100.0, 300.0)));
        let pages = bg.pages();
        assert_eq!(pages.len(), 2);
        assert!((pages[0].rect.min_y - 0.0).abs() < 1e-4);
        assert!((pages[0].rect.max_y - 200.0).abs() < 1e-4);
        assert!((pages[1].rect.min_y - 210.0).abs() < 1e-4);
        assert!((pages[1].rect.max_y - 510.0).abs() < 1e-4);
        let bounds = bg.content_bounds().expect("bounds set");
        assert!((bounds.max_y - 510.0).abs() < 1e-4);
    }

    #[test]
    fn centres_pages_horizontally() {
        let mut bg = StackedPagesBackground::new(0.0, Color::from_rgb(90, 90, 96));
        bg.push(provider((100.0, 100.0)));
        let rect = bg.pages()[0].rect;
        assert!((rect.min_x + 50.0).abs() < 1e-4);
        assert!((rect.max_x - 50.0).abs() < 1e-4);
    }

    #[test]
    fn locate_returns_page_local_coords() {
        let mut bg = StackedPagesBackground::new(0.0, Color::from_rgb(90, 90, 96));
        bg.push(provider((100.0, 100.0)));
        let hit = bg.locate(-40.0, 20.0).expect("inside page 0");
        assert_eq!(hit.page, PageId(0));
        // Page rect is (-50, 0) → (50, 100); local of (-40, 20) is (10, 20).
        assert!((hit.local.0 - 10.0).abs() < 1e-4);
        assert!((hit.local.1 - 20.0).abs() < 1e-4);
        assert!(bg.locate(1_000.0, 1_000.0).is_none());
    }

    #[test]
    fn from_providers_uses_paper_helpers() {
        let bg = StackedPagesBackground::from_providers(
            vec![Arc::new(paper::a4()), Arc::new(paper::letter())],
            12.0,
            Color::from_rgb(90, 90, 96),
        );
        assert_eq!(bg.len(), 2);
        assert_eq!(bg.pages()[0].natural_size, paper::A4);
        assert_eq!(bg.pages()[1].natural_size, paper::LETTER);
    }
}
