//! «Citar»: the shell's menu to cite a place from inside a document, the
//! same for every kind. Plugins open it through
//! [`notin_plugin::ui::HostUi::cite`].
//!
//! It picks how the place goes in (a live quote, a reference chip or a
//! fixed copy of its text) and copies that markdown; pasting it in a note
//! or a canvas box is what inserts it, so it works wherever the reader is.

use freya::prelude::*;
use notin_i18n::tr;
use notin_plugin::bookmark::SourceRef;
use notin_ui_kit::{Button, ListRow, ModalController, Popover, Toast, ToastTone, Toasts};

use crate::references;
use crate::state::LibraryView;

/// How a cited place goes into the document it is pasted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiteForm {
    /// A quote that keeps showing what the place holds.
    Live,
    /// A chip linking to the place.
    Reference,
    /// The place's text as it is now, to edit freely.
    Copy,
}

impl CiteForm {
    /// The forms `target` offers, the one to start with first.
    #[must_use]
    pub fn offered(target: &SourceRef) -> Vec<Self> {
        let kind = match target {
            SourceRef::Plugin { plugin_id, .. } => crate::plugins::kind(plugin_id),
            SourceRef::Canvas { .. } | SourceRef::External { .. } => None,
        };
        match kind {
            Some(kind) if kind.quotes() && kind.quotes_live() => {
                vec![Self::Live, Self::Reference, Self::Copy]
            }
            Some(kind) if kind.quotes() => vec![Self::Copy, Self::Reference],
            _ => vec![Self::Reference],
        }
    }

    /// The markdown that puts `target` in a document this way; `None` when
    /// the place has nothing to quote any more.
    #[must_use]
    #[sismo::function("cite::markdown")]
    pub fn markdown(self, target: &SourceRef) -> Option<String> {
        let data = LibraryView::get().data();
        match self {
            Self::Reference => {
                let shown = references::display(target, data.as_deref());
                Some(references::chip_markdown(&shown.label, target))
            }
            Self::Live => references::quote(target, data.as_deref())
                .map(|quote| references::live_quote_markdown(&quote, target)),
            Self::Copy => references::quote(target, data.as_deref())
                .map(|quote| references::quote_markdown(&quote, target)),
        }
    }

    fn label(self) -> String {
        match self {
            Self::Live => tr!("cite-live"),
            Self::Reference => tr!("cite-reference"),
            Self::Copy => tr!("cite-copy"),
        }
    }

    fn hint(self) -> String {
        match self {
            Self::Live => tr!("cite-live-hint"),
            Self::Reference => tr!("cite-reference-hint"),
            Self::Copy => tr!("cite-copy-hint"),
        }
    }

    fn icon(self) -> Bytes {
        use freya::icons::lucide;
        match self {
            Self::Live => lucide::quote(),
            Self::Reference => lucide::link(),
            Self::Copy => lucide::icon_type(),
        }
    }
}

/// Open the menu for `target`, pointing at `at` (window coordinates).
#[sismo::function("cite::open")]
pub fn open(target: SourceRef, at: Area) {
    let title = {
        let data = LibraryView::get().data();
        references::display(&target, data.as_deref()).label
    };
    Popover::new(at, CiteMenu { target })
        .width(320.0)
        .sheet_title(tr!("cite-title", "what" => title))
        .open();
}

/// Copy `target` cited as `form`, and say so. `false` when there was
/// nothing to copy.
#[sismo::function("cite::copy")]
pub fn copy(target: &SourceRef, form: CiteForm) -> bool {
    let Some(markdown) = form.markdown(target) else {
        Toasts::get().show(Toast::new(tr!("cite-gone")).tone(ToastTone::Error));
        return false;
    };
    if let SourceRef::Plugin {
        plugin_id,
        item,
        anchor,
        ..
    } = target
        && let Some(kind) = crate::plugins::kind(plugin_id)
    {
        kind.cited(notin_plugin::DocId(*item), anchor);
    }
    match freya::clipboard::Clipboard::set(markdown) {
        Ok(()) => {
            Toasts::get().show(Toast::new(tr!("cite-copied")).tone(ToastTone::Done));
            true
        }
        Err(err) => {
            log::warn!("cite: clipboard: {err:?}");
            Toasts::get().show(Toast::new(tr!("cite-copy-failed")).tone(ToastTone::Error));
            false
        }
    }
}

#[derive(Clone, PartialEq)]
struct CiteMenu {
    target: SourceRef,
}

impl Component for CiteMenu {
    #[sismo::function("cite::menu")]
    fn render(&self) -> impl IntoElement {
        let forms = CiteForm::offered(&self.target);
        let first = forms.first().copied().unwrap_or(CiteForm::Reference);
        let picked = use_state(|| first);
        let rows: Vec<Element> = forms
            .into_iter()
            .map(|form| {
                let mut picked = picked;
                ListRow::new(form.label())
                    .subtitle(form.hint())
                    .icon(form.icon())
                    .selected(*picked.read() == form)
                    .on_press(move |_| picked.set(form))
                    .into_element()
            })
            .collect();
        let target = self.target.clone();
        rect()
            .width(Size::fill())
            .padding(8.)
            .spacing(10.)
            .child(notin_ui_kit::Group::default().children(rows))
            .child(
                rect().width(Size::fill()).child(
                    Button::new(tr!("cite-copy-button"))
                        .primary()
                        .icon(freya::icons::lucide::copy())
                        .on_press(move |_| {
                            if copy(&target, *picked.peek()) {
                                ModalController::get().close();
                            }
                        }),
                ),
            )
            .child(
                label()
                    .width(Size::fill())
                    .font_size(notin_ui_kit::tokens::text::META)
                    .color(notin_ui_kit::palette().t3)
                    .text(tr!("cite-paste-hint")),
            )
    }
}
