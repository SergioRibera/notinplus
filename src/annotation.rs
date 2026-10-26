//! The annotation popover shared by verses, PDF fragments and canvas
//! pins, and the sidebar it expands into beside the mesa's panes.
//!
//! One open annotation is a [`Session`]: the request a plugin sent, the
//! draft being edited and the callbacks to report back. The popover and
//! the sidebar edit the same draft, so what is written in one shows in
//! the other, and every colour change previews on the document at once.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use freya::prelude::*;
use notein_plugin::services::{
    AnnotationDone, AnnotationOutcome, AnnotationPreview, AnnotationRequest,
};
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, ButtonVariant, ColorWheel, Icon, IconButton, ModalController, Popover, PopoverAlign,
    PopoverSide, Sheet, SheetFooter, SidePanel, Spacer, tokens, width_class,
};

use crate::editor::{CloseOnNavigate, EditorField};

pub use crate::library::AnnotationId;

/// Width of the sidebar an annotation expands into.
const SIDEBAR_WIDTH: f32 = 380.0;
/// Width of the colour picker hanging from the swatches.
const PICKER_WIDTH: f32 = 276.0;

/// What the user is editing.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Draft {
    color: Option<[u8; 3]>,
    body: String,
}

struct Session {
    request: AnnotationRequest,
    draft: State<Draft>,
    original: Draft,
    preview: Option<AnnotationPreview>,
    done: AnnotationDone,
}

impl Session {
    /// Change the draft's colour and show it on the document right away.
    fn recolor(&self, color: Option<[u8; 3]>) {
        let mut draft = self.draft;
        draft.write().color = color;
        if let Some(preview) = &self.preview {
            preview(color);
        }
    }
}

thread_local! {
    static SESSIONS: RefCell<HashMap<AnnotationId, Rc<Session>>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = const { Cell::new(1) };
    static ASIDE: Cell<Option<State<Option<AnnotationId>>>> = const { Cell::new(None) };
}

fn session(id: AnnotationId) -> Option<Rc<Session>> {
    SESSIONS.with(|s| s.borrow().get(&id).cloned())
}

/// Which annotation the sidebar shows; `None` while no mesa shows one.
fn aside() -> Option<State<Option<AnnotationId>>> {
    ASIDE.get()
}

/// End `id`, reporting `outcome` once.
fn finish(id: AnnotationId, outcome: AnnotationOutcome) {
    if let Some(session) = SESSIONS.with(|s| s.borrow_mut().remove(&id)) {
        (session.done)(outcome);
    }
    if let Some(mut aside) = aside()
        && *aside.peek() == Some(id)
    {
        aside.set(None);
    }
}

/// Save if the draft changed, otherwise cancel.
fn settle(id: AnnotationId) {
    let Some(session) = session(id) else {
        return;
    };
    let draft = session.draft.peek().clone();
    let outcome = if draft == session.original {
        AnnotationOutcome::Cancelled
    } else {
        AnnotationOutcome::Saved {
            color: draft.color,
            body: draft.body,
        }
    };
    finish(id, outcome);
}

/// Save the draft as it is.
fn settle_saving(id: AnnotationId) {
    let Some(session) = session(id) else {
        return;
    };
    let draft = session.draft.peek().clone();
    finish(
        id,
        AnnotationOutcome::Saved {
            color: draft.color,
            body: draft.body,
        },
    );
}

/// Recolour the draft of `id`.
fn recolor(id: AnnotationId, color: Option<[u8; 3]>) {
    if let Some(session) = session(id) {
        session.recolor(color);
    }
}

/// Entry points.
#[derive(Debug)]
pub struct AnnotationPopover;

