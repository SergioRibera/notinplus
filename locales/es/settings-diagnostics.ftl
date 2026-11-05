## Ajustes › Acerca de › Diagnóstico remoto (src/diagnostics/ui.rs).

settings-diagnostics = Diagnóstico
settings-diagnostics-remote = Diagnóstico remoto
settings-diagnostics-remote-hint = Permite que el perfilador sismo, en una computadora de esta red, vea cómo rinde NotIn Plus y lo que registra. Tus notas no se envían.
settings-diagnostics-code = Código de vinculación
settings-diagnostics-code-hint = Escríbelo en sismo la primera vez. Las computadoras ya vinculadas se conectan solas.
settings-diagnostics-export = Exportar diagnóstico
settings-diagnostics-export-hint = Guarda los últimos minutos como archivo .sismo en la carpeta de datos.
settings-diagnostics-export-action = Exportar
# $file: nombre del archivo guardado.
settings-diagnostics-exported = Se guardó { $file }
settings-diagnostics-show = Mostrar

## Por qué falló el diagnóstico remoto.

# $reason es una frase que explica por qué.
errors-diagnostics-start = No se pudo activar el diagnóstico remoto. { $reason }
errors-diagnostics-network = Este dispositivo no pudo abrir la red. Revisa que el Wi-Fi esté encendido.
errors-diagnostics-internal = Cierra la app y vuelve a abrirla.
# $reason es una frase que explica por qué.
errors-diagnostics-export = No se pudo guardar el diagnóstico. { $reason }

## Reportes de error que dejaron los cierres inesperados.

settings-diagnostics-crashed = NotIn Plus se cerró por un error. Se guardó un reporte para sismo.
settings-diagnostics-crashes = Reportes de error
# $count: cuántos reportes hay guardados.
settings-diagnostics-crashes-hint = { $count ->
    [one] Un reporte de un error que cerró la app. Conecta sismo para abrirlo.
   *[other] { $count } reportes de errores que cerraron la app. Conecta sismo para abrirlos.
}
settings-diagnostics-crashes-delete = Borrar
