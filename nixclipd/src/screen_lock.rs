//! Screen lock detection via D-Bus.
//!
//! Listens for the `org.gnome.ScreenSaver.ActiveChanged` signal and pauses
//! clipboard capture while the screen is locked.  If the D-Bus connection
//! fails, this module returns without error — it is non-fatal.

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
    use std::sync::atomic::Ordering;

    use futures_util::StreamExt;
    use zbus::{Connection, MessageStream, MessageType};

    let connection = Connection::session().await?;

    // Subscribe to the ScreenSaver ActiveChanged signal via D-Bus match rule.
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

    // Get the initial lock state so we start with the correct value.
    let initial: std::result::Result<bool, _> = connection
        .call_method(
            Some("org.gnome.ScreenSaver"),
            "/org/gnome/ScreenSaver",
            Some("org.gnome.ScreenSaver"),
            "GetActive",
            &(),
        )
        .await
        .and_then(|reply| reply.body().deserialize());

    if let Ok(active) = initial {
        state.is_locked.store(active, Ordering::Relaxed);
        if active {
            info!("screen is currently locked — clipboard capture paused");
        }
    }

    // Listen for ActiveChanged signals via the message stream instead of
    // polling.  This wakes up only when a D-Bus message arrives, eliminating
    // both the CPU overhead of periodic GetActive calls and the up-to-500ms
    // latency of the old polling loop.
    let mut stream = MessageStream::from(&connection);

    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Filter for signals on the ScreenSaver interface.
        let header = msg.header();
        let is_signal = header.message_type() == MessageType::Signal;
        let is_screen_saver = header
            .interface()
            .is_some_and(|i| i.as_str() == "org.gnome.ScreenSaver");
        let is_active_changed = header
            .member()
            .is_some_and(|m| m.as_str() == "ActiveChanged");

        if is_signal && is_screen_saver && is_active_changed {
            if let Ok(active) = msg.body().deserialize::<bool>() {
                let was_locked = state.is_locked.swap(active, Ordering::Relaxed);
                if active && !was_locked {
                    info!("screen locked — pausing clipboard capture");
                } else if !active && was_locked {
                    info!("screen unlocked — resuming clipboard capture");
                }
            }
        }
    }

    Ok(())
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
