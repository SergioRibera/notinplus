## Working together: share sheet, invitations, joining and sessions ending
## (src/screens/collab.rs).

collab-share-what = What
collab-share-scope-document = This document
collab-share-scope-notebooks = With its notebooks
collab-share-scope-desk = The whole desk
collab-share-notebook-count = { $count ->
    [one] { $count } notebook
   *[other] { $count } notebooks
}
collab-share-document-count = { $count ->
    [one] { $count } document
   *[other] { $count } documents
}
collab-share-how = How
collab-mode-session = This session only
collab-mode-permanent = Permanent
collab-mode-permanent-copy = Permanent · you keep a copy
collab-share-permanent-hint = Everyone keeps a copy and changes merge when you see each other again.
collab-share-session-hint = While you're connected. When it ends, guests can keep a copy.
collab-share-read-only = Read only
collab-share-read-only-hint = Guests watch and follow your page, but can't write.
collab-share-preparing = Preparing the invitation…
collab-share-relay-hint = Guests join with the link or the QR code. If they're not on the same network, a server is needed (Settings › Devices).
collab-share-create = Create invitation
collab-session-end = End session
collab-session-ended-toast = Session ended
collab-copy-link = Copy link
collab-link-copied = Link copied
collab-invite-ready = Invitation ready
collab-session-people = { $count ->
    [one] { $count } person in the session
   *[other] { $count } people in the session
}

collab-role-owner = Organizer
collab-role-editor = Can edit
collab-role-viewer = Can view
# $name: the person's own name.
collab-person-me = { $name } (you)
# $role: one of the collab-role-* values.
collab-person-online = { $role } · connected
collab-person-offline = { $role } · offline
collab-person-allow-edit = Allow editing
collab-person-view-only = View only
collab-person-confirm = Sure?
collab-person-remove = Remove from session
collab-people-title = { $count ->
    [one] { $count } person
   *[other] { $count } people
}
collab-people-search = Search for a person
collab-people-empty = No one with that name

# $owner: name of who sent the invitation.
collab-invite-from = { $owner } invites you
collab-not-now = Not now
collab-join = Join
# $name: the other device's name.
collab-pair-title = Link with { $name }?
collab-pair-hint = Their documents and yours will sync between the two.
collab-pair = Link
collab-pairing-toast = Linking with { $name }…

collab-join-failed = Couldn't join
collab-join-refused = The invitation is no longer valid or the organizer didn't let you in.
collab-join-unreachable = We couldn't find who invited you. You both have to be connected at the same time.
collab-got-it = Got it
collab-joining = Joining…
# $title: shared document or desk title.
collab-joining-title = Joining “{ $title }”…
collab-joining-searching = Looking for who invited you…
# $done, $total: documents downloaded so far and in all.
collab-joining-progress = Downloading { $done } of { $total }

# $title: the session's title.
collab-ended-title = The session “{ $title }” ended
# $count: documents that were open in the session.
collab-ended-by-owner = { $count ->
    [one] The organizer ended the session. You can keep a copy of { $count } document to continue on your own.
   *[other] The organizer ended the session. You can keep a copy of { $count } documents to continue on your own.
}
collab-ended-expelled = { $count ->
    [one] The organizer removed you from the session. You can keep a copy of { $count } document to continue on your own.
   *[other] The organizer removed you from the session. You can keep a copy of { $count } documents to continue on your own.
}
collab-ended-left = { $count ->
    [one] You left the session. You can keep a copy of { $count } document to continue on your own.
   *[other] You left the session. You can keep a copy of { $count } documents to continue on your own.
}
collab-ended-save-copy = Save a copy
collab-ended-saved = { $count ->
    [one] { $count } copy saved to your library
   *[other] { $count } copies saved to your library
}