impl AnnotationPopover {
    /// Show the annotation popover for `request`; in the sidebar instead
    /// while another annotation is expanded there.
    pub fn open(
        request: AnnotationRequest,
        preview: Option<AnnotationPreview>,
        done: AnnotationDone,
    ) {
        let id = AnnotationId(NEXT.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        }));
        let original = Draft {
            color: request.color,
            body: request.body.clone(),
        };
        let draft = State::create_in_scope(original.clone(), ScopeId::ROOT);
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                id,
                Rc::new(Session {
                    request,
                    draft,
                    original,
                    preview,
                    done,
                }),
            );
        });
        let docked = aside().is_some_and(|aside| aside.peek().is_some());
        if !(docked && Self::dock(id)) {
            Self::show(id);
        }
    }

    fn show(id: AnnotationId) {
        let Some(session) = session(id) else {
            return;
        };
        let card = AnnotationCard {
            id,
            expanded: false,
        };
        if width_class().is_compact() {
            Sheet::default().without_close().child(card).open();
        } else {
            let anchor = session
                .request
                .target
                .map_or_else(Area::default, |(x, y, w, h)| {
                    Area::new((x, y).into(), (w, h).into())
                });
            Popover::new(anchor, card)
                .width(tokens::size::POPOVER_WIDTH)
                .side(PopoverSide::Above)
                .align(PopoverAlign::Center)
                .estimated_height(380.0)
                .open();
        }
        let mut modals = ModalController::get();
        modals.set_top_on_close(move || settle(id));
    }

    /// Put `id` in the sidebar, settling the one there before; `false`
    /// when there is no sidebar to put it in.
    fn dock(id: AnnotationId) -> bool {
        let Some(mut aside) = aside().filter(|_| !width_class().is_compact()) else {
            return false;
        };
        let previous = *aside.peek();
        if let Some(previous) = previous.filter(|p| *p != id) {
            settle(previous);
        }
        aside.set(Some(id));
        true
    }

    /// Move the annotation from the popover to the sidebar.
    fn expand(id: AnnotationId) {
        if aside().is_none() {
            return;
        }
        let mut modals = ModalController::get();
        modals.clear_top_on_close();
        modals.close();
        Self::dock(id);
    }

    /// Back from the sidebar to the popover.
    fn collapse(id: AnnotationId) {
        if let Some(mut aside) = aside() {
            aside.set(None);
        }
        Self::show(id);
    }
}

/// The expanded annotation beside the panes of a mesa. A narrow window
/// has no room for it, and the annotation goes back to its sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnotationSidebar;

