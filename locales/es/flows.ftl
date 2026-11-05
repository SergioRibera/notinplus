## Acciones de la biblioteca: fijar, duplicar, compartir, papelera y mesas (src/flows.rs).

flows-pinned-documents = { $count ->
    [one] 1 documento fijado
   *[other] { $count } documentos fijados
}
flows-duplicated = Duplicado
flows-duplicate-failed = No se pudo duplicar
flows-copy-failed = No se pudo copiar
flows-share-failed = No se pudo compartir
flows-undo = Deshacer
flows-trash-failed = No se pudo mover a la papelera
# $name: nombre del documento o la carpeta.
flows-trashed-named = «{ $name }» se movió a la papelera
flows-trashed-documents = { $count ->
    [one] 1 documento se movió a la papelera
   *[other] { $count } documentos se movieron a la papelera
}
# $name: nombre de la carpeta; $count: documentos que contiene.
flows-trashed-folder = { $count ->
    [one] «{ $name }» y 1 documento se movieron a la papelera
   *[other] «{ $name }» y { $count } documentos se movieron a la papelera
}
flows-trash-title-named = Mover «{ $name }» a la papelera
flows-trash-title-documents = { $count ->
    [one] Mover 1 documento a la papelera
   *[other] Mover { $count } documentos a la papelera
}
# $count: cuadernos vinculados a los documentos que se mueven.
flows-trash-notebooks = { $count ->
    [one] Tiene 1 cuaderno con tus apuntes.
   *[other] Tienen { $count } cuadernos con tus apuntes.
}
flows-fate-keep = Mantener los cuadernos
flows-fate-keep-hint = Quedan en la biblioteca, sin vínculo.
flows-fate-trash = Mandarlos también a la papelera
flows-fate-trash-hint = Se pueden restaurar juntos.
# $title: nombre de la mesa.
flows-delete-desk-title = ¿Borrar la mesa «{ $title }»?
flows-delete-desk-hint = Los documentos quedan en la biblioteca. Solo se cierra este grupo de pestañas.
flows-delete-desk = Borrar mesa
