//! Toasts: transient feedback at the bottom of the window, with an
//! optional action (Deshacer, Guardar, Ver) or a progress bar.
//!
//! Any code that runs inside the app can push one through
//! [`Toasts::get`]; [`ToastHost`] paints them and must sit near the
//! root, above the routes.

use std::borrow::Cow;
use std::time::Duration;

use freya::animation::{
    AnimNum, Ease, Function, OnChange, OnCreation, use_animation_with_dependencies,
};
use freya::prelude::*;

use super::feedback::ProgressBar;
use super::icon::Icon;
use crate::theme::palette;
use crate::tokens::{radius, text, toast};

/// Identifies a toast so it can be updated (progress) or dismissed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToastId(u64);

/// Semantic icon of a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastTone {
    #[default]
    Done,
    Info,
    Error,
    Trash,
    People,
    Offline,
    Download,
}

/// A toast description.
#[derive(Clone, PartialEq)]
pub struct Toast {
    text: Cow<'static, str>,
    tone: ToastTone,
    action: Option<(Cow<'static, str>, EventHandler<()>)>,
    on_expire: Option<EventHandler<()>>,
    duration: Option<Duration>,
    progress: Option<(f32, Cow<'static, str>, Option<Color>)>,
}

impl std::fmt::Debug for Toast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Toast")
            .field("text", &self.text)
            .finish_non_exhaustive()
    }
}

impl Toast {
    #[must_use]
    pub fn new(text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            text: text.into(),
            tone: ToastTone::Done,
            action: None,
            on_expire: None,
            duration: Some(Duration::from_millis(toast::INFO)),
            progress: None,
        }
    }

    #[must_use]
    pub const fn tone(mut self, tone: ToastTone) -> Self {
        self.tone = tone;
        self
    }

    /// Add an action button; extends the default duration to the
    /// undo window.
    #[must_use]
    pub fn action(
        mut self,
        label: impl Into<Cow<'static, str>>,
        on_press: impl Into<EventHandler<()>>,
    ) -> Self {
        self.action = Some((label.into(), on_press.into()));
        if self.duration == Some(Duration::from_millis(toast::INFO)) {
            self.duration = Some(Duration::from_millis(toast::UNDO));
        }
        self
    }

    /// Called when the toast leaves without its action being pressed.
    #[must_use]
    pub fn on_expire(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.on_expire = Some(handler.into());
        self
    }

    #[must_use]
    pub const fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Stay until dismissed or replaced (progress toasts).
    #[must_use]
    pub const fn sticky(mut self) -> Self {
        self.duration = None;
        self
    }

    /// Progress card: `value` 0..1 and the percentage or count label.
    #[must_use]
    pub fn progress(mut self, value: f32, caption: impl Into<Cow<'static, str>>) -> Self {
        self.progress = Some((value, caption.into(), None));
        self.duration = None;
        self
    }

    /// Colour of the progress fill (the colour of what is installing).
    #[must_use]
    pub const fn progress_color(mut self, color: Color) -> Self {
        if let Some((_, _, fill)) = &mut self.progress {
            *fill = Some(color);
        }
        self
    }
}

#[derive(Clone, PartialEq, Default)]
struct ToastStack {
    next: u64,
    items: Vec<(ToastId, Toast)>,
    /// Toasts that are gone but still playing their exit.
    leaving: Vec<ToastId>,
}

impl ToastStack {
    /// The toasts still showing, oldest first.
    fn showing(&self) -> impl Iterator<Item = ToastId> + '_ {
        self.items
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !self.leaving.contains(id))
    }
}

/// How long a toast takes to come in and to leave.
const ENTER_MS: u64 = 220;
const EXIT_MS: u64 = 150;

/// Root-scoped toast queue.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Toasts {
    stack: State<ToastStack>,
}

impl std::fmt::Debug for Toasts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Toasts").finish_non_exhaustive()
    }
}

/// At most this many toasts are visible; older ones expire early.
const MAX_VISIBLE: usize = 3;

