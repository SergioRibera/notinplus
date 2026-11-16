//! Any ink: beautifying it never panics, never yields NaN, and a word
//! correction always stays a small change.
#![no_main]

use libfuzzer_sys::fuzz_target;
use notin_beautify::{Line, Pt, Smooth, Target, correct_word, smooth_points};

/// Strokes from raw bytes: 4 bytes per point, a 0xFF x-byte splits.
fn strokes(data: &[u8]) -> Vec<Vec<Pt>> {
    let mut out = vec![Vec::new()];
    for chunk in data.chunks_exact(4) {
        if chunk[0] == 0xFF {
            out.push(Vec::new());
            continue;
        }
        let x = f32::from(u16::from_le_bytes([chunk[0], chunk[1]])) / 64.0;
        let y = f32::from(u16::from_le_bytes([chunk[2], chunk[3]])) / 64.0;
        if let Some(last) = out.last_mut() {
            last.push(Pt::new(x, y));
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

fuzz_target!(|data: &[u8]| {
    let ink = strokes(data);
    for stroke in &ink {
        for level in [Smooth::Light, Smooth::Normal] {
            let out = smooth_points(stroke, level);
            assert_eq!(out.len(), stroke.len());
            assert!(out.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        }
    }
    let refs: Vec<&[Pt]> = ink.iter().map(Vec::as_slice).collect();
    let target = Target {
        baseline: Some(Line::horizontal(300.0)),
        x_height: Some(20.0),
        slant: Some(0.1),
        after: Some((10.0, 8.0)),
    };
    for target in [Target::default(), target] {
        if let Some(placed) = correct_word(&refs, &target) {
            let a = placed.fix.affine;
            assert!(a.is_finite());
            let det = a.a.mul_add(a.d, -(a.b * a.c));
            assert!((0.7..=1.33).contains(&det), "{det}");
        }
    }
});
