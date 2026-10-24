//! Translations of the Bible: the first-use card when none is
//! installed, and the catalog that downloads more.

use std::sync::{Arc, OnceLock};

use freya::prelude::*;
use notein_bible::import::biblionlabs::{BibleEntryView, CatalogueView};
use notein_bible::installer::{InstallState, Installer};
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, EmptyState, Group, ListRow, Notice, NoticeTone, ProgressBar, SearchField, Sheet,
    Skeleton, Tag, Text, TextRole, Trailing, tokens,
};

/// The installer shared by every Bible surface.
#[must_use]
pub fn installer() -> Option<&'static Installer> {
    static INSTALLER: OnceLock<Option<Installer>> = OnceLock::new();
    INSTALLER
        .get_or_init(|| {
            crate::plugins::builtin::bible_kind()
                .map(|kind| Installer::new(kind.db().clone(), kind.config().clone()))
        })
        .as_ref()
}

/// Re-render the caller whenever a download moves.
#[must_use] 
pub fn use_installs() -> State<u64> {
    let tick = use_state(|| 0u64);
    use_hook(move || {
        let Some(installer) = installer() else {
            return;
        };
        let rx = installer.changes();
        let mut tick = tick;
        spawn(async move {
            while rx.recv_async().await.is_ok() {
                while rx.try_recv().is_ok() {}
                let next = tick.peek().wrapping_add(1);
                tick.set(next);
            }
        });
    });
    tick
}

/// The system language as a two-letter code.
fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map_or_else(
            || "es".to_owned(),
            |v| v.chars().take(2).collect::<String>().to_lowercase(),
        )
}

/// ISO 639-3 code the catalogue uses for a two-letter one.
fn catalogue_language(two: &str) -> &'static str {
    match two {
        "en" => "eng",
        "pt" => "por",
        "fr" => "fra",
        "de" => "deu",
        "it" => "ita",
        "ru" => "rus",
        "zh" => "zho",
        "ko" => "kor",
        "ja" => "jpn",
        _ => "spa",
    }
}

fn label(entry: &BibleEntryView) -> String {
    entry
        .name_local
        .clone()
        .or_else(|| entry.name_english.clone())
        .unwrap_or_else(|| entry.id.clone())
}

fn installed_ids() -> Vec<String> {
    crate::plugins::builtin::bible_kind()
        .and_then(|k| k.db().list_bibles().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|b| b.id)
        .collect()
}

/// The catalogue, fetched once per run in the background.
fn use_catalogue() -> State<Option<Result<Arc<CatalogueView>, String>>> {
    static CACHE: OnceLock<std::sync::Mutex<Option<Arc<CatalogueView>>>> = OnceLock::new();
    let cached = CACHE
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|g| g.clone());
    let state = use_state(move || cached.map(Ok));
    use_hook(move || {
        if state.peek().is_some() {
            return;
        }
        let Some(installer) = installer() else {
            let mut state = state;
            state.set(Some(Err(
                "La Biblia no está disponible en este dispositivo.".to_owned(),
            )));
            return;
        };
        let (tx, rx) = flume::bounded(1);
        let installer = installer.clone();
        std::thread::spawn(move || {
            let _ = tx.send(
                installer
                    .catalogue()
                    .map(Arc::new)
                    .map_err(|e| e.to_string()),
            );
        });
        let mut state = state;
        spawn(async move {
            if let Ok(result) = rx.recv_async().await {
                if let Ok(view) = &result
                    && let Ok(mut cache) = CACHE.get_or_init(Default::default).lock()
                {
                    *cache = Some(Arc::clone(view));
                }
                state.set(Some(result));
            }
        });
    });
    state
}

/// Open the catalog of translations.
pub fn open_catalog() {
    Sheet::titled("Traducciones de la Biblia")
        .subtitle("Se descargan una por vez; podés cerrar y siguen.")
        .child(Catalog)
        .push();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Catalog;

impl Component for Catalog {
    fn render(&self) -> impl IntoElement {
        let _installs = *use_installs().read();
        let catalogue = use_catalogue();
        let query = use_state(String::new);
        let mut language = use_state(|| Some(catalogue_language(&system_language()).to_owned()));
        let current = catalogue.read().clone();
        let Some(result) = current else {
            return rect()
                .width(Size::fill())
                .spacing(8.0)
                .children((0..5).map(|i| rect().key(i).child(Skeleton::new(48.0)).into_element()))
                .into_element();
        };
        let view = match result {
            Ok(view) => view,
            Err(why) => {
                return Notice::new(NoticeTone::Danger, "No se pudo cargar el catálogo")
                    .detail(why)
                    .into_element();
            }
        };
        let installed = installed_ids();
        let needle = query.read().trim().to_lowercase();
        let chosen = language.read().clone();
        let mut languages: Vec<(String, String)> = view
            .languages
            .iter()
            .filter(|l| {
                view.bibles
                    .iter()
                    .any(|b| b.language.as_deref() == Some(l.id.as_str()))
            })
            .map(|l| (l.id.clone(), l.local.clone()))
            .collect();
        languages.sort_by(|a, b| a.1.cmp(&b.1));
        let chips = std::iter::once(
            Tag::new("Todos")
                .active(chosen.is_none())
                .on_press(move |_| language.set(None))
                .into_element(),
        )
        .chain(languages.into_iter().take(14).map(|(id, name)| {
            let on = chosen.as_deref() == Some(id.as_str());
            Tag::new(name)
                .active(on)
                .on_press(move |_| language.set(Some(id.clone())))
                .into_element()
        }));
        let rows: Vec<Element> = view
            .bibles
            .iter()
            .filter(|b| chosen.is_none() || b.language == chosen)
            .filter(|b| {
                needle.is_empty()
                    || label(b).to_lowercase().contains(&needle)
                    || b.id.to_lowercase().contains(&needle)
            })
            .take(120)
            .map(|b| {
                Row {
                    entry: b.clone(),
                    installed: installed.contains(&b.id),
                }
                .into_element()
            })
            .collect();
        let empty = rows.is_empty();
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(SearchField::new(query, "Buscar una traducción").height(38.0))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::wrap_spacing(6.0))
                    .spacing(6.0)
                    .children(chips),
            )
            .child(Group::default().children(rows))
            .maybe_child(
                empty
                    .then(|| Text::new("No hay traducciones con ese nombre.").role(TextRole::Hint)),
            )
            .into_element()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    entry: BibleEntryView,
    installed: bool,
}

