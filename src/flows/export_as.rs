//! "Exportar como…": one document as a file with the options its kind
//! declares.
//!
//! The kind describes the options as settings fields
//! ([`DocumentKind::export_fields`]) and the sheet renders them with the
//! same form the plugin settings use, so a plugin gets the full choice of
//! options without drawing anything. The chosen values go back to the
//! kind ([`DocumentKind::export_with`]); one file is saved as is, several
//! are zipped. The bulk exports (bundle, readable zip) are untouched.
//!
//! [`DocumentKind::export_fields`]: notin_plugin::DocumentKind::export_fields
//! [`DocumentKind::export_with`]: notin_plugin::DocumentKind::export_with

use std::io::{Cursor, Write};
use std::sync::{Arc, Mutex};

use freya::prelude::*;
use notin_i18n::tr;
use notin_plugin::config::{ConfigError, ConfigField, ConfigSchema, ConfigValue};
use notin_plugin::kind::{ExportFile, ExportValues};
use notin_plugin::plugin::PluginSettings;
use notin_plugin::{ErasedKind, UserMessage};
use notin_ui_kit::theme::palette;
use notin_ui_kit::{
    Button, ModalController, Sheet, SheetFooter, TextField, Toast, ToastTone, Toasts,
};

use crate::library::{Item, ItemId, bodies};
use crate::plugins::content;
use crate::references::kind_color;

/// The values of the export sheet, held the way the settings form reads
/// and writes them.
struct SheetValues {
    fields: Vec<ConfigField>,
    values: Mutex<ExportValues>,
}

impl SheetValues {
    fn new(fields: Vec<ConfigField>) -> Self {
        Self {
            values: Mutex::new(ExportValues::defaults(&fields)),
            fields,
        }
    }

    fn snapshot(&self) -> ExportValues {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl PluginSettings for SheetValues {
    fn schema(&self) -> ConfigSchema {
        ConfigSchema {
            version: 0,
            fields: self.fields.clone(),
        }
    }

    fn get(&self, key: &str) -> Option<ConfigValue> {
        self.snapshot().get(key).cloned()
    }

    fn set(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
        if !self.fields.iter().any(|field| field.key == key) {
            return Err(ConfigError::UnknownKey(key.to_owned()));
        }
        {
            let mut values = self
                .values
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match values.0.iter_mut().find(|(k, _)| k == key) {
                Some((_, slot)) => *slot = value,
                None => values.0.push((key.to_owned(), value)),
            }
        }
        Ok(())
    }
}

/// Open the export sheet of `item`.
#[sismo::function("flows::export_as")]
pub fn export_as(item: ItemId) {
    spawn_in_window(traced!("task::flows::export_as", async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let Some(found) = handle.lock().await.item(item).cloned() else {
            return;
        };
        let Some(kind) = crate::plugins::kind(&found.kind.plugin_id) else {
            return;
        };
        if !kind.exports() {
            Toasts::get().show(Toast::new(tr!("export-unsupported")));
            return;
        }
        let content = content::read(item, &found.kind.plugin_id).unwrap_or_default();
        let attachment = kind
            .info()
            .attachment
            .and_then(|_| bodies::read_pdf(item).ok());
        let fields = kind.export_fields(content::meta(&found), &content, attachment.as_deref());
        open_sheet(found, kind, content, attachment, fields);
    }));
}

#[sismo::function("flows::export_sheet")]
fn open_sheet(
    item: Item,
    kind: Arc<dyn ErasedKind>,
    content: Vec<u8>,
    attachment: Option<Vec<u8>>,
    fields: Vec<ConfigField>,
) {
    let values = Arc::new(SheetValues::new(fields));
    let name = State::create_in_scope(item.name.clone(), ScopeId::ROOT);
    let info = kind.info();
    let save_values = Arc::clone(&values);
    let footer = SheetFooter::new([
        Button::new(tr!("common-cancel"))
            .ghost()
            .on_press(|_| ModalController::get().close())
            .into_element(),
        Button::new(tr!("export-save"))
            .primary()
            .on_press(move |_| {
                ModalController::get().close();
                run(
                    item.clone(),
                    Arc::clone(&kind),
                    content.clone(),
                    attachment.clone(),
                    save_values.snapshot(),
                    name.peek().trim().to_owned(),
                );
            })
            .into_element(),
    ]);
    Sheet::titled(tr!("export-title"))
        .kind(info.icon.clone(), kind_color(&palette(), info.color))
        .child(TextField::new(name).label(tr!("export-name")))
        .child(crate::screens::settings::schema::SchemaForm {
            settings: values as Arc<dyn PluginSettings>,
        })
        .footer(footer)
        .open();
}

/// The file the sheet produces: one as is, several zipped.
struct Produced {
    extension: String,
    mime: String,
    bytes: Vec<u8>,
}

/// Render off the UI thread, then ask where to save.
#[sismo::function("flows::export_run")]
fn run(
    item: Item,
    kind: Arc<dyn ErasedKind>,
    content: Vec<u8>,
    attachment: Option<Vec<u8>>,
    values: ExportValues,
    name: String,
) {
    let name = if name.is_empty() {
        tr!("common-untitled")
    } else {
        name
    };
    spawn_in_window(traced!("task::flows::export_run", async move {
        let (tx, rx) = flume::bounded(1);
        let worker_name = name.clone();
        std::thread::spawn(move || {
            sismo::scope!("flows::export_render");
            let meta = content::meta(&item);
            let files = kind.export_with(meta, &content, attachment.as_deref(), &values);
            let _ = tx.send(files.map_err(|err| err.user_message()).and_then(|files| {
                bundle(&worker_name, files).ok_or_else(|| tr!("export-nothing"))
            }));
        });
        let produced = match rx.recv_async().await {
            Ok(Ok(produced)) => produced,
            Ok(Err(reason)) => {
                let text = tr!("errors-headline-detail", "headline" => tr!("export-failed"), "detail" => reason);
                Toasts::get().show(Toast::new(text).tone(ToastTone::Error));
                return;
            }
            Err(_) => return,
        };
        let Produced {
            extension,
            mime,
            bytes,
        } = produced;
        let suggested = format!("{name}.{extension}");
        match crate::files::save(&tr!("export-title"), &suggested, &mime, &extension, &bytes).await
        {
            Ok(Some(saved)) => super::transfer::done_sheet(saved, bytes, mime),
            Ok(None) => {}
            Err(err) => {
                log::error!("export as: {err}");
                let text = tr!("errors-headline-detail", "headline" => tr!("export-failed"), "detail" => err.user_message());
                Toasts::get().show(Toast::new(text).tone(ToastTone::Error));
            }
        }
    }));
}

/// One file as is; several as a zip named after the document, each file
/// `<name>-<tag>.<ext>`.
#[sismo::function("flows::export_bundle")]
fn bundle(name: &str, mut files: Vec<ExportFile>) -> Option<Produced> {
    match files.len() {
        0 => None,
        1 => {
            let file = files.remove(0);
            Some(Produced {
                extension: file.extension.into_owned(),
                mime: file.mime.into_owned(),
                bytes: file.bytes,
            })
        }
        _ => {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (index, file) in files.iter().enumerate() {
                let tag = if file.tag.is_empty() {
                    (index + 1).to_string()
                } else {
                    file.tag.clone()
                };
                let path = format!("{name}-{tag}.{}", file.extension);
                zip.start_file(path, options).ok()?;
                zip.write_all(&file.bytes).ok()?;
            }
            Some(Produced {
                extension: "zip".into(),
                mime: "application/zip".into(),
                bytes: zip.finish().ok()?.into_inner(),
            })
        }
    }
}
