//! Library cards: documents and folders, with real covers when there is
//! one and a drawn cover of the kind otherwise.

use freya::prelude::*;
use notein_ui_kit::theme::{Palette, palette};
use notein_ui_kit::{Icon, tokens, use_press_gesture};

use crate::library::{BackgroundStyle, Rgba};
use crate::references::kind_color;

/// Cover aspect ratio (height / width).
pub const COVER_RATIO: f32 = 1.28;

/// What a document card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocCardData {
    pub name: String,
    pub kind: String,
    pub meta: String,
    /// Card tint (paper colour for canvases).
    pub color: Option<Rgba>,
    pub background: BackgroundStyle,
    pub thumbnail: Option<Bytes>,
    pub notebooks: usize,
    pub pinned: bool,
    pub shared: bool,
    /// Text for a note's drawn cover (first lines of the body).
    pub preview: Option<String>,
}

/// Cover of one document.
#[derive(Clone, Debug, PartialEq)]
pub struct Cover {
    pub data: DocCardData,
    pub width: f32,
}

const fn rgba(color: Rgba) -> Color {
    Color::from_argb(color[3], color[0], color[1], color[2])
}

impl Cover {
    fn paper(&self, p: &Palette) -> Color {
        self.data.color.map_or(p.paper, rgba)
    }

    fn lines(width: f32, rows: usize, color: Color, top: f32) -> Rect {
        rect()
            .position(Position::new_absolute().top(top).left(width * 0.12))
            .width(Size::px(width * 0.76))
            .spacing(width * 0.06)
            .children((0..rows).map(|row| {
                rect()
                    .key(row)
                    .width(Size::percent(if row % 3 == 2 { 60.0 } else { 100.0 }))
                    .height(Size::px(4.0))
                    .corner_radius(2.0)
                    .background(color)
                    .into_element()
            }))
    }

    fn pattern(&self, height: f32, line: Color) -> Option<Rect> {
        let w = self.width;
        match self.data.background {
            BackgroundStyle::Blank => None,
            BackgroundStyle::Line => Some(
                rect()
                    .position(Position::new_absolute().top(22.0).left(0.0))
                    .width(Size::px(w))
                    .spacing(17.0)
                    .children((0..fits(height, 18.0)).map(|i| {
                        rect()
                            .key(i)
                            .width(Size::fill())
                            .height(Size::px(1.0))
                            .background(line)
                            .into_element()
                    })),
            ),
            BackgroundStyle::Grid | BackgroundStyle::DotGrid => {
                let dots = self.data.background == BackgroundStyle::DotGrid;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a few rows
                let rows = (height / 16.0) as usize;
                Some(
                    rect()
                        .position(Position::new_absolute().top(8.0).left(8.0))
                        .spacing(if dots { 14.0 } else { 15.0 })
                        .children((0..rows).map(|r| {
                            rect()
                                .key(r)
                                .horizontal()
                                .spacing(if dots { 14.0 } else { 0.0 })
                                .child(if dots {
                                    rect()
                                        .horizontal()
                                        .spacing(14.0)
                                        .children((0..fits(w, 16.0)).map(|c| {
                                            rect()
                                                .key(c)
                                                .width(Size::px(2.0))
                                                .height(Size::px(2.0))
                                                .corner_radius(1.0)
                                                .background(line)
                                                .into_element()
                                        }))
                                        .into_element()
                                } else {
                                    rect()
                                        .width(Size::px(w))
                                        .height(Size::px(1.0))
                                        .background(line)
                                        .into_element()
                                })
                                .into_element()
                        })),
                )
            }
        }
    }
}

