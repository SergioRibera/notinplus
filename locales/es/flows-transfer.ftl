## Exportar e importar copias (src/flows/transfer.rs).

flows-transfer-export-title = Exportar
flows-transfer-whole-library = Toda la biblioteca
flows-transfer-documents = { $count ->
    [one] 1 documento
   *[other] { $count } documentos
}
flows-transfer-desks = { $count ->
    [one] 1 mesa
   *[other] { $count } mesas
}
flows-transfer-bookmarks = { $count ->
    [one] 1 marcador
   *[other] { $count } marcadores
}
# $size: tamaño total, ya formateado.
flows-transfer-contents = { $count ->
    [one] 1 documento · { $size }
   *[other] { $count } documentos · { $size }
}
flows-transfer-include-pdfs = Incluir los PDF originales
# $size: tamaño de los PDF, ya formateado.
flows-transfer-include-pdfs-hint = { $size }. Sin ellos, la copia guarda solo lo escrito encima.
flows-transfer-include-desks = Incluir mesas y marcadores
flows-transfer-include-desks-hint = { $count ->
    [one] 1 mesa y todos los resaltados.
   *[other] { $count } mesas y todos los resaltados.
}
flows-transfer-purpose = Para qué es
flows-transfer-backup = Copia de seguridad
flows-transfer-backup-hint = Todo tal cual: para volver a importarlo en Notein, aquí o en otro dispositivo.
flows-transfer-readable = Leer en otras apps
flows-transfer-readable-hint = PDF con lo escrito encima y notas en Markdown, en una carpeta.
flows-transfer-what = Qué
flows-transfer-export = Exportar
flows-transfer-export-size = Exportar { $size }
# $name: nombre del archivo.
flows-transfer-saved-as = Se guarda como { $name }
flows-transfer-progress = { $total ->
    [one] { $done } de 1 documento
   *[other] { $done } de { $total } documentos
}
flows-transfer-progress-hint = Puedes seguir usando la app. Te avisamos al terminar.
flows-transfer-exporting = Exportando
flows-transfer-export-failed = No se pudo exportar
flows-transfer-save-copy = Guardar copia
flows-transfer-save-failed = No se pudo guardar la copia
flows-transfer-saved-subtitle = { $name } · { $size }
flows-transfer-show-in-folder = Mostrar en la carpeta
flows-transfer-saved = Copia guardada
flows-transfer-no-space = No cabe en este lugar
# $missing: tamaño, ya formateado.
flows-transfer-no-space-hint = Hacen falta { $missing }. Elige otro lugar o exporta sin los PDF originales.
flows-transfer-without-pdfs = Sin PDF originales
flows-transfer-pick-place = Elegir otro lugar
flows-transfer-import-picker = Importar una copia de Notein
flows-transfer-reviewing = Revisando «{ $name }»…
flows-transfer-newer = No se puede abrir este archivo
flows-transfer-newer-hint = Lo creó una versión más nueva de Notein. Actualiza la app y vuelve a intentarlo.
flows-transfer-pick-another = Elegir otro
flows-transfer-check-update = Buscar actualización
flows-transfer-damaged = No se puede abrir «{ $name }»
flows-transfer-damaged-hint = Este archivo está incompleto o dañado.
flows-transfer-import-title = Importar «{ $name }»
flows-transfer-keep-both = Mantener ambos
flows-transfer-replace = Reemplazar
flows-transfer-skip = Omitir
# Se guarda como nombre de la carpeta donde cae una importación.
flows-transfer-imported-folder = Importados
# $source: dispositivo del que vino la copia.
flows-transfer-imported-folder-from = Importados de { $source }
flows-transfer-missing-plugins = { $count ->
    [one] 1 documento usa plugins que no están instalados. Se importa igual y se abre cuando los instales.
   *[other] { $count } documentos usan plugins que no están instalados. Se importan igual y se abren cuando los instales.
}
flows-transfer-conflicts = { $count ->
    [one] 1 ya está en tu biblioteca
   *[other] { $count } ya están en tu biblioteca
}
flows-transfer-for-all = Para todos:
flows-transfer-identical = Iguales · se omite solo
# $mine y $theirs: fechas de edición; $newer es "yes" cuando la de la copia es más nueva.
flows-transfer-conflict = { $newer ->
    [yes] El tuyo: { $mine } · El de la copia: { $theirs } (más nuevo)
   *[no] El tuyo: { $mine } · El de la copia: { $theirs }
}
flows-transfer-new = { $count ->
    [one] 1 nuevo
   *[other] { $count } nuevos
}
flows-transfer-new-hint = Se agregan tal cual
flows-transfer-save-in = Guardar en
# $folder: nombre de la carpeta de destino.
flows-transfer-destination = Biblioteca › { $folder }
flows-transfer-import-count = Importar { $count }
flows-transfer-skipped = { $count ->
    [one] 1 se omite por ser igual
   *[other] { $count } se omiten por ser iguales
}
flows-transfer-imported = { $count ->
    [one] 1 documento importado
   *[other] { $count } documentos importados
}
flows-transfer-view = Ver
flows-transfer-import-failed = No se pudo importar
