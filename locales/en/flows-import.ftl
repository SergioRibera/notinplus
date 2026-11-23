## Files brought in several at a time (dropped, opened with the app) and
## folders dropped on the library.

# $count: documents imported in this run.
flows-import-queue-imported = { $count ->
    [one] 1 document imported
   *[other] { $count } documents imported
}
# $names: comma-separated file names.
flows-import-queue-refused = { $count ->
    [one] Couldn't import: { $names }
   *[other] Couldn't import { $count } files: { $names }
}
# $name: file name.
flows-import-queue-plugin = “{ $name }” is a plugin. To install it, go to Settings, Plugins.
flows-import-drop-library = Drop to import into “{ $folder }”
flows-import-drop-here = Drop to import
flows-import-drop-nowhere = Files can't be dropped here. Go to the library.

# $name: the dropped folder.
flows-import-tree-title = Import “{ $name }”
flows-import-tree-summary = { $documents ->
    [one] 1 document
   *[other] { $documents } documents
} in { $folders ->
    [one] 1 folder
   *[other] { $folders } folders
}
flows-import-tree-skipped = { $count ->
    [one] 1 file will be skipped: no plugin imports it
   *[other] { $count } files will be skipped: no plugin imports them
}
flows-import-tree-skipped-more = { $names } and { $more } more
flows-import-tree-truncated = Only the first { $max } documents and 16 folder levels are taken.
flows-import-tree-submit = { $count ->
    [one] Import 1
   *[other] Import { $count }
}
flows-import-tree-empty = “{ $name }” holds no document that can be imported.
flows-import-tree-importing = Importing documents
flows-import-tree-progress = { $done } of { $total }

## A file dropped on an open document (a note).

# $document: the document's name.
flows-add-drop = Drop to add to “{ $document }”
# $name: the dropped file.
flows-add-title = Add “{ $name }”
flows-add-title-queued = Add “{ $name }” ({ $position } of { $total })
flows-add-subtitle = To “{ $document }”
flows-add-submit = Insert
flows-add-reference = Reference
flows-add-reference-detail = Imports it and puts a link that opens it.
flows-add-link = Link
flows-add-link-detail = Imports it and links it to this document, without touching the text.
flows-add-text = Insert the text
flows-add-text-detail = Puts its whole text here.
flows-add-text-pages-detail = Puts the text of the pages you choose here.
flows-add-image = Insert as image
flows-add-image-detail = Puts the pages you choose here as pictures.
flows-add-pages = Pages
# $count: pages of the PDF.
flows-add-pages-field = Pages (of { $count })
flows-add-pages-placeholder = 1-3, 7
flows-add-pages-hint = Up to 50 pages as text, 20 as images.
flows-add-keep-file = Also keep the PDF in the library
flows-add-keep-file-on = A link under what you insert opens it.
flows-add-keep-file-off = Without the PDF, what you insert won't link to its source.
# $name: what was added.
flows-add-done = “{ $name }” added
flows-add-linked = “{ $name }” imported and linked
flows-add-failed = Couldn't add “{ $name }”
flows-add-closed = The document was closed.
flows-add-unreadable = The PDF couldn't be read.
flows-add-no-text = Those pages have no text to copy. Try inserting them as images.
