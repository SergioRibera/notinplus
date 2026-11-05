## "Export as…" sheet (src/flows/export_as.rs) and the options the
## built-in kinds declare (src/plugins/builtin/canvas.rs, pdf.rs).

export-title = Export as
export-name = File name
export-save = Save…
export-working = Exporting…
export-failed = Could not export
export-nothing = Nothing to export
export-unsupported = This document cannot be exported

export-format = Format
export-format-pdf = PDF
export-format-png = PNG
export-format-jpg = JPG

export-background = Background
export-background-paper = Paper
export-background-none = None
export-scale = Size
export-scale-1 = 1×
export-scale-2 = 2×
export-scale-3 = 3×
export-canvas-text-hint = Text boxes are not drawn yet.

export-pages = Pages
# $count: pages in the document.
export-pages-help = All, or e.g. 1-3, 7 (1–{ $count })
# $page: page asked for; $count: pages in the document.
export-pages-out-of-range = Page { $page } does not exist (1–{ $count })
export-pages-unreadable = Could not read the pages: use numbers and ranges like 1-3, 7
export-content = Content
export-content-annotated = With my notes
export-content-original = Original
export-dpi = Resolution
export-dpi-150 = 150 dpi
export-dpi-300 = 300 dpi
export-files = Files
export-files-single = One image
export-files-separate = One per page
