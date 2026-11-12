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
collab-unshare = Stop sharing
collab-unshare-confirm = Everyone keeps their copy · Confirm
collab-unshared-toast = No longer shared
# $title: the title of the share.
collab-unshared-by-owner = The organizer stopped sharing “{ $title }”. Your copy stays in your library.
# $title: the title of the share.
collab-unshared-expelled = The organizer removed you from “{ $title }”. Your copy stays in your library.
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

collab-role-co-organizer = Co-organizer
# $title: the session title.
collab-already-in = You are already in “{ $title }”

## Session sheet, sessions list and shared folders.

collab-sessions-title = Sessions
collab-sessions-live = In progress · { $count }
collab-sessions-folders = Shared folders · { $count }
collab-sessions-waiting = Waiting · { $count }
collab-sessions-recent = Recent
collab-sessions-empty-title = You are not in any session
collab-sessions-empty-body = Share a document, a desk or a folder, or open the invitation you were sent.
collab-you-organize = You organize
# $name: who organizes.
collab-organized-by = { $name } invited you
# $count: people connected now.
collab-people-now = { $count ->
    [one] { $count } person now
   *[other] { $count } people now
}
collab-kind-folder = Shared folder
# $owner: who organizes.
collab-state-paused = Paused · { $owner } is away
collab-state-live-lan = Live · same network
collab-state-live-direct = Live · direct
collab-state-live-relay = Live · via server
collab-state-up-to-date = Up to date
collab-state-up-to-date-relay = Up to date · via server
collab-state-waiting-people = Nobody yet
collab-state-offline = Offline
# $owner: who organizes.
collab-waiting-owner = Waiting for { $owner }… you both have to be connected at once
# $owner: who organizes.
collab-waiting-approval = Waiting for { $owner } to let you in
# $title: the session title.
collab-waiting-approval-hint = “{ $title }” stays under Sessions › Waiting; we'll tell you when you're in.
# $when: when; $count: documents.
collab-recent-ended = You ended it { $when } · { $count ->
    [one] { $count } document
   *[other] { $count } documents
}
# $when: when; $owner: who organizes.
collab-recent-left = You left { $when } · { $owner } invited you
collab-recent-rejoin = Go back in
collab-recent-reopen = Reopen
collab-path-lan = same network
collab-path-relay = via server
collab-path-direct = direct
# $path: same network, direct or via server.
collab-person-reachable = reachable { $path }
collab-person-offline-short = offline
collab-invitation = Invitation
collab-invitation-hint = Whoever has the link gets in while the session exists. Ending it kills the link.
collab-invitation-lan-only = Without a server this link only works on the same network (Settings › Devices).
collab-entry-role = Enter as
collab-entry-role-hint = The role of whoever opens the link. Changed per person later.
# $role: the role.
collab-invite-as = You enter as: { $role }
collab-approve-entry = Approve who enters
collab-approve-entry-hint = Tells you “wants in” and you pick their role. One-off sessions let people straight in; permanent shares and folders ask.
collab-who-adds-docs = Who adds documents
collab-who-adds-docs-hint = Who brings documents in and takes them out.
collab-adds-organizers = Organizers only
collab-adds-editors = Anyone who edits
# $count: people waiting.
collab-pending-people = { $count ->
    [one] { $count } person wants in
   *[other] { $count } people want in
}
# $who: who.
collab-wants-in = { $who } wants in
# $device: the device they ask from.
collab-wants-in-from = from { $device }
collab-let-in = Let in
# $role: the role.
collab-let-in-as = Let in as { $role }
collab-refuse = No
# $count: documents.
collab-session-docs = Documents · { $count }
collab-doc-add = Add documents
collab-doc-add-hint = Pick from your library; everyone gets them at once.
collab-doc-remove = Remove
collab-doc-on-its-way = On its way…
# $kind: kind; $titles: other sessions.
collab-doc-also-in = { $kind } · also in { $titles }
collab-doc-search = Search for a document
collab-doc-none = Nothing to add
collab-leave = Leave the session
collab-leave-keep-copy = Keep my copy and leave
collab-leave-confirm = Sure? · Leave
# $owner: who organizes.
collab-paused-notice = { $owner } is away
collab-paused-notice-hint = What you see stays as it is until they're back. You can wait with the document open, keep a copy, or leave.
# $owner: who organizes.
collab-status-paused = { $owner } closed the app without ending the session.
collab-status-nobody = Nobody else is connected right now.
# $name: who.
collab-status-one = { $name } is here now.
# $names: who.
collab-status-many = { $names } are here now.
collab-status-owner-pause-hint = If you close the app, the session pauses and goes on when you're back.
collab-share-scope-folder = This folder
# $name: the folder; $count: documents.
collab-share-folder-count = { $name } · { $count ->
    [one] { $count } document
   *[other] { $count } documents
}
collab-share-folder-hint = A shared folder is permanent: anyone who edits adds, removes and creates documents inside, and everyone gets them.
# $titles: other sessions.
collab-share-also-in = Some of this is already in { $titles }. It can be in several sessions at once.
# $who: who; $title: the session.
collab-toast-joined = { $who } joined “{ $title }”
# $who: who; $title: the session.
collab-toast-left = { $who } left “{ $title }”
# $title: the new title.
collab-toast-renamed = The session is now called “{ $title }”
# $who: who; $title: the session.
collab-toast-wants-in = { $who } wants into “{ $title }”
# $title: the session.
collab-toast-entered = You joined “{ $title }”
# $title: the session.
collab-toast-refused = You weren't let into “{ $title }”