impl Toasts {
    /// The app-wide queue (installed on first use at the root scope).
    #[must_use]
    pub fn get() -> Self {
        if let Some(toasts) = try_consume_root_context::<Self>() {
            return toasts;
        }
        let toasts = Self {
            stack: State::create_in_scope(ToastStack::default(), ScopeId::ROOT),
        };
        provide_context_for_scope_id(toasts, ScopeId::ROOT);
        toasts
    }

    /// Show `toast`; returns its id.
    pub fn show(&mut self, toast: Toast) -> ToastId {
        let duration = toast.duration;
        let id = {
            let mut stack = self.stack.write();
            let id = ToastId(stack.next);
            stack.next += 1;
            stack.items.push((id, toast));
            id
        };
        let showing: Vec<ToastId> = self.stack.peek().showing().collect();
        let overflow = showing.len().saturating_sub(MAX_VISIBLE);
        let expired: Vec<ToastId> = showing.into_iter().take(overflow).collect();
        for old in expired {
            self.expire(old);
        }
        if let Some(duration) = duration {
            let mut this = *self;
            spawn_in_window(async move {
                async_io::Timer::after(duration).await;
                this.expire(id);
            });
        }
        id
    }

    /// Replace the content of a visible toast (progress updates). No-op
    /// when it is gone.
    pub fn update(&mut self, id: ToastId, toast: Toast) {
        let mut stack = self.stack.write();
        if stack.leaving.contains(&id) {
            return;
        }
        if let Some(slot) = stack.items.iter_mut().find(|(tid, _)| *tid == id) {
            slot.1 = toast;
        }
    }

    /// Remove a toast without running `on_expire`.
    pub fn dismiss(&mut self, id: ToastId) {
        self.leave(id);
    }

    /// Remove a toast and run its `on_expire`.
    pub fn expire(&mut self, id: ToastId) {
        if let Some(handler) = self.leave(id).and_then(|t| t.on_expire) {
            handler.call(());
        }
    }

    fn press_action(&mut self, id: ToastId) {
        if let Some((_, handler)) = self.leave(id).and_then(|t| t.action) {
            handler.call(());
        }
    }

    /// Take `id` off the queue: it plays its exit, then goes. Returns what
    /// it was, unless it had already left.
    fn leave(&mut self, id: ToastId) -> Option<Toast> {
        let toast = {
            let stack = self.stack.peek();
            if stack.leaving.contains(&id) {
                return None;
            }
            stack
                .items
                .iter()
                .find(|(tid, _)| *tid == id)
                .map(|(_, t)| t.clone())
        }?;
        if crate::theme::reduce_motion() {
            self.stack.write().items.retain(|(tid, _)| *tid != id);
            return Some(toast);
        }
        self.stack.write().leaving.push(id);
        let mut stack = self.stack;
        spawn_in_window(async move {
            async_io::Timer::after(Duration::from_millis(EXIT_MS + 30)).await;
            let mut stack = stack.write();
            stack.items.retain(|(tid, _)| *tid != id);
            stack.leaving.retain(|tid| *tid != id);
        });
        Some(toast)
    }
}

/// Paints the toast queue, bottom-centered above `bottom_inset`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ToastHost {
    bottom_inset: f32,
}

impl ToastHost {
    #[must_use]
    pub const fn new(bottom_inset: f32) -> Self {
        Self { bottom_inset }
    }
}

impl Component for ToastHost {
    fn render(&self) -> impl IntoElement {
        let toasts = Toasts::get();
        let (items, leaving) = {
            let stack = toasts.stack.read();
            (stack.items.clone(), stack.leaving.clone())
        };
        rect()
            .layer(Layer::OverlayLevel(crate::tokens::layer::TOAST))
            .position(
                Position::new_absolute()
                    .bottom(self.bottom_inset + 24.0)
                    .left(0.0),
            )
            .width(Size::fill())
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .children(items.into_iter().map(|(id, toast)| {
                ToastCard {
                    id,
                    leaving: leaving.contains(&id),
                    toast,
                }
                .into_element()
            }))
    }
}

