## Failures shown to users: the library and generic wrappers.

# A short headline ("Couldn't save the desk") followed by a sentence explaining why.
errors-headline-detail = { $headline }. { $detail }

errors-library-runtime = NotIn Plus no pudo acceder a su almacenamiento. Cierra la app y vuelve a abrirla.
errors-library-store = Este dispositivo no pudo leer ni guardar la biblioteca. Inténtalo de nuevo.
errors-library-damaged = Los datos guardados en este dispositivo no se pueden leer. Si los copiaste de otra versión de NotIn Plus, borra la carpeta de datos y vuelve a abrir la app.
errors-library-incompatible = Esta versión de NotIn Plus no puede leer la biblioteca de este dispositivo. Vuelve a abrir la app para elegir qué hacer con ella.
errors-library-missing-folder = Esta carpeta ya no existe.
errors-library-missing-item = Este documento ya no existe.
errors-library-missing-tag = Esta etiqueta ya no existe.
errors-library-missing-desk = Esta mesa ya no existe.
errors-library-missing-link = Este vínculo ya no existe.
errors-library-cycle = Una carpeta no se puede mover dentro de sí misma.
errors-library-root = La carpeta principal no se puede cambiar.
errors-library-self-link = Un documento no se puede vincular consigo mismo.
# $count: personas que siguen en el compartido.
errors-library-shared-folder-in-use = { $count ->
    [one] Esta carpeta está compartida y { $count } persona sigue adentro. Terminá el compartido o esperá a que salga.
   *[other] Esta carpeta está compartida y { $count } personas siguen adentro. Terminá el compartido o esperá a que salgan.
}
