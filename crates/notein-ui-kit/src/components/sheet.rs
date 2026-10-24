//! Sheets and popovers: the two shapes every dialog takes.
//!
//! A [`Sheet`] is centred on tablets and desktop and slides from the
//! bottom on phones. A [`Popover`] hangs from the element that opened
//! it; on phones it becomes a bottom sheet with the same content.

use std::borrow::Cow;

use freya::prelude::*;

use super::divider::Divider;
use super::icon::Icon;
use super::icon_button::IconButton;
use super::kind_badge::KindBadge;
use super::list_row::strut;
use crate::modal::{Modal, ModalController};
use crate::theme::palette;
use crate::tokens::{radius, size, text};
use crate::viewport::{ViewportArea, clamp_popup_left};
use crate::width::{WidthClass, width_class};

/// A dialog body: header, scrollable content and a footer that stays
/// pinned outside the scroll.
#[derive(Clone, PartialEq)]
pub struct Sheet {
    title: Option<Cow<'static, str>>,
    subtitle: Option<Cow<'static, str>>,
    kind: Option<(Bytes, Color)>,
    on_back: Option<EventHandler<()>>,
    closable: bool,
    width: f32,
    children: Vec<Element>,
    footer: Option<Element>,
    dismissable: bool,
}

impl std::fmt::Debug for Sheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sheet")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl ChildrenExt for Sheet {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Default for Sheet {
    fn default() -> Self {
        Self {
            title: None,
            subtitle: None,
            kind: None,
            on_back: None,
            closable: true,
            width: size::SHEET_WIDTH,
            children: Vec::new(),
            footer: None,
            dismissable: true,
        }
    }
}

impl Sheet {
    #[must_use]
    pub fn titled(title: impl Into<Cow<'static, str>>) -> Self {
        Self {
            title: Some(title.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<Cow<'static, str>>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Kind badge before the title (create and edit sheets).
    #[must_use]
    pub fn kind(mut self, icon: Bytes, color: Color) -> Self {
        self.kind = Some((icon, color));
        self
    }

    /// Show a back chevron (multi-step sheets).
    #[must_use]
    pub fn on_back(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.on_back = Some(handler.into());
        self
    }

    /// Hide the × (flows that must end with a button).
    #[must_use]
    pub const fn without_close(mut self) -> Self {
        self.closable = false;
        self
    }

    /// Tapping outside or Escape does not close it.
    #[must_use]
    pub const fn sticky(mut self) -> Self {
        self.dismissable = false;
        self
    }

    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    #[must_use]
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_element());
        self
    }

    fn into_modal(self) -> Modal {
        let compact = width_class().is_compact();
        let dismissable = self.dismissable;
        let width = self.width;
        let modal = Modal::new(SheetCard {
            sheet: self,
            compact,
        })
        .dismiss_on_backdrop(dismissable)
        .dismiss_on_escape(dismissable)
        .backdrop_alpha(128);
        if compact {
            modal.bottom_sheet()
        } else {
            modal.center().width(width)
        }
    }

    /// Replace the top modal with this sheet.
    pub fn open(self) {
        ModalController::get().open(self.into_modal());
    }

    /// Open on top of the current modal.
    pub fn push(self) {
        ModalController::get().push(self.into_modal());
    }

    /// Close the top modal.
    pub fn close() {
        ModalController::get().close();
    }
}

#[derive(Clone, PartialEq)]
struct SheetCard {
    sheet: Sheet,
    compact: bool,
}

impl Component for SheetCard {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let sheet = &self.sheet;
        let viewport = use_try_consume::<ViewportArea>().map(ViewportArea::get);
        let max_body = viewport.map_or(560.0, |area| (area.height() - 220.0).max(160.0));
        let pad = if self.compact { 20.0 } else { 24.0 };
        let on_back = sheet.on_back.clone();
        let has_header = sheet.title.is_some() || sheet.closable || on_back.is_some();

