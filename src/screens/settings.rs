//! Ajustes: seven sections, side by side on wide screens and a list
//! that opens each one on phones.

mod about;
mod appearance;
mod devices;
mod library;
mod notes;
mod pen;
mod plugins;
mod schema;

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    IconButton, ListRow, Modal, ModalController, SearchField, Text, TextRole, tokens, width_class,
};

pub use devices::{open_link_sheet, open_pairing};

/// A section of Ajustes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Appearance,
    Pen,
    Notes,
    Library,
    Devices,
    Plugins,
    About,
}

impl Section {
    pub const ALL: [Self; 7] = [
        Self::Appearance,
        Self::Pen,
        Self::Notes,
        Self::Library,
        Self::Devices,
        Self::Plugins,
        Self::About,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Apariencia",
            Self::Pen => "Lápiz y escritura",
            Self::Notes => "Notas",
            Self::Library => "Biblioteca",
            Self::Devices => "Dispositivos",
            Self::Plugins => "Plugins",
            Self::About => "Acerca de",
        }
    }

    fn icon(self) -> Bytes {
        use freya::icons::lucide;
        match self {
            Self::Appearance => lucide::palette(),
            Self::Pen => lucide::pen_line(),
            Self::Notes => lucide::notebook_text(),
            Self::Library => lucide::library(),
            Self::Devices => lucide::monitor_smartphone(),
            Self::Plugins => lucide::puzzle(),
            Self::About => lucide::info(),
        }
    }

    /// Words that find this section from "Buscar un ajuste".
    const fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Appearance => &[
                "tema",
                "noche",
                "día",
                "sepia",
                "contraste",
                "oled",
                "color",
                "acento",
                "texto",
                "tamaño",
                "movimiento",
                "oscuro",
                "claro",
            ],
            Self::Pen => &[
                "dedo",
                "palma",
                "doble toque",
                "goma",
                "escritura",
                "letra",
                "atajos",
                "lápiz",
                "pen",
                "stylus",
            ],
            Self::Notes => &["markdown", "formato", "referencias", "al lado", "marcas"],
            Self::Library => &[
                "papelera", "exportar", "importar", "espacio", "copia", "backup", "días",
            ],
            Self::Devices => &[
                "vincular",
                "sincronizar",
                "código",
                "qr",
                "escanear",
                "enlace",
                "servidor",
                "nombre",
                "dispositivo",
            ],
            Self::Plugins => &["instalar", "plugin", "extensiones", "biblia", "pdf"],
            Self::About => &["versión", "licencias", "carpeta", "datos"],
        }
    }

    fn matches(self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.label().to_lowercase().contains(&query)
            || self.keywords().iter().any(|k| k.contains(&query))
    }

    fn body(self) -> Element {
        match self {
            Self::Appearance => appearance::Appearance.into_element(),
            Self::Pen => pen::Pen.into_element(),
            Self::Notes => notes::Notes.into_element(),
            Self::Library => library::Library.into_element(),
            Self::Devices => devices::Devices.into_element(),
            Self::Plugins => plugins::Plugins.into_element(),
            Self::About => about::About.into_element(),
        }
    }
}

/// Open Ajustes on its first section.
pub fn open() {
    open_at(Section::default());
}

