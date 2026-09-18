// Desktop entry point. Mobile targets get a stub `main` so the same
// `Cargo.toml` produces `cdylib` + `staticlib` + a `bin` without extra cfgs.

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
)))]
fn main()  {
    use istmo::core::{Runtime, RuntimeInit};

    let RuntimeInit { runtime, outbound } = Runtime::mock();
    notinplus::desktop::spawn(std::sync::Arc::clone(&runtime), outbound);

    // TODO: acquire your plugin clients through the mock runtime so the
    // desktop build can exercise the same code path the mobile shell uses.

    
    // TODO: replace with your `freya` runner.
    notinplus::app::App::run_desktop();
    
}

#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
))]
fn main() {}
