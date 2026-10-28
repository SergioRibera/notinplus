//! The update chip on Home, the release card it opens, the download
//! sheet and the Ajustes rows.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use freya::icons::lucide;
use freya::prelude::*;
use freya_markdown_editor::prelude::use_editor_controller;
use istmo_installer::{InstallError, InstallerClient};
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Action, ActionEntry, ActionList, Button, Group, IconButton, ListRow, ModalController, Notice,
    NoticeTone, Popover, PopoverAlign, ProgressBar, Section, Sheet, SheetFooter, Tag, Text,
    TextRole, Toast, ToastTone, Toasts, Trailing, press_anchor,
};

use super::download::{self, Finish, Progress};
use super::{Asset, CheckStatus, Device, INSTALLED, Release, Updates};

/// How often the download sheet reads the byte count.
const PROGRESS_POLL: Duration = Duration::from_millis(120);

/// Home's announcement of a newer version: a tag on wide screens, an
/// accent icon on phones. Nothing when there is no news.
#[must_use]
pub fn chip(compact: bool) -> Option<Element> {
    let release = Updates::get().read().announced().cloned()?;
    let p = palette();
    let label = format!("{} disponible", release.version);
    Some(if compact {
        IconButton::new(lucide::sparkles(), label)
            .tint(p.accent)
            .dot(p.accent)
            .on_press(move |e: Event<PressEventData>| {
                open_release(Some(press_anchor(e.data())), &release);
            })
            .into_element()
    } else {
        Tag::new(label)
            .icon(lucide::sparkles())
            .active(true)
            .on_press(move |e: Event<PressEventData>| {
                open_release(Some(press_anchor(e.data())), &release);
            })
            .into_element()
    })
}

/// The release card: hanging from `anchor` (a bottom sheet on phones),
/// or as a sheet when opened from Ajustes.
pub fn open_release(anchor: Option<Area>, release: &Arc<Release>) {
    let title = format!("Notein {}", release.version);
    let card = |framed| ReleaseCard {
        release: Arc::clone(release),
        framed,
    };
    match anchor {
        Some(anchor) => Popover::new(anchor, card(!notein_ui_kit::width_class().is_compact()))
            .width(440.0)
            .align(PopoverAlign::End)
            .estimated_height(520.0)
            .sheet_title(title)
            .open(),
        None => Sheet::titled(title).child(card(false)).push(),
    }
}

/// Close whatever the card was shown in.
fn close() {
    if ModalController::get().is_open() {
        ModalController::get().close();
    }
}

#[derive(Clone, PartialEq)]
struct ReleaseCard {
    release: Arc<Release>,
    /// Shown bare in a popover: the card brings its own title and
    /// padding, which a sheet would otherwise give it.
    framed: bool,
}

impl Component for ReleaseCard {
    fn render(&self) -> impl IntoElement {
        let release = &self.release;
        let assets: Vec<Asset> = release
            .assets_for(&Device::current())
            .into_iter()
            .cloned()
            .collect();
        let chosen = use_state(|| 0usize);
        let asset = assets
            .get(*chosen.read())
            .or_else(|| assets.first())
            .cloned();
        let subtitle = release.published_on().map_or_else(
            || format!("Tenés la {INSTALLED}"),
            |day| format!("Tenés la {INSTALLED} · publicada {day}"),
        );
        let url = release.url.clone();
        let version = release.version.clone();
        rect()
            .width(Size::fill())
            .spacing(14.0)
            .padding(if self.framed { 18.0 } else { 0.0 })
            .maybe_child(
                self.framed.then(|| {
                    Text::new(format!("Notein {}", release.version)).role(TextRole::Title)
                }),
            )
            .child(Text::new(subtitle).role(TextRole::Meta))
            .child(Notes {
                markdown: release.notes.clone(),
            })
            .child(
                Button::new("Ver la versión completa en GitHub")
                    .ghost()
                    .small()
                    .icon(lucide::arrow_up_right())
                    .on_press(move |_| crate::external::open_url(&url)),
            )
            .maybe_child(asset.as_ref().map(|asset| file_row(asset, &assets, chosen)))
            .child(actions(&version, &release.url, asset))
    }
}

/// The release notes, rendered read-only.
#[derive(Clone, PartialEq)]
struct Notes {
    markdown: String,
}

