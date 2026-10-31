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

    let logger = env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("info,freya_winit=off,freya_core=off,torin=off,ragnarok=off,"),
    )
    .build();
    let max = logger.filter();
    notinplus::diagnostics::init_logging(Some(Box::new(logger)), max);

    notinplus::desktop::install()?;
    notinplus::diagnostics::start_if_enabled();

    pen_pump::spawn(WINDOW_ID);
    app::run();
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
