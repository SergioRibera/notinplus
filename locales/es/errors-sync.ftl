## Sync, devices and camera problems shown in Settings › Devices.

# $reason, in every errors-sync-* message that has it, is a sentence explaining why.
errors-sync-start = La sincronización no pudo iniciar. { $reason }
errors-sync-profile-load = No se pudo leer el perfil de sincronización de este dispositivo. { $reason }
errors-sync-profile-save = No se pudo guardar el perfil de sincronización de este dispositivo. { $reason }
errors-sync-profile-damaged = El archivo está dañado o lo guardó otra versión de Notein.
errors-sync-shares-save = No se pudo guardar la lista de documentos compartidos. { $reason }
errors-sync-bad-relay = La dirección del servidor no es válida. Debe tener la forma host:puerto/clave.
errors-sync-network = La sincronización no puede usar la red. { $reason }
errors-sync-network-bind = Puede que otra app esté usando el puerto de sincronización. Cierra la app y vuelve a abrirla.
errors-sync-network-stopped = La conexión se detuvo. Cierra la app y vuelve a abrirla.
errors-sync-exchange = Algunos cambios no se pudieron sincronizar. { $reason }
errors-sync-garbled = No se pudo leer un mensaje de otro dispositivo.
# $version: the sync protocol number received.
errors-sync-version = Otro dispositivo usa una versión distinta de la sincronización (protocolo { $version }). Actualiza ambas apps a la última versión.
# $name: the other device; $version: its sync protocol number.
errors-sync-outdated = { $name } tiene otra versión de Notein (protocolo { $version }). Actualízala para sincronizar.

errors-doc-damaged = Los datos de este documento no se pueden leer. Puede que estén dañados.

errors-scan-unavailable = Este dispositivo no tiene cámara.
errors-scan-denied = Notein no tiene permiso para usar la cámara. Puedes dárselo en los ajustes del sistema.
errors-scan-busy = Otra app está usando la cámara. Ciérrala e inténtalo de nuevo.
errors-scan-interrupted = La cámara se interrumpió. Inténtalo de nuevo.
errors-scan-no-format = Esta cámara no ofrece una imagen que Notein pueda leer.
errors-scan-stopped = La cámara se detuvo inesperadamente. Inténtalo de nuevo.
errors-scan-failed = No se pudo abrir la cámara. Inténtalo de nuevo.