        rect()
            .width(Size::fill())
            .background(p.s2)
            .border(
                Border::new()
                    .fill(p.line_strong)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .corner_radius(if self.compact {
                CornerRadius::new(radius::SHEET, radius::SHEET, 0.0, 0.0)
            } else {
                CornerRadius::new_all(radius::SHEET)
            })
            .shadow(Shadow::new().y(16.0).blur(48.0).color(p.shadow))
            .padding((if self.compact { 8.0 } else { pad }, pad, pad, pad))
            .spacing(16.0)
            .a11y_role(AccessibilityRole::Dialog)
            .maybe_child(self.compact.then(|| {
                rect()
                    .width(Size::fill())
                    .height(Size::px(12.0))
                    .center()
                    .child(
                        rect()
                            .width(Size::px(36.0))
                            .height(Size::px(5.0))
                            .corner_radius(radius::FULL)
                            .background(p.line_strong),
                    )
            }))
            .maybe_child(has_header.then(|| {
                rect()
                    .width(Size::fill())
                    .height(Size::px(32.0))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .content(Content::Flex)
                    .spacing(8.0)
                    .maybe_child(on_back.clone().map(|handler| {
                        IconButton::new(freya::icons::lucide::chevron_left(), "Atrás")
                            .size(32.0)
                            .on_press(move |_| handler.call(()))
                    }))
                    .maybe_child(
                        sheet
                            .kind
                            .clone()
                            .map(|(icon, color)| KindBadge::new(icon, color).size(32.0)),
                    )
                    .child(
                        label()
                            .text(sheet.title.clone().unwrap_or_default())
                            .font_size(text::TITLE)
                            .font_weight(FontWeight::BOLD)
                            .color(p.t1)
                            .width(Size::flex(1.0))
                            .max_lines(1)
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .maybe_child(sheet.closable.then(|| {
                        IconButton::new(freya::icons::lucide::x(), "Cerrar")
                            .size(32.0)
                            .on_press(|_| ModalController::get().close())
                    }))
            }))
            .maybe_child(sheet.subtitle.clone().map(|sub| {
                label()
                    .text(sub)
                    .font_size(text::BODY)
                    .color(p.t2)
                    .line_height(text::LINE_BODY)
            }))
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .max_height(Size::px(max_body))
                    .height(Size::auto())
                    .spacing(14.0)
                    .children(sheet.children.clone()),
            )
            .maybe_child(sheet.footer.clone().map(|footer| {
                rect()
                    .width(Size::fill())
                    .spacing(16.0)
                    .child(Divider::horizontal())
                    .child(footer)
            }))
    }
}

/// Footer row of a sheet: optional destructive action on the left, a
/// hint, then the actions on the right (primary last).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SheetFooter {
    leading: Option<Element>,
    hint: Option<Cow<'static, str>>,
    actions: Vec<Element>,
}

impl SheetFooter {
    #[must_use]
    pub fn new(actions: impl IntoIterator<Item = Element>) -> Self {
        Self {
            leading: None,
            hint: None,
            actions: actions.into_iter().collect(),
        }
    }

    /// Element at the far left (Papelera, Terminar sesión).
    #[must_use]
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_element());
        self
    }

    /// Hint before the actions ("Enter crea · Esc cancela").
    #[must_use]
    pub fn hint(mut self, hint: impl Into<Cow<'static, str>>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl Component for SheetFooter {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .maybe_child(self.leading.clone())
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .child(strut(40.0))
                    .maybe_child(self.hint.clone().map(|hint| {
                        label()
                            .text(hint)
                            .width(Size::fill())
                            .font_size(text::LABEL)
                            .color(p.t3)
                            .max_lines(2)
                    })),
            )
            .children(self.actions.clone())
    }
}

/// Which side of the anchor a [`Popover`] prefers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PopoverSide {
    #[default]
    Below,
    Above,
}

/// Which edge of the anchor the popover lines up with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PopoverAlign {
    #[default]
    Start,
    Center,
    End,
}

/// A card hanging from the element that opened it.
#[derive(Clone, PartialEq)]
pub struct Popover {
    anchor: Area,
    body: Element,
    width: f32,
    side: PopoverSide,
    align: PopoverAlign,
    estimated_height: f32,
    sheet_title: Option<Cow<'static, str>>,
}

