//! What opens instead of the app when the library on this device is in
//! a shape this build does not read: nothing is touched until the
//! person chooses to keep a copy, to start over, or to leave.

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_ui_kit::theme::palette;
use notein_ui_kit::tokens::{size, space};
use notein_ui_kit::{Button, ButtonSize, Icon, Notice, NoticeTone, Text, TextRole};

use crate::library::formats::{self, Incompatible};

/// Where the choice stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Choose,
    /// Asked to delete without a copy: one more press does it.
    Confirm,
    Saving,
    Wiping,
}

/// Shown in place of the app while the stored library is unreadable.
#[derive(PartialEq)]
pub struct IncompatibleScreen {
    pub found: Incompatible,
    /// The old library is gone: open the app on an empty one.
    pub done: EventHandler<()>,
}

impl std::fmt::Debug for IncompatibleScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncompatibleScreen")
            .field("found", &self.found)
            .finish_non_exhaustive()
    }
}

impl Component for IncompatibleScreen {
    #[sismo::function("screens::incompatible")]
    fn render(&self) -> impl IntoElement {
        let insets = *crate::hooks::provide_safe_area_root().read();
        let mut step = use_state(|| Step::Choose);
        let mut problem = use_state(|| Option::<String>::None);
        let p = palette();
        let version = self.found.format.written_by;
        let done = self.done.clone();

        let save_done = done.clone();
        let save_copy = move |_| {
            let done = save_done.clone();
            step.set(Step::Saving);
            problem.set(None);
            spawn(traced!("task::incompatible::save_copy", async move {
                match save_then_wipe(version).await {
                    Ok(true) => done.call(()),
                    Ok(false) => step.set(Step::Choose),
                    Err(why) => {
                        problem.set(Some(why));
                        step.set(Step::Choose);
                    }
                }
            }));
        };
        let wipe = move |_| {
            let done = done.clone();
            step.set(Step::Wiping);
            problem.set(None);
            spawn(traced!("task::incompatible::wipe", async move {
                match formats::wipe().await {
                    Ok(()) => done.call(()),
                    Err(err) => {
                        problem.set(Some(err.user_message()));
                        step.set(Step::Confirm);
                    }
                }
            }));
        };
        let quit = |_| Platform::get().exit();

        let current = *step.read();
        let busy = matches!(current, Step::Saving | Step::Wiping);
        let actions = if matches!(current, Step::Confirm | Step::Wiping) {
            rect()
                .width(Size::fill())
                .spacing(space::SM)
                .child(
                    Text::new(tr!("incompatible-confirm"))
                        .role(TextRole::Body)
                        .color(p.t2)
                        .fill(),
                )
                .child(
                    Button::new(tr!("incompatible-confirm-wipe"))
                        .variant(notein_ui_kit::ButtonVariant::DangerFill)
                        .size(ButtonSize::Large)
                        .full_width()
                        .loading(current == Step::Wiping)
                        .on_press(wipe),
                )
                .child(
                    Button::new(tr!("incompatible-keep"))
                        .ghost()
                        .size(ButtonSize::Large)
                        .full_width()
                        .enabled(!busy)
                        .on_press(move |_| step.set(Step::Choose)),
                )
        } else {
            rect()
                .width(Size::fill())
                .spacing(space::SM)
                .child(
                    Button::new(tr!("incompatible-save-copy"))
                        .primary()
                        .size(ButtonSize::Large)
                        .full_width()
                        .loading(current == Step::Saving)
                        .on_press(save_copy),
                )
                .child(
                    Button::new(tr!("incompatible-wipe"))
                        .danger()
                        .size(ButtonSize::Large)
                        .full_width()
                        .enabled(!busy)
                        .on_press(move |_| step.set(Step::Confirm)),
                )
                .child(
                    Button::new(tr!("incompatible-quit"))
                        .ghost()
                        .size(ButtonSize::Large)
                        .full_width()
                        .enabled(!busy)
                        .on_press(quit),
                )
        };

        rect()
            .expanded()
            .background(p.bg)
            .padding((
                insets.top + space::XL,
                insets.right + space::GUTTER_COMPACT,
                insets.bottom + space::XL,
                insets.left + space::GUTTER_COMPACT,
            ))
            .center()
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::auto())
                    .child(
                        rect()
                            .width(Size::fill())
                            .cross_align(Alignment::Center)
                            .child(
                                rect()
                                    .width(Size::fill())
                                    .max_width(Size::px(size::SHEET_WIDTH_SM))
                                    .spacing(space::LG)
                                    .child(
                                        Icon::new(freya::icons::lucide::archive())
                                            .size(32.0)
                                            .color(p.t2),
                                    )
                                    .child(
                                        Text::new(tr!("incompatible-title"))
                                            .role(TextRole::Heading)
                                            .fill(),
                                    )
                                    .child(
                                        Text::new(tr!("incompatible-body", "version" => version))
                                            .role(TextRole::Body)
                                            .color(p.t2)
                                            .fill(),
                                    )
                                    .child(Notice::new(
                                        NoticeTone::Warning,
                                        tr!("incompatible-devices"),
                                    ))
                                    .maybe_child(
                                        problem
                                            .read()
                                            .clone()
                                            .map(|why| Notice::new(NoticeTone::Danger, why)),
                                    )
                                    .child(actions)
                                    .child(
                                        Text::new(tr!("incompatible-quit-hint"))
                                            .role(TextRole::Hint)
                                            .align(TextAlign::Center)
                                            .fill(),
                                    ),
                            ),
                    ),
            )
    }
}

/// Ask where to keep a copy of the old library, write it there and
/// delete it here. `Ok(false)` when the person cancels the picker; the
/// library stays then. Errors are what to tell them.
async fn save_then_wipe(version: &str) -> Result<bool, String> {
    let bytes = formats::backup().await.map_err(|err| err.user_message())?;
    let suggested = format!("notinplus-{version}.tar.zst");
    let saved = crate::files::save(
        &tr!("incompatible-save-title"),
        &suggested,
        "application/zstd",
        "zst",
        &bytes,
    )
    .await
    .map_err(|err| err.user_message())?;
    if saved.is_none() {
        return Ok(false);
    }
    formats::wipe().await.map_err(|err| err.user_message())?;
    Ok(true)
}
