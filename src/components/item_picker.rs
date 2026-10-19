//! Centered picker that answers "what goes in this tab?".
//!
//! Lists quick creations (scratch canvas, new canvas, new note) above
//! the whole library, filtered by name. The sheet only reports the
//! choice; the caller creates items and opens tabs.

use freya::icons::lucide;
use freya::prelude::*;

use crate::components::theme::{CARD_BG, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};
use crate::faces;
use crate::library::{BackgroundStyle, DocKind, ItemId, Library, ROOT_FOLDER, TabTarget};

use super::form_input::FormInput;
use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 520.0;
const ROW_SIZE: f32 = 44.0;
const LIST_HEIGHT: f32 = 300.0;

/// What the user picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerChoice {
    Existing(ItemId),
    Scratch,
    NewCanvas,
    NewNote,
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    id: ItemId,
    name: String,
    kind: DocKind,
    color: Option<[u8; 4]>,
}

impl PickerChoice {
    /// Turn the choice into something a tab can show, creating the
    /// item when the user asked for a new one. `None` when creation
    /// failed (already logged).
    pub async fn resolve(self, lib: &mut Library) -> Option<TabTarget> {
        match self {
            Self::Existing(id) => Some(TabTarget::Item(id)),
            Self::Scratch => Some(TabTarget::Scratch),
            Self::NewCanvas => {
                let kind = DocKind::plugin(crate::builtin::stroke_canvas::KIND_ID);
                create_blank(lib, kind, "Lienzo").await.map(TabTarget::Item)
            }
            Self::NewNote => {
                let kind = DocKind::plugin(notein_notes::KIND_ID);
                let id = create_blank(lib, kind, "Nota").await?;
                crate::builtin::prime_notes(lib.index().items.iter());
                Some(TabTarget::Item(id))
            }
        }
    }
}

async fn create_blank(lib: &mut Library, kind: DocKind, name: &str) -> Option<ItemId> {
    match lib
        .create_item(
            ROOT_FOLDER,
            kind,
            name,
            None,
            Vec::new(),
            BackgroundStyle::default(),
        )
        .await
    {
        Ok(id) => Some(id),
        Err(err) => {
            log::error!("create_item: {err}");
            None
        }
    }
}

/// Builder for the picker. Configure, then [`ItemPicker::open`].
#[derive(Clone)]
pub struct ItemPicker {
    title: String,
    on_pick: Callback<PickerChoice, ()>,
    only_plugin: Option<&'static str>,
    exclude: Vec<ItemId>,
    quick_actions: bool,
}

