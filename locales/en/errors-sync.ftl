## Sync, devices and camera problems shown in Settings › Devices.

# $reason, in every errors-sync-* message that has it, is a sentence explaining why.
errors-sync-start = Sync couldn't start. { $reason }
errors-sync-profile-load = Couldn't read this device's sync profile. { $reason }
errors-sync-profile-save = Couldn't save this device's sync profile. { $reason }
errors-sync-profile-damaged = The file is damaged or was saved by another version of Notein.
errors-sync-shares-save = Couldn't save the list of shared documents. { $reason }
errors-sync-bad-relay = The server address isn't valid. It should look like host:port/key.
errors-sync-network = Sync can't use the network. { $reason }
errors-sync-network-bind = Another app may be using the sync port. Close the app and open it again.
errors-sync-network-stopped = The connection stopped. Close the app and open it again.
errors-sync-exchange = Some changes couldn't be synced. { $reason }
errors-sync-garbled = A message from another device couldn't be read.
# $version: the sync protocol number received.
errors-sync-version = Another device uses a different version of sync (protocol { $version }). Update both apps to the latest version.
# $name: the other device; $version: its sync protocol number.
errors-sync-outdated = { $name } runs another version of Notein (protocol { $version }). Update it to sync.

errors-doc-damaged = This document's data can't be read. It may be damaged.

errors-scan-unavailable = This device has no camera.
errors-scan-denied = Notein isn't allowed to use the camera. You can allow it in the system settings.
errors-scan-busy = Another app is using the camera. Close it and try again.
errors-scan-interrupted = The camera was interrupted. Try again.
errors-scan-no-format = This camera doesn't offer a picture Notein can read.
errors-scan-stopped = The camera stopped unexpectedly. Try again.
errors-scan-failed = Couldn't open the camera. Try again.
