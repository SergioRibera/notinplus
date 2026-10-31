//! Settings › About: version, licenses and where the data lives.

use std::borrow::Cow;

use freya::prelude::*;
use notein_ui_kit::{
    Button, Group, ListRow, Section, Sheet, Text, TextRole, Toast, Toasts, Trailing,
};

use notein_i18n::tr;

/// Shown as is in every language.
const APP_NAME: &str = "NotIn Plus";

/// Third-party work the app ships with, and its license. Work and
/// license names stay as published; only descriptions are translated.
#[sismo::function("settings::about_third_party")]
fn third_party() -> [(Cow<'static, str>, Cow<'static, str>); 20] {
    let fixed = |name: &'static str, license: &'static str| (name.into(), license.into());
    [
        fixed("Freya", "MIT"),
        fixed("Skia", "BSD-3-Clause"),
        fixed("PDFium", "Apache-2.0 / BSD-3-Clause"),
        fixed("Loro", "MIT"),
        fixed("istmo", "MIT / Apache-2.0"),
        (tr!("settings-about-license-lucide").into(), "ISC".into()),
        fixed("Excalifont", "OFL-1.1"),
        fixed("Caveat, Gochi Hand, Handlee", "OFL-1.1"),
        (
            "Reina-Valera 1909".into(),
            tr!("settings-about-license-public-domain").into(),
        ),
        fixed("zstd", "BSD-3-Clause"),
        fixed("qrcode", "MIT / Apache-2.0"),
        fixed("bincode", "MIT"),
        fixed("Typst", "Apache-2.0"),
        fixed("MiTeX", "Apache-2.0"),
        fixed("mermaid-svg", "MIT"),
        fixed("resvg", "Apache-2.0 / MIT"),
        (tr!("settings-about-license-lora").into(), "OFL-1.1".into()),
        fixed("New Computer Modern Math", "GUST Font License"),
        fixed("Libertinus Serif", "OFL-1.1"),
        (
            tr!("settings-about-license-handwriting").into(),
            "CC BY 4.0".into(),
        ),
    ]
}

#[sismo::function("settings::about_licenses")]
fn licenses() {
    Sheet::titled(tr!("settings-about-licenses"))
        .child(
            Group::default().children(third_party().map(|(name, license)| {
                ListRow::new(name)
                    .trailing(Trailing::Text(license))
                    .dense(true)
                    .into_element()
            })),
        )
        .child(
            Section::new(tr!("settings-about-handwriting"))
                .child(Text::new(tr!("settings-about-handwriting-credit")).role(TextRole::Hint)),
        )
        .push();
}

#[sismo::function("settings::about_data_folder")]
fn data_folder() -> String {
    crate::library::bodies::root_dir().display().to_string()
}

/// The About section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct About;

impl Component for About {
    #[sismo::function("settings::about")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let folder = data_folder();
        let desktop = cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        ));
        let shown = folder.clone();
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(
                rect()
                    .spacing(4.0)
                    .child(Text::new(APP_NAME).role(TextRole::Heading))
                    .child(
                        Text::new(
                            tr!("settings-about-version", "version" => env!("CARGO_PKG_VERSION")),
                        )
                        .role(TextRole::Meta),
                    ),
            )
            .child(
                Group::default()
                    .child(
                        ListRow::new(tr!("settings-about-licenses"))
                            .icon(lucide::scale())
                            .chevron()
                            .on_press(|_| licenses()),
                    )
                    .child(
                        ListRow::new(tr!("settings-about-data-folder"))
                            .subtitle(shown)
                            .icon(lucide::folder_open())
                            .trailing(Trailing::Element(
                                Button::new(if desktop {
                                    tr!("settings-about-data-folder-show")
                                } else {
                                    tr!("settings-about-data-folder-copy")
                                })
                                .ghost()
                                .small()
                                .on_press(move |_| {
                                    if desktop {
                                        crate::external::open_url(&format!("file://{folder}"));
                                    } else if freya::clipboard::Clipboard::set(folder.clone())
                                        .is_ok()
                                    {
                                        Toasts::get().show(Toast::new(tr!(
                                            "settings-about-data-folder-copied"
                                        )));
                                    }
                                })
                                .into_element(),
                            )),
                    ),
            )
            .child(crate::update::ui::UpdatesSection)
            .child(crate::diagnostics::ui::DiagnosticsSection)
            .child(
                Section::new(tr!("settings-about-your-data"))
                    .child(Text::new(tr!("settings-about-your-data-hint")).role(TextRole::Hint)),
            )
            .into_element()
    }
}
