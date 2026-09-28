import IstmoRuntime

// Starts the runtime, calls the generated `IstmoPluginRegistry` and
// jumps into the Rust `#[istmo::mobile_app]` entry point. The
// registry auto-registers `data-store` and `file-picker`; `istmo-pen`
// opts out and is wired inside the Rust side of the app.
IstmoApp.run()
