//! Where the settings live, and how they travel between a person's
//! devices.
//!
//! Most settings follow the person: they sit in `settings.bin` as
//! separate [`Part`]s, each with the time it last changed, and every part
//! travels on its own (last writer wins), so changing the pen on the
//! tablet and the look on the computer at once keeps both. Parts are
//! personal: the sync never puts them in a share.
//!
//! A few stay with the device ([`OnThisDevice`], in `local.bin`): text
//! size and reduced motion, which depend on the screen, and how Home is
//! arranged. The interface language travels only once chosen by hand;
//! "follow the device" travels as such, so every device follows its own.
//!
//! The settings of earlier versions (`settings.v2.bin`, one record)
//! are split into parts on first read, stamped with when that file was
//! last written, so the device configured last wins.

use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use istmo::codec::{Decode, Encode};
use notin_i18n::Lang;

use super::{
    AppearanceSettings, LibrarySettings, NotesSettings, PenSettings, PluginPrefs, SessionSettings,
    Settings, ShelfLayout, SortOrder, TextSizePref, ThemePref, TrashRetention,
};
use crate::persist::Persisted;

/// File of the parts that travel.
const FILE: &str = "settings.bin";

/// A part of the settings that travels on its own. Stored and sent by
/// position: add new ones at the end.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// Theme, accent and a language chosen by hand.
    Look,
    Pen,
    Notes,
    /// How long the trash keeps things.
    Library,
    Plugins,
    Sessions,
    /// The writing style learned from beautified words.
    Handwriting,
}

impl Part {
    const ALL: [Self; 7] = [
        Self::Look,
        Self::Pen,
        Self::Notes,
        Self::Library,
        Self::Plugins,
        Self::Sessions,
        Self::Handwriting,
    ];

    /// Sync id of the part.
    #[must_use]
    pub(crate) const fn id(self) -> [u8; 16] {
        let mut id = [0; 16];
        id[0] = self as u8 + 1;
        id
    }

    #[must_use]
    pub(crate) fn from_id(id: [u8; 16]) -> Option<Self> {
        Self::ALL.into_iter().find(|part| part.id() == id)
    }
}

/// [`Part::Look`].
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct Look {
    theme: ThemePref,
    accent: Option<[u8; 3]>,
    language: Option<Lang>,
}

/// The settings that stay with this device.
#[istmo::message]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct OnThisDevice {
    text_size: TextSizePref,
    reduce_motion: bool,
    sort: SortOrder,
    layout: ShelfLayout,
}

impl Persisted for OnThisDevice {
    const FILE: &'static str = "settings-device";
}

/// The settings of earlier versions, all in one record.
#[istmo::message]
#[derive(Clone, Debug, Default)]
struct Legacy {
    settings: Settings,
}

impl Persisted for Legacy {
    const FILE: &'static str = "settings.v2.bin";
}

/// One stored part: what it holds and when it last changed.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPart {
    pub part: Part,
    /// Unix milliseconds.
    pub updated_at: u64,
    pub bytes: Vec<u8>,
}

#[istmo::message]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SettingsFile {
    parts: Vec<StoredPart>,
}

impl SettingsFile {
    fn get(&self, part: Part) -> Option<&StoredPart> {
        self.parts.iter().find(|stored| stored.part == part)
    }

    fn decode<T: Decode + Default>(&self, part: Part) -> T {
        self.get(part)
            .and_then(|stored| notin_plugin::decode::<T>(&stored.bytes).ok())
            .unwrap_or_default()
    }

    /// Store `value` as `part`, stamped `at` when it changed. `true` when
    /// it did. A part stored for the first time with its default value is
    /// stamped 0: a new device must not send its defaults over what the
    /// person set on another one.
    fn set<T: Encode + Default>(&mut self, part: Part, value: &T, at: u64) -> bool {
        let Ok(bytes) = notin_plugin::encode(value) else {
            return false;
        };
        match self.parts.iter_mut().find(|stored| stored.part == part) {
            Some(stored) if stored.bytes == bytes => false,
            Some(stored) => {
                stored.bytes = bytes;
                stored.updated_at = at;
                true
            }
            None => {
                let default = notin_plugin::encode(&T::default()).ok();
                let updated_at = if default.as_ref() == Some(&bytes) {
                    0
                } else {
                    at
                };
                self.parts.push(StoredPart {
                    part,
                    updated_at,
                    bytes,
                });
                true
            }
        }
    }

