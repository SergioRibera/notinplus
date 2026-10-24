//! Ajustes › Plugins: what is installed, turning plugins on and off,
//! their settings, and installing a new one.

use std::sync::Arc;

use freya::prelude::*;
use notein_plugin::catalog::{Installed, Origin};
use notein_plugin::loader::Review;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, Group, Icon, ListRow, ModalController, Notice, NoticeTone, Section, Sheet, SheetFooter,
    Switch, Text, TextRole, Toast, Toasts, Trailing,
};

use super::schema::SchemaForm;
use crate::plugins::dynamic::{self, Staged};
use crate::references::kind_color;
use crate::settings::SettingsStore;

fn status(plugin: &Installed) -> &'static str {
    match (&plugin.origin, plugin.is_active()) {
        (_, false) => "Desactivado",
        (Origin::Builtin, true) => "Incluido",
        (Origin::ThirdParty { .. }, true) => "De terceros",
    }
}

fn set_enabled(id: String, on: bool) {
    crate::plugins::catalog::catalog_mut().set_enabled(&id, on);
    let mut store = SettingsStore::get();
    store.update(|s| {
        s.plugins.disabled.retain(|d| *d != id);
        if !on {
            s.plugins.disabled.push(id.clone());
        }
    });
    crate::plugins::content::index_library();
}

/// The Plugins section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Plugins;

impl Component for Plugins {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        // Re-render when a plugin is switched on or off.
        let _disabled = SettingsStore::get().read().plugins;
        let mut open = use_state(|| None::<String>);
        let catalog = crate::plugins::catalog();
        if let Some(id) = open.read().clone()
            && let Some(plugin) = catalog.plugin(&id)
        {
            let plugin = plugin.clone();
            drop(catalog);
            return rect()
                .width(Size::fill())
                .spacing(16.0)
                .child(
                    Button::new("Todos los plugins")
                        .ghost()
                        .small()
                        .icon(lucide::arrow_left())
                        .on_press(move |_| open.set(None)),
                )
                .child(Detail {
                    plugin: Arc::new(plugin),
                })
                .into_element();
        }
        let rows: Vec<Element> = catalog
            .plugins()
            .iter()
            .map(|plugin| {
                let id = plugin.info.id.to_string();
                let icon = plugin.kinds.first().map(|k| k.info());
                let toggle_id = id.clone();
                ListRow::new(plugin.info.name.clone())
                    .subtitle(format!("{} · {}", status(plugin), plugin.info.description))
                    .kind(
                        icon.as_ref()
                            .map_or_else(lucide::puzzle, |k| k.icon.clone()),
                        icon.map_or(p.t3, |k| kind_color(&p, k.color)),
                    )
                    .trailing(Trailing::Switch {
                        on: plugin.is_active(),
                        on_toggle: EventHandler::new(move |on| set_enabled(toggle_id.clone(), on)),
                    })
                    .on_press(move |_| open.set(Some(id.clone())))
                    .into_element()
            })
            .collect();
        let failed: Vec<Element> = catalog
            .failed()
            .iter()
            .map(|f| {
                ListRow::new(f.name.clone())
                    .subtitle(format!("No se pudo cargar · {}", f.reason))
                    .icon(lucide::triangle_alert())
                    .danger(true)
                    .into_element()
            })
            .collect();
        drop(catalog);
        rect()
            .width(Size::fill())
            .spacing(20.0)
            .child(
                Group::default().child(
                    ListRow::new("Instalar plugin…")
                        .subtitle("Desde un archivo .ntp")
                        .icon(lucide::package_plus())
                        .chevron()
                        .on_press(|_| install()),
                ),
            )
            .child(Section::new("Instalados").child(Group::default().children(rows)))
            .maybe_child(
                (!failed.is_empty()).then(|| {
                    Section::new("Con problemas").child(Group::default().children(failed))
                }),
            )
            .into_element()
    }
}

#[derive(Clone, Debug)]
struct Detail {
    plugin: Arc<Installed>,
}

impl PartialEq for Detail {
    fn eq(&self, other: &Self) -> bool {
        self.plugin.info == other.plugin.info && self.plugin.state == other.plugin.state
    }
}

