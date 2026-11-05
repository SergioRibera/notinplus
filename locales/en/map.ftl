## Link map: every document and how they connect.

map-title = Map
map-back = Back
map-summary = { $documents ->
    [one] { $documents } document
   *[other] { $documents } documents
} · { $connections ->
    [one] { $connections } connection
   *[other] { $connections } connections
}
# $kind: the document's kind (e.g. "Canvas"); $count: its connections.
map-node-meta = { $kind } · { $count ->
    [one] { $count } connection
   *[other] { $count } connections
}
map-node-document = Document
map-open-beside = Beside
map-link-with = Link with…
map-empty-title = Nothing to show yet
map-empty-body = When you create documents and link them, you'll see here how they connect.
map-edge-notebook = Notebook
map-edge-link = Link
map-edge-reference = Reference
