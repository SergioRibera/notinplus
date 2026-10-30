## Sync service messages shown in Settings › Devices (src/sync.rs).

# Name this device takes until renamed, when the system gives none.
sync-default-device-name = This device
# $error: the system's error message, in every sync-error-* message.
sync-error-thread = Sync couldn't start: { $error }
sync-error-profile-load = Device profile: { $error }
sync-error-profile-save = Couldn't save the device profile: { $error }
sync-error-shares-save = Couldn't save shared documents: { $error }
sync-error-server = Server: { $error }
sync-error-network = Network: { $error }
# $name: the other device; $version: its sync protocol number.
sync-error-outdated = { $name } runs another version of NotIn Plus (protocol { $version }); update it to sync.
