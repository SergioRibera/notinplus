// Desktop entry point. Mobile targets get a stub `main` so the same
// `Cargo.toml` produces `cdylib` + `staticlib` + a `bin` without extra cfgs.

// Release builds on Windows must not open a console window next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
)))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use notinplus::{WINDOW_ID, app, pen_pump};

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,freya_winit=off,freya_core=off,torin=off,ragnarok=off",
        )
    });
    notinplus::diagnostics::init_logging(Some(tracing_subscriber::Layer::boxed(
        tracing_subscriber::Layer::with_filter(tracing_subscriber::fmt::layer(), filter),
    )));

    let Some(documents) = notinplus::desktop::install()? else {
        return Ok(());
    };
    notinplus::diagnostics::start_if_enabled();

    pen_pump::spawn(WINDOW_ID);
    app::run();
    drop(documents);
    Ok(())
}

#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
))]
fn main() {}
