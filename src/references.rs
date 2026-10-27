//! References between documents: how a chip looks, what `/ref` offers,
//! copying a reference and putting one in a notebook.

use freya::prelude::*;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::doc::DocMeta;
use notein_plugin::kind::{KindColor, SearchQuery};
use notein_ui_kit::theme::Palette;
use notein_ui_kit::{Toast, ToastTone, Toasts};

use crate::library::{ItemId, LibraryData, TabTarget};
use crate::plugins::content::doc_id;
use crate::space::OpenHint;

/// Most suggestions `/ref` offers per kind.
const SUGGESTIONS_PER_KIND: usize = 6;
/// Most documents `/ref` offers by name.
const SUGGESTED_DOCS: usize = 8;

/// Resolve a kind colour family against the palette.
#[must_use]
pub const fn kind_color(palette: &Palette, color: KindColor) -> Color {
    match color {
        KindColor::Canvas => palette.kinds.canvas,
        KindColor::Pdf => palette.kinds.pdf,
        KindColor::Note => palette.kinds.note,
        KindColor::Bible => palette.kinds.bible,
        KindColor::Folder => palette.kinds.folder,
        KindColor::Scratch => palette.kinds.scratch,
        KindColor::Custom([r, g, b]) => Color::from_rgb(r, g, b),
    }
}

/// Colour of a chip before the palette resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipColor {
    Kind(KindColor),
    Neutral,
}

impl ChipColor {
    #[must_use]
    pub const fn resolve(self, palette: &Palette) -> Color {
        match self {
            Self::Kind(color) => kind_color(palette, color),
            Self::Neutral => palette.t2,
        }
    }
}

/// How a chip for a reference looks right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipDisplay {
    pub label: String,
    pub icon: Bytes,
    pub color: ChipColor,
    pub broken: bool,
}

/// Resolve `src` against the library and the catalog.
#[must_use]
pub fn display(src: &SourceRef, data: Option<&LibraryData>) -> ChipDisplay {
    match src {
        SourceRef::External { url } => ChipDisplay {
            label: url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned(),
            icon: freya::icons::lucide::link_2(),
            color: ChipColor::Neutral,
            broken: false,
        },
        SourceRef::Canvas { bookmark } => {
            let canvas = crate::plugins::builtin::canvas_kind();
            let doc = canvas.doc_of_bookmark(bookmark.0);
            let name = doc.and_then(|d| data?.item(ItemId(d.0)).map(|i| i.name.clone()));
            ChipDisplay {
                broken: name.is_none(),
                label: name.map_or_else(|| "Marcador".to_owned(), |n| format!("{n} · marcador")),
                icon: freya::icons::lucide::bookmark(),
                color: ChipColor::Kind(KindColor::Canvas),
            }
        }
        SourceRef::Plugin {
            plugin_id,
            item,
            anchor,
            ..
        } => {
            let Some(kind) = crate::plugins::kind(plugin_id) else {
                return ChipDisplay {
                    label: "Necesita un plugin".to_owned(),
                    icon: freya::icons::lucide::puzzle(),
                    color: ChipColor::Neutral,
                    broken: true,
                };
            };
            let info = kind.info();
            let id = ItemId(*item);
            let doc = data
                .and_then(|d| d.item(id))
                .filter(|i| i.trashed.is_none());
            let name = doc.map_or_else(|| info.label.to_string(), |d| d.name.clone());
            let meta = DocMeta {
                id: doc_id(id),
                name: &name,
                updated_at: doc.map_or(0, |d| d.updated_at),
                tint: doc.and_then(|d| d.color),
            };
            let chip = kind.chip(meta, anchor);
            let broken = chip.broken || (doc.is_none() && !info.singleton);
            ChipDisplay {
                label: chip.label,
                icon: info.icon,
                color: ChipColor::Kind(info.color),
                broken,
            }
        }
    }
}

/// One `/ref` suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    pub subtitle: String,
    pub target: SourceRef,
}

