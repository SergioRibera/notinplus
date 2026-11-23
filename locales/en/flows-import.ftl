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
