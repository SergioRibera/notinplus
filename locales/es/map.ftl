## Mapa de vínculos: todos los documentos y cómo se conectan.

map-title = Mapa
map-back = Volver
map-summary = { $documents ->
    [one] { $documents } documento
   *[other] { $documents } documentos
} · { $connections ->
    [one] { $connections } conexión
   *[other] { $connections } conexiones
}
# $kind: el tipo del documento (p. ej. "Lienzo"); $count: sus conexiones.
map-node-meta = { $kind } · { $count ->
    [one] { $count } conexión
   *[other] { $count } conexiones
}
map-node-document = Documento
map-open-beside = Al lado
map-link-with = Vincular con…
map-empty-title = Todavía no hay nada que mostrar
map-empty-body = Cuando crees documentos y los vincules, verás aquí cómo se conectan.
map-edge-notebook = Cuaderno
map-edge-link = Vínculo
map-edge-reference = Referencia