impl Cover {
    /// A leather cover with the translation's name.
    fn bible(&self, base: Rect, p: &Palette) -> Element {
        let translation = self.data.meta.split(" · ").next().unwrap_or_default().to_uppercase();
        base.background(Color::from_rgb(59, 47, 30))
            .padding((18.0, 16.0))
            .spacing(4.0)
            .child(
                label()
                    .text("Biblia")
                    .font_size(18.0)
                    .font_family("Lora")
                    .color(Color::from_rgb(243, 226, 190)),
            )
            .child(
                label()
                    .text(translation)
                    .font_size(10.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .letter_spacing(1.0)
                    .color(p.kinds.bible),
            )
            .child(
                rect()
                    .position(Position::new_absolute().bottom(36.0).left(16.0))
                    .width(Size::px(self.width - 32.0))
                    .height(Size::px(2.0))
                    .background(p.kinds.bible),
            )
            .into_element()
    }

    /// The note's title and the start of its text.
    fn note(&self, base: Rect, p: &Palette) -> Element {
        base.background(self.paper(p))
            .padding((20.0, self.width * 0.12))
            .spacing(6.0)
            .child(
                label()
                    .text(self.data.name.clone())
                    .font_size(11.0)
                    .font_weight(FontWeight::BOLD)
                    .color(p.paper_ink)
                    .max_lines(2),
            )
            .child(
                label()
                    .text(self.data.preview.clone().unwrap_or_default())
                    .font_size(8.5)
                    .line_height(1.4)
                    .color(Color::from_rgb(110, 106, 98))
                    .max_lines(9)
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .into_element()
    }

    /// A page with a heading and a highlighted line.
    fn pdf(&self, base: Rect, p: &Palette) -> Element {
        let w = self.width;
        base.background(self.paper(p))
            .child(Self::lines(w, 8, Color::from_rgb(216, 211, 200), 44.0))
            .child(
                rect()
                    .position(Position::new_absolute().top(22.0).left(w * 0.12))
                    .width(Size::px(w * 0.76))
                    .height(Size::px(10.0))
                    .corner_radius(2.0)
                    .background(Color::from_rgb(201, 195, 182)),
            )
            .child(
                rect()
                    .position(Position::new_absolute().top(56.0).left(w * 0.12))
                    .width(Size::px(w * 0.55))
                    .height(Size::px(10.0))
                    .corner_radius(3.0)
                    .background(p.highlighters[0].with_a(180)),
            )
            .into_element()
    }

    /// The paper pattern with the kind's icon.
    fn generic(&self, base: Rect, p: &Palette, h: f32) -> Element {
        base.background(self.paper(p))
            .maybe_child(self.pattern(h, Color::from_rgb(222, 218, 208)))
            .child(
                Icon::new(
                    crate::plugins::kind_info(&self.data.kind)
                        .map_or_else(freya::icons::lucide::file, |k| k.icon),
                )
                .size(self.width * 0.28)
                .color(p.paper_ink.with_a(40)),
            )
            .center()
            .into_element()
    }
}

impl Component for Cover {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let w = self.width;
        let h = (w * COVER_RATIO).round();
        let base = rect()
            .width(Size::px(w))
            .height(Size::px(h))
            .corner_radius(tokens::radius::LG)
            .overflow(Overflow::Clip);
        if let Some(bytes) = self.data.thumbnail.clone() {
            return base
                .background(self.paper(&p))
                .child(ImageViewer::new(bytes).width(Size::px(w)).height(Size::px(h)))
                .into_element();
        }
        match self.data.kind.as_str() {
            notein_bible::kind::KIND => self.bible(base, &p),
            notein_notes::kind::KIND => self.note(base, &p),
            crate::plugins::builtin::pdf::KIND => self.pdf(base, &p),
            _ => self.generic(base, &p, h),
        }
    }
}

/// A small dark disc with an icon, over covers.
fn cover_badge(icon: Bytes, color: Color) -> Rect {
    rect()
        .width(Size::px(24.0))
        .height(Size::px(24.0))
        .corner_radius(tokens::radius::FULL)
        .background(Color::from_argb(220, 30, 30, 34))
        .center()
        .child(Icon::new(icon).size(13.0).color(color))
}

/// How a card reacts to presses.
#[derive(Clone, PartialEq)]
pub struct CardActions {
    pub on_open: EventHandler<()>,
    pub on_menu: EventHandler<CursorPoint>,
}

impl std::fmt::Debug for CardActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardActions").finish_non_exhaustive()
    }
}

