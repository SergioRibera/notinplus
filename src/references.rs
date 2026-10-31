//! References between documents: how a chip looks, what `/ref` offers,
//! copying a reference and putting one in a notebook.

use freya::prelude::*;
use freya_markdown_editor::document::{Block, BlockKind, Document, Inline};
use freya_markdown_editor::markdown::Dialect;
use notein_i18n::tr;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::doc::DocMeta;
use notein_plugin::kind::{KindColor, Quote, SearchQuery};
use notein_ui_kit::theme::Palette;
use notein_ui_kit::{Toast, ToastTone, Toasts};

use crate::library::{ItemId, LibraryData, TabTarget};
use crate::plugins::content::doc_id;
use crate::space::OpenHint;

/// Most suggestions `/ref` offers per kind.
const SUGGESTIONS_PER_KIND: usize = 6;
/// Most documents `/ref` offers by name.
const SUGGESTED_DOCS: usize = 8;
/// Places per kind offered with their content too.
const QUOTED_PER_KIND: usize = 3;

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
#[sismo::function("references::display")]
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
                label: name.map_or_else(
                    || tr!("references-bookmark"),
                    |n| tr!("references-bookmark-in", "name" => n),
                ),
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
                    label: tr!("references-needs-plugin"),
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
            let broken = chip.broken || (doc.is_none() && !info.shared_refs);
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
    /// Picking it inserts what the target holds instead of a chip.
    pub quote: bool,
}

/// What `/ref query` offers: documents by name, then places inside them
/// from every kind's search. The first places of a kind that can quote
/// them come with a second entry that inserts their content.
#[must_use]
#[sismo::function("references::suggestions")]
pub fn suggestions(query: &str, data: Option<&LibraryData>) -> Vec<Suggestion> {
    let needle = notein_plugin::fold(query.trim());
    let mut out: Vec<Suggestion> = data
        .map(|data| {
            data.live_items()
                .filter(|i| needle.is_empty() || notein_plugin::fold(&i.name).contains(&needle))
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
                    quote: false,
                })
                .collect()
        })
        .unwrap_or_default();
    if needle.is_empty() {
        return out;
    }
    let results = crate::plugins::catalog().search(&SearchQuery::new(query, SUGGESTIONS_PER_KIND));
    for found in results {
        let label = crate::plugins::kind_info(&found.kind)
            .map_or_else(String::new, |k| k.label.into_owned());
        let quotes = crate::plugins::kind(&found.kind).is_some_and(|kind| kind.quotes());
        for (rank, hit) in found.hits.into_iter().enumerate() {
            let target = SourceRef::Plugin {
                plugin_id: found.kind.clone(),
                item: hit.doc.map_or([0; 16], |d| d.0),
                anchor: hit.anchor,
                selection: None,
            };
            if quotes && rank < QUOTED_PER_KIND {
                out.push(Suggestion {
                    label: hit.title.clone(),
                    subtitle: tr!("references-suggest-reference", "kind" => label.as_str()),
                    target: target.clone(),
                    quote: false,
                });
                out.push(Suggestion {
                    label: hit.title,
                    subtitle: tr!("references-suggest-insert-text", "kind" => label.as_str()),
                    target,
                    quote: true,
                });
            } else {
                out.push(Suggestion {
                    label: hit.title,
                    subtitle: format!("{label} · {}", hit.snippet),
                    target,
                    quote: false,
                });
            }
        }
    }
    out
}

/// What `target` holds, quoted by its kind. `None` when the kind cannot
/// quote or the place is gone.
#[must_use]
#[sismo::function("references::quote")]
pub fn quote(target: &SourceRef, data: Option<&LibraryData>) -> Option<Quote> {
    let SourceRef::Plugin {
        plugin_id,
        item,
        anchor,
        ..
    } = target
    else {
        return None;
    };
    let kind = crate::plugins::kind(plugin_id).filter(|kind| kind.quotes())?;
    let id = ItemId(*item);
    let doc = data.and_then(|d| d.item(id));
    let name = doc.map_or_else(String::new, |d| d.name.clone());
    let meta = DocMeta {
        id: doc_id(id),
        name: &name,
        updated_at: doc.map_or(0, |d| d.updated_at),
        tint: doc.and_then(|d| d.color),
    };
    kind.quote(meta, anchor)
}

/// Markdown of `quote` as a block quote of plain lines (lists become
/// bullets, headings bold) closed by a chip to `target`.
#[must_use]
#[sismo::function("references::quote_markdown")]
pub fn quote_markdown(quote: &Quote, target: &SourceRef) -> String {
    let mut lines = quote_lines(&quote.body);
    lines.push(format!("— {}", chip_markdown(&quote.source, target)));
    lines
        .iter()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n>\n")
}

/// Markdown of `quote` as a quote that keeps showing what `target` holds
/// now; the lines written are what it held when inserted.
#[must_use]
#[sismo::function("references::live_quote_markdown")]
pub fn live_quote_markdown(quote: &Quote, target: &SourceRef) -> String {
    format!(
        "> [!{}]\n>\n{}",
        crate::editor::LIVE_QUOTE,
        quote_markdown(quote, target)
    )
}

