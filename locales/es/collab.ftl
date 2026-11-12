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

collab-role-co-organizer = Co-organiza
# $title: el título de la sesión.
collab-already-in = Ya estás en «{ $title }»

## Hoja de sesión, lista de sesiones y carpetas compartidas.

collab-sessions-title = Sesiones
collab-sessions-live = En curso · { $count }
collab-sessions-folders = Carpetas compartidas · { $count }
collab-sessions-waiting = Esperando · { $count }
collab-sessions-recent = Recientes
collab-sessions-empty-title = No estás en ninguna sesión
collab-sessions-empty-body = Compartí un documento, una mesa o una carpeta, o abrí la invitación que te mandaron.
collab-you-organize = Organizás vos
# $name: quien organiza.
collab-organized-by = Te invitó { $name }
# $count: personas conectadas ahora.
collab-people-now = { $count ->
    [one] { $count } persona ahora
   *[other] { $count } personas ahora
}
collab-kind-folder = Carpeta compartida
# $owner: quien organiza.
collab-state-paused = En pausa · { $owner } no está
collab-state-live-lan = En vivo · misma red
collab-state-live-direct = En vivo · directo
collab-state-live-relay = En vivo · por servidor
collab-state-up-to-date = Al día
collab-state-up-to-date-relay = Al día · por servidor
collab-state-waiting-people = Nadie todavía
collab-state-offline = Sin conexión
# $owner: quien organiza.
collab-waiting-owner = Esperando a { $owner }… tienen que estar conectados a la vez
# $owner: quien organiza.
collab-waiting-approval = Esperando a que { $owner } te deje entrar
# $title: el título de la sesión.
collab-waiting-approval-hint = «{ $title }» queda en Sesiones › Esperando; te avisamos cuando entres.
# $when: cuándo; $count: documentos.
collab-recent-ended = La terminaste { $when } · { $count ->
    [one] { $count } documento
   *[other] { $count } documentos
}
# $when: cuándo; $owner: quien organiza.
collab-recent-left = Saliste { $when } · te invitó { $owner }
collab-recent-rejoin = Volver a entrar
collab-recent-reopen = Reabrir
collab-path-lan = misma red
collab-path-relay = por servidor
collab-path-direct = directo
# $path: misma red, directo o por servidor.
collab-person-reachable = se alcanza { $path }
collab-person-offline-short = sin conexión
collab-invitation = Invitación
collab-invitation-hint = Quien tenga el enlace entra mientras la sesión exista. Terminarla lo invalida.
collab-invitation-lan-only = Sin servidor, este enlace solo funciona en la misma red (Ajustes › Dispositivos).
collab-entry-role = Entran como
collab-entry-role-hint = El rol de quien abre el enlace. Después se cambia por persona.
# $role: el rol.
collab-invite-as = Entrás como: { $role }
collab-approve-entry = Aprobar a quien entra
collab-approve-entry-hint = Te avisa «quiere entrar» y elegís su rol. Las sesiones de una vez entran solas; las permanentes y las carpetas preguntan.
collab-who-adds-docs = Pueden agregar documentos
collab-who-adds-docs-hint = Quiénes suman o quitan documentos de la sesión.
collab-adds-organizers = Solo organiza
collab-adds-editors = Quienes editan
# $count: personas esperando.
collab-pending-people = { $count ->
    [one] { $count } persona quiere entrar
   *[other] { $count } personas quieren entrar
}
# $who: quién.
collab-wants-in = { $who } quiere entrar
# $device: el dispositivo desde el que pide entrar.
collab-wants-in-from = desde { $device }
collab-let-in = Dejar entrar
# $role: el rol.
collab-let-in-as = Dejar entrar como { $role }
collab-refuse = No
# $count: documentos.
collab-session-docs = Documentos · { $count }
collab-doc-add = Agregar documentos
collab-doc-add-hint = Elegí de tu biblioteca; los demás los reciben al instante.
collab-doc-remove = Quitar
collab-doc-on-its-way = Llegando…
# $kind: tipo; $titles: otras sesiones.
collab-doc-also-in = { $kind } · también en { $titles }
collab-doc-search = Buscar un documento
collab-doc-none = Nada para agregar
collab-leave = Salir de la sesión
collab-leave-keep-copy = Quedarme con mi copia y salir
collab-leave-confirm = ¿Seguro? · Salir
# $owner: quien organiza.
collab-paused-notice = { $owner } no está
collab-paused-notice-hint = Lo que ves queda igual hasta que vuelva. Podés esperar con el documento abierto, guardarte una copia o salir.
# $owner: quien organiza.
collab-status-paused = { $owner } cerró la app sin terminar la sesión.
collab-status-nobody = Nadie más está conectado ahora.
# $name: quién.
collab-status-one = { $name } está ahora.
# $names: quiénes.
collab-status-many = { $names } están ahora.
collab-status-owner-pause-hint = Si cerrás la app, la sesión queda en pausa y sigue cuando volvés.
collab-share-scope-folder = Esta carpeta
# $name: la carpeta; $count: documentos.
collab-share-folder-count = { $name } · { $count ->
    [one] { $count } documento
   *[other] { $count } documentos
}
collab-share-folder-hint = Una carpeta compartida es permanente: quien edita agrega, quita y crea documentos adentro, y todos los reciben.
# $titles: otras sesiones.
collab-share-also-in = Algo de esto ya está en { $titles }. Puede estar en varias sesiones a la vez.
# $who: quién; $title: la sesión.
collab-toast-joined = { $who } entró a «{ $title }»
# $who: quién; $title: la sesión.
collab-toast-left = { $who } salió de «{ $title }»
# $title: el título nuevo.
collab-toast-renamed = La sesión ahora se llama «{ $title }»
# $who: quién; $title: la sesión.
collab-toast-wants-in = { $who } quiere entrar a «{ $title }»
# $title: la sesión.
collab-toast-entered = Entraste a «{ $title }»
# $title: la sesión.
collab-toast-refused = No te dejaron entrar a «{ $title }»