impl std::fmt::Debug for Popover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Popover")
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl Popover {
    /// `anchor` is the global area of the trigger.
    #[must_use]
    pub fn new(anchor: Area, body: impl IntoElement) -> Self {
        Self {
            anchor,
            body: body.into_element(),
            width: 280.0,
            side: PopoverSide::Below,
            align: PopoverAlign::Start,
            estimated_height: 320.0,
            sheet_title: None,
        }
    }

    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    #[must_use]
    pub const fn side(mut self, side: PopoverSide) -> Self {
        self.side = side;
        self
    }

    #[must_use]
    pub const fn align(mut self, align: PopoverAlign) -> Self {
        self.align = align;
        self
    }

    /// Height used to decide whether the preferred side fits.
    #[must_use]
    pub const fn estimated_height(mut self, height: f32) -> Self {
        self.estimated_height = height;
        self
    }

    /// Title of the bottom sheet the popover becomes on phones.
    #[must_use]
    pub fn sheet_title(mut self, title: impl Into<Cow<'static, str>>) -> Self {
        self.sheet_title = Some(title.into());
        self
    }

    /// Open above whatever is showing; a popover never replaces the
    /// sheet it was opened from.
    pub fn open(self) {
        if width_class() == WidthClass::Compact {
            let mut sheet = self
                .sheet_title
                .clone()
                .map_or_else(Sheet::default, Sheet::titled);
            if self.sheet_title.is_none() {
                sheet = sheet.without_close();
            }
            sheet.child(self.body).push();
            return;
        }
        ModalController::get().push(
            Modal::new(PopoverCard { popover: self })
                .manual()
                .backdrop_alpha(0),
        );
    }
}

#[derive(Clone, PartialEq)]
struct PopoverCard {
    popover: Popover,
}

impl Component for PopoverCard {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let pop = &self.popover;
        let viewport = use_try_consume::<ViewportArea>()
            .map(ViewportArea::get)
            .unwrap_or_default();
        let anchor = pop.anchor;
        let gap = 8.0;
        let left = match pop.align {
            PopoverAlign::Start => anchor.min_x(),
            PopoverAlign::Center => anchor.center().x - pop.width / 2.0,
            PopoverAlign::End => anchor.max_x() - pop.width,
        };
        let left = clamp_popup_left(viewport, left, pop.width, 12.0);
        let below = anchor.max_y() + gap;
        let above = anchor.min_y() - gap - pop.estimated_height;
        let fits_below = below + pop.estimated_height <= viewport.max_y() - 12.0;
        let fits_above = above >= viewport.min_y() + 12.0;
        let place_below = match pop.side {
            PopoverSide::Below => fits_below || !fits_above,
            PopoverSide::Above => !fits_above && fits_below,
        };
        let position = if place_below {
            Position::new_absolute().top(below).left(left)
        } else {
            Position::new_absolute()
                .bottom((viewport.max_y() - anchor.min_y() + gap).max(12.0))
                .left(left)
        };
        rect()
            .position(position)
            .width(Size::px(pop.width))
            .max_height(Size::px((viewport.height() - 24.0).max(120.0)))
            .background(p.s2)
            .border(
                Border::new()
                    .fill(p.line_strong)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .corner_radius(radius::LG)
            .shadow(Shadow::new().y(8.0).blur(24.0).color(p.shadow))
            .overflow(Overflow::Clip)
            .a11y_role(AccessibilityRole::Dialog)
            .child(pop.body.clone())
    }
}

/// Section label + content block used inside sheets and settings.
#[derive(Clone, PartialEq, Debug)]
pub struct Section {
    title: Cow<'static, str>,
    icon: Option<Bytes>,
    trailing: Option<Element>,
    children: Vec<Element>,
}

impl ChildrenExt for Section {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Section {
    #[must_use]
    pub fn new(title: impl Into<Cow<'static, str>>) -> Self {
        Self {
            title: title.into(),
            icon: None,
            trailing: None,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn icon(mut self, icon: Bytes) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Element at the right of the label row.
    #[must_use]
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_element());
        self
    }
}

impl Component for Section {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .width(Size::fill())
            .spacing(10.0)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .content(Content::Flex)
                    .spacing(6.0)
                    .maybe_child(
                        self.icon
                            .clone()
                            .map(|icon| Icon::new(icon).size(14.0).color(p.t3)),
                    )
                    .child(
                        label()
                            .text(self.title.to_uppercase())
                            .font_size(text::CAPTION)
                            .font_weight(FontWeight::BOLD)
                            .letter_spacing(text::LABEL_TRACKING)
                            .color(p.t3)
                            .width(Size::flex(1.0)),
                    )
                    .maybe_child(self.trailing.clone()),
            )
            .children(self.children.clone())
    }
}

/// Rounded card that groups rows (settings groups, conflict lists).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Group {
    children: Vec<Element>,
}

impl ChildrenExt for Group {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for Group {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .width(Size::fill())
            .corner_radius(radius::XL)
            .background(p.s1)
            .padding(4.0)
            .children(self.children.clone())
    }
}
