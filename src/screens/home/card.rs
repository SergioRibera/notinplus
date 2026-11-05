//! Library cards: documents and folders, with real covers when there is
//! one and a drawn cover of the kind otherwise.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::AtomicU64;

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::face::CoverRequest;
use notein_ui_kit::theme::{Palette, palette};
use notein_ui_kit::{Icon, tokens, use_press_gesture};

use crate::library::{BackgroundStyle, Rgba};
use crate::references::kind_color;

/// Cover aspect ratio (height / width).
pub const COVER_RATIO: f32 = 1.28;
/// Room under a cover for two lines of name and one of details, so
/// every card in a row is as tall as the others.
const CAPTION: f32 = 58.0;
/// Gap between a cover and its caption.
const CAPTION_GAP: f32 = 8.0;

/// Height of a document or folder card `width` px wide.
#[must_use]
pub fn card_height(width: f32) -> f32 {
    (width * COVER_RATIO).round() + CAPTION_GAP + CAPTION
}

/// What a document card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocCardData {
    pub name: String,
    pub kind: String,
    pub meta: String,
    /// Card tint (paper colour for canvases).
    pub color: Option<Rgba>,
    pub background: BackgroundStyle,
    pub thumbnail: Option<Thumb>,
    pub notebooks: usize,
    pub pinned: bool,
    pub shared: bool,
    /// The first words of the document, for its drawn cover.
    pub preview: Option<String>,
}

/// An encoded cover and the key it is decoded once under. Two thumbs
/// are equal when their keys are, so a card compares without reading
/// the image.
#[derive(Clone, Debug)]
pub struct Thumb {
    pub key: u64,
    pub bytes: Bytes,
}

