//! Settings modal — sidebar + per-section pane.
//!
//! Opened from the home toolbar's gear button via
//! [`SettingsModal::open`]. The sidebar hosts three sections:
//!
//! * **Account** — placeholder, pending auth plumbing.
//! * **General** — app-wide prefs (input mode toggle for now).
//! * **Plugins** — one card per registered plugin with an origin badge
//!   (built-in or dynamic + library path), an enable toggle, and a
//!   form rendered from the plugin's [`notein_plugin::ConfigSchema`].
//!   A header-level `Instalar plugin` row points at the scan directory
//!   where dynamic `.so` / `.dll` / `.dylib` bundles can be dropped.
//!
//! Forms render from schemas without the shell knowing any plugin's
//! domain, so adding a plugin automatically grows the settings surface.

use std::sync::{Arc, Mutex};

use freya::prelude::*;
use notein_plugin::{
    ConfigField, ConfigFieldKind, ConfigValue, PluginOrigin, PluginRegistry, PluginRole,
    RegistrySchemaEntry,
};

use crate::canvas::{Board, InputMode};
use crate::components::form_input::FormInput;
use crate::components::modal::{Modal, ModalController};
use crate::icons;
use crate::plugins;

const PANEL_BG: Color = Color::from_rgb(30, 30, 34);
const SIDEBAR_BG: Color = Color::from_rgb(22, 22, 26);
const CARD_BG: Color = Color::from_rgb(38, 38, 44);
const TEXT_PRIMARY: Color = Color::from_rgb(230, 230, 240);
const TEXT_MUTED: Color = Color::from_rgb(160, 160, 175);
const ACCENT: Color = Color::from_rgb(120, 170, 255);
const BORDER: Color = Color::from_rgb(60, 60, 70);

const MODAL_WIDTH: f32 = 820.0;
const MODAL_HEIGHT: f32 = 580.0;
const SIDEBAR_WIDTH: f32 = 220.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Account,
    General,
    Plugins,
}

impl Section {
    const fn label(self) -> &'static str {
        match self {
            Self::Account => "Cuenta",
            Self::General => "General",
            Self::Plugins => "Plugins",
        }
    }

    fn icon(self) -> Bytes {
        match self {
            Self::Account => icons::user(),
            Self::General => icons::settings(),
            Self::Plugins => icons::puzzle(),
        }
    }
}

/// Builder entry point. `SettingsModal::open()` pushes the modal
/// through the shared [`ModalController`].
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsModal;

