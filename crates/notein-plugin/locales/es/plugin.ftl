## Plugin loading, catalog, creation and settings errors shown to users.

plugin-load-damaged = El archivo del plugin está incompleto o dañado.
plugin-load-not-for-device = No es un plugin de Notein para este dispositivo.
# $device is a platform tag such as linux-x86_64; $targets lists the ones the plugin ships for.
plugin-load-no-library = «{ $plugin }» no trae una versión para este dispositivo ({ $device }). Trae para: { $targets }.
plugin-load-needs-newer-app = «{ $plugin }» pide una versión más nueva de Notein. Actualiza la app o pide una versión compatible.
plugin-load-built-for-older-app = «{ $plugin }» es para una versión anterior de Notein. Pide una versión actualizada.
# Shown in the platform list for a library that runs on any device.
plugin-manifest-any-target = cualquiera

plugin-catalog-duplicate-plugin = Ya hay un plugin con id { $id }.
plugin-catalog-duplicate-kind = Ya hay un tipo de documento con id { $id }.

plugin-create-unknown-entry = Entrada desconocida: { $id }
plugin-create-missing-file = Falta el archivo.

plugin-config-unknown-key = Ajuste desconocido: { $key }
plugin-config-type-mismatch = Este valor no corresponde a este ajuste.
plugin-config-out-of-range = Este valor está fuera de rango.
# $reason is the plugin's own explanation.
plugin-config-rejected = Valor no aceptado: { $reason }
