## Library actions: pin, duplicate, share, trash and desks (src/flows.rs).

flows-pinned-documents = { $count ->
    [one] Pinned 1 document
   *[other] Pinned { $count } documents
}
flows-duplicated = Duplicated
flows-duplicate-failed = Couldn't duplicate
flows-copy-failed = Couldn't copy
flows-share-failed = Couldn't share
flows-undo = Undo
flows-trash-failed = Couldn't move to the trash
# $name: document or folder name.
flows-trashed-named = “{ $name }” moved to the trash
flows-trashed-documents = { $count ->
    [one] 1 document moved to the trash
   *[other] { $count } documents moved to the trash
}
# $name: folder name; $count: documents inside it.
flows-trashed-folder = { $count ->
    [one] “{ $name }” and 1 document moved to the trash
   *[other] “{ $name }” and { $count } documents moved to the trash
}
flows-trash-title-named = Move “{ $name }” to the trash
flows-trash-title-documents = { $count ->
    [one] Move 1 document to the trash
   *[other] Move { $count } documents to the trash
}
# $count: notebooks attached to the documents being trashed.
flows-trash-notebooks = { $count ->
    [one] It has 1 notebook with your notes.
   *[other] They have { $count } notebooks with your notes.
}
flows-fate-keep = Keep the notebooks
flows-fate-keep-hint = They stay in the library, unlinked.
flows-fate-trash = Move them to the trash too
flows-fate-trash-hint = They can be restored together.
# $title: desk name.
flows-delete-desk-title = Delete the desk “{ $title }”?
flows-delete-desk-hint = The documents stay in the library. Only this group of tabs closes.
flows-delete-desk = Delete desk
