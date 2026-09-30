// Desktop entry point. Mobile targets get a stub `main` so the same
// `Cargo.toml` produces `cdylib` + `staticlib` + a `bin` without extra cfgs.

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
)))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use notinplus::{WINDOW_ID, app, canvas, pen_pump};

    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("info,freya_winit=off,freya_core=off,torin=off,ragnarok=off,"),
    )
    .init();

    notinplus::desktop::install()?;

    pen_pump::spawn(WINDOW_ID, canvas::Board::shared());
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