    /// Put every travelling part of `settings`, stamped `at` where it
    /// changed. `true` when any did.
    fn split(&mut self, settings: &Settings, at: u64) -> bool {
        let look = Look {
            theme: settings.appearance.theme,
            accent: settings.appearance.accent,
            language: settings.appearance.language,
        };
        // Not `||`: every part must be stored, changed or not.
        [
            self.set(Part::Look, &look, at),
            self.set(Part::Pen, &settings.pen, at),
            self.set(Part::Notes, &settings.notes, at),
            self.set(Part::Library, &settings.library.trash_retention, at),
            self.set(Part::Plugins, &settings.plugins, at),
            self.set(Part::Sessions, &settings.sessions, at),
        ]
        .contains(&true)
    }
}

/// The parts in memory, read from disk on first use.
static STORED: Mutex<Option<SettingsFile>> = Mutex::new(None);
/// Who wants to know when a part arrives from another device.
static LISTENERS: Mutex<Vec<flume::Sender<Part>>> = Mutex::new(Vec::new());

fn stored() -> MutexGuard<'static, Option<SettingsFile>> {
    STORED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn path() -> PathBuf {
    crate::library::bodies::root_dir().join(FILE)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| u64::try_from(since.as_millis()).ok())
        .unwrap_or(0)
}

/// The parts, read from `settings.bin` or split out of the settings of
/// earlier versions the first time.
fn with_file<T>(f: impl FnOnce(&mut SettingsFile) -> T) -> T {
    let mut slot = stored();
    let file = slot.get_or_insert_with(|| {
        if let Some(file) = std::fs::read(path())
            .ok()
            .and_then(|bytes| notin_plugin::decode::<SettingsFile>(&bytes).ok())
        {
            return file;
        }
        migrate()
    });
    let out = f(file);
    drop(slot);
    out
}

#[sismo::function("settings::stored_migrate")]
fn migrate() -> SettingsFile {
    let old = crate::library::bodies::root_dir().join(Legacy::FILE);
    // When the person last changed them, so the device configured last
    // wins once both move to parts. Read before `Legacy::load` moves
    // the file away.
    let at = std::fs::metadata(&old)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|since| u64::try_from(since.as_millis()).ok())
        .unwrap_or(0);
    let legacy = Legacy::load().settings;
    let mut file = SettingsFile::default();
    file.split(&legacy, at);
    let device = OnThisDevice {
        text_size: legacy.appearance.text_size,
        reduce_motion: legacy.appearance.reduce_motion,
        sort: legacy.library.sort,
        layout: legacy.library.layout,
    };
    if let Err(err) = device.save() {
        tracing::warn!("settings of this device: {err}");
    }
    if let Err(err) = write(&file) {
        tracing::warn!("settings: {err}");
    }
    file
}

fn write(file: &SettingsFile) -> io::Result<()> {
    let bytes = notin_plugin::encode(file)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    crate::persist::write_atomic(&path(), &bytes)
}

/// The settings: the parts that travel and those of this device.
#[must_use]
#[sismo::function("settings::stored_load")]
pub(super) fn load() -> Settings {
    let device = OnThisDevice::load();
    with_file(|file| {
        let look: Look = file.decode(Part::Look);
        Settings {
            appearance: AppearanceSettings {
                theme: look.theme,
                accent: look.accent,
                text_size: device.text_size,
                reduce_motion: device.reduce_motion,
                language: look.language,
            },
            pen: file.decode::<PenSettings>(Part::Pen),
            notes: file.decode::<NotesSettings>(Part::Notes),
            library: LibrarySettings {
                trash_retention: file.decode::<TrashRetention>(Part::Library),
                sort: device.sort,
                layout: device.layout,
            },
            plugins: file.decode::<PluginPrefs>(Part::Plugins),
            sessions: file.decode::<SessionSettings>(Part::Sessions),
        }
    })
}

/// Store `settings`; the parts that changed go to the other devices.
///
/// # Errors
/// Encoding or filesystem failures.
#[sismo::function("settings::stored_save")]
pub(super) fn save(settings: &Settings) -> io::Result<()> {
    OnThisDevice {
        text_size: settings.appearance.text_size,
        reduce_motion: settings.appearance.reduce_motion,
        sort: settings.library.sort,
        layout: settings.library.layout,
    }
    .save()?;
    let changed = with_file(|file| {
        let changed = file.split(settings, now());
        if changed {
            write(file).map(|()| true)
        } else {
            Ok(false)
        }
    })?;
    if changed {
        crate::sync::local_changed();
    }
    Ok(())
}