impl SettingsModal {
    pub fn open() {
        ModalController::get().open(
            Modal::new(Self)
                .center()
                .width(MODAL_WIDTH)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for SettingsModal {
    fn render(&self) -> impl IntoElement {
        let section = use_state(|| Section::General);
        // Reactivity bridge: UI mutations call `bump(&tick)` after
        // touching the registry so the modal re-renders. Needed because
        // the registry lives behind a `Mutex` outside freya's reactive
        // graph.
        let tick = use_state(|| 0u64);
        let _ = *tick.read();
        let registry = plugins::registry();

        rect()
            .horizontal()
            .width(Size::px(MODAL_WIDTH))
            .height(Size::px(MODAL_HEIGHT))
            .background(PANEL_BG)
            .overflow(Overflow::Clip)
            .with_corner_radius(18.0)
            .child(sidebar(section))
            .child(content(section, tick, registry))
    }
}

fn sidebar(mut section: State<Section>) -> impl IntoElement {
    let current = *section.read();
    let mut col = rect()
        .vertical()
        .width(Size::px(SIDEBAR_WIDTH))
        .height(Size::fill())
        .background(SIDEBAR_BG)
        .padding((20.0, 14.0))
        .spacing(4.0)
        .child(
            label()
                .color(TEXT_MUTED)
                .font_size(12.0)
                .text("Ajustes")
                .margin((0.0, 0.0, 14.0, 6.0)),
        );
    for s in [Section::Account, Section::General, Section::Plugins] {
        let active = s == current;
        col = col.child(sidebar_row(s, active, move |_| section.set(s)));
    }
    col
}

fn sidebar_row(
    sec: Section,
    active: bool,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> impl IntoElement {
    let text_color = if active { TEXT_PRIMARY } else { TEXT_MUTED };
    let bg = if active {
        Color::from_rgb(44, 44, 54)
    } else {
        Color::from_rgb(0, 0, 0).with_a(0)
    };
    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(38.0))
        .padding((0.0, 12.0))
        .spacing(10.0)
        .background(bg)
        .with_corner_radius(10.0)
        .cross_align(Alignment::Center)
        .on_press(on_press)
        .child(
            SvgViewer::new(sec.icon())
                .width(Size::px(16.0))
                .height(Size::px(16.0))
                .color(text_color),
        )
        .child(label().color(text_color).font_size(14.0).text(sec.label()))
}

fn content(
    section: State<Section>,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> impl IntoElement {
    let current = *section.read();
    // Each section lives in its own Component so the hook table is
    // scoped to that section — switching tabs swaps the child entirely
    // rather than growing/shrinking the parent's hook list, which
    // would violate freya's "same number of hooks per render" rule.
    let body: Element = match current {
        Section::Account => AccountSection.into_element(),
        Section::General => GeneralSection { tick }.into_element(),
        Section::Plugins => PluginsSection { tick, registry }.into_element(),
    };
    rect()
        .vertical()
        .width(Size::fill())
        .height(Size::fill())
        .padding((24.0, 28.0))
        .child(header_row(current))
        .child(spacer(14.0))
        .child(
            ScrollView::new()
                .child(body)
                .width(Size::fill())
                .height(Size::fill()),
        )
}

#[derive(Debug, PartialEq)]
struct AccountSection;

impl Component for AccountSection {
    fn render(&self) -> impl IntoElement {
        account_section()
    }
}

#[derive(Clone)]
struct GeneralSection {
    tick: State<u64>,
}

impl std::fmt::Debug for GeneralSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneralSection").finish_non_exhaustive()
    }
}

impl PartialEq for GeneralSection {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for GeneralSection {
    fn render(&self) -> impl IntoElement {
        general_section(self.tick)
    }
}

#[derive(Clone)]
struct PluginsSection {
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
}

impl std::fmt::Debug for PluginsSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginsSection").finish_non_exhaustive()
    }
}

impl PartialEq for PluginsSection {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for PluginsSection {
    fn render(&self) -> impl IntoElement {
        plugins_section(self.tick, self.registry.clone())
    }
}

fn header_row(sec: Section) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .child(
            label()
                .color(TEXT_PRIMARY)
                .font_size(20.0)
                .font_weight(FontWeight::BOLD)
                .text(sec.label()),
        )
        .child(flex_spacer())
        .child(
            rect()
                .width(Size::px(28.0))
                .height(Size::px(28.0))
                .center()
                .on_press(|_| ModalController::get().close())
                .child(
                    SvgViewer::new(icons::x())
                        .width(Size::px(16.0))
                        .height(Size::px(16.0))
                        .color(TEXT_MUTED),
                ),
        )
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

fn account_section() -> impl IntoElement {
    rect().vertical().spacing(12.0).child(
        rect()
            .vertical()
            .width(Size::fill())
            .padding((20.0, 20.0))
            .spacing(10.0)
            .background(CARD_BG)
            .with_corner_radius(14.0)
            .child(
                label()
                    .color(TEXT_PRIMARY)
                    .font_size(16.0)
                    .text("Cuenta — próximamente"),
            )
            .child(
                label()
                    .color(TEXT_MUTED)
                    .font_size(13.0)
                    .text("Sign-in, sincronización y colaboración llegarán en una fase posterior."),
            ),
    )
}

fn general_section(tick: State<u64>) -> impl IntoElement {
    let board = Board::shared();
    let mode = {
        let g = crate::canvas::lock(&board);
        g.input_mode()
    };
    let is_pan = matches!(mode, InputMode::Pan);
    let toggle = {
        let board = board.clone();
        move |_: Event<PressEventData>| {
            let mut g = crate::canvas::lock(&board);
            let next = if is_pan {
                InputMode::Draw
            } else {
                InputMode::Pan
            };
            g.set_input_mode(next);
        }
    };
    rect()
        .vertical()
        .spacing(12.0)
        .child(settings_row(
            "Modo de entrada",
            if is_pan {
                "El lápiz desplaza el lienzo"
            } else {
                "El lápiz dibuja"
            },
            toggle_switch(is_pan, toggle),
        ))
        .child(shortcut_picker_card(tick))
}

// ---------------------------------------------------------------------------
// FAB shortcut picker (General)
// ---------------------------------------------------------------------------

fn shortcut_picker_card(tick: State<u64>) -> impl IntoElement {
    let open = use_state(|| Option::<usize>::None);
    let slots = crate::plugins::fab_shortcut_slots();
    rect()
        .vertical()
        .width(Size::fill())
        .padding((18.0, 20.0))
        .spacing(10.0)
        .background(CARD_BG)
        .with_corner_radius(14.0)
        .child(
            label()
                .color(TEXT_PRIMARY)
                .font_size(14.0)
                .text("Accesos directos del lienzo"),
        )
        .child(label().color(TEXT_MUTED).font_size(12.0).text(
            "Los dos botones arriba del `+` disparan la acción que elijas de cualquier plugin.",
        ))
        .child(shortcut_slot_row(0, slots[0].as_ref(), open, tick))
        .child(shortcut_slot_row(1, slots[1].as_ref(), open, tick))
}

fn shortcut_slot_row(
    index: usize,
    slot: Option<&crate::plugin_settings::FabShortcut>,
    open: State<Option<usize>>,
    tick: State<u64>,
) -> impl IntoElement {
    let resolved = slot.and_then(crate::plugins::resolve_shortcut);
    let is_open = *open.read() == Some(index);
    let label_text = resolved
        .as_ref()
        .map_or_else(|| "(vacío)".to_owned(), |e| e.entry.label.clone());
    let icon = resolved
        .as_ref()
        .map_or_else(|| "·".to_owned(), |e| e.entry.icon.clone());

    let mut col = rect().vertical().width(Size::fill()).spacing(6.0).child(
        rect()
            .horizontal()
            .width(Size::fill())
            .padding((10.0, 12.0))
            .spacing(10.0)
            .background(Color::from_rgb(46, 46, 56))
            .with_corner_radius(10.0)
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .child(icon_slot(icon, TEXT_PRIMARY))
            .child(label().color(TEXT_PRIMARY).font_size(13.0).text(format!(
                "Slot {}: {}",
                index + 1,
                label_text
            )))
            .child(flex_spacer())
            .child({
                let mut open = open;
                rect()
                    .padding((6.0, 12.0))
                    .background(Color::from_rgb(60, 60, 72))
                    .with_corner_radius(8.0)
                    .on_press(move |_| {
                        let next = if is_open { None } else { Some(index) };
                        open.set(next);
                    })
                    .child(label().color(ACCENT).font_size(12.0).text(if is_open {
                        "Cerrar"
                    } else {
                        "Cambiar"
                    }))
            }),
    );

    if is_open {
        col = col.child(shortcut_options(index, open, tick));
    }
    col
}

fn shortcut_options(
    index: usize,
    open: State<Option<usize>>,
    tick: State<u64>,
) -> impl IntoElement {
    let mut options = rect()
        .vertical()
        .width(Size::fill())
        .padding((8.0, 10.0))
        .spacing(4.0)
        .background(Color::from_rgb(26, 26, 32))
        .with_corner_radius(10.0);

    // "Vacío" (clear) first.
    options = options.child(option_row(
        None,
        "(vacío — mostrar bubble gris)".to_owned(),
        {
            let mut open = open;
            let mut tick = tick;
            move |_: Event<PressEventData>| {
                crate::plugins::set_fab_shortcut(index, None);
                open.set(None);
                let next = *tick.read() + 1;
                tick.set(next);
            }
        },
    ));

    for entry in crate::plugins::available_entries() {
        let shortcut = crate::plugin_settings::FabShortcut {
            plugin_id: entry.plugin_id.clone(),
            entry_id: entry.entry.id.clone(),
        };
        let label_text = match entry.source {
            crate::plugins::EntrySource::ShellBuiltin => entry.entry.label.clone(),
            crate::plugins::EntrySource::Plugin(_) => {
                format!("{}  ·  {}", entry.entry.label, entry.plugin_label)
            }
        };
        let icon = icons::resolve(&entry.entry.icon);
        options = options.child(option_row(icon, label_text, {
            let mut open = open;
            let mut tick = tick;
            move |_: Event<PressEventData>| {
                crate::plugins::set_fab_shortcut(index, Some(shortcut.clone()));
                open.set(None);
                let next = *tick.read() + 1;
                tick.set(next);
            }
        }));
    }
    options
}

fn option_row(
    icon: Option<Bytes>,
    text: String,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .padding((6.0, 10.0))
        .spacing(10.0)
        .cross_align(Alignment::Center)
        .on_press(on_press)
        .child(icon_slot(icon, TEXT_PRIMARY))
        .child(label().color(TEXT_PRIMARY).font_size(13.0).text(text))
}

/// Fixed-size icon slot: renders the SVG when present, otherwise a
/// small centered dot so rows stay aligned.
fn icon_slot(icon: Option<Bytes>, color: Color) -> Rect {
    let mut slot = rect()
        .width(Size::px(18.0))
        .height(Size::px(18.0))
        .center();
    slot = match icon {
        Some(bytes) => slot.child(
            SvgViewer::new(bytes)
                .width(Size::px(16.0))
                .height(Size::px(16.0))
                .color(color),
        ),
        None => slot.child(
            rect()
                .width(Size::px(4.0))
                .height(Size::px(4.0))
                .background(color)
                .with_corner_radius(2.0),
        ),
    };
    slot
}

fn plugins_section(tick: State<u64>, registry: Arc<Mutex<PluginRegistry>>) -> impl IntoElement {
    let (schemas, disabled_ids, origins): (
        Vec<RegistrySchemaEntry>,
        Vec<String>,
        Vec<PluginOrigin>,
    ) = {
        let g = plugins::lock(&registry);
        let schemas = g.all_schemas();
        let disabled = g.disabled_ids().map(ToOwned::to_owned).collect();
        let origins = schemas.iter().map(|s| g.origin(&s.plugin_id)).collect();
        (schemas, disabled, origins)
    };

    let mut col = rect().vertical().spacing(14.0).child(install_row());

    if schemas.is_empty() {
        col = col.child(empty_state());
        return col;
    }

    for (entry, origin) in schemas.into_iter().zip(origins) {
        let enabled = !disabled_ids.contains(&entry.plugin_id);
        col = col.child(plugin_card(entry, origin, enabled, tick, registry.clone()));
    }
    col
}

fn install_row() -> impl IntoElement {
    let scan_dir = crate::dynamic_plugins::scan_root().display().to_string();
    rect()
        .horizontal()
        .width(Size::fill())
        .padding((14.0, 16.0))
        .spacing(10.0)
        .background(CARD_BG)
        .with_corner_radius(12.0)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .vertical()
                .child(
                    label()
                        .color(TEXT_PRIMARY)
                        .font_size(14.0)
                        .text("Instalar plugin"),
                )
                .child(
                    label()
                        .color(TEXT_MUTED)
                        .font_size(12.0)
                        .text(format!(
                            "Colocá `<id>/plugin.ntp.kdl` + .so/.dll/.dylib en {scan_dir} y reiniciá."
                        )),
                ),
        )
        .child(flex_spacer())
        .child(
            rect()
                .padding((8.0, 14.0))
                .background(Color::from_rgb(46, 46, 56))
                .with_corner_radius(8.0)
                .on_press(move |_| {
                    spawn(pick_plugin_file());
                })
                .child(
                    label()
                        .color(ACCENT)
                        .font_size(13.0)
                        .text("Elegir archivo…"),
                ),
        )
}

async fn pick_plugin_file() {
    use istmo_file_picker::{FileFilter, FilePickerClient, PickConfig};
    let picker = match FilePickerClient::acquire() {
        Ok(p) => p,
        Err(err) => {
            log::warn!("install plugin: file picker unavailable: {err:?}");
            return;
        }
    };
    let config = PickConfig {
        dialog_title: Some("Elegí el plugin a instalar".into()),
        filter: FileFilter {
            mime_types: Vec::new(),
            extensions: vec!["so".into(), "dll".into(), "dylib".into()],
            uti_types: Vec::new(),
        },
        start_directory_hint: None,
        allow_multiple: false,
    };
    match picker.pick_file_owned(config).await {
        Ok(Some(f)) => {
            log::info!(
                "plugin install stub: user picked {} — dynamic loader lands in Phase 3",
                f.display_name
            );
        }
        Ok(None) => {}
        Err(err) => log::warn!("plugin picker error: {err:?}"),
    }
}

fn empty_state() -> impl IntoElement {
    rect()
        .vertical()
        .width(Size::fill())
        .padding((24.0, 24.0))
        .spacing(6.0)
        .background(CARD_BG)
        .with_corner_radius(12.0)
        .cross_align(Alignment::Center)
        .child(
            label()
                .color(TEXT_PRIMARY)
                .font_size(14.0)
                .text("Todavía no hay plugins instalados."),
        )
        .child(
            label()
                .color(TEXT_MUTED)
                .font_size(12.0)
                .text("Los built-ins se registrarán en próximas fases."),
        )
}

fn plugin_card(
    entry: RegistrySchemaEntry,
    origin: PluginOrigin,
    enabled: bool,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> impl IntoElement {
    let plugin_id = entry.plugin_id.clone();
    let plugin_label = entry.plugin_label.clone();
    let role_label = match entry.plugin_role {
        PluginRole::Source => "Source",
        PluginRole::Canvas => "Canvas",
    };
    let (origin_badge, origin_detail) = match &origin {
        PluginOrigin::Builtin => ("Built-in".to_owned(), None),
        PluginOrigin::Dynamic(d) => (
            format!("Dynamic · v{}", d.manifest.version),
            Some(d.library_path.display().to_string()),
        ),
    };
    let toggle_cb = {
        let plugin_id = plugin_id.clone();
        let registry = registry.clone();
        let mut tick = tick;
        move |_: Event<PressEventData>| {
            {
                let mut g = plugins::lock(&registry);
                g.set_enabled(&plugin_id, !enabled);
            }
            plugins::persist_to_disk();
            let next = *tick.read() + 1;
            tick.set(next);
        }
    };

    let mut col = rect()
        .vertical()
        .width(Size::fill())
        .padding((16.0, 18.0))
        .spacing(12.0)
        .background(CARD_BG)
        .with_corner_radius(14.0)
        .child(
            rect()
                .horizontal()
                .width(Size::fill())
                .cross_align(Alignment::Center)
                .child({
                    let mut meta = rect()
                        .vertical()
                        .child(
                            label()
                                .color(TEXT_PRIMARY)
                                .font_size(15.0)
                                .font_weight(FontWeight::BOLD)
                                .text(plugin_label.clone()),
                        )
                        .child(
                            label()
                                .color(TEXT_MUTED)
                                .font_size(11.0)
                                .text(format!("{role_label} · {origin_badge} · {plugin_id}")),
                        );
                    if let Some(path) = origin_detail {
                        meta = meta.child(
                            label().color(TEXT_MUTED).font_size(10.0).text(path),
                        );
                    }
                    meta
                })
                .child(flex_spacer())
                .child(toggle_switch(enabled, toggle_cb)),
        );

    if !enabled {
        col =
            col.child(label().color(TEXT_MUTED).font_size(12.0).text(
                "Plugin deshabilitado — los campos siguen editables para no perder cambios.",
            ));
    }

    if entry.schema.fields.is_empty() {
        col = col.child(
            label()
                .color(TEXT_MUTED)
                .font_size(12.0)
                .text("Este plugin no expone ajustes."),
        );
        return col;
    }

    for field in entry.schema.fields.clone() {
        col = col.child(field_row(
            entry.plugin_id.clone(),
            entry.plugin_role,
            field,
            tick,
            registry.clone(),
        ));
    }
    col
}

// ---------------------------------------------------------------------------
// Form renderer
// ---------------------------------------------------------------------------

fn field_row(
    plugin_id: String,
    role: PluginRole,
    field: ConfigField,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> impl IntoElement {
    let current = read_value(&registry, &plugin_id, role, &field);
    let control = render_control(
        plugin_id.clone(),
        role,
        field.clone(),
        current.clone(),
        tick,
        registry.clone(),
    );
    rect()
        .vertical()
        .width(Size::fill())
        .spacing(6.0)
        .padding((10.0, 0.0, 0.0, 0.0))
        .child(
            rect()
                .horizontal()
                .width(Size::fill())
                .cross_align(Alignment::Center)
                .child(
                    rect()
                        .vertical()
                        .child(
                            label()
                                .color(TEXT_PRIMARY)
                                .font_size(13.0)
                                .text(field.label.clone()),
                        )
                        .child(if field.help.is_empty() {
                            rect().into_element()
                        } else {
                            label()
                                .color(TEXT_MUTED)
                                .font_size(11.0)
                                .text(field.help.clone())
                                .into_element()
                        }),
                )
                .child(flex_spacer())
                .child(control),
        )
        .child(divider())
}

fn render_control(
    plugin_id: String,
    role: PluginRole,
    field: ConfigField,
    current: ConfigValue,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> Element {
    match field.kind.clone() {
        ConfigFieldKind::Bool => bool_control(plugin_id, role, field, current, tick, registry),
        ConfigFieldKind::Int { min, max } => text_control(
            plugin_id,
            role,
            field,
            current,
            min,
            max,
            0.0,
            tick,
            registry,
            FieldNumeric::Int,
        ),
        ConfigFieldKind::Float { min, max, step } => text_control(
            plugin_id,
            role,
            field,
            current,
            min as i64,
            max as i64,
            step,
            tick,
            registry,
            FieldNumeric::Float,
        ),
        ConfigFieldKind::String { max_len } => string_control(
            plugin_id,
            role,
            field,
            current,
            max_len as usize,
            tick,
            registry,
        ),
        ConfigFieldKind::Enum { options } => {
            enum_control(plugin_id, role, field, current, options, tick, registry)
        }
        ConfigFieldKind::Color => color_control(plugin_id, role, field, current, tick, registry),
        ConfigFieldKind::FilePath { .. } => {
            string_control(plugin_id, role, field, current, 1024, tick, registry)
        }
    }
}

fn bool_control(
    plugin_id: String,
    role: PluginRole,
    field: ConfigField,
    current: ConfigValue,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> Element {
    let v = match current {
        ConfigValue::Bool(b) => b,
        _ => matches!(field.default, ConfigValue::Bool(true)),
    };
    let on_press = move |_: Event<PressEventData>| {
        write_value(
            &registry,
            &plugin_id,
            role,
            &field.key,
            ConfigValue::Bool(!v),
            tick,
        );
    };
    toggle_switch(v, on_press).into_element()
}

#[derive(Clone, Copy)]
enum FieldNumeric {
    Int,
    Float,
}

fn text_control(
    plugin_id: String,
    role: PluginRole,
    field: ConfigField,
    current: ConfigValue,
    min: i64,
    max: i64,
    _step: f64,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
    kind: FieldNumeric,
) -> Element {
    let text = use_state(|| match &current {
        ConfigValue::Int(i) => i.to_string(),
        ConfigValue::Float(f) => format!("{f}"),
        _ => match &field.default {
            ConfigValue::Int(i) => i.to_string(),
            ConfigValue::Float(f) => format!("{f}"),
            _ => String::new(),
        },
    });
    let on_change = {
        let plugin_id = plugin_id.clone();
        let field_key = field.key.clone();
        let registry = registry.clone();
        move |next: String| {
            let value = match kind {
                FieldNumeric::Int => next
                    .parse::<i64>()
                    .ok()
                    .map(|n| n.clamp(min, max))
                    .map(ConfigValue::Int),
                FieldNumeric::Float => next.parse::<f64>().ok().map(ConfigValue::Float),
            };
            if let Some(v) = value {
                write_value(&registry, &plugin_id, role, &field_key, v, tick);
            }
        }
    };
    FormInput::new(text)
        .width(Size::px(140.0))
        .on_change(on_change)
        .into_element()
}

fn string_control(
    plugin_id: String,
    role: PluginRole,
    field: ConfigField,
    current: ConfigValue,
    max_len: usize,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> Element {
    let initial = match current {
        ConfigValue::String(s) | ConfigValue::FilePath(s) => s,
        _ => match &field.default {
            ConfigValue::String(s) | ConfigValue::FilePath(s) => s.clone(),
            _ => String::new(),
        },
    };
    let text = use_state(|| initial);
    let on_change = {
        let plugin_id = plugin_id.clone();
        let field_key = field.key.clone();
        let registry = registry.clone();
        let kind = field.kind.clone();
        move |next: String| {
            let trimmed = if next.len() > max_len {
                next[..max_len].to_owned()
            } else {
                next
            };
            let value = match kind {
                ConfigFieldKind::FilePath { .. } => ConfigValue::FilePath(trimmed),
                _ => ConfigValue::String(trimmed),
            };
            write_value(&registry, &plugin_id, role, &field_key, value, tick);
        }
    };
    FormInput::new(text)
        .width(Size::px(220.0))
        .on_change(on_change)
        .into_element()
}

fn enum_control(
    plugin_id: String,
    role: PluginRole,
    field: ConfigField,
    current: ConfigValue,
    options: Vec<notein_plugin::ConfigEnumOption>,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> Element {
    let selected = match current {
        ConfigValue::String(s) => s,
        _ => match &field.default {
            ConfigValue::String(s) => s.clone(),
            _ => String::new(),
        },
    };
    let mut row = rect().horizontal().spacing(6.0);
    for opt in options {
        let is_selected = opt.value == selected;
        let bg = if is_selected {
            Color::from_rgb(60, 90, 160)
        } else {
            Color::from_rgb(50, 50, 60)
        };
        let text_color = if is_selected {
            TEXT_PRIMARY
        } else {
            TEXT_MUTED
        };
        let pick_value = opt.value.clone();
        let plugin_id = plugin_id.clone();
        let field_key = field.key.clone();
        let registry = registry.clone();
        let on_press = move |_: Event<PressEventData>| {
            write_value(
                &registry,
                &plugin_id,
                role,
                &field_key,
                ConfigValue::String(pick_value.clone()),
                tick,
            );
        };
        row = row.child(
            rect()
                .padding((6.0, 10.0))
                .background(bg)
                .with_corner_radius(8.0)
                .on_press(on_press)
                .child(label().color(text_color).font_size(12.0).text(opt.label)),
        );
    }
    row.into_element()
}

fn color_control(
    plugin_id: String,
    role: PluginRole,
    field: ConfigField,
    current: ConfigValue,
    tick: State<u64>,
    registry: Arc<Mutex<PluginRegistry>>,
) -> Element {
    let packed = match current {
        ConfigValue::Color(c) => c,
        _ => match &field.default {
            ConfigValue::Color(c) => *c,
            _ => 0,
        },
    };
    let hex_initial = format!("#{packed:08x}");
    let hex = use_state(|| hex_initial);
    let swatch_color = unpack_rgba(packed);

    let on_change = {
        let plugin_id = plugin_id.clone();
        let field_key = field.key.clone();
        let registry = registry.clone();
        move |next: String| {
            if let Some(parsed) = parse_hex_rgba(&next) {
                write_value(
                    &registry,
                    &plugin_id,
                    role,
                    &field_key,
                    ConfigValue::Color(parsed),
                    tick,
                );
            }
        }
    };

    rect()
        .horizontal()
        .spacing(8.0)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .width(Size::px(28.0))
                .height(Size::px(28.0))
                .background(swatch_color)
                .with_corner_radius(6.0)
                .border(Border::new().width(1.0).fill(BORDER)),
        )
        .child(
            FormInput::new(hex)
                .width(Size::px(140.0))
                .on_change(on_change),
        )
        .into_element()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_value(
    registry: &Arc<Mutex<PluginRegistry>>,
    plugin_id: &str,
    role: PluginRole,
    field: &ConfigField,
) -> ConfigValue {
    let g = plugins::lock(registry);
    let v = match role {
        PluginRole::Source => g
            .get_source(plugin_id)
            .and_then(|p| p.get_config(&field.key)),
        PluginRole::Canvas => g
            .get_canvas(plugin_id)
            .and_then(|p| p.get_config(&field.key)),
    };
    v.unwrap_or_else(|| field.default.clone())
}

fn write_value(
    registry: &Arc<Mutex<PluginRegistry>>,
    plugin_id: &str,
    role: PluginRole,
    key: &str,
    value: ConfigValue,
    mut tick: State<u64>,
) {
    let outcome = {
        let g = plugins::lock(registry);
        match role {
            PluginRole::Source => g.get_source(plugin_id).map(|p| p.set_config(key, value)),
            PluginRole::Canvas => g.get_canvas(plugin_id).map(|p| p.set_config(key, value)),
        }
    };
    if let Some(Err(err)) = outcome {
        log::warn!("plugin {plugin_id} rejected setting `{key}`: {err}");
        return;
    }
    plugins::persist_to_disk();
    let next = *tick.read() + 1;
    tick.set(next);
}

fn toggle_switch(
    on: bool,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> impl IntoElement {
    let track = if on {
        Color::from_rgb(80, 140, 240)
    } else {
        Color::from_rgb(70, 70, 80)
    };
    let knob_pad_left = if on { 20.0 } else { 2.0 };
    rect()
        .horizontal()
        .width(Size::px(44.0))
        .height(Size::px(24.0))
        .background(track)
        .with_corner_radius(99.0)
        .padding((2.0, knob_pad_left, 2.0, 2.0))
        .on_press(on_press)
        .child(
            rect()
                .width(Size::px(20.0))
                .height(Size::px(20.0))
                .background(Color::from_rgb(240, 240, 245))
                .with_corner_radius(99.0),
        )
}

fn settings_row(
    title: &'static str,
    subtitle: impl Into<String>,
    control: impl IntoElement + 'static,
) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .padding((14.0, 18.0))
        .background(CARD_BG)
        .with_corner_radius(12.0)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .vertical()
                .child(label().color(TEXT_PRIMARY).font_size(14.0).text(title))
                .child(
                    label()
                        .color(TEXT_MUTED)
                        .font_size(12.0)
                        .text(subtitle.into()),
                ),
        )
        .child(flex_spacer())
        .child(control)
}

fn divider() -> impl IntoElement {
    rect()
        .width(Size::fill())
        .height(Size::px(1.0))
        .background(BORDER)
}

fn spacer(h: f32) -> impl IntoElement {
    rect().width(Size::px(1.0)).height(Size::px(h))
}

fn flex_spacer() -> impl IntoElement {
    rect().width(Size::flex(1.0)).height(Size::px(1.0))
}

const fn unpack_rgba(packed: u32) -> Color {
    let r = ((packed >> 24) & 0xff) as u8;
    let g = ((packed >> 16) & 0xff) as u8;
    let b = ((packed >> 8) & 0xff) as u8;
    Color::from_rgb(r, g, b)
}

fn parse_hex_rgba(s: &str) -> Option<u32> {
    let trimmed = s.trim().trim_start_matches('#');
    match trimmed.len() {
        6 => {
            let v = u32::from_str_radix(trimmed, 16).ok()?;
            Some((v << 8) | 0xff)
        }
        8 => u32::from_str_radix(trimmed, 16).ok(),
        _ => None,
    }
}
