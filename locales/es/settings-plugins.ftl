## Ajustes › Plugins y formularios de ajustes de plugins (src/screens/settings/plugins.rs, schema.rs).

settings-plugins-status-off = Desactivado
settings-plugins-status-builtin = Incluido
settings-plugins-status-third-party = De terceros
# $status: uno de los valores settings-plugins-status-*; $description: la descripción propia del plugin.
settings-plugins-summary = { $status } · { $description }
# $reason: por qué no se cargó el plugin.
settings-plugins-failed = No se pudo cargar · { $reason }
settings-plugins-all = Todos los plugins
settings-plugins-install = Instalar plugin…
settings-plugins-install-hint = Desde un archivo .ntp
settings-plugins-install-pick = Instalar plugin
settings-plugins-installed = Instalados
settings-plugins-with-problems = Con problemas
settings-plugins-adds = Qué agrega
# $entries: nombres, separados por comas, de lo que crea el tipo.
settings-plugins-creates = Crear: { $entries }
settings-plugins-no-settings = Este plugin no tiene ajustes.
settings-plugins-tech = Detalles técnicos
settings-plugins-file-builtin = Incluido en NotIn Plus
settings-plugins-tech-id = Id: { $id }
settings-plugins-tech-version = Versión: { $version }
settings-plugins-tech-author = Autor: { $author }
settings-plugins-tech-file = Archivo: { $file }
settings-plugins-tech-sdk = SDK: { $sdk }
# $name: nombre del plugin o del archivo.
settings-plugins-incompatible = No se puede instalar «{ $name }»
settings-plugins-choose-another = Elegir otro
settings-plugins-review-title = Instalar «{ $name }»
settings-plugins-review-subtitle = Versión { $version } · { $author }
settings-plugins-review-kind = Un tipo de documento nuevo
settings-plugins-review-entry = En el menú Crear
settings-plugins-review-settings = { $count ->
    [one] Trae { $count } ajuste que puedes cambiar en Ajustes › Plugins.
   *[other] Trae { $count } ajustes que puedes cambiar en Ajustes › Plugins.
}
settings-plugins-review-warning = Los plugins de terceros se ejecutan con los mismos permisos que NotIn Plus. Instala solo los que conozcas.
settings-plugins-review-install = Instalar
settings-plugins-installed-toast = «{ $name }» instalado

settings-plugins-field-range = Un número entre { $min } y { $max }
settings-plugins-field-max-len = { $max ->
    [one] Hasta { $max } carácter
   *[other] Hasta { $max } caracteres
}
settings-plugins-field-choose = Elegir
settings-plugins-field-color = Cambiar color
settings-plugins-field-no-file = Ningún archivo
settings-plugins-field-choose-file = Elegir…
settings-plugins-field-pick-file = Elegir archivo
# $error: el mensaje de error del sistema.
settings-plugins-field-save-failed = No se pudo guardar el archivo: { $error }
settings-plugins-field-confirm = ¿Seguro? Toca de nuevo
