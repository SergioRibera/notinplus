## Los plugins integrados de lienzo y PDF y el anfitrión de plugins
## (src/plugins/).

plugins-canvas-name = Lienzo
plugins-canvas-description = Escribe y dibuja a mano en un lienzo infinito.
plugins-pdf-description = Importa PDFs, escribe encima y busca en su texto.
plugins-missing-title = Necesita un plugin
plugins-missing-body = Este documento se abre con un plugin que no está instalado o está desactivado ({ $kind }). Instálalo desde Ajustes › Plugins.
plugins-navigate-bookmark-gone = Ese marcador ya no existe
plugins-navigate-needs-plugin = Esa referencia necesita un plugin que no está instalado
plugins-navigate-document-gone = La referencia apunta a un documento que ya no existe
plugins-progress-percent = { $percent } %

## Tipo lienzo (src/plugins/builtin/canvas.rs).
plugins-canvas-label = Lienzo
plugins-canvas-plural = Lienzos
plugins-canvas-search-group = Marcadores
plugins-canvas-bookmark = Marcador
plugins-canvas-hit-snippet = Lienzo · { $name }
plugins-canvas-layer-hidden = oculta
plugins-canvas-outline-layers = Capas
plugins-canvas-summary = { $count ->
    [one] { $count } marcador
   *[other] { $count } marcadores
}
plugins-canvas-chip-bookmark = { $name } · marcador
plugins-canvas-entry-canvas = Escribir a mano
plugins-canvas-scratch = Borrador
plugins-canvas-entry-scratch = Se descarta al cerrar si no lo guardas
plugins-canvas-notebook = { -notebook }
plugins-canvas-entry-notebook = Al lado de un documento
plugins-canvas-ink-layer = Tinta

## Tipo PDF (src/plugins/builtin/pdf.rs).
plugins-pdf-unreadable = Puede estar dañado o protegido con contraseña. Prueba abrirlo en otro programa y guardarlo de nuevo.
plugins-pdf-default-name = Documento PDF
plugins-pdf-search-group = En PDFs
plugins-pdf-page-short = p. { $page }
plugins-pdf-pages = { $count ->
    [one] { $count } página
   *[other] { $count } páginas
}
plugins-pdf-chip-excerpt = PDF p. { $page } · fragmento
plugins-pdf-chip-page = { $name } p. { $page }
plugins-pdf-entry-subtitle = Importar y escribir encima
plugins-pdf-pick-title = Elige un PDF
