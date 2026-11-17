//! Debug recording of pen ink, to tune beautification on real devices.
//!
//! With `NOTIN_RECORD_INK` naming a folder, every stroke drawn is added
//! to `<folder>/<label>-<unix seconds>.ink` (label from
//! `NOTIN_RECORD_LABEL`, default `session`). The files are
//! [`InkRecording`]s, read by the `notin-beautify` fixture tests.

use std::path::PathBuf;

use notin_beautify::{InkRecording, InkSample};

use super::Board;
use crate::brush::Stroke;

/// Where and what is being recorded.
#[derive(Debug)]
pub(super) struct Recorder {
    path: PathBuf,
    recording: InkRecording,
}

impl Recorder {
    /// A recorder when `NOTIN_RECORD_INK` asks for one.
    pub(super) fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("NOTIN_RECORD_INK")?);
        let label = std::env::var("NOTIN_RECORD_LABEL").unwrap_or_else(|_| "session".into());
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Some(Self {
            path: dir.join(format!("{label}-{stamp}.ink")),
            recording: InkRecording {
                label,
                strokes: Vec::new(),
            },
        })
    }
}

impl Board {
    /// Add `stroke`, as drawn, to the recording (if one runs) and save it.
    pub(super) fn record_stroke(&mut self, stroke: &Stroke) {
        let Some(recorder) = self.recorder.as_mut() else {
            return;
        };
        recorder.recording.strokes.push(
            stroke
                .points
                .iter()
                .map(|p| InkSample {
                    x: p.x,
                    y: p.y,
                    pressure: p.pressure,
                    dt_us: p.dt_us,
                })
                .collect(),
        );
        let saved = recorder
            .recording
            .to_bytes()
            .map_err(|e| e.to_string())
            .and_then(|bytes| std::fs::write(&recorder.path, bytes).map_err(|e| e.to_string()));
        if let Err(err) = saved {
            tracing::warn!("ink recording {}: {err}", recorder.path.display());
        }
    }
}
