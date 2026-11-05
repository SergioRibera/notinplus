## Settings › Sessions: name, defaults for a new invitation and lasting
## shares (src/screens/settings/sessions.rs).

settings-sessions-you = You
settings-sessions-name = Your name
# $device: this device's name.
settings-sessions-name-hint = How others see you in a session. With the device: “{ $device }”.
settings-sessions-defaults = New invitations · defaults
settings-sessions-entry-role-hint = Role of whoever opens a link of yours. Changed per session and per person.
settings-sessions-approve-hint = One-off sessions let people straight in. Permanent shares and folders ask first.
settings-sessions-approve-by-kind = By kind
settings-sessions-approve-always = Always
settings-sessions-approve-never = Never
settings-sessions-keep-invites = Keep invitations of sessions you left
settings-sessions-keep-invites-hint = To go back in while the organizer keeps it open.
settings-sessions-lasting = Permanent shares and shared folders
settings-sessions-lasting-empty = Nothing shared permanently.
settings-sessions-kind-folder = Folder
settings-sessions-kind-document = Documents
# $kind: Folder or Documents; $who: who organizes; $count: documents.
settings-sessions-share-meta = { $kind } · { $who } · { $count ->
    [one] { $count } document
   *[other] { $count } documents
}
settings-sessions-now = Now
# $live, $waiting, $recent: counts.
settings-sessions-now-summary = { $live ->
    [one] { $live } session in progress
   *[other] { $live } sessions in progress
} · { $waiting } waiting · { $recent } recent
settings-sessions-now-hint = Opens the same sheet as the Home button.
settings-sessions-see = See sessions
