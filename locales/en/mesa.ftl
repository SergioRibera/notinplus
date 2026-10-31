## Desk: panes with tabs, the document panel and the command palette.

mesa-draft-discarded = Draft discarded
# $date: day and month the draft was saved (e.g. "29/10").
mesa-draft-name = Draft { $date }
# $name: name the draft was saved under.
mesa-draft-saved = “{ $name }” saved
mesa-failed-title = Couldn't open the desk
mesa-gone-title = This desk no longer exists
mesa-gone-body = It was deleted on this or another device.
mesa-go-home = Go home

mesa-bar-options = { -desk } options
mesa-bar-search = Search the desk
mesa-bar-back-home = Back to home
# $count: people working on this desk's documents right now.
mesa-bar-people = { $count ->
    [one] { $count } person on the desk
   *[other] { $count } people on the desk
}
mesa-document-panel = Document panel
mesa-more-actions = More actions

mesa-tab-new = New tab
mesa-tab-close = Close tab
mesa-tab-gone = Gone
mesa-tab-draft = Draft
mesa-tab-annotation = Annotation
mesa-pane-other = Other pane
mesa-pane-close = Close pane
mesa-pane-empty-title = Empty pane
mesa-pane-empty-body = Open a document here, or a draft to write something quickly.
mesa-pane-open-document = Open document

mesa-notice-this-document = This document
mesa-notice-offline = Offline. Keep working; it syncs when you're back.
# $owner: name of the person who runs the shared session.
mesa-notice-read-only = You can only view. { $owner } runs this session.
mesa-notice-trashed = “{ $name }” is in the trash
# $days: days left before it is deleted automatically.
mesa-notice-trashed-days = “{ $name }” is in the trash · { $days ->
    [one] { $days } more day
   *[other] { $days } more days
}
mesa-notice-restore = Restore
mesa-notice-gone = { $name } was deleted forever on another device.

mesa-cmd-split-pane = Split pane
mesa-cmd-draft-here = Draft in this pane
mesa-cmd-open-beside = Open beside
mesa-cmd-notebook = { -notebook }
mesa-cmd-link-with = Link with…
mesa-cmd-share = Share…
mesa-cmd-export = Export…
mesa-cmd-edit = Edit…
mesa-cmd-link-map = Link map
mesa-cmd-new-desk = New desk

mesa-palette-placeholder = Search or do something… (“/pdf” filters by type)
# $query: what the user typed.
mesa-palette-no-results = Nothing for “{ $query }”
# $kinds: plural name of a document kind (e.g. "PDFs").
mesa-palette-only-kind = Only { $kinds }
mesa-palette-actions = Actions
mesa-palette-recent = Recent
mesa-palette-documents = Documents
mesa-palette-document = Document

mesa-panel-sheet-title = Document
mesa-panel-contents = Contents
mesa-panel-bookmarks = Bookmarks
mesa-panel-links = Links
mesa-panel-search-contents = Search the contents
mesa-panel-search-bookmarks = Search bookmarks
mesa-panel-bookmarks-all = All
mesa-panel-bookmarks-empty = No bookmarks. Press and hold on the text or the page to add one.
mesa-panel-none-title = Choose a document
mesa-panel-none-body = The panel shows the document in the focused pane.

mesa-links-notebooks = Notebooks
mesa-links-notebooks-empty = No notebooks yet.
mesa-links-new-notebook = New notebook
mesa-links-linked = Linked
mesa-links-linked-empty = It isn't linked to anything.
mesa-links-mentions = Mentioned by
mesa-links-mentions-empty = No document cites it yet.
mesa-links-broken = Broken references
mesa-links-broken-row = Reference to a document that's gone
mesa-links-from-bookmark = From a bookmark
mesa-links-from-text-box = From a text box
mesa-links-from-note = From the note's text

mesa-outline-collapse = Collapse
mesa-outline-expand = Expand
mesa-outline-empty = This document has no outline.
mesa-outline-no-match = Nothing with that name.
