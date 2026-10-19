# Handwriting data attribution

Baked by `tools/glyph-bake`; regenerate with:

```sh
cargo run --release -p glyph-bake -- templates <ujipenchars2.txt>
cargo run --release -p glyph-bake -- fonts assets/fonts/*.ttf
```

## `templates.rkyv`

Derived from **UJI Pen Characters (Version 2)** by F. Prat, M. J. Castro,
D. Llorens, A. Marzal and J. M. Vilar, UCI Machine Learning Repository,
<https://doi.org/10.24432/C5FG8S>. Licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Changes:
samples resampled, normalized, accents split from their letters and
reduced to representative medoids.

## `fonts/*.rkyv`

Glyph outlines extracted from the fonts in `assets/fonts/`, all under the
SIL Open Font License 1.1:

- Caveat — Impallari Type
- Excalifont — Excalidraw
- Gochi Hand — Huerta Tipográfica
- Handlee — Joe Prince
