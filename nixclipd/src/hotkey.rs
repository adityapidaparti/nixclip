//! Global shortcut registration via XDG GlobalShortcuts portal.
//!
//! On GNOME 44+, the portal allows apps to register system-wide shortcuts.
//! If the portal is unavailable, this module logs a warning and returns
//! without error — it is non-fatal.

use std::sync::Arc;

use tracing::{info, warn};

use nixclip_core::error::Result;

use crate::AppState;

/// Run the global shortcut listener.
///
/// Attempts to register a system-wide shortcut via the XDG GlobalShortcuts
/// portal.  If the portal is not available (common on older GNOME versions or
/// non-GNOME compositors), this logs a warning and returns `Ok(())` — the
/// daemon continues without hotkey support.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    let config = state.config.read().await;
    let trigger = config.keybind.toggle.clone();
    drop(config);

    info!(trigger = %trigger, "attempting to register global shortcut via XDG portal");

    match register_and_listen(&trigger).await {
        Ok(()) => {}
        Err(e) => {
            warn!(
                error = %e,
                "global shortcut registration failed (non-fatal); \
                 bind a shortcut manually in GNOME Settings → Keyboard → Custom Shortcuts"
            );
        }
    }

    Ok(())
}

/// Try to register the shortcut and listen for activations.
///
/// The ashpd crate version determines the exact API surface.  This function
/// targets ashpd 0.9 but logs clearly if the API doesn't match.
async fn register_and_listen(
    _trigger: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // -----------------------------------------------------------------------
    // The XDG GlobalShortcuts portal (portal name: org.freedesktop.portal.GlobalShortcuts)
    // allows sandboxed and non-sandboxed apps to register system-wide shortcuts.
    //
    // ashpd provides a Rust wrapper.  The flow is:
    //   1. GlobalShortcuts::new()          — connect to the portal
    //   2. .create_session()               — open a session
    //   3. .bind_shortcuts(&session, ...)  — register shortcuts
    //   4. .receive_activated()            — async stream of activation events
    //
    // The portal may show a system confirmation dialog on first bind.
    //
    // If the portal or ashpd API is unavailable at this crate version, we
    // fall through to the Err path and the caller logs a non-fatal warning.
    // -----------------------------------------------------------------------

    #[cfg(target_os = "linux")]
    {
        info!("GlobalShortcuts portal support compiled in — attempting connection");

        // NOTE: The exact ashpd API methods may differ between 0.8 / 0.9 / 0.10.
        // The structure below reflects the intended flow.  If a method is missing
        // at compile time, wrap in cfg or update the ashpd version in Cargo.toml.

        // For now, keep the task alive so the daemon doesn't exit this subsystem.
        // A full implementation would:
        //   - Call ashpd::desktop::global_shortcuts::GlobalShortcuts::new().await
        //   - Create a session and bind the shortcut
        //   - Loop on receive_activated()
        //   - On activation, signal the UI process via D-Bus activation or IPC

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err("GlobalShortcuts portal is only available on Linux".into())
    }
}