/// Open Ajustes on `section`.
pub fn open_at(section: Section) {
    let compact = width_class().is_compact();
    let modal = Modal::new(Settings {
        start: (!compact).then_some(section),
    })
    .dismiss_on_backdrop(true);
    ModalController::get().open(if compact {
        modal.manual()
    } else {
        modal.center().width(880.0)
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings {
    /// `None` starts on the list (phones).
    start: Option<Section>,
}

/// The searchable list of sections.
fn section_list(mut open: State<Option<Section>>, query: State<String>, compact: bool) -> Rect {
    let text = query.read().clone();
    let current = *open.read();
    let nav: Vec<Element> = Section::ALL
        .into_iter()
        .filter(|s| s.matches(&text))
        .map(|section| {
            let row = ListRow::new(section.label())
                .icon(section.icon())
                .dense(!compact)
                .selected(!compact && current == Some(section))
                .on_press(move |_| open.set(Some(section)));
            if compact { row.chevron() } else { row }.into_element()
        })
        .collect();
    let none = nav.is_empty();
    rect()
        .width(Size::fill())
        .spacing(10.0)
        .child(SearchField::new(query, "Buscar un ajuste").height(38.0))
        .child(notein_ui_kit::Group::default().children(nav))
        .maybe_child(none.then(|| {
            Text::new(format!("Ningún ajuste con «{}»", text.trim())).role(TextRole::Hint)
        }))
}

/// Phones: the list, or one section with back; the whole screen.
fn phone_layout(mut open: State<Option<Section>>, list: Rect) -> Element {
    use freya::icons::lucide;
    let size = *Platform::get().current_window().root_size.read();
    let insets = *crate::hooks::use_safe_area_insets().read();
    let header = |title: &'static str, back: bool| {
        rect()
            .width(Size::fill())
            .height(Size::px(tokens::size::BAR))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(4.0)
            .child(if back {
                IconButton::new(lucide::arrow_left(), "Volver")
                    .on_press(move |_| open.set(None))
                    .into_element()
            } else {
                IconButton::new(lucide::x(), "Cerrar")
                    .on_press(|_| ModalController::get().close())
                    .into_element()
            })
            .child(Text::new(title).role(TextRole::Title).fill())
    };
    let (head, content) = open.read().map_or_else(
        || (header("Ajustes", false), list.into_element()),
        |section| (header(section.label(), true), section.body()),
    );
    rect()
        .position(Position::new_absolute().top(0.0).left(0.0))
        .width(Size::px(size.width))
        .height(Size::px(size.height))
        .background(palette().bg)
        .padding((
            insets.top,
            insets.right + 12.0,
            insets.bottom,
            insets.left + 12.0,
        ))
        .content(Content::Flex)
        .on_global_key_down(move |e: Event<KeyboardEventData>| {
            if e.key == Key::Named(NamedKey::BrowserBack) && open.peek().is_some() {
                e.stop_propagation();
                open.set(None);
            }
        })
        .child(head)
        .child(
            ScrollView::new()
                .width(Size::fill())
                .height(Size::flex(1.0))
                .child(notein_ui_kit::ScrollContent::new(
                    rect()
                        .width(Size::fill())
                        .padding((8.0, 0.0, 32.0, 0.0))
                        .child(content),
                )),
        )
        .into_element()
}

/// Wider screens: the list beside the open section.
fn wide_layout(section: Section, list: Rect) -> Element {
    let p = palette();
    // Tall enough to browse, never past the system bars or the keyboard.
    let window = notein_ui_kit::safe_frame().height();
    let height = (window * 0.82).clamp(420.0, 760.0).min(window - 32.0);
    // The panes sit 1px inside the frame so neither covers its border.
    let inner = tokens::radius::XL - 1.0;
    rect()
        .width(Size::fill())
        .height(Size::px(height))
        .corner_radius(tokens::radius::XL)
        .overflow(Overflow::Clip)
        .background(p.bg)
        .border(
            Border::new()
                .fill(p.line_strong)
                .width(1.0)
                .alignment(BorderAlignment::Inner),
        )
        .padding(1.0)
        .horizontal()
        .content(Content::Flex)
        .child(
            rect()
                .width(Size::px(260.0))
                .height(Size::fill())
                .padding(16.0)
                .spacing(12.0)
                .corner_radius(CornerRadius::new(inner, 0.0, 0.0, inner))
                .background(p.s1)
                .child(Text::new("Ajustes").role(TextRole::Heading))
                .child(list),
        )
        .child(
            rect()
                .width(Size::flex(1.0))
                .height(Size::fill())
                .content(Content::Flex)
                .child(
                    rect()
                        .width(Size::fill())
                        .height(Size::px(tokens::size::BAR))
                        .padding((0.0, 12.0, 0.0, 24.0))
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .content(Content::Flex)
                        .child(Text::new(section.label()).role(TextRole::Title).fill())
                        .child(
                            IconButton::new(freya::icons::lucide::x(), "Cerrar")
                                .on_press(|_| ModalController::get().close()),
                        ),
                )
                .child(
                    ScrollView::new()
                        .width(Size::fill())
                        .height(Size::flex(1.0))
                        .child(notein_ui_kit::ScrollContent::new(
                            rect()
                                .width(Size::fill())
                                .padding((4.0, 24.0, 32.0, 24.0))
                                .child(section.body()),
                        )),
                ),
        )
        .into_element()
}

impl Component for Settings {
    fn render(&self) -> impl IntoElement {
        let start = self.start;
        let open = use_state(move || start);
        let query = use_state(String::new);
        let compact = width_class().is_compact();
        let list = section_list(open, query, compact);
        if compact {
            phone_layout(open, list)
        } else {
            let section = open.read().unwrap_or_default();
            wide_layout(section, list)
        }
    }
}
