## The built-in canvas and PDF plugins and the plugin host
## (src/plugins/).

plugins-canvas-name = Canvas
plugins-canvas-description = Write and draw by hand on an infinite canvas.
plugins-pdf-description = Import PDFs, write on them and search their text.
plugins-missing-title = Needs a plugin
# $kind: id of the missing plugin.
plugins-missing-body = This document opens with a plugin that isn't installed or is turned off ({ $kind }). Install it from Settings › Plugins.
plugins-navigate-bookmark-gone = That bookmark no longer exists
plugins-navigate-needs-plugin = That reference needs a plugin that isn't installed
plugins-navigate-document-gone = The reference points to a document that no longer exists
# $percent: whole number from 0 to 100.
plugins-progress-percent = { $percent }%

## Canvas kind (src/plugins/builtin/canvas.rs).
plugins-canvas-label = Canvas
plugins-canvas-plural = Canvases
plugins-canvas-search-group = Bookmarks
plugins-canvas-bookmark = Bookmark
# $name: the canvas a bookmark search hit is on.
plugins-canvas-hit-snippet = Canvas · { $name }
plugins-canvas-layer-hidden = hidden
plugins-canvas-outline-layers = Layers
plugins-canvas-summary = { $count ->
    [one] { $count } bookmark
   *[other] { $count } bookmarks
}
# $name: the canvas the bookmark is on.
plugins-canvas-chip-bookmark = { $name } · bookmark
plugins-canvas-entry-canvas = Write by hand
plugins-canvas-scratch = Draft
plugins-canvas-entry-scratch = Discarded on close unless you save it
plugins-canvas-notebook = { -notebook }
plugins-canvas-entry-notebook = Beside a document
plugins-canvas-ink-layer = Ink

## PDF kind (src/plugins/builtin/pdf.rs).
plugins-pdf-unreadable = It may be damaged or password protected. Try opening it in another app and saving it again.
plugins-pdf-default-name = PDF document
plugins-pdf-search-group = In PDFs
# $page: page number, starting at 1.
plugins-pdf-page-short = p. { $page }
plugins-pdf-pages = { $count ->
    [one] { $count } page
   *[other] { $count } pages
}
plugins-pdf-chip-excerpt = PDF p. { $page } · excerpt
# $name: document name; $page: page number.
plugins-pdf-chip-page = { $name } p. { $page }
plugins-pdf-entry-subtitle = Import and write on it
plugins-pdf-pick-title = Choose a PDF
