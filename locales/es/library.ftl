## Nombres de la biblioteca que crea la app (src/library.rs).

# Se guarda como nombre de un documento duplicado.
library-item-copy-name = { $name } (copia)
# Se guarda como nombre de una mesa duplicada.
library-desk-copy-name = Copia de { $name }
library-empty-desk = Mesa vacía
# Título de una mesa sin nombre: el nombre de su primer documento y cuántos más tiene.
library-desk-title-more = { $count ->
    [one] { $first } y 1 más
   *[other] { $first } y { $count } más
}
