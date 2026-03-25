//! NixClip — GTK4/libadwaita clipboard history popup.

mod app;
mod ipc_client;
mod settings;
mod widgets;
mod window;

use libadwaita as adw;
use adw::prelude::*;

fn main() {
    // Initialise tracing (logs to stderr).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let application = adw::Application::builder()
        .application_id("com.nixclip.NixClip")
        .build();

    application.connect_activate(|app| {
        app::activate(app);
    });

    application.run();
}
