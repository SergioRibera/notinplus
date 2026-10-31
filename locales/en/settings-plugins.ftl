## Settings › Plugins and plugin settings forms (src/screens/settings/plugins.rs, schema.rs).

settings-plugins-status-off = Off
settings-plugins-status-builtin = Built in
settings-plugins-status-third-party = Third party
# $status: one of the settings-plugins-status-* values; $description: the plugin's own description.
settings-plugins-summary = { $status } · { $description }
# $reason: why the plugin did not load.
settings-plugins-failed = Couldn't load · { $reason }
settings-plugins-all = All plugins
settings-plugins-install = Install plugin…
settings-plugins-install-hint = From a .ntp file
settings-plugins-install-pick = Install plugin
settings-plugins-installed = Installed
settings-plugins-with-problems = With problems
settings-plugins-adds = What it adds
# $entries: comma-separated names of what the kind creates.
settings-plugins-creates = Create: { $entries }
settings-plugins-no-settings = This plugin has no settings.
settings-plugins-tech = Technical details
settings-plugins-file-builtin = Built into NotIn Plus
settings-plugins-tech-id = Id: { $id }
settings-plugins-tech-version = Version: { $version }
settings-plugins-tech-author = Author: { $author }
settings-plugins-tech-file = File: { $file }
settings-plugins-tech-sdk = SDK: { $sdk }
# $name: plugin or file name.
settings-plugins-incompatible = Can't install “{ $name }”
settings-plugins-choose-another = Choose another
settings-plugins-review-title = Install “{ $name }”
settings-plugins-review-subtitle = Version { $version } · { $author }
settings-plugins-review-kind = A new document type
settings-plugins-review-entry = In the Create menu
settings-plugins-review-settings = { $count ->
    [one] Comes with { $count } setting you can change in Settings › Plugins.
   *[other] Comes with { $count } settings you can change in Settings › Plugins.
}
settings-plugins-review-warning = Third-party plugins run with the same permissions as NotIn Plus. Only install ones you know.
settings-plugins-review-install = Install
settings-plugins-installed-toast = “{ $name }” installed

settings-plugins-field-range = A number between { $min } and { $max }
settings-plugins-field-max-len = { $max ->
    [one] Up to { $max } character
   *[other] Up to { $max } characters
}
settings-plugins-field-choose = Choose
settings-plugins-field-color = Change color
settings-plugins-field-no-file = No file
settings-plugins-field-choose-file = Choose…
settings-plugins-field-pick-file = Choose file
# $error: a sentence explaining why.
settings-plugins-field-save-failed = Couldn't save the file. { $error }
settings-plugins-field-confirm = Sure? Tap again