impl Component for Row {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let id = self.entry.id.clone();
        let state = installer().and_then(|i| i.state(&id));
        let trailing: Element = match (&state, self.installed) {
            (Some(InstallState::Downloading { done, total }), _) => {
                #[allow(clippy::cast_precision_loss)] // book counts
                let fraction = if *total == 0 {
                    0.02
                } else {
                    *done as f32 / *total as f32
                };
                rect()
                    .width(Size::px(120.0))
                    .spacing(4.0)
                    .child(
                        Text::new(format!("{} %", (fraction * 100.0).round())).role(TextRole::Meta),
                    )
                    .child(ProgressBar::new(fraction))
                    .into_element()
            }
            (Some(InstallState::Queued), _) => {
                Text::new("En cola").role(TextRole::Meta).into_element()
            }
            (Some(InstallState::Failed(_)), _) => Button::new("Reintentar")
                .danger()
                .small()
                .on_press(move |_| {
                    if let Some(installer) = installer() {
                        installer.enqueue(&id);
                    }
                })
                .into_element(),
            (None, true) => Text::new("Instalada")
                .role(TextRole::Meta)
                .color(p.ok)
                .into_element(),
            (None, false) => Button::new("Instalar")
                .tonal()
                .small()
                .on_press(move |_| {
                    if let Some(installer) = installer() {
                        installer.enqueue(&id);
                    }
                })
                .into_element(),
        };
        let subtitle = match &state {
            Some(InstallState::Failed(why)) => why.clone(),
            _ => [self.entry.name_english.clone(), self.entry.language.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
        };
        ListRow::new(label(&self.entry))
            .subtitle(subtitle)
            .trailing(Trailing::Element(trailing))
            .into_element()
    }
}

/// What the Bible shows before any translation is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstUse;

impl Component for FirstUse {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let _installs = *use_installs().read();
        let catalogue = use_catalogue();
        let language = catalogue_language(&system_language());
        let current = catalogue.read().clone();
        let recommended = current
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|view| {
                view.bibles
                    .iter()
                    .filter(|b| b.language.as_deref() == Some(language))
                    .min_by_key(|b| !label(b).to_lowercase().contains("reina"))
                    .or_else(|| view.bibles.first())
                    .cloned()
            });
        let state = recommended
            .as_ref()
            .and_then(|r| installer().and_then(|i| i.state(&r.id)));
        let action: Element = match (&recommended, &state) {
            (_, Some(InstallState::Downloading { done, total })) => {
                #[allow(clippy::cast_precision_loss)] // book counts
                let fraction = if *total == 0 {
                    0.02
                } else {
                    *done as f32 / *total as f32
                };
                rect()
                    .width(Size::px(320.0))
                    .spacing(6.0)
                    .child(ProgressBar::new(fraction))
                    .child(
                        Text::new(if *total == 0 {
                            "Preparando la descarga…".to_owned()
                        } else {
                            format!("Descargando {done} de {total} libros. Podés empezar a leer cuando aparezca el primero.")
                        })
                        .role(TextRole::Meta),
                    )
                    .into_element()
            }
            (_, Some(InstallState::Queued)) => {
                Text::new("En cola…").role(TextRole::Meta).into_element()
            }
            (Some(entry), failed) => {
                let id = entry.id.clone();
                rect()
                    .spacing(8.0)
                    .cross_align(Alignment::Center)
                    .maybe_child(match failed {
                        Some(InstallState::Failed(why)) => Some(
                            Notice::new(NoticeTone::Danger, "No se pudo descargar")
                                .detail(why.clone()),
                        ),
                        _ => None,
                    })
                    .child(
                        Button::new(format!("Descargar {} y abrir", label(entry)))
                            .primary()
                            .icon(lucide::download())
                            .on_press(move |_| {
                                if let Some(installer) = installer() {
                                    installer.enqueue(&id);
                                }
                            }),
                    )
                    .into_element()
            }
            (None, _) => match current {
                None => Skeleton::new(40.0).width(260.0).into_element(),
                Some(Err(why)) => Notice::new(NoticeTone::Warning, "Sin conexión al catálogo")
                    .detail(why)
                    .into_element(),
                Some(Ok(_)) => rect().into_element(),
            },
        };
        rect()
            .expanded()
            .center()
            .padding(tokens::space::XL)
            .child(
                EmptyState::new(lucide::book_open(), "Elegí una traducción para empezar")
                    .body("Se descarga una vez y queda en este dispositivo. Después podés leer sin conexión.")
                    .action(
                        rect()
                            .spacing(12.0)
                            .cross_align(Alignment::Center)
                            .child(action)
                            .child(Button::new("Ver todas las traducciones").ghost().on_press(|_| open_catalog())),
                    ),
            )
    }
}
