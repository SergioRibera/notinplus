//! Quotes that keep showing what their source holds.

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use notin_i18n::tr;
use notin_plugin::bookmark::SourceRef;
use notin_plugin::services::Placement;
use notin_ui_kit::theme::palette;
use notin_ui_kit::{IconButton, Popover, press_anchor, tokens};

use super::CloseOnNavigate;
use crate::references;
use crate::state::LibraryView;

/// Marker of a quote that shows its source as it is now.
pub const LIVE_QUOTE: &str = "notein:live";

/// Draws [`LIVE_QUOTE`] transclusions from their source.
pub(super) struct LiveQuotes;

/// Provided around the body of a quote: a quote inside it shows what it
/// held when inserted, so a section quoting itself cannot recurse.
#[derive(Clone, Copy)]
struct InsideQuote;

impl Decorator for LiveQuotes {
    #[sismo::function("editor::live_quotes")]
    fn render_transclusion(&self, transclusion: &TransclusionContext<'_>) -> Option<Element> {
        if transclusion.kind != LIVE_QUOTE || try_consume_context::<InsideQuote>().is_some() {
            return None;
        }
        let mut theme = transclusion.theme.clone();
        if let Some(color) = transclusion.color {
            theme.colors.text = color.into();
        }
        Some(
            LiveQuote {
                target: SourceRef::from_link(transclusion.url),
                label: transclusion.label.to_owned(),
                fallback: transclusion.fallback.to_owned(),
                theme,
                color: transclusion.color,
                recolor: (!transclusion.read_only)
                    .then_some((transclusion.controller, transclusion.block)),
            }
            .into_element(),
        )
    }
}

#[derive(Clone, PartialEq)]
struct LiveQuote {
    target: SourceRef,
    /// Source as written when inserted.
    label: String,
    /// Content as written when inserted.
    fallback: String,
    /// The editor's theme with the quote's text colour.
    theme: EditorTheme,
    /// Colour chosen for this quote, `None` while it follows its text.
    color: Option<Rgba>,
    /// Where a new colour is written; `None` while read only.
    recolor: Option<(EditorController, BlockId)>,
}

impl Component for LiveQuote {
    #[sismo::function("editor::live_quote")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovered = use_state(|| false);
        let data = LibraryView::get().data();
        let chip = references::display(&self.target, data.as_deref());
        let (body, source, gone) = match references::quote(&self.target, data.as_deref()) {
            Some(quote) => (quote.body, quote.source, false),
            None => (self.fallback.clone(), self.label.clone(), true),
        };
        let zoom = self.theme.zoom;
        let px = |length: f32| length * zoom;
        let tint = chip.color.resolve(&p);
        let mut theme = self.theme.clone();
        if gone {
            theme.colors.text = p.t3;
        }
        let target = self.target.clone();
        let recolor = self.recolor.map(|(controller, block)| {
            let current = self.color;
            IconButton::new(freya::icons::lucide::palette(), tr!("editor-quote-color"))
                .size(tokens::size::BUTTON_SM * zoom.max(0.6))
                .on_press(move |event: Event<PressEventData>| {
                    event.stop_propagation();
                    open_colors(press_anchor(&event), controller, block, current);
                })
        });
        rect()
            .width(Size::fill())
            .padding((px(8.), px(12.)))
            .spacing(px(6.))
            .corner_radius(px(6.))
            .background(if hovered() { p.s2 } else { Color::TRANSPARENT })
            .border(
                Border::new()
                    .width(BorderWidth {
                        top: 0.,
                        right: 0.,
                        bottom: 0.,
                        left: px(3.),
                    })
                    .alignment(BorderAlignment::Inner)
                    .fill(if gone { p.t3 } else { tint }),
            )
            .cursor(CursorIcon::Pointer)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set_if_modified(false))
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                crate::plugins::navigate::open(&target, Placement::Preferred);
                if let Some(close) = try_consume_context::<CloseOnNavigate>() {
                    (close.0)();
                }
            })
            // Taps go through to the quote, which opens its source.
            .child(
                rect()
                    .width(Size::fill())
                    .interactive(false)
                    .child(QuoteBody {
                        markdown: body,
                        theme,
                    }),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::flex())
                    .cross_align(Alignment::Center)
                    .spacing(px(6.))
                    .child(
                        SvgViewer::new(chip.icon)
                            .color(if gone { p.t3 } else { tint })
                            .width(Size::px(px(13.)))
                            .height(Size::px(px(13.))),
                    )
                    .child(
                        label()
                            .width(Size::flex(1.))
                            .color(p.t2)
                            .font_size(px(13.))
                            .max_lines(1)
                            .text_overflow(TextOverflow::Ellipsis)
                            .text(if gone {
                                tr!("editor-quote-gone", "source" => source)
                            } else {
                                source
                            }),
                    )
                    .maybe_child(recolor)
                    .child(
                        SvgViewer::new(freya::icons::lucide::refresh_cw())
                            .color(p.t3)
                            .width(Size::px(px(12.)))
                            .height(Size::px(px(12.))),
                    ),
            )
    }
}

/// The quoted markdown, drawn by a read only editor in the quote's theme.
#[derive(Clone, PartialEq)]
struct QuoteBody {
    markdown: String,
    theme: EditorTheme,
}

impl Component for QuoteBody {
    #[sismo::function("editor::quote_body")]
    fn render(&self) -> impl IntoElement {
        use_provide_context(|| InsideQuote);
        let controller = use_editor_controller();
        let mut value = use_state(|| self.markdown.clone());
        if *value.peek() != self.markdown {
            value.set(self.markdown.clone());
        }
        super::editor(value, controller)
            .read_only(true)
            .theme(self.theme.clone())
            .width(Size::fill())
    }
}

/// Swatches for the quote's text colour; "default" follows its text again.
fn open_colors(anchor: Area, controller: EditorController, block: BlockId, current: Option<Rgba>) {
    let theme = super::theme(&palette());
    let body = color_palette(&theme, current, move |color| {
        controller.set_transclusion_color(block, color);
        notin_ui_kit::ModalController::get().close();
    });
    Popover::new(anchor, body).width(170.0).open();
}
