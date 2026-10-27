//! Translations of the Bible: the first-use card when none is
//! installed, and the catalog that downloads more.

use std::sync::{Arc, OnceLock};

use freya::prelude::*;
use notein_sdk::services::{Host, HostToast, ToastTone as HostTone};

use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, EmptyState, Group, ListRow, Notice, NoticeTone, ProgressBar, SearchField, Sheet,
    Skeleton, Tag, Text, TextRole, Trailing, tokens,
};

use crate::import::biblionlabs::{BibleEntryView, CatalogueView};
use crate::installer::{InstallState, Installer, Outcome, Phase, Step};
use crate::kind::BibleKind;

/// The Bible the catalog installs into.
static KIND: OnceLock<Arc<BibleKind>> = OnceLock::new();

/// Make `kind` the Bible the catalog installs into; the first one stays.
pub(crate) fn register(kind: &Arc<BibleKind>) {
    let _ = KIND.set(Arc::clone(kind));
}

fn kind() -> Option<&'static Arc<BibleKind>> {
    KIND.get()
}

/// The installer shared by every Bible surface.
fn installer() -> Option<&'static Installer> {
    kind().map(|kind| kind.installer())
}

/// Re-render the caller whenever a download moves.
#[must_use]
pub(crate) fn use_installs() -> State<u64> {
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

/// Toast every download as it ends, with a retry on failures, and pick up
/// the ones a closed app cut short. Runs for the life of the app.
pub(crate) fn announce(host: Host) {
    let Some(installer) = installer() else {
        return;
    };
    let rx = installer.outcomes();
    let spawned = std::thread::Builder::new()
        .name("bible-installs".to_owned())
        .spawn(move || {
            while let Ok(outcome) = rx.recv() {
                host.toast(outcome_toast(installer, outcome));
            }
        });
    if let Err(err) = spawned {
        log::error!("bible: install announcer: {err}");
    }
    installer.resume();
}

fn outcome_toast(installer: &'static Installer, outcome: Outcome) -> HostToast {
    match outcome {
        Outcome::Installed(bible_id) => HostToast::new(
            format!("«{}» lista para leer", bible_name(&bible_id)),
            HostTone::Download,
        ),
        Outcome::Failed { bible_id, reason } => HostToast::new(
            format!("No se pudo descargar «{}»: {reason}", bible_name(&bible_id)),
            HostTone::Error,
        )
        .action("Reintentar", move || installer.enqueue(&bible_id)),
    }
}

/// What a download is doing, for the line under its bar.
fn step_text(step: Step) -> String {
    let Step { phase, done, total } = step;
    match phase {
        Phase::Books if total == 0 => "Preparando la descarga…".to_owned(),
        Phase::Books => format!("Libros: {done} de {total}"),
        Phase::CrossRefs => format!("Referencias cruzadas: {done} de {total} libros"),
        Phase::Headings if total == 0 => "Subtítulos de sección…".to_owned(),
        Phase::Headings => format!("Subtítulos: {done} de {total} libros"),
    }
}

/// Short name of an installed translation, its id otherwise.
fn bible_name(bible_id: &str) -> String {
    kind()
        .and_then(|kind| kind.db().list_bibles().ok())
        .and_then(|bibles| bibles.into_iter().find(|b| b.id == bible_id))
        .map_or_else(|| bible_id.to_owned(), |b| b.abbrev)
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
    kind()
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
pub(crate) fn open() {
    Sheet::titled("Traducciones de la Biblia")
        .subtitle("Se descargan una por vez; podés cerrar y siguen.")
        .child(Catalog)
        .push();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Catalog;

/// Most chips the language filter shows; the rest are a search away.
const LANGUAGE_CHIPS: usize = 12;
/// Most rows the list builds at once.
const MAX_ROWS: usize = 120;

/// Whether `entry` matches the lower-cased `needle` by name, id or
/// language.
fn matches(entry: &BibleEntryView, needle: &str) -> bool {
    needle.is_empty()
        || label(entry).to_lowercase().contains(needle)
        || entry
            .name_english
            .as_deref()
            .is_some_and(|n| n.to_lowercase().contains(needle))
        || entry.id.to_lowercase().contains(needle)
        || entry
            .language
            .as_deref()
            .is_some_and(|l| l.to_lowercase().contains(needle))
}

/// The languages to offer as chips: the system one first, then the ones
/// with the most translations, and `chosen` even when it is neither.
fn language_chips(
    view: &CatalogueView,
    system: &str,
    chosen: Option<&str>,
) -> Vec<(String, String)> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for bible in &view.bibles {
        *counts.entry(bible.language_code.as_str()).or_default() += 1;
    }
    let mut languages: Vec<(&str, &str, usize)> = view
        .languages
        .iter()
        .filter_map(|l| {
            counts
                .get(l.id.as_str())
                .map(|n| (l.id.as_str(), l.local.as_str(), *n))
        })
        .collect();
    languages.sort_by(|a, b| {
        (b.0 == system)
            .cmp(&(a.0 == system))
            .then(b.2.cmp(&a.2))
            .then(a.1.cmp(b.1))
    });
    let mut chips: Vec<(String, String)> = languages
        .iter()
        .take(LANGUAGE_CHIPS)
        .map(|(id, name, _)| ((*id).to_owned(), (*name).to_owned()))
        .collect();
    if let Some(code) = chosen
        && !chips.iter().any(|(id, _)| id == code)
        && let Some((id, name, _)) = languages.iter().find(|(id, _, _)| *id == code)
    {
        chips.push(((*id).to_owned(), (*name).to_owned()));
    }
    chips
}

impl Component for Catalog {
    fn render(&self) -> impl IntoElement {
        let _installs = *use_installs().read();
        let catalogue = use_catalogue();
        let query = use_state(String::new);
        let system = catalogue_language(&system_language());
        // `None` until the reader picks a chip: then the system language
        // when the catalogue has it, every language otherwise.
        let mut language = use_state(|| None::<Option<String>>);
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
        let chosen = language.read().clone().unwrap_or_else(|| {
            view.bibles
                .iter()
                .any(|b| b.language_code == system)
                .then(|| system.to_owned())
        });
        let chips = std::iter::once(
            Tag::new("Todos")
                .active(chosen.is_none())
                .on_press(move |_| language.set(Some(None)))
                .into_element(),
        )
        .chain(
            language_chips(&view, system, chosen.as_deref())
                .into_iter()
                .map(|(id, name)| {
                    let on = chosen.as_deref() == Some(id.as_str());
                    Tag::new(name)
                        .key(id.clone())
                        .active(on)
                        .on_press(move |_| language.set(Some(Some(id.clone()))))
                        .into_element()
                }),
        );
        // A search looks through every language: the chip only narrows
        // the list while browsing.
        let shown: Vec<&BibleEntryView> = view
            .bibles
            .iter()
            .filter(|b| {
                !needle.is_empty() || chosen.as_deref().is_none_or(|code| b.language_code == code)
            })
            .filter(|b| matches(b, &needle))
            .collect();
        let total = shown.len();
        let rows: Vec<Element> = shown
            .into_iter()
            .take(MAX_ROWS)
            .map(|b| {
                Row {
                    entry: b.clone(),
                    installed: installed.contains(&b.id),
                    state: installer().and_then(|i| i.state(&b.id)),
                }
                .into_element()
            })
            .collect();
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(SearchField::new(query, "Buscar por nombre o idioma").height(38.0))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::wrap_spacing(6.0))
                    .spacing(6.0)
                    .children(chips),
            )
            .child(Group::default().children(rows))
            .maybe_child((total == 0).then(|| {
                Text::new("No hay traducciones con ese nombre.").role(TextRole::Hint)
            }))
            .maybe_child((total > MAX_ROWS).then(|| {
                Text::new(format!(
                    "Se muestran {MAX_ROWS} de {total}. Buscá por nombre o idioma para encontrar el resto."
                ))
                .role(TextRole::Hint)
            }))
            .into_element()
    }
}

