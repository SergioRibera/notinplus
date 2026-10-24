//! Which four creation entries get the big tiles.
//!
//! Every finished creation is logged on this device. Each use weighs
//! half as much every fourteen days and uses older than ninety days
//! are dropped. The tiles change only when the app opens: an entry
//! takes the weakest tile's place when it scores 25 % more, and the
//! other tiles stay where they are.

use crate::persist::Persisted;
use crate::settings::EntryKey;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const WINDOW_MS: u64 = 90 * DAY_MS;
const HALF_LIFE_DAYS: f64 = 14.0;
/// A challenger must beat the weakest tile by this factor.
const REPLACE_FACTOR: f64 = 1.25;
/// Fewer creations than this keeps the starting tiles.
const COLD_START_USES: usize = 5;
/// Number of big tiles.
pub const TILES: usize = 4;

/// One finished creation.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Use {
    pub key: EntryKey,
    /// Unix millis.
    pub at: u64,
}

/// Device-local creation history plus the tiles chosen at the last
/// app start.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UsageLog {
    pub uses: Vec<Use>,
    pub tiles: Vec<EntryKey>,
}

impl Persisted for UsageLog {
    const FILE: &'static str = "creation-usage.v2.bin";
}

impl UsageLog {
    /// Log a finished creation.
    pub fn record(&mut self, key: EntryKey, now: u64) {
        self.uses.push(Use { key, at: now });
        self.prune(now);
    }

    /// Drop uses outside the window.
    pub fn prune(&mut self, now: u64) {
        self.uses.retain(|u| now.saturating_sub(u.at) <= WINDOW_MS);
    }

    /// Whether the menu still shows the starting tiles.
    #[must_use]
    pub fn is_cold(&self) -> bool {
        self.uses.len() < COLD_START_USES
    }

    /// Decayed score of `key` at `now`.
    #[must_use]
    pub fn score(&self, key: &EntryKey, now: u64) -> f64 {
        self.uses
            .iter()
            .filter(|u| &u.key == key)
            .map(|u| {
                #[allow(clippy::cast_precision_loss)] // ages fit well inside f64's exact range
                let age_days = now.saturating_sub(u.at) as f64 / DAY_MS as f64;
                0.5f64.powf(age_days / HALF_LIFE_DAYS)
            })
            .sum()
    }

    fn last_use(&self, key: &EntryKey) -> u64 {
        self.uses
            .iter()
            .filter(|u| &u.key == key)
            .map(|u| u.at)
            .max()
            .unwrap_or(0)
    }

    /// Choose the tiles for this session and remember them.
    ///
    /// `available` lists the entries that compete, in starting order
    /// (the order used on a cold start and to break ties).
    pub fn choose_tiles(&mut self, available: &[EntryKey], now: u64) -> Vec<EntryKey> {
        self.prune(now);
        let tiles = if self.is_cold() {
            available.iter().take(TILES).cloned().collect()
        } else {
            self.rank(available, now)
        };
        self.tiles.clone_from(&tiles);
        tiles
    }

    fn rank(&self, available: &[EntryKey], now: u64) -> Vec<EntryKey> {
        let order = |key: &EntryKey| {
            available
                .iter()
                .position(|k| k == key)
                .unwrap_or(usize::MAX)
        };
        let better = |a: &EntryKey, b: &EntryKey| {
            self.score(b, now)
                .total_cmp(&self.score(a, now))
                .then_with(|| self.last_use(b).cmp(&self.last_use(a)))
                .then_with(|| order(a).cmp(&order(b)))
        };
        let mut tiles: Vec<EntryKey> = self
            .tiles
            .iter()
            .filter(|k| available.contains(k))
            .take(TILES)
            .cloned()
            .collect();
        let mut challengers: Vec<EntryKey> = available
            .iter()
            .filter(|k| !tiles.contains(k))
            .cloned()
            .collect();
        challengers.sort_by(better);
        let mut challengers = challengers.into_iter();
        while tiles.len() < TILES {
            match challengers.next() {
                Some(next) => tiles.push(next),
                None => break,
            }
        }
        for challenger in challengers {
            let Some((weakest, weakest_score)) = tiles
                .iter()
                .enumerate()
                .map(|(i, k)| (i, self.score(k, now)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
            else {
                break;
            };
            if self.score(&challenger, now) > weakest_score * REPLACE_FACTOR {
                tiles[weakest] = challenger;
            }
        }
        tiles
    }
}

#[cfg(test)]
mod tests {
    use super::{DAY_MS, UsageLog};
    use crate::settings::EntryKey;

    fn key(name: &str) -> EntryKey {
        EntryKey::new("p", name)
    }

    fn defaults() -> Vec<EntryKey> {
        ["canvas", "note", "pdf", "bible", "notebook", "markdown"]
            .into_iter()
            .map(key)
            .collect()
    }

    #[test]
    fn cold_start_uses_starting_order() {
        let mut log = UsageLog::default();
        log.record(key("pdf"), 0);
        assert_eq!(
            log.choose_tiles(&defaults(), DAY_MS),
            defaults()[..4].to_vec()
        );
    }

    #[test]
    fn heavy_use_replaces_only_the_weakest_tile() {
        let now = 60 * DAY_MS;
        let mut log = UsageLog {
            tiles: defaults()[..4].to_vec(),
            ..UsageLog::default()
        };
        for day in 50..60 {
            log.record(key("notebook"), day * DAY_MS);
        }
        log.record(key("canvas"), 59 * DAY_MS);
        log.record(key("note"), 59 * DAY_MS);
        log.record(key("pdf"), 59 * DAY_MS);
        let tiles = log.choose_tiles(&defaults(), now);
        assert_eq!(tiles[0], key("canvas"));
        assert_eq!(tiles[1], key("note"));
        assert_eq!(tiles[2], key("pdf"));
        assert_eq!(tiles[3], key("notebook"));
    }

    #[test]
    fn a_small_lead_does_not_move_tiles() {
        let now = 80 * DAY_MS;
        let mut log = UsageLog {
            tiles: defaults()[..4].to_vec(),
            ..UsageLog::default()
        };
        for k in ["canvas", "note", "pdf", "bible"] {
            log.record(key(k), now);
        }
        log.record(key("markdown"), now);
        log.record(key("markdown"), now - 60 * DAY_MS);
        let before = log.tiles.clone();
        let tiles = log.choose_tiles(&defaults(), now);
        assert_eq!(tiles, before, "1.05 does not beat 1.0 by a quarter");
    }

    #[test]
    fn uninstalled_entry_gives_its_slot_to_the_next_best() {
        let now = 30 * DAY_MS;
        let mut log = UsageLog {
            tiles: defaults()[..4].to_vec(),
            ..UsageLog::default()
        };
        for k in ["canvas", "note", "pdf", "markdown", "markdown"] {
            log.record(key(k), now);
        }
        let available: Vec<EntryKey> = defaults()
            .into_iter()
            .filter(|k| k != &key("bible"))
            .collect();
        let tiles = log.choose_tiles(&available, now);
        assert_eq!(tiles.len(), 4);
        assert_eq!(tiles[3], key("markdown"));
    }

    #[test]
    fn old_uses_fade_out_of_the_window() {
        let mut log = UsageLog::default();
        log.record(key("pdf"), 0);
        log.prune(91 * DAY_MS);
        assert!(log.uses.is_empty());
    }
}
