//! Settings › About › Remote diagnostics.

use freya::icons::lucide;
use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_ui_kit::{Button, Group, ListRow, Section, Toast, ToastTone, Toasts, Trailing};

use super::{DiagnosticsSettings, MOBILE};
use crate::persist::Persisted;

fn remember(remote: bool) {
    if let Err(err) = (DiagnosticsSettings { remote }).save() {
        log::error!("diagnostics settings save: {err}");
    }
}

fn export() {
    let toast = match super::export() {
        Ok(path) => {
            let folder = super::export_dir().display().to_string();
            let file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Toast::new(tr!("settings-diagnostics-exported", "file" => file)).action(
                tr!("settings-diagnostics-show"),
                move |()| {
                    crate::external::open_url(&format!("file://{folder}"));
                },
            )
        }
        Err(err) => {
            Toast::new(tr!("errors-diagnostics-export", "reason" => err.kind().user_message()))
                .tone(ToastTone::Error)
        }
    };
    Toasts::get().show(toast);
}

/// The remote diagnostics switch, the pairing code while it runs and,
/// on desktop, the export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticsSection;

impl Component for DiagnosticsSection {
    fn render(&self) -> impl IntoElement {
        let mut code = use_state(super::pairing_code);
        let on = code.read().is_some();
        let toggle = move |enable: bool| {
            if !enable {
                super::stop();
                remember(false);
                code.set(None);
                return;
            }
            match super::start() {
                Ok(()) => {
                    remember(true);
                    code.set(super::pairing_code());
                }
                Err(err) => {
                    Toasts::get().show(
                        Toast::new(tr!("errors-diagnostics-start", "reason" => err.user_message()))
                            .tone(ToastTone::Error),
                    );
                }
            }
        };
        let current = code.read().clone();
        Section::new(tr!("settings-diagnostics")).child(
            Group::default()
                .child(
                    ListRow::new(tr!("settings-diagnostics-remote"))
                        .subtitle(tr!("settings-diagnostics-remote-hint"))
                        .icon(lucide::activity())
                        .trailing(Trailing::Switch {
                            on,
                            on_toggle: EventHandler::new(toggle),
                        }),
                )
                .maybe_child(current.map(|code| {
                    ListRow::new(tr!("settings-diagnostics-code"))
                        .subtitle(tr!("settings-diagnostics-code-hint"))
                        .icon(lucide::key_round())
                        .trailing(Trailing::Text(code.into()))
                }))
                .maybe_child((on && !MOBILE).then(|| {
                    ListRow::new(tr!("settings-diagnostics-export"))
                        .subtitle(tr!("settings-diagnostics-export-hint"))
                        .icon(lucide::file_down())
                        .trailing(Trailing::Element(
                            Button::new(tr!("settings-diagnostics-export-action"))
                                .ghost()
                                .small()
                                .on_press(|_| export())
                                .into_element(),
                        ))
                })),
        )
    }
}
