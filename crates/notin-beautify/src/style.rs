//! The writer's own style: typical slant and size, learned from the last
//! words written. Steers the slant correction (towards the writer's own
//! lean, never to upright) and sizes the first word of an empty line.

use wincode::{SchemaRead, SchemaWrite};

use crate::metrics::median;

/// Words remembered.
const WINDOW: usize = 200;

/// Rolling record of measured words.
#[derive(Clone, Debug, Default, PartialEq, SchemaWrite, SchemaRead)]
pub struct StyleProfile {
    slants: Vec<f32>,
    x_heights: Vec<f32>,
}

impl StyleProfile {
    /// Remember a word that measured `slant` (radians) and `x_height`.
    pub fn record(&mut self, slant: f32, x_height: f32) {
        if !slant.is_finite() || !x_height.is_finite() || x_height <= 0.0 {
            return;
        }
        for (values, value) in [(&mut self.slants, slant), (&mut self.x_heights, x_height)] {
            values.push(value);
            if values.len() > WINDOW {
                let excess = values.len() - WINDOW;
                values.drain(..excess);
            }
        }
    }

    /// Median slant, once a few words are known.
    #[must_use]
    pub fn slant(&self) -> Option<f32> {
        self.median_of(&self.slants)
    }

    /// Median x-height, once a few words are known.
    #[must_use]
    pub fn x_height(&self) -> Option<f32> {
        self.median_of(&self.x_heights)
    }

    #[must_use]
    pub const fn words(&self) -> usize {
        self.slants.len()
    }

    fn median_of(&self, values: &[f32]) -> Option<f32> {
        (self.words() >= 5)
            .then(|| median(&mut values.to_vec()))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keeps_the_latest_words() {
        let mut profile = StyleProfile::default();
        assert_eq!(profile.slant(), None);
        for _ in 0..WINDOW {
            profile.record(0.1, 10.0);
        }
        for _ in 0..WINDOW {
            profile.record(0.3, 20.0);
        }
        assert_eq!(profile.words(), WINDOW);
        assert_eq!(profile.slant(), Some(0.3));
        assert_eq!(profile.x_height(), Some(20.0));
        profile.record(f32::NAN, 3.0);
        assert_eq!(profile.words(), WINDOW);
    }
}