impl Component for Detail {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let mut technical = use_state(|| false);
        let plugin = &self.plugin;
        let id = plugin.info.id.to_string();
        let adds: Vec<Element> = plugin
            .kinds
            .iter()
            .map(|k| k.info())
            .map(|info| {
                let entries = plugin
                    .kinds
                    .iter()
                    .find(|k| k.info().id == info.id)
                    .map(|k| k.creation_entries())
                    .unwrap_or_default();
                let creates = entries
                    .iter()
                    .map(|e| e.label.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                ListRow::new(info.label.clone())
                    .subtitle(if creates.is_empty() {
                        info.search_group.to_string()
                    } else {
                        format!("Crear: {creates}")
                    })
                    .kind(info.icon.clone(), kind_color(&p, info.color))
                    .into_element()
            })
            .collect();
        let file = match &plugin.origin {
            Origin::Builtin => "Incluido en Notein".to_owned(),
            Origin::ThirdParty { library, .. } => library.display().to_string(),
        };
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .content(Content::Flex)
                    .spacing(12.0)
                    .child(
                        rect()
                            .width(Size::flex(1.0))
                            .spacing(4.0)
                            .child(Text::new(plugin.info.name.clone()).role(TextRole::Heading))
                            .child(
                                Text::new(format!(
                                    "{} · {}",
                                    status(plugin),
                                    plugin.info.description
                                ))
                                .role(TextRole::Meta),
                            ),
                    )
                    .child(
                        Switch::new(plugin.is_active())
                            .on_toggle(move |on| set_enabled(id.clone(), on)),
                    ),
            )
            .child(Section::new("Qué agrega").child(Group::default().children(adds)))
            .child(
                Section::new("Ajustes").child(match &plugin.settings {
                    Some(settings) => SchemaForm {
                        settings: Arc::clone(settings),
                    }
                    .into_element(),
                    None => Text::new("Este plugin no tiene ajustes.")
                        .role(TextRole::Hint)
                        .into_element(),
                }),
            )
            .child(
                Group::default()
                    .child(
                        ListRow::new("Detalles técnicos")
                            .icon(lucide::code())
                            .trailing(Trailing::Element(
                                Icon::new(if technical() {
                                    lucide::chevron_up()
                                } else {
                                    lucide::chevron_down()
                                })
                                .size(16.0)
                                .color(p.t3)
                                .into_element(),
                            ))
                            .on_press(move |_| {
                                let next = !technical();
                                technical.set(next);
                            }),
                    )
                    .maybe_child(technical().then(|| {
                        rect()
                            .padding((4.0, 16.0, 14.0, 16.0))
                            .spacing(4.0)
                            .child(
                                Text::new(format!("Id: {}", plugin.info.id)).role(TextRole::Meta),
                            )
                            .child(
                                Text::new(format!("Versión: {}", plugin.info.version))
                                    .role(TextRole::Meta),
                            )
                            .child(
                                Text::new(format!("Autor: {}", plugin.info.author))
                                    .role(TextRole::Meta),
                            )
                            .child(Text::new(format!("Archivo: {file}")).role(TextRole::Meta))
                            .child(
                                Text::new(format!(
                                    "SDK: {}",
                                    notein_plugin::loader::SDK_ABI_VERSION
                                ))
                                .role(TextRole::Meta),
                            )
                    })),
            )
    }
}

/// Pick a `.ntp` package and review it.
fn install() {
    spawn(async move {
        let picked = match crate::files::pick(
            "Instalar plugin",
            &["ntp"],
            &["application/zip", "application/octet-stream"],
        )
        .await
        {
            Ok(Some(picked)) => picked,
            Ok(None) => return,
            Err(err) => {
                log::warn!("pick plugin: {err}");
                return;
            }
        };
        let staged = match Staged::unpack(&picked.bytes) {
            Ok(staged) => staged,
            Err(err) => {
                incompatible(picked.name, err.to_string());
                return;
            }
        };
        match dynamic::inspect(staged.folder()) {
            Ok(loaded) => {
                let review = loaded.review();
                drop(loaded);
                review_sheet(Arc::new(staged), review);
            }
            Err(err) => incompatible(picked.name, err.user_message()),
        }
    });
}

fn incompatible(name: String, why: String) {
    Sheet::titled(format!("No se puede instalar «{name}»"))
        .child(Notice::new(NoticeTone::Danger, why))
        .footer(SheetFooter::new([
            Button::new("Cerrar")
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new("Elegir otro")
                .primary()
                .on_press(|_| {
                    ModalController::get().close();
                    install();
                })
                .into_element(),
        ]))
        .push();
}

fn review_sheet(staged: Arc<Staged>, review: Review) {
    use freya::icons::lucide;
    let p = palette();
    let rows: Vec<Element> = review
        .kinds
        .iter()
        .map(|k| {
            ListRow::new(k.label.clone())
                .subtitle("Un tipo de documento nuevo")
                .kind(k.icon.clone(), kind_color(&p, k.color))
                .into_element()
        })
        .chain(review.entries.iter().map(|e| {
            ListRow::new(e.label.clone())
                .subtitle("En el menú Crear")
                .kind(e.icon.clone(), kind_color(&p, e.color))
                .into_element()
        }))
        .collect();
    let name = review.info.name.to_string();
    Sheet::titled(format!("Instalar «{name}»"))
        .subtitle(format!("Versión {} · {}", review.info.version, review.info.author))
        .child(
            rect()
                .width(Size::fill())
                .spacing(14.0)
                .child(Text::new(review.info.description.clone()))
                .child(Section::new("Qué agrega").child(Group::default().children(rows)))
                .maybe_child((review.settings > 0).then(|| {
                    Text::new(format!("Trae {} que podés cambiar en Ajustes › Plugins.", crate::format::count(review.settings, "ajuste", "ajustes")))
                        .role(TextRole::Hint)
                }))
                .child(Notice::new(NoticeTone::Warning, "Los plugins de terceros corren con los mismos permisos que Notein. Instalá solo los que conozcas.")),
        )
        .footer(SheetFooter::new([
            Button::new("Cancelar").ghost().on_press(|_| ModalController::get().close()).into_element(),
            Button::new("Instalar")
                .primary()
                .icon(lucide::download())
                .on_press(move |_| {
                    ModalController::get().close();
                    match dynamic::install(staged.folder()) {
                        Ok(installed) => {
                            crate::plugins::content::index_library();
                            Toasts::get().show(Toast::new(format!("«{installed}» instalado")));
                        }
                        Err(err) => incompatible(name.clone(), err.to_string()),
                    }
                })
                .into_element(),
        ]))
        .push();
}
