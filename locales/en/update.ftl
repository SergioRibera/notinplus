## Updates: the Home chip, the release card, the download sheet and
## Settings › About › Updates (src/update/ui.rs, src/update/release.rs).

# $version: release version, like 0.4.1.
update-chip = { $version } available
update-release-title = NotIn Plus { $version }
# $installed: version this device runs.
update-release-installed = You have { $installed }
# $day: when the release was published, already formatted.
update-release-installed-published = You have { $installed } · published { $day }
update-release-full-notes = See the full release on GitHub
# $format: file format; $size: file size, formatted.
update-file = { $format } · { $size }
update-file-hint-android = Android asks you to confirm the install; your documents aren't touched.
update-file-hint-install = The installer opens when it finishes.
update-file-hint-reveal = It's saved to Downloads; you install it yourself.
update-file-other-formats = Other formats
update-format-dmg = Disk image (.dmg)
update-format-pkg = Installer (.pkg)
update-format-app = App (.zip)
update-format-nsis = Installer (.exe)
update-format-msi = MSI installer
update-format-zip = Portable (.zip)
update-open-github = Open on GitHub
update-download = Download
update-skip = Skip this version
update-not-now = Not now

update-downloading-title = Downloading NotIn Plus { $version }
# $done, $total: formatted sizes.
update-download-progress = { $done } of { $total }
update-download-needs-permission = Allow NotIn Plus to install apps and tap “Install” again.
update-download-ready = Downloaded and verified.
# $path: folder and file name on disk.
update-download-ready-at = Downloaded and verified at { $path }.
update-handed-windows = The installer is open. Close NotIn Plus so it can replace it.
update-handed-android = Follow the steps of the Android installer.
update-failed-checksum = The file arrived damaged and was deleted. Try again.
update-failed = Couldn't download.
update-failed-installer = Couldn't open the installer.
update-install = Install
update-reveal = Show in folder

update-section = Updates
update-automatic = Check for updates
update-automatic-hint = Every two days. It only reads a public file on GitHub; nothing of yours is sent.
update-never-checked = Not checked yet
# $ago: how long ago, already formatted and lowercase.
update-last-checked = Last checked: { $ago }
update-checking = Checking…
update-newer = A new version is available: { $version }
# $last: update-last-checked or update-never-checked.
update-up-to-date = You're up to date · { $last }
update-check-now = Check now
update-whats-new = See what's new in { $version }
update-skipped = Skipped version
update-skipped-hint = You won't be notified about { $version }. You will be about the next one.
update-unskip = Show again
update-unskipped-toast = You'll be notified again