impl Component for Notes {
    fn render(&self) -> impl IntoElement {
        let controller = use_editor_controller();
        let value = use_state(|| strip_heading(&self.markdown).to_owned());
        crate::editor::editor(value, controller)
            .read_only(true)
            .theme(crate::editor::theme(&palette()))
            .width(Size::fill())
    }
}

/// The notes open with `## 0.2.0 — date`, which the card already says.
fn strip_heading(notes: &str) -> &str {
    let notes = notes.trim_start();
    if notes.starts_with("## ") {
        notes.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        notes
    }
}

/// The file that will download, with a menu of the other formats when
/// the platform has several (Linux).
fn file_row(asset: &Asset, all: &[Asset], chosen: State<usize>) -> Element {
    let others = all.len() > 1;
    let menu: Vec<ActionEntry> = all
        .iter()
        .enumerate()
        .map(|(index, other)| {
            ActionEntry::Item(
                Action::closing(other.format_label().to_owned(), move || {
                    let mut chosen = chosen;
                    chosen.set(index);
                })
                .subtitle(format!(
                    "{} · {}",
                    other.arch,
                    crate::format::size(other.size)
                ))
                .checked(other == asset),
            )
        })
        .collect();
    let hint = match Finish::current() {
        Finish::Install if cfg!(target_os = "android") => {
            "Android te pide confirmar la instalación; tus documentos no se tocan."
        }
        Finish::Install => "Al terminar se abre el instalador.",
        Finish::Reveal => "Se guarda en Descargas; lo instalás vos.",
    };
    Group::default()
        .child(
            ListRow::new(format!(
                "{} · {}",
                asset.format_label(),
                crate::format::size(asset.size)
            ))
            .subtitle(hint)
            .icon(lucide::download())
            .trailing(if others {
                Trailing::Element(
                    Button::new("Otros formatos")
                        .ghost()
                        .small()
                        .on_press(move |e: Event<PressEventData>| {
                            ActionList::new(menu.clone())
                                .width(280.0)
                                .show_at(press_anchor(e.data()));
                        })
                        .into_element(),
                )
            } else {
                Trailing::Text(String::new().into())
            }),
        )
        .into_element()
}

fn actions(version: &str, page: &str, asset: Option<Asset>) -> Element {
    let skip = version.to_owned();
    let primary = asset.map_or_else(
        || {
            let page = page.to_owned();
            Button::new("Abrir en GitHub")
                .primary()
                .icon(lucide::arrow_up_right())
                .on_press(move |_| {
                    close();
                    crate::external::open_url(&page);
                })
        },
        |asset| {
            let version = version.to_owned();
            Button::new("Descargar")
                .primary()
                .icon(lucide::download())
                .on_press(move |_| {
                    close();
                    start_download(&version, asset.clone());
                })
        },
    );
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(8.0)
        .child(
            Button::new("Omitir esta versión")
                .ghost()
                .small()
                .on_press(move |_| {
                    Updates::get().skip(&skip);
                    close();
                }),
        )
        .child(rect().width(Size::flex(1.0)))
        .child(Button::new("Ahora no").small().on_press(|_| close()))
        .child(primary)
        .into_element()
}

/// Open the download sheet; the download runs while it is open.
fn start_download(version: &str, asset: Asset) {
    Sheet::titled(format!("Descargando Notein {version}"))
        .child(DownloadSheet { asset })
        .sticky()
        .open();
}

/// Where a download stands.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Downloading,
    Ready(PathBuf),
    /// Android sent the person to allow installs; "Instalar" retries.
    NeedsPermission(PathBuf),
    /// The installer is open (Windows) or showing (Android).
    Handed,
    Failed(String),
}

#[derive(Clone, PartialEq)]
struct DownloadSheet {
    asset: Asset,
}

