## Archivos que llegan varios a la vez (soltados, abiertos con la app) y
## carpetas soltadas en la biblioteca.

# $count: documentos importados en esta tanda.
flows-import-queue-imported = { $count ->
    [one] 1 documento importado
   *[other] { $count } documentos importados
}
# $names: nombres de archivo separados por comas.
flows-import-queue-refused = { $count ->
    [one] No se pudo importar: { $names }
   *[other] No se pudieron importar { $count } archivos: { $names }
}
# $name: nombre del archivo.
flows-import-queue-plugin = «{ $name }» es un plugin. Para instalarlo, ve a Ajustes, Plugins.
flows-import-drop-library = Suelta para importar en «{ $folder }»
flows-import-drop-here = Suelta para importar
flows-import-drop-nowhere = Aquí no se pueden soltar archivos. Ve a la biblioteca.

# $name: la carpeta soltada.
flows-import-tree-title = Importar «{ $name }»
flows-import-tree-summary = { $documents ->
    [one] 1 documento
   *[other] { $documents } documentos
} en { $folders ->
    [one] 1 carpeta
   *[other] { $folders } carpetas
}
flows-import-tree-skipped = { $count ->
    [one] Se va a saltar 1 archivo: ningún plugin lo importa
   *[other] Se van a saltar { $count } archivos: ningún plugin los importa
}
flows-import-tree-skipped-more = { $names } y { $more } más
flows-import-tree-truncated = Solo se toman los primeros { $max } documentos y 16 niveles de carpetas.
flows-import-tree-submit = { $count ->
    [one] Importar 1
   *[other] Importar { $count }
}
flows-import-tree-empty = «{ $name }» no tiene documentos que se puedan importar.
flows-import-tree-importing = Importando documentos
flows-import-tree-progress = { $done } de { $total }

## Un archivo soltado sobre un documento abierto (una nota).

# $document: nombre del documento.
flows-add-drop = Suelta para añadir a «{ $document }»
# $name: el archivo soltado.
flows-add-title = Añadir «{ $name }»
flows-add-title-queued = Añadir «{ $name }» ({ $position } de { $total })
flows-add-subtitle = A «{ $document }»
flows-add-submit = Insertar
flows-add-reference = Referencia
flows-add-reference-detail = Lo importa y pone un enlace que lo abre.
flows-add-link = Vincular
flows-add-link-detail = Lo importa y lo vincula con este documento, sin tocar el texto.
flows-add-text = Insertar el texto
flows-add-text-detail = Pone aquí todo su texto.
flows-add-text-pages-detail = Pone aquí el texto de las páginas que elijas.
flows-add-image = Insertar como imagen
flows-add-image-detail = Pone aquí las páginas que elijas como imágenes.
flows-add-pages = Páginas
# $count: páginas del PDF.
flows-add-pages-field = Páginas (de { $count })
flows-add-pages-placeholder = 1-3, 7
flows-add-pages-hint = Hasta 50 páginas como texto y 20 como imágenes.
flows-add-keep-file = Guardar también el PDF en la biblioteca
flows-add-keep-file-on = Un enlace debajo de lo que insertes lo abre.
flows-add-keep-file-off = Sin el PDF, lo que insertes no tendrá enlace a su origen.
# $name: lo que se añadió.
flows-add-done = «{ $name }» añadido
flows-add-linked = «{ $name }» importado y vinculado
flows-add-failed = No se pudo añadir «{ $name }»
flows-add-closed = El documento se cerró.
flows-add-unreadable = No se pudo leer el PDF.
flows-add-no-text = Esas páginas no tienen texto que copiar. Prueba a insertarlas como imágenes.
flows-add-no-images = Este documento no admite imágenes.
# $share: nombre de la sesión compartida; $people: el resto de quienes están.
flows-add-shared = Este documento está compartido en «{ $share }» con { $people ->
    [one] 1 persona más
   *[other] { $people } personas más
}. Lo que insertes les llega al instante, imágenes incluidas.
flows-add-read-only = «{ $document }» está compartido contigo solo para verlo.