/// What `/ref query` offers: documents by name, then places inside them
/// from every kind's search.
#[must_use]
pub fn suggestions(query: &str, data: Option<&LibraryData>) -> Vec<Suggestion> {
    let needle = query.trim().to_lowercase();
    let mut out: Vec<Suggestion> = data
        .map(|data| {
            data.live_items()
                .filter(|i| needle.is_empty() || i.name.to_lowercase().contains(&needle))
                .take(SUGGESTED_DOCS)
                .map(|item| Suggestion {
                    label: item.name.clone(),
                    subtitle: crate::plugins::kind_info(&item.kind.plugin_id)
                        .map_or_else(String::new, |k| k.label.into_owned()),
                    target: SourceRef::Plugin {
                        plugin_id: item.kind.plugin_id.clone(),
                        item: item.id.0,
                        anchor: Vec::new(),
                        selection: None,
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    if needle.is_empty() {
        return out;
    }
    let catalog = crate::plugins::catalog();
    for found in catalog.search(&SearchQuery::new(query, SUGGESTIONS_PER_KIND)) {
        let label = crate::plugins::kind_info(&found.kind)
            .map_or_else(String::new, |k| k.label.into_owned());
        out.extend(found.hits.into_iter().map(|hit| Suggestion {
            label: hit.title,
            subtitle: format!("{label} · {}", hit.snippet),
            target: SourceRef::Plugin {
                plugin_id: found.kind.clone(),
                item: hit.doc.map_or([0; 16], |d| d.0),
                anchor: hit.anchor,
                selection: None,
            },
        }));
    }
    drop(catalog);
    out
}

/// Markdown that renders as a chip for `target`.
#[must_use]
pub fn chip_markdown(label: &str, target: &SourceRef) -> String {
    format!("[{}]({})", label.replace(['[', ']'], ""), target.to_url())
}

/// Copy a reference: the clipboard gets a chip, and a toast offers to
/// paste it in the notebook beside its document.
pub fn copy(target: &SourceRef, label: &str) {
    let markdown = chip_markdown(label, target);
    if let Err(err) = freya::clipboard::Clipboard::set(markdown) {
        log::warn!("clipboard: {err:?}");
    }
    let mut toast = Toast::new(format!("Referencia «{label}» copiada")).tone(ToastTone::Done);
    if let SourceRef::Plugin { item, .. } = target
        && *item != [0; 16]
    {
        let from = ItemId(*item);
        let target = target.clone();
        toast = toast.action("Pegar en el cuaderno", move |()| {
            insert_in_notebook(from, target.clone());
        });
    }
    Toasts::get().show(toast);
}

/// Insert a chip for `target` in `from`'s notebook, creating the
/// notebook when `from` has none, and show it beside.
pub fn insert_in_notebook(from: ItemId, target: SourceRef) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let label = {
            let lib = handle.lock().await;
            display(&target, Some(&lib.snapshot())).label
        };
        let markdown = chip_markdown(&label, &target);
        let notebook = {
            let mut lib = handle.lock().await;
            match lib.live_notebooks_of(from).first().copied() {
                Some(existing) => Ok(existing),
                None => crate::notebooks::create_notebook(&mut lib, from).await,
            }
        };
        let notebook = match notebook {
            Ok(id) => id,
            Err(err) => {
                log::error!("notebook for {from}: {err}");
                Toasts::get()
                    .show(Toast::new("No se pudo usar el cuaderno").tone(ToastTone::Error));
                return;
            }
        };
        if let Some(board) = crate::active_boards::get_item(notebook) {
            crate::canvas::lock(&board).insert_text_at_view_center(&markdown);
        } else {
            let mut lib = handle.lock().await;
            let saved = match lib.load_doc(notebook).await {
                Ok(mut doc) => {
                    doc.append_text(&markdown, [38, 37, 42, 255]);
                    lib.save_doc(notebook, &doc).await
                }
                Err(err) => Err(err),
            };
            if let Err(err) = saved {
                log::error!("append to notebook {notebook}: {err}");
            }
        }
        crate::space::open(TabTarget::Item(notebook), OpenHint::Beside);
        Toasts::get().show(Toast::new("Agregado al cuaderno").tone(ToastTone::Done));
    });
}