impl Component for AnnotationSidebar {
    fn render(&self) -> impl IntoElement {
        let mut aside = use_state(|| None);
        use_hook(|| ASIDE.set(Some(aside)));
        use_drop(move || {
            // Unregistered first: settling must not write to the state
            // that is going away.
            ASIDE.set(None);
            if let Some(id) = *aside.peek() {
                settle(id);
            }
        });
        let current = *aside.read();
        let compact = width_class().is_compact();
        if compact && let Some(id) = current {
            aside.set(None);
            AnnotationPopover::show(id);
        }
        // The last annotation stays on screen while the sidebar slides out.
        let mut shown = use_state(|| current);
        if current.is_some() && *shown.peek() != current {
            shown.set(current);
        }
        let p = palette();
        let body = shown.read().map_or_else(
            || rect().into_element(),
            |id| {
                rect()
                    .expanded()
                    .background(p.s1)
                    .border(Border::new().fill(p.line).width(BorderWidth {
                        left: 1.0,
                        ..BorderWidth::default()
                    }))
                    .child(
                        ScrollView::new()
                            .expanded()
                            .child(notein_ui_kit::ScrollContent::new(
                                rect()
                                    .width(Size::fill())
                                    .padding((12.0, 12.0, 16.0, 16.0))
                                    .child(AnnotationCard { id, expanded: true }),
                            )),
                    )
                    .into_element()
            },
        );
        SidePanel::new(current.is_some() && !compact, SIDEBAR_WIDTH, body)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnnotationCard {
    id: AnnotationId,
    expanded: bool,
}

impl AnnotationCard {
    /// Close the popover, or the sidebar when expanded, keeping what was
    /// written.
    fn close(self) {
        if self.expanded {
            settle(self.id);
        } else {
            ModalController::get().close();
        }
    }

    /// Close after a footer button already reported how it ended.
    fn dismiss(self) {
        if !self.expanded {
            ModalController::get().close();
        }
    }

    /// Colour, title, sidebar toggle and close.
    fn header(self, title: String, dot: Color) -> Rect {
        let id = self.id;
        let toggle = if width_class().is_compact() || aside().is_none() {
            None
        } else if self.expanded {
            Some(
                IconButton::new(freya::icons::lucide::panel_right_close(), "Volver al popup")
                    .size(32.0)
                    .on_press(move |_| AnnotationPopover::collapse(id)),
            )
        } else {
            Some(
                IconButton::new(freya::icons::lucide::panel_right_open(), "Abrir al costado")
                    .size(32.0)
                    .on_press(move |_| AnnotationPopover::expand(id)),
            )
        };
        rect()
            .width(Size::fill())
            .height(Size::px(32.0))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .child(
                rect()
                    .width(Size::px(10.0))
                    .height(Size::px(10.0))
                    .corner_radius(3.0)
                    .background(dot),
            )
            .child(
                label()
                    .width(Size::flex(1.0))
                    .text(title)
                    .font_size(tokens::text::BODY_LG)
                    .font_weight(FontWeight::BOLD)
                    .color(palette().t1)
                    .max_lines(1)
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(toggle)
            .child(
                IconButton::new(freya::icons::lucide::x(), "Cerrar")
                    .size(32.0)
                    .on_press(move |_| self.close()),
            )
    }

    /// The quoted text, with a bar in the highlight colour.
    fn quote(self, quote: String, dot: Color) -> Rect {
        let p = palette();
        rect()
            .width(Size::fill())
            .padding((10.0, 12.0))
            .corner_radius(tokens::radius::MD)
            // Inset from what is around it: the popover card, or the
            // sidebar a step darker.
            .background(if self.expanded { p.s2 } else { p.s1 })
            // The bar is a border: a filled child would stretch to the
            // height of the whole popover.
            .child(
                rect()
                    .width(Size::fill())
                    .padding((0.0, 0.0, 0.0, 10.0))
                    .border(Border::new().fill(dot).width(BorderWidth {
                        left: 3.0,
                        ..BorderWidth::default()
                    }))
                    .child(
                        label()
                            .width(Size::fill())
                            .text(quote)
                            .font_size(tokens::text::BODY)
                            .font_slant(FontSlant::Italic)
                            .color(p.t2)
                            .max_lines(if self.expanded { 6 } else { 3 })
                            .text_overflow(TextOverflow::Ellipsis),
                    ),
            )
    }

    /// The five inks, another colour and no colour.
    fn swatches(self, current: Option<[u8; 3]>, custom: State<Area>) -> Rect {
        let p = palette();
        let id = self.id;
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(10.0)
            .children(p.highlighters.iter().map(|color| {
                let rgb = [color.r(), color.g(), color.b()];
                swatch(*color, current == Some(rgb))
                    .key(color.to_hex_string())
                    .a11y_alt("Color de resaltado")
                    .on_press(move |_| recolor(id, Some(rgb)))
                    .into_element()
            }))
            .child(Self::custom_swatch(id, current, custom))
            .child(Spacer)
            .child(
                Button::new("Sin color")
                    .ghost()
                    .small()
                    .on_press(move |_| recolor(id, None)),
            )
    }

    /// Opens the colour picker; filled with the colour when it is none of
    /// the inks.
    fn custom_swatch(id: AnnotationId, current: Option<[u8; 3]>, mut area: State<Area>) -> Rect {
        let p = palette();
        let custom = current
            .map(|[r, g, b]| Color::from_rgb(r, g, b))
            .filter(|c| !p.highlighters.contains(c));
        swatch(custom.unwrap_or(p.s3), custom.is_some())
            .a11y_alt("Otro color")
            .center()
            .on_sized(move |e: Event<SizedEventData>| area.set_if_modified(e.area))
            .on_press(move |_| {
                Popover::new(*area.peek(), ColorPicker { id })
                    .width(PICKER_WIDTH + 24.0)
                    .align(PopoverAlign::Center)
                    .estimated_height(340.0)
                    .sheet_title("Otro color")
                    .open();
            })
            .maybe_child(custom.is_none().then(|| {
                Icon::new(freya::icons::lucide::pipette())
                    .size(14.0)
                    .color(p.t1)
            }))
    }

    /// Cancelar and Guardar, with Borrar for an existing annotation.
    fn footer(self, existing: bool) -> SheetFooter {
        let id = self.id;
        let footer = SheetFooter::new([
            Button::new("Cancelar")
                .ghost()
                .on_press(move |_| {
                    finish(id, AnnotationOutcome::Cancelled);
                    self.dismiss();
                })
                .into_element(),
            Button::new("Guardar")
                .primary()
                .on_press(move |_| {
                    settle_saving(id);
                    self.dismiss();
                })
                .into_element(),
        ]);
        if !existing {
            return footer;
        }
        footer.leading(
            Button::new("Borrar")
                .variant(ButtonVariant::Danger)
                .on_press(move |_| {
                    finish(id, AnnotationOutcome::Deleted);
                    self.dismiss();
                }),
        )
    }
}

impl Component for AnnotationCard {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let id = self.id;
        let expanded = self.expanded;
        let Some(session) = session(id) else {
            return rect().into_element();
        };
        // A reference in the body takes the reader there. The popover would
        // cover the destination and closes, keeping what was written; the
        // sidebar sits beside it and stays open.
        provide_context(CloseOnNavigate(Rc::new(move || {
            if !expanded {
                ModalController::get().close();
            }
        })));
        let mut draft = session.draft;
        let current = draft.read().clone();
        let custom = use_state(Area::default);
        let body = use_state(move || current.body);
        use_side_effect(move || {
            let text = body.read().clone();
            if draft.peek().body != text {
                draft.write().body = text;
            }
        });
        let request = &session.request;
        let dot = current
            .color
            .map_or(p.line_strong, |[r, g, b]| Color::from_rgb(r, g, b));
        rect()
            .width(Size::fill())
            .padding(if expanded { 0.0 } else { 16.0 })
            .spacing(14.0)
            .child(self.header(request.title.clone(), dot))
            .maybe_child(request.quote.clone().map(|quote| self.quote(quote, dot)))
            .child(self.swatches(current.color, custom))
            .child(
                EditorField::new(body)
                    .placeholder("Escribí una nota… «/ref» para citar")
                    .min_height(if expanded { 360.0 } else { 96.0 })
                    .auto_focus(),
            )
            .child(self.footer(request.existing))
            .into_element()
    }
}

/// A round colour to pick, ringed when chosen.
fn swatch(color: Color, chosen: bool) -> Rect {
    let p = palette();
    rect()
        .a11y_role(AccessibilityRole::Button)
        .width(Size::px(28.0))
        .height(Size::px(28.0))
        .corner_radius(tokens::radius::FULL)
        .background(color)
        .maybe(chosen, |r| {
            r.border(
                Border::new()
                    .fill(p.t1)
                    .width(2.0)
                    .alignment(BorderAlignment::Outer),
            )
        })
}

/// The free colour picker. Every change shows on the document as it
/// happens, as the ink pickers do; the draft keeps the last one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ColorPicker {
    id: AnnotationId,
}

impl Component for ColorPicker {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let Some(session) = session(id) else {
            return rect().into_element();
        };
        let p = palette();
        let initial = session
            .draft
            .peek()
            .color
            .map_or(p.highlighters[0], |[r, g, b]| Color::from_rgb(r, g, b));
        rect()
            .padding(12.0)
            .child(
                ColorWheel::new()
                    .swatches(Vec::new())
                    .allow_custom(true)
                    .alpha(false)
                    .bare()
                    .initial(initial)
                    .width(PICKER_WIDTH)
                    .on_change(move |c: Color| recolor(id, Some([c.r(), c.g(), c.b()]))),
            )
            .into_element()
    }
}
