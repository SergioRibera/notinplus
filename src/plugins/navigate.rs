//! Opening references: chips, search results, outline rows and marks
//! all end here.

use notein_plugin::bookmark::SourceRef;
use notein_plugin::payload::Payload;
use notein_plugin::services::Placement;
use notein_ui_kit::{Toast, ToastTone, Toasts};

use super::builtin::canvas::{self, CanvasAnchor};
use super::content::doc_id;
use crate::library::{ItemId, TabTarget};
use crate::space::OpenHint;

fn hint(placement: Placement) -> OpenHint {
    match placement {
        Placement::Here => OpenHint::Focused,
        Placement::Beside => OpenHint::Beside,
        Placement::Preferred => {
            let beside = crate::settings::SettingsStore::try_get()
                .is_none_or(|s| s.read().notes.open_refs_beside);
            if beside {
                OpenHint::Beside
            } else {
                OpenHint::Focused
            }
        }
    }
}

fn missing(text: &str) {
    Toasts::get().show(Toast::new(text.to_owned()).tone(ToastTone::Error));
}

/// Open `target` with `placement`.
pub fn open(target: &SourceRef, placement: Placement) {
    match target {
        SourceRef::External { url } => crate::source_ref_nav::open_external_url(url),
        SourceRef::Canvas { bookmark } => {
            let Some(doc) = super::builtin::canvas_kind().doc_of_bookmark(bookmark.0) else {
                missing("Ese marcador ya no existe");
                return;
            };
            let anchor = CanvasAnchor {
                bookmark: Some(bookmark.0),
                layer: None,
            };
            open_at(
                canvas::KIND.to_owned(),
                ItemId(doc.0),
                anchor.to_bytes(),
                hint(placement),
            );
        }
        SourceRef::Plugin {
            plugin_id,
            item,
            anchor,
            ..
        } => {
            open_plugin_ref(
                plugin_id.clone(),
                ItemId(*item),
                anchor.clone(),
                hint(placement),
            );
        }
    }
}

fn open_plugin_ref(kind_id: String, item: ItemId, anchor: Vec<u8>, hint: OpenHint) {
    let Some(kind) = super::kind(&kind_id) else {
        missing("Esa referencia necesita un plugin que no está instalado");
        return;
    };
    let singleton = kind.info().singleton;
    freya::prelude::spawn_forever(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let target = {
            let lib = handle.lock().await;
            let live = |id: ItemId| lib.item(id).is_some_and(|i| i.trashed.is_none());
            if live(item) {
                Some(item)
            } else if singleton {
                lib.live_items().find(|i| i.kind.is(&kind_id)).map(|i| i.id)
            } else {
                None
            }
        };
        match target {
            Some(doc) => open_at(kind_id, doc, anchor, hint),
            None => missing("La referencia apunta a un documento que ya no existe"),
        }
    });
}

fn open_at(kind: String, doc: ItemId, anchor: Vec<u8>, hint: OpenHint) {
    super::host::shell().queue_jump(&kind, doc_id(doc), anchor);
    crate::space::open(TabTarget::Item(doc), hint);
}
