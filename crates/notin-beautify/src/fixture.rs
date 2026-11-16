//! Recorded ink: strokes as a pen produced them, saved so thresholds can
//! be tuned and checked against real handwriting and figures from
//! tablets and desktop pens.

use wincode::{SchemaRead, SchemaWrite};

use crate::Pt;

/// One pen sample.
#[derive(Clone, Copy, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct InkSample {
    pub x: f32,
    pub y: f32,
    pub pressure: u8,
    /// Time since the previous sample, microseconds.
    pub dt_us: u16,
}

/// A session of recorded strokes, in the order they were drawn.
#[derive(Clone, Debug, Default, PartialEq, SchemaWrite, SchemaRead)]
pub struct InkRecording {
    /// What was written or drawn, for whoever reads the fixture.
    pub label: String,
    pub strokes: Vec<Vec<InkSample>>,
}

/// A recording that could not be read.
#[derive(Debug)]
pub struct RecordingError(String);

impl std::fmt::Display for RecordingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unreadable ink recording: {}", self.0)
    }
}

impl std::error::Error for RecordingError {}

impl InkRecording {
    /// The recording's bytes.
    ///
    /// # Errors
    /// Encoding failures (none expected for in-memory data).
    pub fn to_bytes(&self) -> Result<Vec<u8>, RecordingError> {
        wincode::serialize(self).map_err(|e| RecordingError(e.to_string()))
    }

    /// Read a recording.
    ///
    /// # Errors
    /// Bytes that are not a recording.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RecordingError> {
        wincode::deserialize(bytes).map_err(|e| RecordingError(e.to_string()))
    }

    /// Positions of every stroke.
    #[must_use]
    pub fn points(&self) -> Vec<Vec<Pt>> {
        self.strokes
            .iter()
            .map(|s| s.iter().map(|p| Pt::new(p.x, p.y)).collect())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let recording = InkRecording {
            label: "a line".into(),
            strokes: vec![vec![
                InkSample {
                    x: 1.0,
                    y: 2.0,
                    pressure: 90,
                    dt_us: 0,
                },
                InkSample {
                    x: 3.0,
                    y: 4.0,
                    pressure: 120,
                    dt_us: 8_000,
                },
            ]],
        };
        let bytes = recording.to_bytes().unwrap();
        assert_eq!(InkRecording::from_bytes(&bytes).unwrap(), recording);
        assert!(InkRecording::from_bytes(&bytes[..3]).is_err());
    }
}
