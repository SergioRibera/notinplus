//! Ajustes › Acerca de: version, licenses and where the data lives.

use freya::prelude::*;
use notein_ui_kit::{
    Button, Group, ListRow, Section, Sheet, Text, TextRole, Toast, Toasts, Trailing,
};

/// Third-party work Notein ships with, and its license.
const LICENSES: [(&str, &str); 20] = [
    ("Freya", "MIT"),
    ("Skia", "BSD-3-Clause"),
    ("PDFium", "Apache-2.0 / BSD-3-Clause"),
    ("Loro", "MIT"),
    ("istmo", "MIT / Apache-2.0"),
    ("Lucide (iconos)", "ISC"),
    ("Excalifont", "OFL-1.1"),
    ("Caveat, Gochi Hand, Handlee", "OFL-1.1"),
    ("Reina-Valera 1909", "Dominio público"),
    ("zstd", "BSD-3-Clause"),
    ("qrcode", "MIT / Apache-2.0"),
    ("bincode", "MIT"),
    ("Typst", "Apache-2.0"),
    ("MiTeX", "Apache-2.0"),
    ("mermaid-svg", "MIT"),
    ("resvg", "Apache-2.0 / MIT"),
    ("Lora (texto de la Biblia)", "OFL-1.1"),
    ("New Computer Modern Math", "GUST Font License"),
    ("Libertinus Serif", "OFL-1.1"),
    (
        "Datos de escritura a mano (UJI Pen Characters v2)",
        "CC BY 4.0",
    ),
];

/// Credit the handwriting templates owe under CC BY 4.0: work, authors,
/// source, license and what was changed.
const HANDWRITING_CREDIT: &str = "El reconocimiento de escritura a mano usa datos derivados de UJI Pen Characters (versión 2), de F. Prat, M. J. Castro, D. Llorens, A. Marzal y J. M. Vilar, UCI Machine Learning Repository, https://doi.org/10.24432/C5FG8S, con licencia CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). Cambios: las muestras se volvieron a muestrear y normalizar, los acentos se separaron de sus letras y cada carácter se redujo a unas pocas muestras representativas. Las formas de letras que se dibujan al convertir salen de las fuentes Caveat (Impallari Type), Excalifont (Excalidraw), Gochi Hand (Huerta Tipográfica) y Handlee (Joe Prince), con licencia SIL Open Font License 1.1.";

fn licenses() {
    Sheet::titled("Licencias")
        .child(Group::default().children(LICENSES.map(|(name, license)| {
            ListRow::new(name)
                .trailing(Trailing::Text(license.into()))
                .dense(true)
                .into_element()
        })))
        .child(
            Section::new("Escritura a mano")
                .child(Text::new(HANDWRITING_CREDIT).role(TextRole::Hint)),
        )
        .push();
}

fn data_folder() -> String {
    crate::library::bodies::root_dir().display().to_string()
}

/// The Acerca de section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct About;

impl Component for About {
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
                    .child(Text::new("Notein").role(TextRole::Heading))
                    .child(Text::new(format!("Versión {}", env!("CARGO_PKG_VERSION"))).role(TextRole::Meta)),
            )
            .child(
                Group::default()
                    .child(ListRow::new("Licencias").icon(lucide::scale()).chevron().on_press(|_| licenses()))
                    .child(
                        ListRow::new("Carpeta de datos")
                            .subtitle(shown)
                            .icon(lucide::folder_open())
                            .trailing(Trailing::Element(
                                Button::new(if desktop { "Mostrar" } else { "Copiar" })
                                    .ghost()
                                    .small()
                                    .on_press(move |_| {
                                        if desktop {
                                            crate::external::open_url(&format!("file://{folder}"));
                                        } else if freya::clipboard::Clipboard::set(folder.clone()).is_ok() {
                                            Toasts::get().show(Toast::new("Ruta copiada"));
                                        }
                                    })
                                    .into_element(),
                            )),
                    ),
            )
            .child(crate::update::ui::UpdatesSection)
            .child(Section::new("Tus datos").child(
                Text::new("Todo queda en este dispositivo y en los que vincules. Notein no manda tus documentos a ningún servidor que no elijas.")
                    .role(TextRole::Hint),
            ))
            .into_element()
    }
}
