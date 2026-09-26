import Foundation
import IstmoRuntime

@_silgen_name("istmo_run_ios")
func istmo_run_ios() -> Int32

do {
    try IstmoRuntime.shared.start()
} catch {
    fatalError("IstmoRuntime.start() failed: \(error)")
}

// Registers every plugin whose `istmo.toml` declares
// `auto_register = true` (the default). Plugins with a bespoke
// constructor opt out and are registered manually here.
IstmoPluginRegistry.registerAll()

// `UIDocumentPickerViewController` backend for `istmo.file_picker`.
// Opts out of auto-registration because the picker plugin ships a
// generic ctor that most apps override — wire it explicitly.
IstmoRuntime.shared.registerHandler(
    FilePickerDispatcher.PLUGIN_ID,
    FilePickerDispatcher(backend: FilePickerBackendImpl(), codecs: FilePickerCodecsImpl())
)

_ = istmo_run_ios()
