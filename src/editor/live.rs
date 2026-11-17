//! Quotes that keep showing what their source holds.
//!
//! Works for every document kind through the plugin contract: the kind
//! writes the quote ([`notin_plugin::DocumentKind::quote_with`]) and names
//! the parts a reader may show or hide
//! ([`notin_plugin::DocumentKind::quote_options`]). The reader's choices
//! live on the quote's marker, next to its text colour:
//! `> [!notein:live color=#rrggbb hide=a,b show=c]`.

use freya::prelude::*;
use freya_markdown_editor::document::TRANSCLUSION_COLOR;
use freya_markdown_editor::prelude::*;
use notin_i18n::tr;
use notin_plugin::bookmark::SourceRef;
use notin_plugin::kind::{QuoteOption, QuoteParts};
use notin_plugin::services::Placement;
use notin_ui_kit::theme::palette;
use notin_ui_kit::{IconButton, ListRow, Popover, Trailing, press_anchor, tokens};

use super::CloseOnNavigate;
use crate::references;
use crate::state::LibraryView;

/// Marker of a quote that shows its source as it is now.
pub const LIVE_QUOTE: &str = "notein:live";
/// Marker setting: ids of the kind's quote options the reader hid.
pub(crate) const QUOTE_HIDE: &str = "hide";
/// Marker setting: ids of the kind's quote options the reader showed.
pub(crate) const QUOTE_SHOW: &str = "show";

/// Marker settings this host reads; any other is kept but logged.
const KNOWN_SETTINGS: [&str; 3] = [TRANSCLUSION_COLOR, QUOTE_HIDE, QUOTE_SHOW];

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
        for (key, value) in transclusion.attrs {
            if !KNOWN_SETTINGS.contains(&key.as_str()) {
                log::debug!("live quote: unknown setting {key}={value}, kept as is");
            }
        }
        let color = transclusion.color();
        let theme = color.map_or_else(
            || transclusion.theme.clone(),
            |color| transclusion.theme.clone().tinted(color.into()),
        );
        let target = SourceRef::from_link(transclusion.url);
        Some(
            LiveQuote {
                options: references::quote_options(&target),
                target,
                label: transclusion.label.to_owned(),
                fallback: transclusion.fallback.to_owned(),
                theme,
                settings: Settings {
                    color,
                    parts: QuoteParts::from_lists(
                        transclusion.attr(QUOTE_HIDE),
                        transclusion.attr(QUOTE_SHOW),
                    ),
                },
                edit: (!transclusion.read_only)
                    .then_some((transclusion.controller, transclusion.block)),
            }
            .into_element(),
        )
    }
}

/// What the reader chose for one quote.
#[derive(Clone, Debug, Default, PartialEq)]
struct Settings {
    /// Text colour; `None` follows the text around the quote.
    color: Option<Rgba>,
    parts: QuoteParts,
}

#[derive(Clone, PartialEq)]
struct LiveQuote {
    target: SourceRef,
    /// Parts its kind lets the reader show or hide.
    options: Vec<QuoteOption>,
    /// Source as written when inserted.
    label: String,
    /// Content as written when inserted.
    fallback: String,
    /// The editor's theme with the quote's text colour.
    theme: EditorTheme,
    settings: Settings,
    /// Where new settings are written; `None` while read only.
    edit: Option<(EditorController, BlockId)>,
}

