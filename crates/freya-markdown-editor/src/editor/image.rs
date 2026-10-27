//! An image inside the text: drawn by a decorator, and pressed to replace,
//! describe or remove it without touching markdown.

use freya::icons::lucide;
use freya::prelude::*;

use super::Shared;
use crate::controller::{AtomSlot, ImageRequest};
use crate::decorator::ImageContext;
use crate::labels::Label;
use crate::popup::{Anchored, MenuItem, input_colors, menu_separator, popup_frame};

#[derive(Clone, PartialEq)]
pub(crate) struct ImageAtom {
    pub(crate) url: String,
    pub(crate) alt: String,
    pub(crate) title: Option<String>,
    pub(crate) slot: AtomSlot,
    pub(crate) max_width: f32,
    pub(crate) shared: Shared,
}

impl ImageAtom {
    /// Shown when no decorator draws the image.
    fn placeholder(&self) -> Element {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let name = if self.alt.is_empty() {
            self.url.rsplit('/').next().unwrap_or(&self.url).to_owned()
        } else {
            self.alt.clone()
        };
        rect()
            .padding((theme.px(6.), theme.px(10.)))
            .corner_radius(theme.px(8.))
            .background(colors.code_background)
            .horizontal()
            .spacing(theme.px(8.))
            .cross_align(Alignment::center())
            .child(
                SvgViewer::new(lucide::image())
                    .color(colors.muted)
                    .width(Size::px(theme.px(16.)))
                    .height(Size::px(theme.px(16.))),
            )
            .child(
                label()
                    .font_size(theme.px(13.))
                    .color(colors.muted)
                    .max_lines(1)
                    .text(name),
            )
            .into_element()
    }
}

impl Component for ImageAtom {
    fn render(&self) -> impl IntoElement {
        // The text block gets the press and opens the menu through the
        // controller, see `image_at` there.
        let describing = use_state(|| false);
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let read_only = self.shared.read_only;
        let shown = self
            .shared
            .decorators
            .render_image(&ImageContext {
                url: &self.url,
                alt: &self.alt,
                title: self.title.as_deref(),
                max_width: self.max_width,
                read_only,
                theme,
            })
            .unwrap_or_else(|| self.placeholder());
        if read_only {
            return shown;
        }

        let controller = self.shared.controller;
        let open = controller.atom_menu() == Some(self.slot);
        let frame = rect()
            .corner_radius(theme.px(8.))
            .border(
                Border::new()
                    .width(theme.px(2.))
                    .alignment(BorderAlignment::Outer)
                    .fill(if open {
                        colors.accent
                    } else {
                        Color::TRANSPARENT
                    }),
            )
            .cursor(CursorIcon::Pointer)
            .child(shown)
            .into_element();
        let popup = open.then(|| {
            if describing() {
                self.describe(describing)
            } else {
                self.actions(describing)
            }
        });
        let mut describing = describing;
        Anchored::new(frame, popup)
            .on_dismiss(move || {
                describing.set(false);
                controller.open_atom_menu(None);
            })
            .into_element()
    }
}

impl ImageAtom {
    fn actions(&self, mut describing: State<bool>) -> Element {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let controller = self.shared.controller;
        let slot = self.slot;
        let close = EventHandler::new(move |()| controller.open_atom_menu(None));
        let mut rows = Vec::new();
        if controller.can_pick_image() {
            rows.push(
                MenuItem::new(theme.labels.get(Label::ReplaceImage), move || {
                    controller.request_image(ImageRequest::Replace(slot));
                })
                .icon(lucide::replace(), colors.text)
                .row(0, colors, Some(close.clone())),
            );
        }
        // Describing swaps the menu for a field, so it must not close it.
        rows.push(
            MenuItem::new(theme.labels.get(Label::AltText), move || {
                describing.set(true);
            })
            .icon(lucide::text_quote(), colors.text)
            .row(1, colors, None),
        );
        rows.push(menu_separator(colors));
        rows.push(
            MenuItem::new(theme.labels.get(Label::RemoveImage), move || {
                controller.remove_image(slot);
            })
            .icon(lucide::trash_2(), colors.caution)
            .row(2, colors, Some(close)),
        );
        popup_frame(colors)
            .padding(4.)
            .width(Size::px(220.))
            .children(rows)
            .into_element()
    }

    fn describe(&self, mut describing: State<bool>) -> Element {
        DescribeField {
            alt: self.alt.clone(),
            slot: self.slot,
            on_done: EventHandler::new(move |()| describing.set(false)),
            shared: self.shared.clone(),
        }
        .into_element()
    }
}

/// Field to write an image's description, starting from the current one.
#[derive(Clone, PartialEq)]
struct DescribeField {
    alt: String,
    slot: AtomSlot,
    on_done: EventHandler<()>,
    shared: Shared,
}

impl Component for DescribeField {
    fn render(&self) -> impl IntoElement {
        let initial = self.alt.clone();
        let draft = use_state(move || initial);
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let controller = self.shared.controller;
        let slot = self.slot;
        let on_done = self.on_done.clone();
        popup_frame(colors)
            .padding(6.)
            .width(Size::px(280.))
            .spacing(6.)
            .child(
                Input::new(draft)
                    .placeholder(theme.labels.get(Label::AltText))
                    .theme_colors(input_colors(colors))
                    .auto_focus(true)
                    .width(Size::fill())
                    .on_submit(move |alt: String| {
                        controller.set_image_alt(slot, &alt);
                        controller.open_atom_menu(None);
                        on_done.call(());
                    }),
            )
            .child(
                label()
                    .font_size(11.5)
                    .color(colors.muted)
                    .text(theme.labels.get(Label::AltTextHint)),
            )
    }
}