impl PartialEq for ItemPicker {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for ItemPicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemPicker")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl ItemPicker {
    #[must_use]
    pub fn new(on_pick: impl Into<Callback<PickerChoice, ()>>) -> Self {
        Self {
            title: "Abrir".to_owned(),
            on_pick: on_pick.into(),
            only_plugin: None,
            exclude: Vec::new(),
            quick_actions: true,
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// List only items rendered by `plugin_id`.
    #[must_use]
    pub const fn only_plugin(mut self, plugin_id: &'static str) -> Self {
        self.only_plugin = Some(plugin_id);
        self
    }

    /// Leave `item` out of the list.
    #[must_use]
    pub fn exclude(mut self, item: ItemId) -> Self {
        self.exclude.push(item);
        self
    }

    /// Whether the scratch / new-canvas / new-note shortcuts show.
    #[must_use]
    pub const fn quick_actions(mut self, show: bool) -> Self {
        self.quick_actions = show;
        self
    }

    pub fn open(self) {
        ModalController::get().open(
            Modal::new(self)
                .center()
                .width(SHEET_WIDTH)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for ItemPicker {
    fn render(&self) -> impl IntoElement {
        let query = use_state(String::new);
        let entries = use_state(Vec::<Entry>::new);
        use_hook({
            let mut entries = entries;
            move || {
                spawn(async move {
                    let Ok(handle) = crate::home::open_library().await else {
                        return;
                    };
                    let mut list: Vec<Entry> = {
                        let lib = handle.lock().await;
                        lib.index()
                            .items
                            .iter()
                            .map(|it| Entry {
                                id: it.id,
                                name: it.name.clone(),
                                kind: it.kind.clone(),
                                color: it.color,
                            })
                            .collect()
                    };
                    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                    entries.set(list);
                });
            }
        });

        let on_pick = self.on_pick.clone();
        let choose = move |choice: PickerChoice| {
            ModalController::get().close();
            on_pick.call(choice);
        };

        let needle = query.read().trim().to_lowercase();
        let only_plugin = self.only_plugin;
        let exclude = self.exclude.clone();
        let filtered: Vec<Entry> = entries
            .read()
            .iter()
            .filter(|e| only_plugin.is_none_or(|p| e.kind.is(p)))
            .filter(|e| !exclude.contains(&e.id))
            .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();

        let quick = rect()
            .horizontal()
            .width(Size::fill())
            .spacing(8.0)
            .content(Content::Flex)
            .child(quick_button(
                lucide::hat_glasses(),
                "Borrador",
                choose.clone(),
                PickerChoice::Scratch,
            ))
            .child(quick_button(
                lucide::pencil(),
                "Nuevo lienzo",
                choose.clone(),
                PickerChoice::NewCanvas,
            ))
            .child(quick_button(
                lucide::notebook_text(),
                "Nueva nota",
                choose.clone(),
                PickerChoice::NewNote,
            ));

        let list: Element = if filtered.is_empty() {
            rect()
                .width(Size::fill())
                .height(Size::px(LIST_HEIGHT))
                .center()
                .child(
                    label()
                        .color(TEXT_MUTED)
                        .font_size(13.0)
                        .text(if needle.is_empty() {
                            "La biblioteca está vacía."
                        } else {
                            "Nada coincide con la búsqueda."
                        }),
                )
                .into_element()
        } else {
            let length = filtered.len();
            let choose = choose.clone();
            VirtualScrollView::new_with_data(filtered, move |item, data: &Vec<Entry>| {
                let entry = data[item.index].clone();
                entry_row(entry, choose.clone()).into_element()
            })
            .length(length)
            .width(Size::fill())
            .height(Size::px(LIST_HEIGHT))
            .item_size(ROW_SIZE)
            .into_element()
        };

        rect()
            .vertical()
            .width(Size::fill())
            .padding(16.0)
            .spacing(12.0)
            .background(PANEL_BG)
            .with_corner_radius(14.0)
            .color(TEXT_PRIMARY)
            .child(
                label()
                    .color(TEXT_PRIMARY)
                    .font_size(16.0)
                    .text(self.title.clone()),
            )
            .maybe_child(self.quick_actions.then_some(quick))
            .child(
                FormInput::new(query)
                    .left_icon(lucide::search())
                    .placeholder("Buscar en la biblioteca")
                    .width(Size::fill()),
            )
            .child(list)
    }
}

fn quick_button(
    icon: Bytes,
    text: &'static str,
    choose: impl Fn(PickerChoice) + 'static,
    choice: PickerChoice,
) -> Rect {
    rect()
        .horizontal()
        .width(Size::flex(1.0))
        .height(Size::px(40.0))
        .cross_align(Alignment::Center)
        .main_align(Alignment::Center)
        .spacing(6.0)
        .with_corner_radius(8.0)
        .background(CARD_BG)
        .on_press(move |_| choose(choice))
        .child(
            SvgViewer::new(icon)
                .color(TEXT_PRIMARY)
                .width(Size::px(16.0))
                .height(Size::px(16.0)),
        )
        .child(label().color(TEXT_PRIMARY).font_size(13.0).text(text))
}

fn entry_row(entry: Entry, choose: impl Fn(PickerChoice) + 'static) -> Rect {
    let id = entry.id;
    let dot = entry
        .color
        .map(|[r, g, b, _]| Color::from_rgb(r, g, b))
        .unwrap_or(TEXT_MUTED);
    let title = if entry.name.is_empty() {
        "(sin título)".to_owned()
    } else {
        entry.name.clone()
    };
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .padding((8.0, 10.0))
        .margin((2.0, 0.0))
        .width(Size::fill())
        .height(Size::px(ROW_SIZE - 4.0))
        .background(CARD_BG)
        .with_corner_radius(6.0)
        .on_press(move |_| choose(PickerChoice::Existing(id)))
        .content(Content::Flex)
        .child(
            SvgViewer::new(faces::icon(&entry.kind))
                .color(dot)
                .width(Size::px(16.0))
                .height(Size::px(16.0)),
        )
        .child(
            rect()
                .vertical()
                .width(Size::flex(1.0))
                .spacing(2.0)
                .child(label().color(TEXT_PRIMARY).font_size(13.0).text(title))
                .child(
                    label()
                        .color(TEXT_MUTED)
                        .font_size(11.0)
                        .text(faces::kind_label(&entry.kind)),
                ),
        )
}
