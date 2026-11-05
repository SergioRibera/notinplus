## Small pieces of the shell with no file of their own.

## Following someone (src/follow.rs).
# $name: the person being followed.
misc-follow-following = Following { $name }
misc-follow-stop = Stop following

## The bar over a text selection (src/selection.rs).
misc-selection-note = Note
misc-selection-to-notebook = To notebook

## Selecting text on a PDF page (src/pdf_select_overlay.rs).
# $page: page number, starting at 1.
misc-pdf-page = Page { $page }
misc-pdf-bookmark = Bookmark

## The readable export zip (src/readable.rs).
misc-export-bookmarks-heading = Bookmarks
# $name: path of the exported document, without extension. Keep ".md".
misc-export-bookmarks-file = { $name } — bookmarks.md
