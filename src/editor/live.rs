//! Quotes that keep showing what their source holds.

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use notein_i18n::tr;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::services::Placement;
use notein_ui_kit::theme::palette;

use super::CloseOnNavigate;
use crate::references::{self, QuoteLine};
use crate::state::LibraryView;

/// Marker of a quote that shows its source as it is now.
pub const LIVE_QUOTE: &str = "notein:live";

/// Draws [`LIVE_QUOTE`] transclusions from their source.
pub(super) struct LiveQuotes;

impl Decorator for LiveQuotes {
    fn render_transclusion(&self, transclusion: &TransclusionContext<'_>) -> Option<Element> {
        (transclusion.kind == LIVE_QUOTE).then(|| {
            LiveQuote {
                target: SourceRef::from_link(transclusion.url),
                label: transclusion.label.to_owned(),
                fallback: transclusion.fallback.to_owned(),
                size: transclusion.theme.body_size,
                zoom: transclusion.theme.zoom,
            }
            .into_element()
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct LiveQuote {
    target: SourceRef,
    /// Source as written when inserted.
    label: String,
    /// Content as written when inserted.
    fallback: String,
    size: f32,
    zoom: f32,
}

impl Component for LiveQuote {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovered = use_state(|| false);
        let data = LibraryView::get().data();
        let chip = references::display(&self.target, data.as_deref());
        let (lines, source, gone) = match references::quote(&self.target, data.as_deref()) {
            Some(quote) => (references::quote_text(&quote.body), quote.source, false),
            None => (
                references::quote_text(&self.fallback),
                self.label.clone(),
                true,
            ),
        };
        let px = |length: f32| length * self.zoom;
        let tint = chip.color.resolve(&p);
        let text = if gone { p.t3 } else { p.t1 };
        let body = lines
            .into_iter()
            .map(|line| quote_line(line, self.size, text, px(16.)));
        let target = self.target.clone();
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
            .children(body)
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
                    .child(
                        SvgViewer::new(freya::icons::lucide::refresh_cw())
                            .color(p.t3)
                            .width(Size::px(px(12.)))
                            .height(Size::px(px(12.))),
                    ),
            )
    }
}

fn quote_line(line: QuoteLine, size: f32, color: Color, indent: f32) -> Element {
    let text = match line.bullet {
        Some(_) => format!("• {}", line.text),
        None => line.text,
    };
    let depth = line.bullet.map_or(0., |depth| f32::from(depth) * indent);
    rect()
        .width(Size::fill())
        .padding((0., 0., 0., depth))
        .child(
            label()
                .width(Size::fill())
                .color(color)
                .font_size(size)
                .font_weight(if line.heading {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .text(text),
        )
        .into_element()
}
