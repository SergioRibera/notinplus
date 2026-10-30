//! Settings › Plugins: what is installed, turning plugins on and off,
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

use notein_i18n::tr;

use super::schema::SchemaForm;
use crate::plugins::dynamic::{self, Staged};
use crate::references::kind_color;
use crate::settings::SettingsStore;

fn status(plugin: &Installed) -> String {
    match (&plugin.origin, plugin.is_active()) {
        (_, false) => tr!("settings-plugins-status-off"),
        (Origin::Builtin, true) => tr!("settings-plugins-status-builtin"),
        (Origin::ThirdParty { .. }, true) => tr!("settings-plugins-status-third-party"),
    }
}

fn set_enabled(id: &str, on: bool) {
    crate::plugins::catalog::catalog_mut().set_enabled(id, on);
    let mut store = SettingsStore::get();
    store.update(|s| {
        s.plugins.disabled.retain(|d| *d != id);
        if !on {
            s.plugins.disabled.push(id.to_owned());
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
                    Button::new(tr!("settings-plugins-all"))
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
                    .subtitle(tr!("settings-plugins-summary", "status" => status(plugin), "description" => plugin.info.description.to_string()))
                    .kind(
                        icon.as_ref()
                            .map_or_else(lucide::puzzle, |k| k.icon.clone()),
                        icon.map_or(p.t3, |k| kind_color(&p, k.color)),
                    )
                    .trailing(Trailing::Switch {
                        on: plugin.is_active(),
                        on_toggle: EventHandler::new(move |on| set_enabled(&toggle_id, on)),
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
                    .subtitle(tr!("settings-plugins-failed", "reason" => f.reason.clone()))
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
                    ListRow::new(tr!("settings-plugins-install"))
                        .subtitle(tr!("settings-plugins-install-hint"))
                        .icon(lucide::package_plus())
                        .chevron()
                        .on_press(|_| install()),
                ),
            )
            .child(
                Section::new(tr!("settings-plugins-installed"))
                    .child(Group::default().children(rows)),
            )
            .maybe_child((!failed.is_empty()).then(|| {
                Section::new(tr!("settings-plugins-with-problems"))
                    .child(Group::default().children(failed))
            }))
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

impl Detail {
    /// Name, state, description and the on/off switch.
    fn header(&self) -> Rect {
        let plugin = &self.plugin;
        let id = plugin.info.id.to_string();
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
                        Text::new(tr!("settings-plugins-summary", "status" => status(plugin), "description" => plugin.info.description.to_string()))
                            .role(TextRole::Meta),
                    ),
            )
            .child(Switch::new(plugin.is_active()).on_toggle(move |on| set_enabled(&id, on)))
    }

    /// The kinds the plugin adds and what each creates.
    fn adds(&self) -> Vec<Element> {
        let p = palette();
        self.plugin
            .kinds
            .iter()
            .map(|kind| {
                let info = kind.info();
                let creates = kind
                    .creation_entries()
                    .iter()
                    .map(|e| e.label.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                ListRow::new(info.label.clone())
                    .subtitle(if creates.is_empty() {
                        info.search_group.to_string()
                    } else {
                        tr!("settings-plugins-creates", "entries" => creates)
                    })
                    .kind(info.icon.clone(), kind_color(&p, info.color))
                    .into_element()
            })
            .collect()
    }

    /// Id, version, author, file and SDK, folded.
    fn technical(&self, mut open: State<bool>) -> Group {
        use freya::icons::lucide;
        let plugin = &self.plugin;
        let file = match &plugin.origin {
            Origin::Builtin => tr!("settings-plugins-file-builtin"),
            Origin::ThirdParty { library, .. } => library.display().to_string(),
        };
        let details = [
            tr!("settings-plugins-tech-id", "id" => plugin.info.id.to_string()),
            tr!("settings-plugins-tech-version", "version" => plugin.info.version.to_string()),
            tr!("settings-plugins-tech-author", "author" => plugin.info.author.to_string()),
            tr!("settings-plugins-tech-file", "file" => file),
            tr!("settings-plugins-tech-sdk", "sdk" => notein_plugin::loader::SDK_ABI_VERSION.to_string()),
        ];
        Group::default()
            .child(
                ListRow::new(tr!("settings-plugins-tech"))
                    .icon(lucide::code())
                    .trailing(Trailing::Element(
                        Icon::new(if open() {
                            lucide::chevron_up()
                        } else {
                            lucide::chevron_down()
                        })
                        .size(16.0)
                        .color(palette().t3)
                        .into_element(),
                    ))
                    .on_press(move |_| {
                        let next = !open();
                        open.set(next);
                    }),
            )
            .maybe_child(open().then(|| {
                rect()
                    .padding((4.0, 16.0, 14.0, 16.0))
                    .spacing(4.0)
                    .children(details.map(|d| Text::new(d).role(TextRole::Meta).into_element()))
            }))
    }
}

impl Component for Detail {
    fn render(&self) -> impl IntoElement {
        let technical = use_state(|| false);
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .child(self.header())
            .child(
                Section::new(tr!("settings-plugins-adds"))
                    .child(Group::default().children(self.adds())),
            )
            .child(Section::new(tr!("common-settings")).child(
                self.plugin.settings.as_ref().map_or_else(
                    || {
                        Text::new(tr!("settings-plugins-no-settings"))
                            .role(TextRole::Hint)
                            .into_element()
                    },
                    |settings| {
                        SchemaForm {
                            settings: Arc::clone(settings),
                        }
                        .into_element()
                    },
                ),
            ))
            .child(self.technical(technical))
    }
}

/// Pick a `.ntp` package and review it.
fn install() {
    spawn(async move {
        let picked = match crate::files::pick(
            &tr!("settings-plugins-install-pick"),
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
                incompatible(&picked.name, err.to_string());
                return;
            }
        };
        match dynamic::inspect(staged.folder()) {
            Ok(loaded) => {
                let review = loaded.review();
                drop(loaded);
                review_sheet(Arc::new(staged), &review);
            }
            Err(err) => incompatible(&picked.name, err.user_message()),
        }
    });
}

fn incompatible(name: &str, why: String) {
    Sheet::titled(tr!("settings-plugins-incompatible", "name" => name))
        .child(Notice::new(NoticeTone::Danger, why))
        .footer(SheetFooter::new([
            Button::new(tr!("common-close"))
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new(tr!("settings-plugins-choose-another"))
                .primary()
                .on_press(|_| {
                    ModalController::get().close();
                    install();
                })
                .into_element(),
        ]))
        .push();
}

fn review_sheet(staged: Arc<Staged>, review: &Review) {
    use freya::icons::lucide;
    let p = palette();
    let rows: Vec<Element> = review
        .kinds
        .iter()
        .map(|k| {
            ListRow::new(k.label.clone())
                .subtitle(tr!("settings-plugins-review-kind"))
                .kind(k.icon.clone(), kind_color(&p, k.color))
                .into_element()
        })
        .chain(review.entries.iter().map(|e| {
            ListRow::new(e.label.clone())
                .subtitle(tr!("settings-plugins-review-entry"))
                .kind(e.icon.clone(), kind_color(&p, e.color))
                .into_element()
        }))
        .collect();
    let name = review.info.name.to_string();
    Sheet::titled(tr!("settings-plugins-review-title", "name" => name.clone()))
        .subtitle(tr!("settings-plugins-review-subtitle", "version" => review.info.version.to_string(), "author" => review.info.author.to_string()))
        .child(
            rect()
                .width(Size::fill())
                .spacing(14.0)
                .child(Text::new(review.info.description.clone()))
                .child(Section::new(tr!("settings-plugins-adds")).child(Group::default().children(rows)))
                .maybe_child((review.settings > 0).then(|| {
                    Text::new(tr!("settings-plugins-review-settings", "count" => review.settings))
                        .role(TextRole::Hint)
                }))
                .child(Notice::new(NoticeTone::Warning, tr!("settings-plugins-review-warning"))),
        )
        .footer(SheetFooter::new([
            Button::new(tr!("common-cancel")).ghost().on_press(|_| ModalController::get().close()).into_element(),
            Button::new(tr!("settings-plugins-review-install"))
                .primary()
                .icon(lucide::download())
                .on_press(move |_| {
                    ModalController::get().close();
                    match dynamic::install(staged.folder()) {
                        Ok(installed) => {
                            crate::plugins::content::index_library();
                            Toasts::get().show(Toast::new(tr!("settings-plugins-installed-toast", "name" => installed)));
                        }
                        Err(err) => incompatible(&name, err.to_string()),
                    }
                })
                .into_element(),
        ]))
        .push();
}
