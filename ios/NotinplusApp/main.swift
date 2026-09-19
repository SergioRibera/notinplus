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

_ = istmo_run_ios()
