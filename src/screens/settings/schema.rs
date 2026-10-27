//! A plugin's settings form, drawn from its schema with one fixed
//! control per field kind.

use std::sync::Arc;
use std::time::{Duration, Instant};

use freya::prelude::*;
use notein_plugin::config::{ConfigEnumOption, ConfigField, ConfigFieldKind, ConfigValue};
use notein_plugin::plugin::PluginSettings;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Action, ActionEntry, ActionList, Button, ButtonVariant, ColorWheel, Group, ListRow, Popover,
    PopoverAlign, PopoverSide, Segmented, Stepper, Switch, Text, TextField, TextRole, Trailing,
};

/// Widest integer range drawn as a stepper; wider ranges get a field.
const STEPPER_RANGE: i64 = 50;
/// Most enum options drawn as a segmented control.
const SEGMENTED_OPTIONS: usize = 4;
/// Time a destructive action waits for its second tap.
const CONFIRM_WINDOW: Duration = Duration::from_secs(3);

const fn rgba(packed: u32) -> Color {
    let [r, g, b, a] = packed.to_be_bytes();
    Color::from_argb(a, r, g, b)
}

fn packed(color: Color) -> u32 {
    u32::from_be_bytes([color.r(), color.g(), color.b(), color.a()])
}

/// The settings form of one plugin.
#[derive(Clone)]
pub(super) struct SchemaForm {
    pub(super) settings: Arc<dyn PluginSettings>,
}

impl PartialEq for SchemaForm {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.settings, &other.settings)
    }
}

impl std::fmt::Debug for SchemaForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchemaForm").finish_non_exhaustive()
    }
}

impl Component for SchemaForm {
    fn render(&self) -> impl IntoElement {
        let revision = use_state(|| 0u32);
        let _ = revision.read();
        let schema = self.settings.schema();
        let rows: Vec<Element> = schema
            .fields
            .into_iter()
            .map(|field| {
                FieldRow {
                    settings: Arc::clone(&self.settings),
                    field,
                    revision,
                }
                .into_element()
            })
            .collect();
        if rows.is_empty() {
            return Text::new("Este plugin no tiene ajustes.")
                .role(TextRole::Hint)
                .into_element();
        }
        Group::default().children(rows).into_element()
    }
}

#[derive(Clone)]
struct FieldRow {
    settings: Arc<dyn PluginSettings>,
    field: ConfigField,
    revision: State<u32>,
}

impl PartialEq for FieldRow {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.settings, &other.settings) && self.field == other.field
    }
}

impl FieldRow {
    fn current(&self) -> ConfigValue {
        self.settings
            .get(&self.field.key)
            .unwrap_or_else(|| self.field.default.clone())
    }

