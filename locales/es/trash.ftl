## Papelera: documentos borrados agrupados por la acción que los borró.

# $first: nombre del primer documento; $more: cuántos otros vinieron con él.
trash-group-title = { $first } y { $more } más
trash-restored = { $count ->
    [one] { $count } documento de vuelta en la biblioteca
   *[other] { $count } documentos de vuelta en la biblioteca
}
trash-purged = Borrado para siempre
trash-empty-title = La papelera está vacía
trash-empty-body = Lo que borres se queda aquí un tiempo, por si cambias de idea.
# $size: tamaño total, ya formateado (p. ej. "12,4 MB").
trash-summary-kept = { $count ->
    [one] { $count } documento
   *[other] { $count } documentos
} · { $size } · se guardan hasta que los borres
trash-summary-days = { $count ->
    [one] { $count } documento
   *[other] { $count } documentos
} · { $size } · se borran solos a los { $days } días
trash-back = Volver
trash-select = Seleccionar
# $ago: cuándo se borró (p. ej. "Hace 5 min"); $from: carpeta donde estaba.
trash-row-meta = Borrado { $ago } · de { $from }
# $days: días que faltan para que se borre solo.
trash-row-meta-left = Borrado { $ago } · de { $from } · { $days ->
    [one] { $days } día más
   *[other] { $days } días más
}
trash-restore = Restaurar
trash-empty-all = Vaciar papelera
trash-empty-all-warning = No se puede deshacer. También se borran en tus otros dispositivos.
trash-purge-count = { $count ->
    [one] Borrar { $count } documento para siempre
   *[other] Borrar { $count } documentos para siempre
}
trash-chosen-count = { $count ->
    [one] { $count } elegido
   *[other] { $count } elegidos
}
trash-purge = Borrar para siempre
