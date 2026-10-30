## Actualizaciones: el aviso en Inicio, la tarjeta de la versión, la hoja de descarga y
## Ajustes › Acerca de › Actualizaciones (src/update/ui.rs, src/update/release.rs).

# $version: versión publicada, como 0.4.1.
update-chip = { $version } disponible
update-release-title = NotIn Plus { $version }
# $installed: versión que tiene este dispositivo.
update-release-installed = Tienes la { $installed }
# $day: cuándo se publicó, ya con formato.
update-release-installed-published = Tienes la { $installed } · publicada { $day }
update-release-full-notes = Ver la versión completa en GitHub
# $format: formato del archivo; $size: tamaño con formato.
update-file = { $format } · { $size }
update-file-hint-android = Android te pide confirmar la instalación; tus documentos no se tocan.
update-file-hint-install = Al terminar se abre el instalador.
update-file-hint-reveal = Se guarda en Descargas; lo instalas tú.
update-file-other-formats = Otros formatos
update-format-dmg = Imagen de disco (.dmg)
update-format-pkg = Instalador (.pkg)
update-format-app = App (.zip)
update-format-nsis = Instalador (.exe)
update-format-msi = Instalador MSI
update-format-zip = Portátil (.zip)
update-open-github = Abrir en GitHub
update-download = Descargar
update-skip = Omitir esta versión
update-not-now = Ahora no

update-downloading-title = Descargando NotIn Plus { $version }
# $done, $total: tamaños con formato.
update-download-progress = { $done } de { $total }
update-download-needs-permission = Permite que NotIn Plus instale apps y toca «Instalar» de nuevo.
update-download-ready = Descargada y verificada.
# $path: carpeta y nombre del archivo en el disco.
update-download-ready-at = Descargada y verificada en { $path }.
update-handed-windows = El instalador está abierto. Cierra NotIn Plus para que pueda reemplazarla.
update-handed-android = Sigue los pasos del instalador de Android.
update-failed-checksum = El archivo llegó dañado y se borró. Prueba de nuevo.
update-failed = No se pudo descargar.
update-failed-installer = No se pudo abrir el instalador.
update-install = Instalar
update-reveal = Mostrar en la carpeta

update-section = Actualizaciones
update-automatic = Buscar actualizaciones
update-automatic-hint = Cada dos días. Solo lee un archivo público de GitHub; no envía nada tuyo.
update-never-checked = Todavía no se buscó
# $ago: hace cuánto, ya con formato y en minúscula.
update-last-checked = Última búsqueda: { $ago }
update-checking = Buscando…
update-newer = Hay una versión nueva: { $version }
# $last: update-last-checked o update-never-checked.
update-up-to-date = Estás al día · { $last }
update-check-now = Buscar ahora
update-whats-new = Ver novedades de la { $version }
update-skipped = Versión omitida
update-skipped-hint = No te avisará de la { $version }. Te avisará de la siguiente.
update-unskip = Mostrar de nuevo
update-unskipped-toast = Se te volverá a avisar