    /// Store `value`; returns the plugin's refusal as text.
    fn setter(&self) -> impl Fn(ConfigValue) -> Option<String> + Clone + 'static {
        let settings = Arc::clone(&self.settings);
        let key = self.field.key.clone();
        let revision = self.revision;
        move |value| match settings.set(&key, value) {
            Ok(()) => {
                let mut revision = revision;
                let next = revision.peek().wrapping_add(1);
                revision.set(next);
                None
            }
            Err(err) => Some(err.to_string()),
        }
    }

    fn switch(&self, current: &ConfigValue, mut error: State<Option<String>>) -> Element {
        let set = self.setter();
        Switch::new(matches!(current, ConfigValue::Bool(true)))
            .on_toggle(move |on| error.set(set(ConfigValue::Bool(on))))
            .into_element()
    }

    fn stepper(
        &self,
        current: &ConfigValue,
        (min, max): (i64, i64),
        mut error: State<Option<String>>,
    ) -> Element {
        let set = self.setter();
        let value = match current {
            ConfigValue::Int(n) => *n,
            _ => min,
        };
        Stepper::new(value, min, max)
            .on_change(move |n| error.set(set(ConfigValue::Int(n))))
            .into_element()
    }

    fn int_text(
        &self,
        text: State<String>,
        (min, max): (i64, i64),
        mut error: State<Option<String>>,
    ) -> Element {
        let set = self.setter();
        rect()
            .width(Size::px(140.0))
            .child(TextField::new(text).on_submit(move |typed: String| {
                match typed.trim().parse::<i64>() {
                    Ok(n) if (min..=max).contains(&n) => error.set(set(ConfigValue::Int(n))),
                    _ => error.set(Some(format!("Un número entre {min} y {max}"))),
                }
            }))
            .into_element()
    }

    fn slider(
        &self,
        current: &ConfigValue,
        (min, max, step): (f64, f64, f64),
        mut error: State<Option<String>>,
    ) -> Element {
        let set = self.setter();
        let value = match current {
            ConfigValue::Float(f) => *f,
            _ => min,
        };
        let span = (max - min).max(f64::EPSILON);
        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .child(
                rect().width(Size::px(160.0)).child(
                    Slider::new(move |percent: f64| {
                        let raw = min + span * percent / 100.0;
                        let snapped = if step > 0.0 {
                            (raw / step).round() * step
                        } else {
                            raw
                        };
                        error.set(set(ConfigValue::Float(snapped.clamp(min, max))));
                    })
                    .value((value - min) / span * 100.0),
                ),
            )
            .child(Text::new(format!("{value:.2}")).role(TextRole::Meta))
            .into_element()
    }

    fn text(&self, text: State<String>, max_len: u32, mut error: State<Option<String>>) -> Element {
        let set = self.setter();
        let max_len = max_len as usize;
        rect()
            .width(Size::px(220.0))
            .child(TextField::new(text).error(error.read().clone()).on_submit(
                move |typed: String| {
                    if typed.chars().count() > max_len {
                        error.set(Some(format!("Hasta {max_len} caracteres")));
                    } else {
                        error.set(set(ConfigValue::String(typed)));
                    }
                },
            ))
            .into_element()
    }

    fn segmented(
        &self,
        options: &[ConfigEnumOption],
        current: &ConfigValue,
        mut error: State<Option<String>>,
    ) -> Element {
        let set = self.setter();
        let selected = match current {
            ConfigValue::String(v) => options.iter().position(|o| o.value == *v).unwrap_or(0),
            _ => 0,
        };
        let values: Vec<String> = options.iter().map(|o| o.value.clone()).collect();
        Segmented::new(options.iter().map(|o| o.label.clone()), selected)
            .on_select(move |i: usize| {
                if let Some(v) = values.get(i) {
                    error.set(set(ConfigValue::String(v.clone())));
                }
            })
            .into_element()
    }

    /// A button opening the options as a menu, for long lists.
    fn menu(
        &self,
        options: &[ConfigEnumOption],
        current: &ConfigValue,
        error: State<Option<String>>,
    ) -> Element {
        let set = self.setter();
        let chosen = match current {
            ConfigValue::String(v) => options.iter().find(|o| o.value == *v),
            _ => None,
        };
        let shown = chosen.map_or_else(|| "Elegir".to_owned(), |o| o.label.clone());
        let options = options.to_vec();
        let selected = chosen.map(|o| o.value.clone());
        Button::new(shown)
            .ghost()
            .small()
            .icon(freya::icons::lucide::chevrons_up_down())
            .on_press(move |e: Event<PressEventData>| {
                let anchor = notein_ui_kit::press_anchor(e.data());
                let entries = options.iter().map(|o| {
                    let set = set.clone();
                    let value = o.value.clone();
                    ActionEntry::Item(
                        Action::new(o.label.clone(), move |()| {
                            notein_ui_kit::ModalController::get().close();
                            let mut error = error;
                            error.set(set(ConfigValue::String(value.clone())));
                        })
                        .checked(selected.as_deref() == Some(o.value.as_str())),
                    )
                });
                Popover::new(anchor, ActionList::new(entries).width(240.0))
                    .width(240.0)
                    .side(PopoverSide::Below)
                    .align(PopoverAlign::End)
                    .open();
            })
            .into_element()
    }

    fn color(
        &self,
        current: &ConfigValue,
        error: State<Option<String>>,
        mut wheel_open: State<bool>,
    ) -> Element {
        let p = palette();
        let set = self.setter();
        let color = match current {
            ConfigValue::Color(c) => rgba(*c),
            _ => p.accent,
        };
        rect()
            .spacing(8.0)
            .cross_align(Alignment::End)
            .child(
                rect()
                    .width(Size::px(32.0))
                    .height(Size::px(32.0))
                    .corner_radius(16.0)
                    .background(color)
                    .border(
                        Border::new()
                            .fill(p.line_strong)
                            .width(1.0)
                            .alignment(BorderAlignment::Outer),
                    )
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt("Cambiar color")
                    .on_tap(move |_| {
                        let next = !wheel_open();
                        wheel_open.set(next);
                    }),
            )
            .maybe_child(wheel_open().then(|| {
                ColorWheel::new()
                    .swatches(notein_ui_kit::theme::ACCENT_SWATCHES.to_vec())
                    .allow_custom(true)
                    .initial(color)
                    .width(260.0)
                    .on_change(move |c: Color| {
                        let mut error = error;
                        error.set(set(ConfigValue::Color(packed(c))));
                    })
            }))
            .into_element()
    }

    /// The chosen file's name and a picker; the file is copied next to
    /// the plugin so it survives the original moving.
    fn file(
        &self,
        extensions: &[String],
        current: &ConfigValue,
        error: State<Option<String>>,
    ) -> Element {
        let set = self.setter();
        let shown = match current {
            ConfigValue::FilePath(path) if !path.is_empty() => std::path::Path::new(path)
                .file_name()
                .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned()),
            _ => "Ningún archivo".to_owned(),
        };
        let extensions = extensions.to_vec();
        let key = self.field.key.clone();
        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .child(Text::new(shown).role(TextRole::Meta).lines(1))
            .child(Button::new("Elegir…").ghost().small().on_press(move |_| {
                let set = set.clone();
                let extensions = extensions.clone();
                let key = key.clone();
                spawn(async move {
                    let exts: Vec<&str> = extensions.iter().map(String::as_str).collect();
                    match crate::files::pick("Elegir archivo", &exts, &[]).await {
                        Ok(Some(picked)) => {
                            let dir = crate::plugins::dynamic::plugins_dir()
                                .join(".files")
                                .join(&key);
                            let path = dir.join(&picked.name);
                            let stored = std::fs::create_dir_all(&dir)
                                .and_then(|()| std::fs::write(&path, &picked.bytes));
                            let mut error = error;
                            match stored {
                                Ok(()) => error.set(set(ConfigValue::FilePath(
                                    path.to_string_lossy().into_owned(),
                                ))),
                                Err(err) => error
                                    .set(Some(format!("No se pudo guardar el archivo: {err}"))),
                            }
                        }
                        Ok(None) => {}
                        Err(err) => log::warn!("plugin file: {err}"),
                    }
                });
            }))
            .into_element()
    }

    /// A button; one with a confirmation asks for a second tap.
    fn action(
        &self,
        button_label: &str,
        needs_confirm: bool,
        mut error: State<Option<String>>,
        mut armed: State<Option<Instant>>,
    ) -> Element {
        let set = self.setter();
        let is_armed = armed.read().is_some_and(|at| at.elapsed() < CONFIRM_WINDOW);
        let label = if is_armed {
            "¿Seguro? Tocá de nuevo".to_owned()
        } else {
            button_label.to_owned()
        };
        Button::new(label)
            .variant(if is_armed {
                ButtonVariant::DangerFill
            } else {
                ButtonVariant::Tonal
            })
            .small()
            .on_press(move |_| {
                if needs_confirm && !is_armed {
                    armed.set(Some(Instant::now()));
                    return;
                }
                armed.set(None);
                error.set(set(ConfigValue::Trigger));
            })
            .into_element()
    }
}