/// Selection state of a card while selecting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selecting {
    No,
    Unselected,
    Selected,
}

/// A document card.
#[derive(Clone, Debug, PartialEq)]
pub struct DocCard {
    pub data: DocCardData,
    pub width: f32,
    pub selecting: Selecting,
    pub actions: CardActions,
}

/// How many notebooks the document has, over the cover.
fn notebooks_badge(count: usize) -> Rect {
    rect()
        .position(Position::new_absolute().bottom(8.0).right(8.0))
        .height(Size::px(24.0))
        .padding((0.0, 7.0))
        .corner_radius(tokens::radius::FULL)
        .background(Color::from_argb(220, 30, 30, 34))
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .child(
            Icon::new(freya::icons::lucide::notebook_pen())
                .size(12.0)
                .color(Color::WHITE),
        )
        .child(
            label()
                .text(count.to_string())
                .font_size(11.0)
                .font_weight(FontWeight::BOLD)
                .color(Color::WHITE),
        )
}

/// The check over a card while selecting.
fn select_mark(selected: bool) -> Rect {
    let p = palette();
    rect()
        .position(Position::new_absolute().top(8.0).left(8.0))
        .width(Size::px(24.0))
        .height(Size::px(24.0))
        .corner_radius(tokens::radius::FULL)
        .map(selected.then_some(p.accent), Rect::background)
        .maybe(!selected, |r| {
            r.border(
                Border::new()
                    .fill(Color::WHITE)
                    .width(2.0)
                    .alignment(BorderAlignment::Inner),
            )
            .background(Color::from_argb(80, 0, 0, 0))
        })
        .center()
        .maybe_child(selected.then(|| {
            Icon::new(freya::icons::lucide::check())
                .size(14.0)
                .color(p.on_accent)
        }))
}

/// Name and what it is, under the cover.
fn caption(data: &DocCardData) -> Rect {
    let p = palette();
    rect()
        .width(Size::fill())
        .spacing(2.0)
        .child(
            label()
                .text(data.name.clone())
                .font_size(tokens::text::BODY)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(p.t1)
                .max_lines(2)
                .text_overflow(TextOverflow::Ellipsis),
        )
        .child(
            label()
                .text(data.meta.clone())
                .font_size(tokens::text::LABEL)
                .color(p.t3)
                .max_lines(1),
        )
}

impl DocCard {
    /// The cover with its kind, notebooks, pin, sharing and selection
    /// marks.
    fn cover(&self, hovering: bool) -> Rect {
        let p = palette();
        let data = &self.data;
        let info = crate::plugins::kind_info(&data.kind);
        let kind_icon = info
            .as_ref()
            .map_or_else(freya::icons::lucide::file, |k| k.icon.clone());
        let kind_fill = info.as_ref().map_or(p.t3, |k| kind_color(&p, k.color));
        let selected = self.selecting == Selecting::Selected;
        rect()
            .width(Size::px(self.width))
            .height(Size::px((self.width * COVER_RATIO).round()))
            .corner_radius(tokens::radius::LG)
            .maybe(selected, |r| {
                r.border(
                    Border::new()
                        .fill(p.accent)
                        .width(3.0)
                        .alignment(BorderAlignment::Outer),
                )
            })
            .maybe(hovering && !selected, |r| {
                r.shadow(Shadow::new().y(6.0).blur(16.0).color(p.shadow))
            })
            .child(Cover {
                data: data.clone(),
                width: self.width,
            })
            .child(
                rect()
                    .position(Position::new_absolute().bottom(8.0).left(8.0))
                    .child(cover_badge(kind_icon, kind_fill)),
            )
            .maybe_child((data.notebooks > 0).then(|| notebooks_badge(data.notebooks)))
            .child(
                rect()
                    .position(Position::new_absolute().top(8.0).right(8.0))
                    .horizontal()
                    .spacing(4.0)
                    .maybe_child(
                        data.pinned
                            .then(|| cover_badge(freya::icons::lucide::pin(), p.kinds.bible)),
                    )
                    .maybe_child(
                        data.shared
                            .then(|| cover_badge(freya::icons::lucide::users(), p.highlighters[3])),
                    ),
            )
            .maybe_child((self.selecting != Selecting::No).then(|| select_mark(selected)))
    }
}

