//! Screen lock detection via D-Bus.
//!
//! Listens for the `org.gnome.ScreenSaver.ActiveChanged` signal and pauses
//! clipboard capture while the screen is locked.  If the D-Bus connection
//! fails, this module returns without error — it is non-fatal.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tracing::{info, warn};

use nixclip_core::error::Result;

use crate::AppState;

/// Run the screen-lock monitor.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    info!("starting screen lock monitor");

    match monitor(state).await {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!(
                error = %e,
                "screen lock monitoring failed (non-fatal); \
                 clipboard capture will not pause during screen lock"
            );
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
async fn monitor(
    state: Arc<AppState>,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use zbus::Connection;

    let connection = Connection::session().await?;

    // We use the low-level match-rule approach to avoid needing the `futures-util`
    // crate for StreamExt.  Instead we manually receive messages from the
    // connection's message stream.

    // Subscribe to the ScreenSaver signal.
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &("type='signal',interface='org.gnome.ScreenSaver',member='ActiveChanged'",),
        )
        .await?;

    info!("subscribed to org.gnome.ScreenSaver.ActiveChanged");

    // Poll for messages.  This is a blocking loop that runs until the
    // connection is closed.
    loop {
        // Use a short sleep + non-blocking check pattern to avoid needing
        // StreamExt.  A more efficient implementation would use
        // `futures_util::StreamExt::next()` on a `MessageStream`.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Try to read pending messages from the connection.
        // zbus processes signals automatically when we have match rules,
        // but without a proxy we need to check differently.
        //
        // Alternative: use zbus::proxy macro + futures-util for a cleaner stream.
        // For now, poll the property directly.
        let result: std::result::Result<bool, _> = connection
            .call_method(
                Some("org.gnome.ScreenSaver"),
                "/org/gnome/ScreenSaver",
                Some("org.gnome.ScreenSaver"),
                "GetActive",
                &(),
            )
            .await
            .and_then(|reply| reply.body().deserialize());

        match result {
            Ok(active) => {
                let was_locked = state.is_locked.swap(active, Ordering::Relaxed);
                if active && !was_locked {
                    info!("screen locked — pausing clipboard capture");
                } else if !active && was_locked {
                    info!("screen unlocked — resuming clipboard capture");
                }
            }
            Err(_) => {
                // ScreenSaver service might not be running; continue silently.
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
async fn monitor(
    _state: Arc<AppState>,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Screen lock monitoring is only available on Linux with D-Bus.
    // On other platforms, just sleep forever.
    std::future::pending::<()>().await;
    Ok(())
}
