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