/// One translation of the catalog. `state` travels as a prop: read inside
/// `render`, a download moving would not render the row again, since its
/// other props stay equal.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    entry: BibleEntryView,
    installed: bool,
    state: Option<InstallState>,
}

impl Component for Row {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let id = self.entry.id.clone();
        let state = self.state.clone();
        let trailing: Element = match (&state, self.installed) {
            (Some(InstallState::Downloading(step)), _) => {
                let fraction = step.fraction();
                rect()
                    .width(Size::px(120.0))
                    .spacing(4.0)
                    .child(
                        Text::new(format!("{} %", (fraction * 100.0).round())).role(TextRole::Meta),
                    )
                    .child(ProgressBar::new(fraction.max(0.02)))
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
            Some(InstallState::Downloading(step)) => step_text(*step),
            Some(InstallState::Queued) | None => {
                [self.entry.name_english.clone(), self.entry.language.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        };
        ListRow::new(label(&self.entry))
            .subtitle(subtitle)
            .trailing(Trailing::Element(trailing))
            .into_element()
    }
}

/// What the Bible shows before any translation is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FirstUse;

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
                    .filter(|b| b.language_code == language)
                    .min_by_key(|b| !label(b).to_lowercase().contains("reina"))
                    .or_else(|| view.bibles.first())
                    .cloned()
            });
        // Whatever downloads now, also one picked from the catalog.
        let state = installer()
            .and_then(Installer::active)
            .map(|(_, step)| InstallState::Downloading(step))
            .or_else(|| {
                recommended
                    .as_ref()
                    .and_then(|r| installer().and_then(|i| i.state(&r.id)))
            });
        let action: Element = match (&recommended, &state) {
            (_, Some(InstallState::Downloading(step))) => rect()
                .width(Size::px(320.0))
                .spacing(6.0)
                .child(ProgressBar::new(step.fraction().max(0.02)))
                .child(
                    Text::new(format!("{} · Se abre al terminar", step_text(*step)))
                        .role(TextRole::Meta),
                )
                .into_element(),
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
                            .child(Button::new("Ver todas las traducciones").ghost().on_press(|_| open())),
                    ),
            )
    }
}
