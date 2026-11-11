## Búsqueda en la traducción abierta (src/ui/face/search.rs).

search-title = Buscar en la Biblia
search-empty-title = Sin resultados
search-empty-body = Prueba con otras palabras o con una cita como Juan 3:16.
search-placeholder = Palabras o una cita (Juan 3:16)
search-scope-this = Esta traducción
search-scope-all = Todas
# Se muestra hasta que se escribe algo; las comillas y OR son sintaxis de búsqueda.
search-help = Todas las palabras deben aparecer, también como comienzo de otra (amor encuentra amoroso). "frase exacta" · fe OR esperanza · -palabra para excluir.
# $count es el máximo de coincidencias listadas.
search-closest = Los { $count } más cercanos
search-results = { $count ->
    [one] 1 resultado
   *[other] { $count } resultados
}
# $place es libro, capítulo y quizá versículo, p. ej. «Juan 3:16».
search-go-to = Ir a { $place }
