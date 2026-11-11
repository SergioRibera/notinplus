## Plugin loading, catalog, creation and settings errors shown to users.

plugin-load-damaged = The plugin file is incomplete or damaged.
plugin-load-not-for-device = This isn't a NotIn Plus plugin for this device.
# $device is a platform tag such as linux-x86_64; $targets lists the ones the plugin ships for.
plugin-load-no-library = “{ $plugin }” doesn't include a version for this device ({ $device }). It's available for: { $targets }.
plugin-load-needs-newer-app = “{ $plugin }” needs a newer version of NotIn Plus. Update the app or ask for a compatible version.
plugin-load-built-for-older-app = “{ $plugin }” was made for an older version of NotIn Plus. Ask for an updated version.
# Shown in the platform list for a library that runs on any device.
plugin-manifest-any-target = any

plugin-catalog-duplicate-plugin = A plugin with id { $id } is already installed.
plugin-catalog-duplicate-kind = A document type with id { $id } already exists.

plugin-create-unknown-entry = Unknown entry: { $id }
plugin-create-missing-file = The file is missing.

plugin-config-unknown-key = Unknown setting: { $key }
plugin-config-type-mismatch = This value doesn't fit this setting.
plugin-config-out-of-range = This value is out of range.
# $reason is the plugin's own explanation.
plugin-config-rejected = Value not accepted: { $reason }