impl Component for FieldRow {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let current = self.current();
        let error = use_state(|| None::<String>);
        let armed = use_state(|| None::<Instant>);
        let text = use_state({
            let current = current.clone();
            move || match current {
                ConfigValue::String(s) | ConfigValue::FilePath(s) => s,
                ConfigValue::Int(n) => n.to_string(),
                ConfigValue::Float(f) => f.to_string(),
                ConfigValue::Bool(_) | ConfigValue::Color(_) | ConfigValue::Trigger => {
                    String::new()
                }
            }
        });
        let wheel_open = use_state(|| false);
        let control = match &self.field.kind {
            ConfigFieldKind::Bool => self.switch(&current, error),
            ConfigFieldKind::Int { min, max } if max - min <= STEPPER_RANGE => {
                self.stepper(&current, (*min, *max), error)
            }
            ConfigFieldKind::Int { min, max } => self.int_text(text, (*min, *max), error),
            ConfigFieldKind::Float { min, max, step } => {
                self.slider(&current, (*min, *max, *step), error)
            }
            ConfigFieldKind::String { max_len } => self.text(text, *max_len, error),
            ConfigFieldKind::Enum { options } if options.len() <= SEGMENTED_OPTIONS => {
                self.segmented(options, &current, error)
            }
            ConfigFieldKind::Enum { options } => self.menu(options, &current, error),
            ConfigFieldKind::Color => self.color(&current, error, wheel_open),
            ConfigFieldKind::FilePath { extensions } => self.file(extensions, &current, error),
            ConfigFieldKind::Action {
                button_label,
                confirm,
            } => self.action(button_label, confirm.is_some(), error, armed),
        };
        rect()
            .width(Size::fill())
            .child(
                ListRow::new(self.field.label.clone())
                    .subtitle(self.field.help.clone())
                    .trailing(Trailing::Element(control)),
            )
            .maybe_child(error.read().clone().map(|why| {
                rect()
                    .padding((0.0, 16.0, 10.0, 16.0))
                    .child(Text::new(why).role(TextRole::Meta).color(p.danger))
            }))
    }
}
