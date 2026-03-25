//! Global shortcut registration via the XDG GlobalShortcuts portal.
//!
//! GNOME 48+ is the expected full-support floor for app-managed global
//! shortcuts via the GlobalShortcuts portal.

#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_os = "linux")]
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
#[cfg(target_os = "linux")]
use ashpd::WindowIdentifier;
#[cfg(target_os = "linux")]
use futures_util::StreamExt;
use nixclip_core::error::Result;
use tracing::{info, warn};

use crate::AppState;

#[cfg(target_os = "linux")]
const SHORTCUT_ID: &str = "toggle";
#[cfg(target_os = "linux")]
const SHORTCUT_DESCRIPTION: &str = "Toggle NixClip clipboard history";
const RETRY_DELAY: Duration = Duration::from_secs(15);

/// Run the global shortcut listener.
///
/// Attempts to register a system-wide shortcut via the XDG GlobalShortcuts
/// portal. If the portal is unavailable, this keeps retrying in the
/// background instead of terminating the daemon.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    loop {
        let trigger = state.config.read().await.keybind.toggle.clone();
        info!(trigger = %trigger, "starting GlobalShortcuts portal listener");

        match register_and_listen(&trigger).await {
            Ok(()) => {
                warn!("global shortcut listener exited; retrying after backoff");
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "global shortcut registration failed; retrying after backoff"
                );
            }
        }

        tokio::time::sleep(RETRY_DELAY).await;
    }
}

async fn register_and_listen(
    #[allow(unused_variables)] trigger: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("GlobalShortcuts portal is only available on Linux".into())
    }

    #[cfg(target_os = "linux")]
    {
        let portal = GlobalShortcuts::new().await?;
        let session = portal.create_session().await?;

        let listed = portal.list_shortcuts(&session).await?.response()?;
        if listed
            .shortcuts()
            .iter()
            .all(|shortcut| shortcut.id() != SHORTCUT_ID)
        {
            let shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION)
                .preferred_trigger(Some(trigger));
            let bound = portal
                .bind_shortcuts(&session, &[shortcut], &WindowIdentifier::default())
                .await?
                .response()?;
            if bound.shortcuts().is_empty() {
                warn!("GlobalShortcuts portal returned an empty bound shortcut set");
            } else {
                for shortcut in bound.shortcuts() {
                    info!(
                        shortcut_id = shortcut.id(),
                        trigger_description = shortcut.trigger_description(),
                        "global shortcut bound"
                    );
                }
            }
        } else {
            info!(trigger = %trigger, "reusing existing global shortcut binding");
        }

        for shortcut in listed.shortcuts() {
            info!(
                shortcut_id = shortcut.id(),
                trigger_description = shortcut.trigger_description(),
                "global shortcut available"
            );
        }

        let activated = portal.receive_activated().await?;
        let session_closed = session.receive_closed().await?;
        tokio::pin!(activated);
        tokio::pin!(session_closed);

        loop {
            tokio::select! {
                Some(_details) = session_closed.next() => {
                    warn!("GlobalShortcuts session closed");
                    break;
                }
                Some(event) = activated.next() => {
                    if event.shortcut_id() != SHORTCUT_ID {
                        continue;
                    }

                    let activation_token = activation_token(event.options());
                    if let Err(error) = spawn_ui(activation_token.as_deref()) {
                        warn!(error = %error, "failed to spawn nixclip-ui");
                    }
                }
                else => break,
            }
        }

        Err("GlobalShortcuts stream ended".into())
    }
}

#[cfg(any(test, target_os = "linux"))]
fn activation_token(
    options: &std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
) -> Option<String> {
    options.get("activation_token").and_then(|value| {
        <&str>::try_from(value).ok().map(str::to_owned).or_else(|| {
            value
                .try_clone()
                .ok()
                .and_then(|value| String::try_from(value).ok())
        })
    })
}

#[cfg(target_os = "linux")]
fn spawn_ui(activation_token: Option<&str>) -> std::io::Result<()> {
    let executable = resolve_ui_binary();
    let mut command = std::process::Command::new(&executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(token) = activation_token {
        command
            .arg("--activation-token")
            .arg(token)
            .env("NIXCLIP_ACTIVATION_TOKEN", token)
            .env("XDG_ACTIVATION_TOKEN", token);
    }

    info!(
        executable = %executable.display(),
        activation_token = activation_token.is_some(),
        "spawning nixclip-ui"
    );

    command.spawn().map(|_| ())
}

#[cfg(target_os = "linux")]
fn resolve_ui_binary() -> PathBuf {
    match std::env::current_exe() {
        Ok(mut path) => {
            path.set_file_name("nixclip-ui");
            if path.exists() {
                path
            } else {
                PathBuf::from("nixclip-ui")
            }
        }
        Err(_) => PathBuf::from("nixclip-ui"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::zvariant::{OwnedValue, Str};

    use super::activation_token;

    #[test]
    fn extracts_activation_token_from_options() {
        let mut options = HashMap::new();
        options.insert(
            "activation_token".to_string(),
            OwnedValue::from(Str::from("token-123")),
        );

        assert_eq!(activation_token(&options).as_deref(), Some("token-123"));
    }

    #[test]
    fn ignores_missing_activation_token() {
        let options = HashMap::new();
        assert!(activation_token(&options).is_none());
    }
}