impl Component for DownloadSheet {
    fn render(&self) -> impl IntoElement {
        let mut phase = use_state(|| Phase::Downloading);
        let mut received = use_state(|| 0u64);
        let progress = use_hook({
            let asset = self.asset.clone();
            move || {
                let progress = Arc::new(Progress::default());
                let (tx, rx) = flume::bounded(1);
                {
                    let progress = Arc::clone(&progress);
                    std::thread::spawn(move || {
                        let _ = tx.send(download::fetch(&asset, &progress));
                    });
                }
                let watched = Arc::clone(&progress);
                spawn_in_window(async move {
                    let result = loop {
                        if let Ok(result) = rx.try_recv() {
                            break Some(result);
                        }
                        if rx.is_disconnected() {
                            break None;
                        }
                        received.set(watched.received());
                        async_io::Timer::after(PROGRESS_POLL).await;
                    };
                    received.set(watched.received());
                    match result {
                        Some(Ok(path)) => {
                            phase.set(Phase::Ready(path.clone()));
                            if Finish::current() == Finish::Install {
                                install(path, phase);
                            }
                        }
                        Some(Err(super::UpdateError::Cancelled)) | None => {}
                        Some(Err(err)) => {
                            log::warn!("update download: {err}");
                            phase.set(Phase::Failed(failure(&err)));
                        }
                    }
                });
                progress
            }
        });
        use_drop({
            let progress = Arc::clone(&progress);
            move || progress.cancel()
        });
        let current = phase.read().clone();
        let total = self.asset.size.max(1);
        let got = (*received.read()).min(total);
        #[allow(clippy::cast_precision_loss)] // a progress fraction
        let fraction = got as f32 / total as f32;
        let p = palette();
        let status: Element = match &current {
            Phase::Downloading => Text::new(format!(
                "{} de {}",
                crate::format::size(got),
                crate::format::size(self.asset.size)
            ))
            .role(TextRole::Meta)
            .into_element(),
            Phase::Ready(path) => Notice::new(NoticeTone::Success, ready_text(path)).into_element(),
            Phase::NeedsPermission(_) => Notice::new(
                NoticeTone::Warning,
                "Permití que Notein instale apps y tocá «Instalar» de nuevo.",
            )
            .into_element(),
            Phase::Handed => Notice::new(NoticeTone::Info, handed_text()).into_element(),
            Phase::Failed(message) => {
                Notice::new(NoticeTone::Danger, message.clone()).into_element()
            }
        };
        rect()
            .width(Size::fill())
            .spacing(14.0)
            .child(
                Text::new(self.asset.name.clone())
                    .role(TextRole::Strong)
                    .lines(1),
            )
            .child(ProgressBar::new(fraction).color(match current {
                Phase::Failed(_) => p.danger,
                _ => p.accent,
            }))
            .child(status)
            .child(footer(&current, &progress, phase))
    }
}

fn footer(current: &Phase, progress: &Arc<Progress>, phase: State<Phase>) -> Element {
    let buttons: Vec<Element> = match current {
        Phase::Downloading => {
            let progress = Arc::clone(progress);
            vec![
                Button::new("Cancelar")
                    .on_press(move |_| {
                        progress.cancel();
                        close();
                    })
                    .into_element(),
            ]
        }
        Phase::Ready(path) | Phase::NeedsPermission(path) => {
            let path = path.clone();
            let main = match Finish::current() {
                Finish::Install => Button::new("Instalar")
                    .primary()
                    .on_press(move |_| install(path.clone(), phase)),
                Finish::Reveal => Button::new("Mostrar en la carpeta")
                    .primary()
                    .icon(lucide::folder_open())
                    .on_press(move |_| reveal(&path)),
            };
            vec![
                Button::new("Cerrar").on_press(|_| close()).into_element(),
                main.into_element(),
            ]
        }
        Phase::Handed | Phase::Failed(_) => {
            vec![Button::new("Cerrar").on_press(|_| close()).into_element()]
        }
    };
    SheetFooter::new(buttons).into_element()
}

fn ready_text(path: &std::path::Path) -> String {
    match Finish::current() {
        Finish::Install => "Descargada y verificada.".to_owned(),
        Finish::Reveal => format!("Descargada y verificada en {}.", path.display()),
    }
}

const fn handed_text() -> &'static str {
    if cfg!(target_os = "windows") {
        "El instalador está abierto. Cerrá Notein para que pueda reemplazarla."
    } else {
        "Seguí los pasos del instalador de Android."
    }
}

fn failure(err: &super::UpdateError) -> String {
    match err {
        super::UpdateError::Checksum => {
            "El archivo llegó dañado y se borró. Probá de nuevo.".to_owned()
        }
        super::UpdateError::Http(_) => "No se pudo descargar. Revisá la conexión.".to_owned(),
        super::UpdateError::Io(_) => {
            "No se pudo guardar el archivo en este dispositivo.".to_owned()
        }
        super::UpdateError::Manifest(_) | super::UpdateError::Cancelled => {
            "No se pudo descargar.".to_owned()
        }
    }
}