/// The writing style learned here or on another device, once there is
/// one.
#[must_use]
pub(crate) fn handwriting() -> Option<notin_beautify::StyleProfile> {
    with_file(|file| {
        file.get(Part::Handwriting)
            .and_then(|stored| notin_plugin::decode(&stored.bytes).ok())
    })
}

/// Store the writing style; it goes to the other devices when it changed.
pub(crate) fn save_handwriting(profile: &notin_beautify::StyleProfile) {
    let changed = with_file(|file| {
        file.set(Part::Handwriting, profile, now()) && {
            if let Err(err) = write(file) {
                tracing::warn!("writing style: {err}");
            }
            true
        }
    });
    if changed {
        crate::sync::local_changed();
    }
}

/// Every stored part with when it last changed, for the sync. Parts never
/// changed (stamped 0) do not travel.
#[must_use]
pub(crate) fn heads() -> Vec<(Part, u64)> {
    with_file(|file| {
        file.parts
            .iter()
            .filter(|stored| stored.updated_at > 0)
            .map(|stored| (stored.part, stored.updated_at))
            .collect()
    })
}

/// `part` as stored, for the sync.
#[must_use]
pub(crate) fn part(part: Part) -> Option<StoredPart> {
    with_file(|file| file.get(part).cloned())
}

/// A part from another device: kept when it is newer than this one's.
/// `true` when it was.
#[sismo::function("settings::stored_apply_remote")]
pub(crate) fn apply_remote(incoming: StoredPart) -> io::Result<bool> {
    let part = incoming.part;
    let kept = with_file(|file| {
        let newer = file
            .get(part)
            .is_none_or(|stored| stored.updated_at < incoming.updated_at);
        if newer {
            file.parts.retain(|stored| stored.part != part);
            file.parts.push(incoming);
            write(file).map(|()| true)
        } else {
            Ok(false)
        }
    })?;
    if kept {
        LISTENERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.send(part).is_ok());
    }
    Ok(kept)
}

/// Parts as they arrive from other devices.
#[must_use]
pub(crate) fn changes() -> flume::Receiver<Part> {
    let (tx, rx) = flume::unbounded();
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(tx);
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_parts_get_a_new_stamp() {
        let mut file = SettingsFile::default();
        let mut settings = Settings::default();
        assert!(file.split(&settings, 10), "first store, at 0 since all defaults");
        settings.pen.palm_rejection = !settings.pen.palm_rejection;
        settings.library.sort = SortOrder::default();
        assert!(file.split(&settings, 20));
        let stamp = |part| file.get(part).map(|stored| stored.updated_at);
        assert_eq!(stamp(Part::Pen), Some(20));
        assert_eq!(
            stamp(Part::Notes),
            Some(0),
            "untouched parts keep their stamp"
        );
        assert!(!file.split(&settings, 30), "nothing changed");
    }

    #[test]
    fn layout_and_text_size_never_travel() {
        let mut a = SettingsFile::default();
        let mut b = SettingsFile::default();
        let mut settings = Settings::default();
        a.split(&settings, 10);
        settings.appearance.text_size = TextSizePref::default();
        settings.library.layout = ShelfLayout::default();
        settings.appearance.reduce_motion = !settings.appearance.reduce_motion;
        b.split(&settings, 10);
        assert_eq!(a, b, "only device settings differ");
    }

    #[test]
    fn a_new_device_never_sends_its_defaults() {
        let mut fresh = SettingsFile::default();
        let mut settings = Settings::default();
        fresh.split(&settings, 50);
        assert!(fresh.parts.iter().all(|stored| stored.updated_at == 0));
        settings.appearance.language = Some(Lang::Es);
        fresh.split(&settings, 60);
        let stamp = |part| fresh.get(part).map(|stored| stored.updated_at);
        assert_eq!(stamp(Part::Look), Some(60), "what changed travels");
        assert_eq!(
            stamp(Part::Pen),
            Some(0),
            "the rest waits for the other devices"
        );
    }

    #[test]
    fn part_ids_round_trip() {
        for part in Part::ALL {
            assert_eq!(Part::from_id(part.id()), Some(part));
        }
        assert_eq!(Part::from_id([0; 16]), None);
    }
}