/// How far a toast is in, 0 to 1: it rises a little into place and sinks
/// back out the same way when `leaving`. Not faded: the card has panels of
/// its own (the progress bar).
fn use_shown(leaving: bool) -> f32 {
    let motion = use_animation_with_dependencies(&leaving, |conf, leaving| {
        conf.on_creation(OnCreation::Run);
        conf.on_change(OnChange::Rerun);
        if *leaving {
            AnimNum::new(1.0, 0.0)
                .time(EXIT_MS)
                .ease(Ease::In)
                .function(Function::Cubic)
        } else {
            AnimNum::new(0.0, 1.0)
                .time(ENTER_MS)
                .ease(Ease::Out)
                .function(Function::Expo)
        }
    });
    if crate::theme::reduce_motion() {
        1.0
    } else {
        motion.get().value()
    }
}

#[derive(Clone, PartialEq)]
struct ToastCard {
    id: ToastId,
    /// Gone from the queue, playing its exit.
    leaving: bool,
    toast: Toast,
}

impl Component for ToastCard {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let toast = &self.toast;
        let id = self.id;
        let (icon, icon_color) = match toast.tone {
            ToastTone::Done => (freya::icons::lucide::circle_check(), p.ok),
            ToastTone::Info => (freya::icons::lucide::info(), p.inv_text),
            ToastTone::Error => (freya::icons::lucide::circle_alert(), p.danger),
            ToastTone::Trash => (freya::icons::lucide::trash_2(), p.inv_text),
            ToastTone::People => (freya::icons::lucide::users(), p.kinds.canvas),
            ToastTone::Offline => (freya::icons::lucide::cloud_off(), p.warn),
            ToastTone::Download => (freya::icons::lucide::download(), p.kinds.bible),
        };
        let action_color = if crate::theme::relative_luminance(p.inv) > 0.5 {
            Color::lerp(p.accent, Color::BLACK, 0.35)
        } else if p.dark {
            p.accent
        } else {
            Color::lerp(p.accent, Color::WHITE, 0.45)
        };
        let shown = use_shown(self.leaving);
        let card = rect()
            .key(id.0)
            .offset_y(16.0 * (1.0 - shown))
            .scale((0.04f32.mul_add(shown, 0.96), 0.04f32.mul_add(shown, 0.96)))
            .interactive(!self.leaving)
            .a11y_role(AccessibilityRole::Status)
            .max_width(Size::px(560.0))
            .corner_radius(radius::LG)
            .background(p.inv)
            .border(
                Border::new()
                    .fill(p.inv_line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .shadow(Shadow::new().y(8.0).blur(24.0).color(p.shadow));
        match &toast.progress {
            Some((value, caption, fill)) => card
                .width(Size::px(380.0))
                .padding((12.0, 16.0, 14.0, 16.0))
                .spacing(10.0)
                .child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .content(Content::Flex)
                        .spacing(12.0)
                        .child(Icon::new(icon).size(18.0).color(fill.unwrap_or(icon_color)))
                        .child(
                            label()
                                .text(toast.text.clone())
                                .font_size(text::BODY)
                                .color(p.inv_text)
                                .width(Size::flex(1.0))
                                .max_lines(1),
                        )
                        .child(
                            label()
                                .text(caption.clone())
                                .font_size(text::LABEL)
                                .color(p.t3),
                        ),
                )
                .child(ProgressBar::new(*value).color(fill.unwrap_or(p.accent)))
                .into_element(),
            None => card
                .min_height(Size::px(48.0))
                .padding((0.0, 8.0, 0.0, 16.0))
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(12.0)
                .child(Icon::new(icon).size(18.0).color(icon_color))
                .child(
                    label()
                        .text(toast.text.clone())
                        .font_size(text::BODY)
                        .color(p.inv_text)
                        .max_lines(2),
                )
                .maybe_child(toast.action.clone().map(|(action_label, _)| {
                    rect()
                        .height(Size::px(34.0))
                        .padding((0.0, 12.0))
                        .corner_radius(radius::SM)
                        .center()
                        .a11y_role(AccessibilityRole::Button)
                        .on_press(move |_| Toasts::get().press_action(id))
                        .child(
                            label()
                                .text(action_label)
                                .font_size(text::BODY)
                                .font_weight(FontWeight::SEMI_BOLD)
                                .color(action_color),
                        )
                }))
                .maybe_child(toast.action.is_none().then(|| rect().width(Size::px(8.0))))
                .into_element(),
        }
    }
}
