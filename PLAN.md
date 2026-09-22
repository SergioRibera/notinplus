# notinplus — Brush Engine Plan

Procreate-quality brush engine, split into PR-sized phases. Each phase
compiles + ships; nothing is a big-bang rewrite.

## Constraints (non-negotiable)

- **Strokes must serialize compactly.** `.notinplus` document holds every
  stroke as vector data — resolution-independent re-render at any zoom.
  Target: ~7 bytes/point quantized, so a 500-point stroke is ~3.5 KB and
  a 100-stroke doc is <400 KB.
- **Eraser removes strokes**, does not paint transparent over them. Real
  vector erase: hit-test against existing polylines, split segments,
  drop the parts inside the eraser radius. Undo restores original.
- **End goal = Procreate parity.** Dab-based renderer (textured tip
  stamps along smoothed path) with per-dab dynamics driven by pressure,
  velocity, tilt, and direction. Not line-drawing.

## Serialization shape (target)

```rust
#[istmo::message]
pub struct Doc {
    pub brushes: Vec<BrushPreset>,   // shared across strokes
    pub strokes: Vec<Stroke>,
}

#[istmo::message]
pub struct Stroke {
    pub brush: BrushId,              // u16 index into Doc.brushes
    pub color: [u8; 4],              // RGBA, overrides preset color
    pub points: Vec<InkPoint>,
}

#[istmo::message]
pub struct InkPoint {
    pub x: f32,                      // could quantize to i32 sub-pixel*8 if needed
    pub y: f32,
    pub pressure: u8,                // 0..255 (was f32 in 0..1)
    pub tilt: u8,                    // 0..255
    pub dt_us: u16,                  // delta µs from previous point in stroke
}
```

Load/save via bincode. Doc format lives under `docs/format.md` once phase
1 lands. Human-readable JSON export as a debug/interchange path.

## Eraser (vector erase)

- Every `PenEvent::Move` for an eraser brush: hit-test each committed
  stroke against the eraser circle at that sample (center + radius).
- On intersection: split the stroke's polyline into 0..N sub-strokes,
  cutting where the polyline enters/exits the circle. Drop the segments
  inside.
- Use a **uniform spatial grid** (128 dp buckets) to prune candidate
  strokes so hit-test isn't O(strokes × samples).
- Undo restores the original stroke (grid re-insert).
- Eraser stops using `BlendMode::Clear` — that path is gone.

## Procreate-style brush engine

- **Dab-based.** Each stroke = train of textured stamps ("dabs") placed
  along the smoothed path at `spacing × dab_diameter`.
- **Path smoothing.** Catmull-Rom (or Chaikin ×2) over raw samples so the
  dab train follows a clean curve.
- **Per-dab params:**
  - `size` — `pressure` mapped through a per-brush curve.
  - `opacity` (flow) — `pressure` / `velocity` curve.
  - `rotation` — combines `tilt` direction + stroke direction + jitter.
  - `scatter` — random offset perpendicular to path.
  - `color` — base color with per-dab HSV jitter.
- **Textures.** Tip PNG (dab shape/silhouette) + grain PNG (pigment
  micro-texture) sampled via skia `ImageShader`. Ship a base set.
- **Presets.** `BrushPreset` bundles tip + grain + dynamics curves +
  spacing/flow/size params. Persist in `Doc.brushes`.

## Phases

Each phase = one session, one PR. Land in order.

### Phase 1 — Serialization + brush registry

- Redefine `Stroke`, `InkPoint`, `Brush` as `#[istmo::message]` types.
- Move brush enum → `BrushPreset` struct + `BrushId(u16)` referring to
  a per-`Doc` registry.
- `Doc::save(path)` / `Doc::load(path)` via bincode.
- No visual change; renderer still uses today's `Brush::plan` per
  segment. Provides the foundation everything else builds on.

### Phase 2 — SKIPPED (ribbon renderer)

Chaikin + variable-width mesh ribbon (like `pen-demo/src/app.rs:453`).
**Skip.** Dab renderer in phase 4 gets the same "clean monoline" look
with a small-spacing round dab and it's what we need for phases 5+
anyway. Ribbon would be thrown away.

### Phase 3 — Eraser vectorial

- Add spatial grid (`SpatialIndex { buckets: HashMap<(i32,i32), Vec<StrokeId>> }`).
- Populate on `Board::end()` / mutate on erase-split.
- Eraser handler: for each move sample, query buckets around eraser
  circle, split intersecting strokes at circle boundary.
- Remove `BlendMode::Erase` path from `canvas.rs::configure_paint`.
- Undo stack entry per erase = list of `(StrokeId, original_stroke)`.

### Phase 4 — Dab renderer

- Replace `paint_segment` line drawing with a dab loop.
- Smooth path (Catmull-Rom, 2 subdivisions).
- Walk path at `spacing × dab_diameter`, stamp round filled circles
  with per-dab size/opacity from pressure/velocity curves.
- No textures yet — result looks like Procreate "monoline" brush.
- Brush presets get `spacing: f32`, `size_curve: Curve`, `flow_curve: Curve`.

### Phase 5 — Textured dabs + grain

- Load tip + grain PNGs at startup (bundled in `assets/brushes/`).
- Skia `ImageShader` with `TileMode::Repeat` for grain.
- Rotate tip per dab (tilt + stroke direction).
- Ship real presets: Procreate 6B Pencil, Studio Pen, Marker, etc.
  Rough parity, not identical curves.

### Phase 6 — Dynamics + jitter

- Editable curves: pressure → size, pressure → opacity, velocity → size,
  velocity → opacity, tilt → size, tilt → rotation.
- Per-dab jitter: size ±%, opacity ±%, hue ±°, saturation ±%, position
  scatter ±px, rotation ±°.
- Brush editor UI (later).

## Order (recommended)

1 → 3 → 4 → 5 → 6. Serialization first (safety net + doc format), then
eraser (fixes the misleading `BlendMode::Clear`), then dabs (the visible
Procreate feel), then textures (polish), then dynamics (breadth).

## Open questions (park until relevant)

- Coordinate quantization: `f32` vs `i32 sub-pixel × 8`. Bench when doc
  sizes matter.
- Predictive input on Android (`event.historySize` already coalesces; do
  we surface `predicted` from `PenMove.predicted` for the head of the
  stroke?).
- Layer support. Procreate is layered; today we're single-layer. Adding
  layers means `Doc.layers: Vec<Layer>` and per-layer stroke ownership.
  Defer until multi-layer becomes a real requirement.
- GPU tessellation. Skia already GPU-accelerated; per-dab batching may
  need `Canvas::save_layer` per stroke to avoid overdraw bleed with
  low-opacity flow. Measure before optimizing.

## Files touched (per phase, approximate)

- Phase 1: `src/brush.rs`, `src/canvas.rs` (Stroke type), `src/lib.rs`
  (Doc type + save/load), new `src/doc.rs`.
- Phase 3: `src/canvas.rs` (grid + split), new `src/spatial.rs`, new
  `src/history.rs` for undo entries.
- Phase 4: `src/brush.rs` (BrushPreset), `src/canvas.rs` (dab renderer),
  new `src/render/dab.rs`.
- Phase 5: `src/render/dab.rs` (textures), `assets/brushes/*.png`.
- Phase 6: `src/brush.rs` (curves + jitter), new `src/render/curve.rs`.
