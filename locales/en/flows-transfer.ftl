## Export and import of copies (src/flows/transfer.rs).

flows-transfer-export-title = Export
flows-transfer-whole-library = The whole library
flows-transfer-documents = { $count ->
    [one] 1 document
   *[other] { $count } documents
}
flows-transfer-desks = { $count ->
    [one] 1 desk
   *[other] { $count } desks
}
flows-transfer-bookmarks = { $count ->
    [one] 1 bookmark
   *[other] { $count } bookmarks
}
# $size: total size, already formatted.
flows-transfer-contents = { $count ->
    [one] 1 document · { $size }
   *[other] { $count } documents · { $size }
}
flows-transfer-include-pdfs = Include the original PDFs
# $size: size of the PDFs, already formatted.
flows-transfer-include-pdfs-hint = { $size }. Without them, the copy keeps only what's written on top.
flows-transfer-include-desks = Include desks and bookmarks
flows-transfer-include-desks-hint = { $count ->
    [one] 1 desk and every highlight.
   *[other] { $count } desks and every highlight.
}
flows-transfer-purpose = What it's for
flows-transfer-backup = Backup
flows-transfer-backup-hint = Everything as it is: to import it again into Notein, here or on another device.
flows-transfer-readable = Read in other apps
flows-transfer-readable-hint = PDFs with what's written on top and notes in Markdown, in a folder.
flows-transfer-what = What
flows-transfer-export = Export
flows-transfer-export-size = Export { $size }
# $name: file name.
flows-transfer-saved-as = Saved as { $name }
flows-transfer-progress = { $total ->
    [one] { $done } of 1 document
   *[other] { $done } of { $total } documents
}
flows-transfer-progress-hint = You can keep using the app. We'll let you know when it's done.
flows-transfer-exporting = Exporting
flows-transfer-export-failed = Couldn't export
flows-transfer-save-copy = Save copy
flows-transfer-save-failed = Couldn't save the copy
flows-transfer-saved-subtitle = { $name } · { $size }
flows-transfer-show-in-folder = Show in folder
flows-transfer-saved = Copy saved
flows-transfer-no-space = It doesn't fit here
# $missing: size, already formatted.
flows-transfer-no-space-hint = It needs { $missing }. Choose another place or export without the original PDFs.
flows-transfer-without-pdfs = Without original PDFs
flows-transfer-pick-place = Choose another place
flows-transfer-import-picker = Import a Notein copy
flows-transfer-reviewing = Reviewing “{ $name }”…
flows-transfer-newer = This file can't be opened
flows-transfer-newer-hint = It was made by a newer version of Notein. Update the app and try again.
flows-transfer-pick-another = Choose another
flows-transfer-check-update = Check for updates
flows-transfer-damaged = “{ $name }” can't be opened
flows-transfer-damaged-hint = This file is incomplete or damaged.
flows-transfer-import-title = Import “{ $name }”
flows-transfer-keep-both = Keep both
flows-transfer-replace = Replace
flows-transfer-skip = Skip
# Stored as the name of the folder an import lands in.
flows-transfer-imported-folder = Imported
# $source: device the copy came from.
flows-transfer-imported-folder-from = Imported from { $source }
flows-transfer-missing-plugins = { $count ->
    [one] 1 document uses plugins that aren't installed. It's imported anyway and opens once you install them.
   *[other] { $count } documents use plugins that aren't installed. They're imported anyway and open once you install them.
}
flows-transfer-conflicts = { $count ->
    [one] 1 is already in your library
   *[other] { $count } are already in your library
}
flows-transfer-for-all = For all:
flows-transfer-identical = Identical · skipped automatically
# $mine and $theirs: edit dates; $newer is "yes" when the copy's is newer.
flows-transfer-conflict = { $newer ->
    [yes] Yours: { $mine } · The copy's: { $theirs } (newer)
   *[no] Yours: { $mine } · The copy's: { $theirs }
}
flows-transfer-new = { $count ->
    [one] 1 new
   *[other] { $count } new
}
flows-transfer-new-hint = Added as they are
flows-transfer-save-in = Save in
# $folder: destination folder name.
flows-transfer-destination = Library › { $folder }
flows-transfer-import-count = Import { $count }
flows-transfer-skipped = { $count ->
    [one] 1 skipped for being identical
   *[other] { $count } skipped for being identical
}
flows-transfer-imported = { $count ->
    [one] 1 document imported
   *[other] { $count } documents imported
}
flows-transfer-view = View
flows-transfer-import-failed = Couldn't import
