//! The annotation popover shared by verses, PDF fragments and canvas
//! pins, and its expanded form as a tab with the full editor.
//!
//! One open annotation is a [`Session`]: the request a plugin sent, the
//! draft being edited and the callbacks to report back. The popover and
//! the expanded tab edit the same draft, so what is written in one shows
//! in the other.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use freya::prelude::*;
use notein_plugin::services::{
    AnnotationDone, AnnotationOutcome, AnnotationPreview, AnnotationRequest,
};
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, ButtonVariant, ColorWheel, IconButton, ModalController, Popover, PopoverAlign,
    PopoverSide, Sheet, SheetFooter, Spacer, tokens, width_class,
};

use crate::editor::{CloseOnNavigate, EditorField};
use crate::library::{ItemId, TabTarget};
use crate::space::OpenHint;

pub use crate::library::AnnotationId;

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

thread_local! {
    static SESSIONS: RefCell<HashMap<AnnotationId, Rc<Session>>> = RefCell::new(HashMap::new());
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

fn session(id: AnnotationId) -> Option<Rc<Session>> {
    SESSIONS.with(|s| s.borrow().get(&id).cloned())
}

/// End `id`, reporting `outcome` once.
fn finish(id: AnnotationId, outcome: AnnotationOutcome) {
    if let Some(session) = SESSIONS.with(|s| s.borrow_mut().remove(&id)) {
        (session.done)(outcome);
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

/// Title of the expanded tab.
#[must_use]
pub fn tab_title(id: AnnotationId) -> String {
    session(id).map_or_else(
        || "Anotación".to_owned(),
        |s| {
            format!(
                "Anotación · {}",
                s.request.title.trim_start_matches("Resaltar ")
            )
        },
    )
}

/// Entry points.
#[derive(Debug)]
pub struct AnnotationPopover;

impl AnnotationPopover {
    /// Show the annotation popover for `request`.
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
        Self::show(id);
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

    /// Move the annotation from the popover to a tab beside the text.
    fn expand(id: AnnotationId) {
        let mut modals = ModalController::get();
        modals.clear_top_on_close();
        modals.close();
        crate::space::open(TabTarget::Annotation(id), OpenHint::Beside);
    }

    /// Back from the tab to the popover.
    fn collapse(id: AnnotationId, item: ItemId) {
        crate::space::close_tab(TabTarget::Annotation(id));
        crate::space::open(TabTarget::Item(item), OpenHint::Focused);
        Self::show(id);
    }
}

/// The expanded annotation, as a tab body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpandedAnnotation {
    pub id: AnnotationId,
}

impl Component for ExpandedAnnotation {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        use_drop(move || settle(id));
        if session(id).is_none() {
            return notein_ui_kit::EmptyState::new(
                freya::icons::lucide::sticky_note(),
                "Esta anotación ya se cerró",
            )
            .body("Lo escrito quedó guardado en su marcador. Cerrá esta pestaña.")
            .into_element();
        }
        rect()
            .expanded()
            .background(palette().s1)
            .padding((24.0, 32.0))
            .child(AnnotationCard { id, expanded: true })
            .into_element()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnnotationCard {
    id: AnnotationId,
    expanded: bool,
}

impl AnnotationCard {
    /// Close the popover, or the tab when expanded.
    fn dismiss(self) {
        if self.expanded {
            crate::space::close_tab(TabTarget::Annotation(self.id));
        } else {
            ModalController::get().close();
        }
    }

    /// Colour, title, expand / collapse and close.
    fn header(self, title: String, dot: Color, item: ItemId) -> Rect {
        let id = self.id;
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
                    .text(title)
                    .font_size(tokens::text::BODY_LG)
                    .font_weight(FontWeight::BOLD)
                    .color(palette().t1)
                    .max_lines(1),
            )
            .child(Spacer)
            .child(if self.expanded {
                IconButton::new(freya::icons::lucide::minimize_2(), "Volver al popup")
                    .size(32.0)
                    .on_press(move |_| AnnotationPopover::collapse(id, item))
            } else {
                IconButton::new(freya::icons::lucide::maximize_2(), "Expandir")
                    .size(32.0)
                    .on_press(move |_| AnnotationPopover::expand(id))
            })
            .maybe_child((!self.expanded).then(|| {
                IconButton::new(freya::icons::lucide::x(), "Cerrar")
                    .size(32.0)
                    .on_press(|_| ModalController::get().close())
            }))
    }

    /// The quoted text, with a bar in the highlight colour.
    fn quote(self, quote: String, dot: Color) -> Rect {
        let p = palette();
        rect()
            .width(Size::fill())
            .padding((10.0, 12.0))
            .corner_radius(tokens::radius::MD)
            .background(p.s1)
            .horizontal()
            .spacing(10.0)
            .child(
                rect()
                    .width(Size::px(3.0))
                    .height(Size::fill())
                    .min_height(Size::px(28.0))
                    .corner_radius(2.0)
                    .background(dot),
            )
            .child(
                label()
                    .text(quote)
                    .font_size(tokens::text::BODY)
                    .font_slant(FontSlant::Italic)
                    .color(p.t2)
                    .max_lines(if self.expanded { 6 } else { 3 })
                    .text_overflow(TextOverflow::Ellipsis),
            )
    }

    /// The five inks, another colour and no colour.
    fn swatches(
        current: Option<[u8; 3]>,
        pick: impl FnMut(Option<[u8; 3]>) + Clone + 'static,
        mut picking: State<bool>,
    ) -> Rect {
        let p = palette();
        let mut clear = pick.clone();
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(10.0)
            .children(p.highlighters.iter().map(|color| {
                let rgb = [color.r(), color.g(), color.b()];
                let mut pick = pick.clone();
                rect()
                    .key(color.to_hex_string())
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt("Color de resaltado")
                    .width(Size::px(28.0))
                    .height(Size::px(28.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(*color)
                    .maybe(current == Some(rgb), |r| {
                        r.border(
                            Border::new()
                                .fill(p.t1)
                                .width(2.0)
                                .alignment(BorderAlignment::Outer),
                        )
                    })
                    .on_press(move |_| pick(Some(rgb)))
                    .into_element()
            }))
            .child(
                IconButton::new(freya::icons::lucide::palette(), "Otro color")
                    .size(32.0)
                    .active(picking())
                    .on_press(move |_| picking.toggle()),
            )
            .child(Spacer)
            .child(Button::new("Sin color").ghost().small().on_press(move |_| clear(None)))
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
        provide_context(CloseOnNavigate(Rc::new(move || settle(id))));
        let mut draft = session.draft;
        let current = draft.read().clone();
        let picking = use_state(|| false);
        let body = use_state(move || current.body);
        use_side_effect(move || {
            let text = body.read().clone();
            if draft.peek().body != text {
                draft.write().body = text;
            }
        });
        let preview = session.preview.clone();
        let pick = move |color: Option<[u8; 3]>| {
            draft.write().color = color;
            if let Some(preview) = &preview {
                preview(color);
            }
        };
        let request = &session.request;
        let dot = current
            .color
            .map_or(p.line_strong, |[r, g, b]| Color::from_rgb(r, g, b));
        rect()
            .width(Size::fill())
            .padding(if expanded { 0.0 } else { 16.0 })
            .spacing(14.0)
            .child(self.header(request.title.clone(), dot, ItemId(request.doc.0)))
            .maybe_child(request.quote.clone().map(|quote| self.quote(quote, dot)))
            .child(Self::swatches(current.color, pick.clone(), picking))
            .maybe_child(picking().then(|| {
                let mut pick = pick.clone();
                ColorWheel::new()
                    .initial(dot)
                    .on_change(move |color: Color| pick(Some([color.r(), color.g(), color.b()])))
            }))
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
