//! Shared UI shell — same code drives desktop and mobile.
//!
//! Replace the freya scaffold below if you picked a different
//! framework at `cargo generate` time. The Rust side is framework-agnostic:
//! the runtime `Arc` + plugin clients live in an app-owned struct, the UI
//! only draws from them.


// TODO: freya scaffold. See https://freyaui.dev/ for `fn app()` + `launch(app)`.
pub struct App;
impl App {
    pub fn run_desktop() { /* freya::launch(app); */ }
    pub fn run_mobile()  { /* freya::launch(app); */ }
}

