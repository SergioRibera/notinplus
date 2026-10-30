## Mensajes del servicio de sincronización en Ajustes › Dispositivos (src/sync.rs).

# Nombre que toma este dispositivo hasta que se renombra, cuando el sistema no da ninguno.
sync-default-device-name = Este dispositivo
# $error: el mensaje de error del sistema, en cada mensaje sync-error-*.
sync-error-thread = No se pudo iniciar la sincronización: { $error }
sync-error-profile-load = Perfil del dispositivo: { $error }
sync-error-profile-save = No se pudo guardar el perfil del dispositivo: { $error }
sync-error-shares-save = No se pudieron guardar los documentos compartidos: { $error }
sync-error-server = Servidor: { $error }
sync-error-network = Red: { $error }
# $name: el otro dispositivo; $version: su número de protocolo de sincronización.
sync-error-outdated = { $name } usa otra versión de NotIn Plus (protocolo { $version }); actualízalo para sincronizar.
