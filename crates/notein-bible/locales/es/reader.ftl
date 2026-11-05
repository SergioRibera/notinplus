## El lector de la Biblia: acciones del encabezado, barra de capítulos, selector de libros, barra de selección y referencias cruzadas (src/ui/face*, src/ui/bottom_nav.rs).

reader-action-translation = Traducción
reader-action-search = Buscar en la Biblia
reader-loading-chapter = Cargando capítulo…
reader-nav-previous-chapter = Capítulo anterior
reader-nav-next-chapter = Capítulo siguiente
reader-nav-previous-passage = Pasaje anterior
reader-nav-next-passage = Pasaje siguiente
reader-books-title = Libros
reader-selection-verses = { $count ->
    [one] 1 versículo
   *[other] { $count } versículos
}
reader-selection-verse = Verso { $verse }
# $first y $last son números de versículo de un mismo capítulo.
reader-selection-verse-range = Versos { $first }–{ $last }
reader-selection-share = Compartir
reader-refs-count = { $count ->
    [one] 1 referencia
   *[other] { $count } referencias
}
# $place es el versículo del que parten las referencias, p. ej. «Juan 3:16».
reader-refs-title = Referencias de { $place }
reader-refs-close = Cerrar
