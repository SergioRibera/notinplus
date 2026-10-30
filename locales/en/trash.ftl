## Trash: deleted documents grouped by the action that deleted them.

# $first: name of the first document; $more: how many others came with it.
trash-group-title = { $first } and { $more } more
trash-restored = { $count ->
    [one] { $count } document back in the library
   *[other] { $count } documents back in the library
}
trash-purged = Deleted forever
trash-empty-title = The trash is empty
trash-empty-body = What you delete stays here for a while, in case you change your mind.
# $size: total size, already formatted (e.g. "12.4 MB").
trash-summary-kept = { $count ->
    [one] { $count } document
   *[other] { $count } documents
} · { $size } · kept until you delete them
trash-summary-days = { $count ->
    [one] { $count } document
   *[other] { $count } documents
} · { $size } · deleted automatically after { $days } days
trash-back = Back
trash-select = Select
# $ago: when it was deleted (e.g. "5 min ago"); $from: folder it was in.
trash-row-meta = Deleted { $ago } · from { $from }
# $days: days left before it is deleted automatically.
trash-row-meta-left = Deleted { $ago } · from { $from } · { $days ->
    [one] { $days } more day
   *[other] { $days } more days
}
trash-restore = Restore
trash-empty-all = Empty trash
trash-empty-all-warning = This can't be undone. They're also deleted on your other devices.
trash-purge-count = { $count ->
    [one] Delete { $count } document forever
   *[other] Delete { $count } documents forever
}
trash-chosen-count = { $count } chosen
trash-purge = Delete forever