/// Hand `path` to the system installer.
fn install(path: PathBuf, mut phase: State<Phase>) {
    let client = match InstallerClient::acquire() {
        Ok(client) => client,
        Err(err) => {
            log::warn!("installer: {err}");
            reveal(&path);
            return;
        }
    };
    spawn_in_window(async move {
        let shown = path.display().to_string();
        match client.install(shown).await.map_err(InstallError::from) {
            Ok(()) => phase.set(Phase::Handed),
            Err(InstallError::NeedsPermission) => phase.set(Phase::NeedsPermission(path)),
            Err(InstallError::Unsupported) => reveal(&path),
            Err(err) => {
                log::warn!("install {}: {err}", path.display());
                phase.set(Phase::Failed("No se pudo abrir el instalador.".to_owned()));
            }
        }
    });
}

/// Show the downloaded file's folder in the file manager.
fn reveal(path: &std::path::Path) {
    let folder = path.parent().unwrap_or(path);
    crate::external::open_url(&format!("file://{}", folder.display()));
}

/// Ajustes › Acerca de › Actualizaciones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdatesSection;

impl Component for UpdatesSection {
    fn render(&self) -> impl IntoElement {
        let updates = Updates::get();
        let snapshot = updates.read();
        let checking = snapshot.status == CheckStatus::Checking;
        let last = if snapshot.record.fetched_at == 0 {
            "Todavía no se buscó".to_owned()
        } else {
            format!(
                "Última búsqueda: {}",
                crate::format::ago(snapshot.record.fetched_at, crate::format::now()).to_lowercase()
            )
        };
        let state = match (&snapshot.newer, snapshot.status) {
            (_, CheckStatus::Checking) => "Buscando…".to_owned(),
            (_, CheckStatus::Failed) => "No se pudo buscar. Revisá la conexión.".to_owned(),
            (Some(release), _) => format!("Hay una versión nueva: {}", release.version),
            (None, CheckStatus::UpToDate) => format!("Estás al día · {last}"),
            (None, CheckStatus::Idle) => last,
        };
        let mut group = Group::default()
            .child(
                ListRow::new("Buscar actualizaciones")
                    .subtitle(
                        "Cada dos días. Solo lee un archivo público de GitHub; no manda nada tuyo.",
                    )
                    .icon(lucide::refresh_cw())
                    .trailing(Trailing::Switch {
                        on: !snapshot.record.manual_only,
                        on_toggle: EventHandler::new(move |on| updates.set_automatic(on)),
                    }),
            )
            .child(
                ListRow::new(format!("Notein {INSTALLED}"))
                    .subtitle(state)
                    .icon(lucide::package())
                    .trailing(Trailing::Element(
                        Button::new("Buscar ahora")
                            .ghost()
                            .small()
                            .loading(checking)
                            .enabled(!checking)
                            .on_press(move |_| updates.check(true))
                            .into_element(),
                    )),
            );
        if let Some(release) = snapshot.newer.clone() {
            group = group.child(
                ListRow::new(format!("Ver novedades de la {}", release.version))
                    .icon(lucide::sparkles())
                    .chevron()
                    .on_press(move |_| open_release(None, &release)),
            );
        }
        if let Some(skipped) = snapshot.record.skipped {
            group = group.child(
                ListRow::new("Versión omitida")
                    .subtitle(format!(
                        "No te avisa de la {skipped}. Te avisa de la siguiente."
                    ))
                    .icon(lucide::eye_off())
                    .trailing(Trailing::Element(
                        Button::new("Mostrar de nuevo")
                            .ghost()
                            .small()
                            .on_press(move |_| {
                                updates.unskip();
                                Toasts::get().show(
                                    Toast::new("Se va a volver a avisar").tone(ToastTone::Info),
                                );
                            })
                            .into_element(),
                    )),
            );
        }
        Section::new("Actualizaciones").child(group)
    }
}

#[cfg(test)]
mod tests {
    use super::strip_heading;

    #[test]
    fn the_card_drops_the_heading_the_notes_open_with() {
        assert_eq!(
            strip_heading("## 0.2.0 — 2026-10-26\n### New\n- x"),
            "### New\n- x"
        );
        assert_eq!(strip_heading("### New\n- x"), "### New\n- x");
    }
}