impl PartialEq for Thumb {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for Thumb {}

/// Cover of one document.
#[derive(Clone, Debug, PartialEq)]
pub struct Cover {
    pub data: DocCardData,
    pub width: f32,
}

/// Running totals for the profiler: cards and covers built, covers drawn
/// from a picture, and pictures seen for the first time (each one costs
/// a decode).
static CARDS: AtomicU64 = AtomicU64::new(0);
static COVERS: AtomicU64 = AtomicU64::new(0);
static IMAGE_COVERS: AtomicU64 = AtomicU64::new(0);
static IMAGES_DECODED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static SEEN_IMAGES: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

const fn rgba(color: Rgba) -> Color {
    Color::from_argb(color[3], color[0], color[1], color[2])
}

impl Cover {
    fn paper(&self, p: &Palette) -> Color {
        self.data.color.map_or(p.paper, rgba)
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
    /// The paper pattern with the kind's icon.
    #[sismo::function("home::cover_generic")]
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
    #[sismo::function("home::cover")]
    fn render(&self) -> impl IntoElement {
        sismo::counter!("home.covers_rendered", super::tally(&COVERS, 1));
        let p = palette();
        let w = self.width;
        let h = (w * COVER_RATIO).round();
        let base = rect()
            .width(Size::px(w))
            .height(Size::px(h))
            .corner_radius(tokens::radius::LG)
            .overflow(Overflow::Clip);
        if let Some(thumb) = self.data.thumbnail.clone() {
            sismo::scope!("home::cover_image");
            sismo::counter!("home.covers_image", super::tally(&IMAGE_COVERS, 1));
            if SEEN_IMAGES.with(|seen| seen.borrow_mut().insert(thumb.key)) {
                sismo::counter!("home.images_decoded", super::tally(&IMAGES_DECODED, 1));
            }
            return base
                .background(self.paper(&p))
                .child(
                    ImageViewer::new((thumb.key, thumb.bytes))
                        .width(Size::px(w))
                        .height(Size::px(h)),
                )
                .into_element();
        }
        let request = CoverRequest {
            name: self.data.name.clone(),
            meta: self.data.meta.clone(),
            excerpt: self.data.preview.clone(),
            tint: self.data.color,
            width: w,
            height: h,
        };
        let drawn = {
            sismo::scope!("home::cover_face");
            crate::plugins::catalog()
                .face(&self.data.kind)
                .and_then(|face| face.cover(&request))
        };
        match drawn {
            Some(cover) => base.child(cover).into_element(),
            None => self.generic(base, &p, h),
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
        .layer(MARKS)
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

/// Layer of the marks over a cover, above anything the cover nests.
/// Siblings on one layer paint in no particular order, so a cover drawn
/// later would otherwise hide them.
const MARKS: Layer = Layer::Relative(12);

/// The check over a card while selecting.
fn select_mark(selected: bool) -> Rect {
    let p = palette();
    rect()
        .position(Position::new_absolute().top(8.0).left(8.0))
        .layer(MARKS)
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
        .height(Size::px(CAPTION))
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
    #[sismo::function("home::doc_card_cover")]
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
                    .layer(MARKS)
                    .child(cover_badge(kind_icon, kind_fill)),
            )
            .maybe_child((data.notebooks > 0).then(|| notebooks_badge(data.notebooks)))
            .child(
                rect()
                    .position(Position::new_absolute().top(8.0).right(8.0))
                    .layer(MARKS)
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
    #[sismo::function("home::doc_card")]
    fn render(&self) -> impl IntoElement {
        sismo::counter!("home.cards_rendered", super::tally(&CARDS, 1));
        let gesture = use_press_gesture();
        let mut hovering = use_state(|| false);
        let open = self.actions.on_open.clone();
        let menu = self.actions.on_menu.clone();
        let card = rect()
            .width(Size::px(self.width))
            .spacing(CAPTION_GAP)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.data.name.clone())
            .on_pointer_enter(move |e: Event<PointerEventData>| {
                hovering.set(notein_ui_kit::is_mouse(&e));
            })
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
    /// Covers of its latest documents, newest first; three at most.
    pub previews: Vec<DocCardData>,
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

impl FolderCard {
    /// Where each preview sits, newest first: in front in the middle,
    /// then tilted behind it to the left and to the right. Offsets are
    /// shares of the card width; tops and tilts are fixed.
    const FAN: [(f32, f32, f32); 3] = [(0.0, 30.0, 0.0), (-0.19, 18.0, -9.0), (0.19, 18.0, 9.0)];

    /// The covers of its latest documents, fanned out.
    #[sismo::function("home::folder_fan")]
    fn fan(&self, w: f32, p: &Palette) -> Vec<Element> {
        let mini = (w * 0.56).round();
        let middle = (w - mini) / 2.0;
        self.data
            .previews
            .iter()
            .zip(Self::FAN)
            .enumerate()
            .map(|(i, (data, (shift, top, tilt)))| {
                rect()
                    .key(i)
                    .position(
                        Position::new_absolute()
                            .top(top)
                            .left(w.mul_add(shift, middle)),
                    )
                    .rotate(tilt)
                    // Siblings on one layer paint in no set order, and each level
                    // of a cover adds one: the steps clear a whole cover, and
                    // stay under the marks.
                    .layer(Layer::Relative(
                        i16::try_from((2 - i) * 5).unwrap_or_default(),
                    ))
                    .corner_radius(tokens::radius::MD)
                    .shadow(Shadow::new().y(4.0).blur(10.0).color(p.shadow))
                    .child(Cover {
                        data: data.clone(),
                        width: mini,
                    })
                    .into_element()
            })
            .collect()
    }

    /// Empty sheets of paper, for a folder with no documents.
    fn sheets(w: f32, h: f32, p: &Palette) -> Vec<Element> {
        (0..3)
            .map(|i| {
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
            })
            .collect()
    }
}

impl Component for FolderCard {
    #[sismo::function("home::folder_card")]
    fn render(&self) -> impl IntoElement {
        sismo::counter!("home.cards_rendered", super::tally(&CARDS, 1));
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
            .children(if self.data.previews.is_empty() {
                Self::sheets(w, h, &p)
            } else {
                self.fan(w, &p)
            })
            .child(
                rect()
                    .position(Position::new_absolute().bottom(8.0).left(8.0))
                    .layer(MARKS)
                    .child(cover_badge(freya::icons::lucide::folder(), Color::WHITE)),
            )
            .maybe_child((self.selecting != Selecting::No).then(|| select_mark(selected)));
        let card = rect()
            .width(Size::px(w))
            .spacing(CAPTION_GAP)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.data.name.clone())
            .child(cover)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(CAPTION))
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
                            .text(tr!("home-item-count", "count" => self.data.count))
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
