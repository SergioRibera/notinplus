import Foundation
import IstmoRuntime

@_silgen_name("istmo_run_ios")
func istmo_run_ios() -> Int32

do {
    try IstmoRuntime.shared.start()
} catch {
    fatalError("IstmoRuntime.start() failed: \(error)")
}

// TODO: register your plugin dispatchers here. Generated Swift classes
// land under `NotinplusApp/Plugins/<Name>/Generated/`.
//
// IstmoRuntime.shared.registerHandler(
//     DataStoreDispatcher.PLUGIN_ID,
//     DataStoreDispatcher(backend: DataStoreBackendImpl(), codecs: DataStoreCodecsImpl())
// )

_ = istmo_run_ios()
