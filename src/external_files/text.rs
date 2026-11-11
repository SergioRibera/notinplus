//! Markdown and plain text files in the app's editor, their edits
//! written back to the file a second after the typing stops.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use notin_ui_kit::theme::palette;

use super::view::{Mode, SaveState, ViewState};
use super::{FileKind, Session};

/// How long the typing has to stop before the file is written.
const DEBOUNCE: Duration = Duration::from_secs(1);

/// Width of the text column.
const COLUMN_WIDTH: f32 = 760.0;

/// The body of a Markdown or text file.
#[derive(Clone, PartialEq)]
pub(super) struct TextBody {
    pub session: Session,
    pub state: ViewState,
    /// Whether the file can be changed at all.
    pub editable: bool,
}

/// How much formatting `markdown` carries: blocks other than paragraphs
/// and marks inside paragraphs. A `.txt` that gains some keeps it only as
/// symbols.
fn formatting(markdown: &str) -> usize {
    Document::parse(markdown)
        .blocks()
        .iter()
        .map(|block| match &block.kind {
            BlockKind::Paragraph(content) => content.marks().len(),
            _ => 1,
        })
        .sum()
}

/// Write `text` to `path` off the UI thread.
async fn write(path: std::path::PathBuf, text: String) -> std::io::Result<()> {
    let (tx, rx) = flume::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send(std::fs::write(path, text));
    });
    rx.recv_async()
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("writer thread stopped")))
}

impl TextBody {
    /// Write what waits in the autosave once the typing stops, for as long
    /// as the body is on screen.
    fn spawn_writer(&self) {
        let autosave = Arc::clone(&self.session.autosave);
        let mut save = self.state.save;
        let mut formatted = self.state.formatted;
        let plain = self.session.file.kind == FileKind::Text;
        let baseline = formatting(&self.state.text.peek());
        spawn(traced!("task::external_files::autosave", async move {
            let wake = autosave.wake.1.clone();
            while wake.recv_async().await.is_ok() {
                loop {
                    async_io::Timer::after(DEBOUNCE).await;
                    if wake.try_recv().is_err() {
                        break;
                    }
                    while wake.try_recv().is_ok() {}
                }
                let Some(text) = autosave.take() else {
                    continue;
                };
                if plain && !*formatted.peek() && formatting(&text) > baseline {
                    formatted.set(true);
                }
                save.set(SaveState::Saving);
                let result = write(autosave.path.clone(), text).await;
                save.set(match result {
                    Ok(()) => SaveState::Saved,
                    Err(err) => {
                        log::error!("save {}: {err}", autosave.path.display());
                        SaveState::Failed
                    }
                });
            }
        }));
    }
}

impl Component for TextBody {
    #[sismo::function("external_files::text_body")]
    fn render(&self) -> impl IntoElement {
        let controller = use_editor_controller();
        let state = self.state;
        let editable = self.editable;
        use_hook(|| self.spawn_writer());
        use_drop({
            let autosave = Arc::clone(&self.session.autosave);
            move || autosave.flush()
        });
        // The text last written or seen before editing started: what the
        // editor shows on opening (even reformatted) is never written back
        // until the person changes something.
        let seen = use_hook(|| Rc::new(RefCell::new(state.text.peek().clone())));
        let autosave = Arc::clone(&self.session.autosave);
        use_side_effect(move || {
            let editing = editable && *state.mode.read() == Mode::Edit;
            controller.set_read_only(!editing);
            let current = state.text.read().clone();
            let mut seen = seen.borrow_mut();
            if !editing || *seen == current {
                *seen = current;
                return;
            }
            seen.clone_from(&current);
            autosave.schedule(current);
        });
        let editing = editable && *state.mode.read() == Mode::Edit;
        let editor =
            crate::editor::configure(MarkdownEditor::new(state.text).controller(controller))
                .on_link(|url| {
                    let url = url.to_owned();
                    super::in_main_window(move || {
                        crate::plugins::navigate::open(
                            &crate::bookmark::SourceRef::from_link(&url),
                            notin_plugin::services::Placement::Preferred,
                        );
                    });
                })
                .width(Size::fill())
                .max_width(Size::px(COLUMN_WIDTH))
                .padding((24., 20., 120., 20.));
        rect()
            .expanded()
            .background(palette().bg)
            .content(Content::flex())
            .maybe_child(editing.then(|| {
                rect()
                    .width(Size::fill())
                    .padding((8., 16., 8., 16.))
                    .child(
                        MarkdownToolbar::new(controller)
                            .theme(crate::editor::theme(&palette()))
                            .mode_toggle(false)
                            .popup_area(notin_ui_kit::safe_frame()),
                    )
            }))
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(notin_ui_kit::ScrollContent::new(
                        rect()
                            .width(Size::fill())
                            .cross_align(Alignment::Center)
                            .child(editor),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::formatting;

    #[test]
    fn formatting_counts_what_a_txt_cannot_keep() {
        assert_eq!(formatting("una línea\notra línea"), 0);
        assert_eq!(formatting("# Título\n\ntexto"), 1);
        assert_eq!(formatting("hay **negrita** aquí"), 1);
    }
}
