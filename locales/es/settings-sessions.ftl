## Ajustes › Sesiones: nombre, valores por defecto de una invitación y
## compartidos permanentes (src/screens/settings/sessions.rs).

settings-sessions-you = Vos
settings-sessions-name = Tu nombre
# $device: el nombre de este dispositivo.
settings-sessions-name-hint = Así te ven en una sesión. Con el dispositivo: «{ $device }».
settings-sessions-defaults = Invitaciones nuevas · valores por defecto
settings-sessions-entry-role-hint = Rol de quien abre un enlace tuyo. Se cambia por sesión y por persona.
settings-sessions-approve-hint = Sesiones de una vez: entran solas. Permanentes y carpetas: preguntan primero.
settings-sessions-approve-by-kind = Según el tipo
settings-sessions-approve-always = Siempre
settings-sessions-approve-never = Nunca
settings-sessions-keep-invites = Guardar invitaciones de sesiones que dejaste
settings-sessions-keep-invites-hint = Para «Volver a entrar» mientras el organizador no la termine.
settings-sessions-lasting = Permanentes y carpetas compartidas
settings-sessions-lasting-empty = Nada compartido de forma permanente.
settings-sessions-kind-folder = Carpeta
settings-sessions-kind-document = Documentos
# $kind: Carpeta o Documentos; $who: quién organiza; $count: documentos.
settings-sessions-share-meta = { $kind } · { $who } · { $count ->
    [one] { $count } documento
   *[other] { $count } documentos
}
settings-sessions-now = Ahora
# $live, $waiting, $recent: cantidades.
settings-sessions-now-summary = { $live ->
    [one] { $live } sesión en curso
   *[other] { $live } sesiones en curso
} · { $waiting } esperando · { $recent } recientes
settings-sessions-now-hint = Abre la misma hoja que el botón de Inicio.
settings-sessions-see = Ver sesiones
