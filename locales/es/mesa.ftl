## Mesa: paneles con pestañas, el panel del documento y la paleta de comandos.

mesa-draft-discarded = Borrador descartado
# $date: día y mes en que se guardó el borrador (p. ej. "29/10").
mesa-draft-name = Borrador del { $date }
# $name: nombre con el que se guardó el borrador.
mesa-draft-saved = «{ $name }» guardado
mesa-failed-title = No se pudo abrir la mesa
mesa-gone-title = Esta mesa ya no existe
mesa-gone-body = Se borró en este u otro dispositivo.
mesa-go-home = Ir al inicio

mesa-bar-options = Opciones de la mesa
mesa-bar-search = Buscar en la mesa
mesa-bar-back-home = Volver al inicio
# $count: personas trabajando ahora en los documentos de esta mesa.
mesa-bar-people = { $count ->
    [one] { $count } persona en la mesa
   *[other] { $count } personas en la mesa
}
mesa-document-panel = Panel del documento
mesa-more-actions = Más acciones

mesa-tab-new = Nueva pestaña
mesa-tab-close = Cerrar pestaña
mesa-tab-gone = Ya no está
mesa-tab-draft = Borrador
mesa-tab-annotation = Anotación
mesa-pane-other = Otro panel
mesa-pane-close = Cerrar el panel
mesa-pane-empty-title = Panel vacío
mesa-pane-empty-body = Abre aquí un documento o un borrador para escribir rápido.
mesa-pane-open-document = Abrir documento

mesa-notice-this-document = Este documento
mesa-notice-offline = Sin conexión. Sigues trabajando; se sincroniza al volver.
# $owner: nombre de quien organiza la sesión compartida.
mesa-notice-read-only = Solo puedes mirar. { $owner } organiza esta sesión.
mesa-notice-trashed = «{ $name }» está en la papelera
# $days: días que faltan para que se borre solo.
mesa-notice-trashed-days = «{ $name }» está en la papelera · { $days ->
    [one] { $days } día más
   *[other] { $days } días más
}
mesa-notice-restore = Restaurar
mesa-notice-gone = { $name } se borró para siempre en otro dispositivo.

mesa-cmd-split-pane = Dividir el panel
mesa-cmd-draft-here = Borrador en este panel
mesa-cmd-open-beside = Abrir al lado
mesa-cmd-notebook = { -notebook }
mesa-cmd-link-with = Vincular con…
mesa-cmd-share = Compartir…
mesa-cmd-export = Exportar…
mesa-cmd-edit = Editar…
mesa-cmd-link-map = Mapa de vínculos
mesa-cmd-new-desk = Nueva mesa

mesa-palette-placeholder = Busca o haz algo… («/pdf» filtra por tipo)
# $query: lo que escribió el usuario.
mesa-palette-no-results = Nada con «{ $query }»
# $kinds: nombre en plural de un tipo de documento (p. ej. "PDF").
mesa-palette-only-kind = Solo { $kinds }
mesa-palette-actions = Acciones
mesa-palette-recent = Recientes
mesa-palette-documents = Documentos
mesa-palette-document = Documento

mesa-panel-sheet-title = Documento
mesa-panel-contents = Contenido
mesa-panel-bookmarks = Marcadores
mesa-panel-links = Vínculos
mesa-panel-search-contents = Buscar en el contenido
mesa-panel-search-bookmarks = Buscar marcadores
mesa-panel-bookmarks-all = Todos
mesa-panel-bookmarks-empty = Sin marcadores. Mantén pulsado sobre el texto o la hoja para marcar.
mesa-panel-none-title = Elige un documento
mesa-panel-none-body = El panel muestra el documento del panel enfocado.

mesa-links-notebooks = Cuadernos
mesa-links-notebooks-empty = Todavía no hay cuadernos.
mesa-links-new-notebook = Nuevo cuaderno
mesa-links-linked = Vinculados
mesa-links-linked-empty = No está vinculado con nada.
mesa-links-mentions = Lo mencionan
mesa-links-mentions-empty = Ningún documento lo cita todavía.
mesa-links-broken = Referencias rotas
mesa-links-broken-row = Referencia a un documento que ya no está
mesa-links-from-bookmark = Desde un marcador
mesa-links-from-text-box = Desde un cuadro de texto
mesa-links-from-note = Desde el texto de la nota

mesa-outline-collapse = Contraer
mesa-outline-expand = Expandir
mesa-outline-empty = Este documento no tiene índice.
mesa-outline-no-match = Nada con ese nombre.
