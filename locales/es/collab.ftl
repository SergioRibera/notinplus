## Trabajo en conjunto: hoja de compartir, invitaciones, unirse y fin de sesiones
## (src/screens/collab.rs).

collab-share-what = Qué
collab-share-scope-document = Este documento
collab-share-scope-notebooks = Con sus cuadernos
collab-share-scope-desk = Toda la mesa
collab-share-notebook-count = { $count ->
    [one] { $count } cuaderno
   *[other] { $count } cuadernos
}
collab-share-document-count = { $count ->
    [one] { $count } documento
   *[other] { $count } documentos
}
collab-share-how = Cómo
collab-mode-session = Solo esta sesión
collab-mode-permanent = Permanente
collab-mode-permanent-copy = Permanente · te queda una copia
collab-share-permanent-hint = Cada uno guarda una copia y los cambios se juntan cuando vuelven a verse.
collab-share-session-hint = Mientras estés conectado. Al terminar, los invitados pueden guardarse una copia.
collab-share-read-only = Solo lectura
collab-share-read-only-hint = Los invitados miran y siguen tu página, pero no escriben.
collab-share-preparing = Preparando la invitación…
collab-share-relay-hint = Los invitados entran con el enlace o el código QR. Si no están en la misma red, hace falta un servidor (Ajustes › Dispositivos).
collab-share-create = Crear invitación
collab-session-end = Terminar sesión
collab-session-ended-toast = Sesión terminada
collab-unshare = Dejar de compartir
collab-unshare-confirm = Cada uno conserva su copia · Confirmar
collab-unshared-toast = Ya no se comparte
# $title: el título del compartido.
collab-unshared-by-owner = El organizador dejó de compartir «{ $title }». Tu copia sigue en tu biblioteca.
# $title: el título del compartido.
collab-unshared-expelled = El organizador te quitó de «{ $title }». Tu copia sigue en tu biblioteca.
collab-copy-link = Copiar enlace
collab-link-copied = Enlace copiado
collab-invite-ready = Invitación lista
collab-session-people = { $count ->
    [one] { $count } persona en la sesión
   *[other] { $count } personas en la sesión
}

collab-role-owner = Organiza
collab-role-editor = Edita
collab-role-viewer = Mira
# $name: el nombre propio de la persona.
collab-person-me = { $name } (tú)
# $role: uno de los valores collab-role-*.
collab-person-online = { $role } · conectado
collab-person-offline = { $role } · sin conexión
collab-person-allow-edit = Puede editar
collab-person-view-only = Solo puede mirar
collab-person-confirm = ¿Seguro?
collab-person-remove = Quitar de la sesión
collab-people-title = { $count ->
    [one] { $count } persona
   *[other] { $count } personas
}
collab-people-search = Buscar una persona
collab-people-empty = Nadie con ese nombre

# $owner: nombre de quien envió la invitación.
collab-invite-from = { $owner } te invita
collab-not-now = Ahora no
collab-join = Unirme
# $name: el nombre del otro dispositivo.
collab-pair-title = ¿Vincular con { $name }?
collab-pair-hint = Sus documentos y los tuyos se sincronizarán entre los dos.
collab-pair = Vincular
collab-pairing-toast = Vinculando con { $name }…

collab-join-failed = No se pudo entrar
collab-join-refused = La invitación ya no es válida o el organizador no te dejó entrar.
collab-join-unreachable = No encontramos a quien te invitó. Tienen que estar conectados al mismo tiempo.
collab-got-it = Entendido
collab-joining = Uniéndose…
# $title: título del documento o la mesa compartidos.
collab-joining-title = Uniéndose a «{ $title }»…
collab-joining-searching = Buscando a quien te invitó…
# $done, $total: documentos descargados hasta ahora y en total.
collab-joining-progress = Descargando { $done } de { $total }

# $title: el título de la sesión.
collab-ended-title = La sesión «{ $title }» terminó
# $count: documentos que estaban abiertos en la sesión.
collab-ended-by-owner = { $count ->
    [one] El organizador terminó la sesión. Puedes guardarte una copia de { $count } documento para seguir por tu cuenta.
   *[other] El organizador terminó la sesión. Puedes guardarte una copia de { $count } documentos para seguir por tu cuenta.
}
collab-ended-expelled = { $count ->
    [one] El organizador te quitó de la sesión. Puedes guardarte una copia de { $count } documento para seguir por tu cuenta.
   *[other] El organizador te quitó de la sesión. Puedes guardarte una copia de { $count } documentos para seguir por tu cuenta.
}
collab-ended-left = { $count ->
    [one] Saliste de la sesión. Puedes guardarte una copia de { $count } documento para seguir por tu cuenta.
   *[other] Saliste de la sesión. Puedes guardarte una copia de { $count } documentos para seguir por tu cuenta.
}
collab-ended-save-copy = Guardar una copia
collab-ended-saved = { $count ->
    [one] { $count } copia guardada en tu biblioteca
   *[other] { $count } copias guardadas en tu biblioteca
}