/// A line of quoted content as it reads, without markdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteLine {
    pub text: String,
    pub heading: bool,
    /// List nesting; `None` outside a list.
    pub bullet: Option<u8>,
}

/// The blocks of `markdown` as plain lines.
#[must_use]
#[sismo::function("references::quote_text")]
pub fn quote_text(markdown: &str) -> Vec<QuoteLine> {
    Document::parse(markdown)
        .blocks()
        .iter()
        .filter_map(|block| {
            let line = |content: &Inline, heading, bullet| QuoteLine {
                text: content.plain_text().trim().to_owned(),
                heading,
                bullet,
            };
            let line = match &block.kind {
                BlockKind::Heading { content, .. } => line(content, true, None),
                BlockKind::ListItem { depth, content, .. } => line(content, false, Some(*depth)),
                kind => line(kind.content()?, false, None),
            };
            (!line.text.is_empty()).then_some(line)
        })
        .collect()
}

/// One line per block of `markdown`, in the shape a quote keeps.
#[sismo::function("references::quote_lines")]
fn quote_lines(markdown: &str) -> Vec<String> {
    let document = Document::parse(markdown);
    document
        .blocks()
        .iter()
        .filter_map(|block| {
            let source = |content: &Inline| {
                Document::from(vec![Block::paragraph(content.clone())])
                    .to_markdown(Dialect::Rich)
                    .trim()
                    .to_owned()
            };
            let line = match &block.kind {
                BlockKind::Heading { content, .. } => format!("**{}**", source(content)),
                BlockKind::ListItem { depth, content, .. } => {
                    format!(
                        "{}• {}",
                        "\u{a0}\u{a0}".repeat(usize::from(*depth)),
                        source(content)
                    )
                }
                kind => source(kind.content()?),
            };
            (!line.trim().is_empty()).then_some(line)
        })
        .collect()
}

/// Markdown that renders as a chip for `target`.
#[must_use]
#[sismo::function("references::chip_markdown")]
pub fn chip_markdown(label: &str, target: &SourceRef) -> String {
    format!("[{}]({})", label.replace(['[', ']'], ""), target.to_url())
}

/// Insert a chip for `target` in `from`'s notebook, creating the
/// notebook when `from` has none, and show it beside.
#[sismo::function("references::insert_in_notebook")]
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
                    .show(Toast::new(tr!("references-notebook-failed")).tone(ToastTone::Error));
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
        Toasts::get().show(Toast::new(tr!("references-added-to-notebook")).tone(ToastTone::Done));
    });
}

#[cfg(test)]
mod quote_tests {
    use super::*;

    #[test]
    fn a_live_quote_reads_back_as_one_transclusion_of_its_source() {
        let quote = Quote {
            body: "Sea la luz.".to_owned(),
            source: "Génesis 1:3 (RVR1960)".to_owned(),
        };
        let target = SourceRef::External {
            url: "https://example.com".to_owned(),
        };
        let document = Document::parse(&live_quote_markdown(&quote, &target));
        let BlockKind::Transclusion(live) = &document.blocks()[0].kind else {
            panic!("{:?}", document.blocks());
        };
        assert_eq!(live.kind, crate::editor::LIVE_QUOTE);
        assert_eq!(live.url, "https://example.com");
        assert_eq!(live.label, "Génesis 1:3 (RVR1960)");
        assert_eq!(live.fallback, "Sea la luz.");
    }

    #[test]
    fn quoted_text_drops_markdown_but_keeps_the_shape() {
        let lines = quote_text("## Tema\n\nUna *idea*.\n\n- uno\n  - dos");
        let shape: Vec<_> = lines
            .iter()
            .map(|l| (l.text.as_str(), l.heading, l.bullet))
            .collect();
        assert_eq!(
            shape,
            [
                ("Tema", true, None),
                ("Una idea.", false, None),
                ("uno", false, Some(0)),
                ("dos", false, Some(1)),
            ]
        );
    }

    #[test]
    fn a_quote_becomes_plain_quoted_lines_closed_by_its_source() {
        let quote = Quote {
            body: "Una *idea*.\n\n### Detalle\n\n- uno\n  - dos".to_owned(),
            source: "Nota › Ideas".to_owned(),
        };
        let target = SourceRef::External {
            url: "https://example.com".to_owned(),
        };
        let markdown = quote_markdown(&quote, &target);
        assert_eq!(
            markdown,
            "> Una *idea*.\n>\n> **Detalle**\n>\n> • uno\n>\n> \u{a0}\u{a0}• dos\n>\n> — [Nota › Ideas](https://example.com)"
        );
        let document = Document::parse(&markdown);
        let quoted = document.blocks().iter().filter(
            |block| !matches!(&block.kind, BlockKind::Paragraph(content) if content.is_empty()),
        );
        assert!(
            quoted
                .clone()
                .all(|block| matches!(block.kind, BlockKind::Quote(_)))
                && quoted.count() == 5,
            "{:?}",
            document.blocks()
        );
    }
}