impl Component for LiveQuote {
    #[sismo::function("editor::live_quote")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovered = use_state(|| false);
        let data = LibraryView::get().data();
        let chip = references::display(&self.target, data.as_deref());
        let found = references::quote_with(&self.target, data.as_deref(), &self.settings.parts);
        let (body, interlinear, source, gone) = match found {
            Some(quote) => (quote.body, quote.interlinear, quote.source, false),
            None => (self.fallback.clone(), None, self.label.clone(), true),
        };
        let zoom = self.theme.zoom;
        let px = |length: f32| length * zoom;
        let tint = chip.color.resolve(&p);
        let mut theme = self.theme.clone();
        if gone {
            theme = theme.tinted(p.t3);
        }
        let open = {
            let target = self.target.clone();
            move || {
                crate::plugins::navigate::open(&target, Placement::Preferred);
                if let Some(close) = try_consume_context::<CloseOnNavigate>() {
                    (close.0)();
                }
            }
        };
        let menu = self.edit.map(|(controller, block)| {
            let options = self.options.clone();
            let settings = self.settings.clone();
            let (icon, hint) = if options.is_empty() {
                (freya::icons::lucide::palette(), tr!("editor-quote-color"))
            } else {
                (
                    freya::icons::lucide::ellipsis(),
                    tr!("editor-quote-options"),
                )
            };
            IconButton::new(icon, hint)
                .size(tokens::size::BUTTON_SM * zoom.max(0.6))
                .on_press(move |event: Event<PressEventData>| {
                    event.stop_propagation();
                    QuoteMenu {
                        controller,
                        block,
                        options: options.clone(),
                        settings: settings.clone(),
                    }
                    .open(press_anchor(&event));
                })
        });
        let footer_open = open.clone();
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
            // Links and chips inside the body take their own taps; the rest
            // of the quote opens its source.
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                open();
            })
            .maybe_child(interlinear.map(|interlinear| {
                super::interlinear::InterlinearView {
                    interlinear,
                    theme: theme.clone(),
                }
                .into_element()
            }))
            .maybe_child((!body.trim().is_empty()).then(|| {
                QuoteBody {
                    markdown: body,
                    theme,
                }
                .into_element()
            }))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::flex())
                    .cross_align(Alignment::Center)
                    .spacing(px(6.))
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(px(6.))
                            .on_press(move |event: Event<PressEventData>| {
                                event.stop_propagation();
                                footer_open();
                            })
                            .child(
                                SvgViewer::new(chip.icon)
                                    .color(if gone { p.t3 } else { tint })
                                    .width(Size::px(px(13.)))
                                    .height(Size::px(px(13.))),
                            )
                            .child(
                                label()
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
                                SvgViewer::new(freya::icons::lucide::arrow_up_right())
                                    .color(p.t3)
                                    .width(Size::px(px(12.)))
                                    .height(Size::px(px(12.))),
                            ),
                    )
                    .maybe_child(menu),
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

/// The parts a quote's kind lets the reader show or hide, and its text
/// colour. Opened from the quote's menu button.
#[derive(Clone, PartialEq)]
struct QuoteMenu {
    controller: EditorController,
    block: BlockId,
    options: Vec<QuoteOption>,
    settings: Settings,
}

impl QuoteMenu {
    fn open(self, anchor: Area) {
        Popover::new(anchor, self)
            .width(280.0)
            .sheet_title(tr!("editor-quote-options"))
            .open();
    }
}

impl Component for QuoteMenu {
    #[sismo::function("editor::quote_menu")]
    fn render(&self) -> impl IntoElement {
        let parts = use_state(|| self.settings.parts.clone());
        let (controller, block) = (self.controller, self.block);
        let rows: Vec<Element> = self
            .options
            .iter()
            .map(|option| {
                let option = option.clone();
                let on = parts.read().shows(&option);
                let mut parts = parts;
                ListRow::new(option.label.clone())
                    .dense(true)
                    .trailing(Trailing::Switch {
                        on,
                        on_toggle: EventHandler::new(move |show: bool| {
                            let mut next = parts.peek().clone();
                            next.set(&option, show);
                            let (hidden, shown) = next.to_lists();
                            controller.set_transclusion_attr(block, QUOTE_HIDE, hidden.as_deref());
                            controller.set_transclusion_attr(block, QUOTE_SHOW, shown.as_deref());
                            parts.set(next);
                        }),
                    })
                    .into_element()
            })
            .collect();
        let theme = super::theme(&palette());
        let colors = color_palette(&theme, self.settings.color, move |color: Option<Rgba>| {
            let hex = color.map(Rgba::to_hex);
            controller.set_transclusion_attr(block, TRANSCLUSION_COLOR, hex.as_deref());
            notin_ui_kit::ModalController::get().close();
        });
        rect()
            .width(Size::fill())
            .padding(8.)
            .spacing(8.)
            .maybe_child(
                (!rows.is_empty())
                    .then(|| notin_ui_kit::Group::default().children(rows).into_element()),
            )
            .child(
                label()
                    .font_size(12.)
                    .color(palette().t2)
                    .text(tr!("editor-quote-color")),
            )
            .child(colors)
    }
}