impl Component for DocCard {
    fn render(&self) -> impl IntoElement {
        let gesture = use_press_gesture();
        let mut hovering = use_state(|| false);
        let open = self.actions.on_open.clone();
        let menu = self.actions.on_menu.clone();
        let card = rect()
            .width(Size::px(self.width))
            .spacing(8.0)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.data.name.clone())
            .on_pointer_enter(move |_| hovering.set(true))
            .on_pointer_leave(move |_| hovering.set(false))
            .child(self.cover(hovering()))
            .child(caption(&self.data));
        gesture.apply(card, move || open.call(()), move |at| menu.call(at))
    }
}

/// What a folder card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderCardData {
    pub name: String,
    pub color: Option<Rgba>,
    pub count: usize,
}

/// A folder card.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderCard {
    pub data: FolderCardData,
    pub width: f32,
    pub selecting: Selecting,
    pub drop_target: bool,
    pub actions: CardActions,
}

impl Component for FolderCard {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let gesture = use_press_gesture();
        let w = self.width;
        let h = (w * COVER_RATIO).round();
        let tint = self.data.color.map_or(Color::from_rgb(58, 56, 51), rgba);
        let open = self.actions.on_open.clone();
        let menu = self.actions.on_menu.clone();
        let selected = self.selecting == Selecting::Selected;
        let cover = rect()
            .width(Size::px(w))
            .height(Size::px(h))
            .corner_radius(tokens::radius::LG)
            .background(tint)
            .overflow(Overflow::Clip)
            .maybe(self.drop_target || selected, |r| {
                r.border(
                    Border::new()
                        .fill(p.accent)
                        .width(3.0)
                        .alignment(BorderAlignment::Outer),
                )
            })
            .children((0..3).map(|i| {
                #[allow(clippy::cast_precision_loss)] // 0..3
                let step = f32::from(i);
                rect()
                    .key(i)
                    .position(
                        Position::new_absolute()
                            .top(step.mul_add(10.0, 20.0))
                            .left(step.mul_add(5.0, 18.0)),
                    )
                    .width(Size::px(step.mul_add(-10.0, w - 36.0)))
                    .height(Size::px(step.mul_add(-14.0, h - 50.0)))
                    .corner_radius(8.0)
                    .background(p.paper.with_a(46 + 46 * i))
                    .into_element()
            }))
            .child(
                rect()
                    .position(Position::new_absolute().bottom(8.0).left(8.0))
                    .child(cover_badge(freya::icons::lucide::folder(), Color::WHITE)),
            );
        let card = rect()
            .width(Size::px(w))
            .spacing(8.0)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.data.name.clone())
            .child(cover)
            .child(
                rect()
                    .spacing(2.0)
                    .child(
                        label()
                            .text(self.data.name.clone())
                            .font_size(tokens::text::BODY)
                            .font_weight(FontWeight::SEMI_BOLD)
                            .color(p.t1)
                            .max_lines(2),
                    )
                    .child(
                        label()
                            .text(crate::format::count(
                                self.data.count,
                                "elemento",
                                "elementos",
                            ))
                            .font_size(tokens::text::LABEL)
                            .color(p.t3),
                    ),
            );
        gesture.apply(card, move || open.call(()), move |at| menu.call(at))
    }
}

/// How many `pitch`-sized steps fit in `span`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a small, non-negative count
fn fits(span: f32, pitch: f32) -> usize {
    (span / pitch).max(0.0) as usize
}
