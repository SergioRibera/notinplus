//! Recorded ink (`tests/ink/*.ink`, written by the canvas when
//! `NOTIN_RECORD_INK` names a folder): beautifying real handwriting and
//! figures stays sane. A recording whose label starts with `figure:`
//! must snap when held; one starting with `scribble:` must not.

use std::path::Path;

use notin_beautify::{Context, InkRecording, Pt, beautify_all, fit_shape};

fn recordings() -> Vec<(String, InkRecording)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ink");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, InkRecording)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ink"))
        .map(|p| {
            let bytes = std::fs::read(&p).expect("readable fixture");
            let name = p.display().to_string();
            (
                name,
                InkRecording::from_bytes(&bytes).expect("valid fixture"),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn recorded_ink_beautifies_sanely() {
    for (name, recording) in recordings() {
        let ink = recording.points();
        let refs: Vec<&[Pt]> = ink.iter().map(Vec::as_slice).collect();
        for outcome in beautify_all(&refs, &Context::default()) {
            if let notin_beautify::Outcome::Word { fix, .. } = outcome {
                assert!(fix.affine.is_finite(), "{name}");
            }
        }
        for stroke in &refs {
            let held = fit_shape(stroke, false);
            if recording.label.starts_with("figure:") && refs.len() == 1 {
                assert!(held.is_some(), "{name}: {} did not snap", recording.label);
            }
            if recording.label.starts_with("scribble:") {
                assert!(
                    held.is_none(),
                    "{name}: {} snapped to {held:?}",
                    recording.label
                );
            }
        }
    }
}
