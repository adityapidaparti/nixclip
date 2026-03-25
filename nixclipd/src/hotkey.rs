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
const FAST_RETRY_DELAY: Duration = Duration::from_secs(15);
const SLOW_RETRY_DELAY: Duration = Duration::from_secs(300);
const FAST_RETRY_LIMIT: u32 = 10;

/// Run the global shortcut listener.
///
/// Attempts to register a system-wide shortcut via the XDG GlobalShortcuts
/// portal. If the portal is unavailable, this keeps retrying in the
/// background instead of terminating the daemon.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    let mut consecutive_failures = 0_u32;

    loop {
        let trigger = state.config.read().await.keybind.toggle.clone();
        info!(trigger = %trigger, "starting GlobalShortcuts portal listener");

        let retry_delay = match register_and_listen(&trigger).await {
            Ok(()) => {
                if consecutive_failures > 0 {
                    info!(consecutive_failures, "global shortcut listener recovered");
                }
                consecutive_failures = 0;
                warn!(
                    retry_delay_secs = FAST_RETRY_DELAY.as_secs(),
                    "global shortcut listener exited; retrying after backoff"
                );
                FAST_RETRY_DELAY
            }
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let retry_delay = retry_delay_for(consecutive_failures);
                warn!(
                    error = %error,
                    error_debug = ?error,
                    error_chain = %format_error_chain(error.as_ref()),
                    error_type = %error_type_name(error.as_ref()),
                    consecutive_failures,
                    retry_delay_secs = retry_delay.as_secs(),
                    "global shortcut registration failed; retrying after backoff"
                );
                if consecutive_failures == FAST_RETRY_LIMIT {
                    warn!(
                        retry_delay_secs = retry_delay.as_secs(),
                        "global shortcut registration hit fast-retry limit; slowing retry cadence"
                    );
                }
                retry_delay
            }
        };

        tokio::time::sleep(retry_delay).await;
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
                    return Ok(());
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
                else => {
                    warn!("GlobalShortcuts event stream ended");
                    return Ok(());
                }
            }
        }
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

fn retry_delay_for(consecutive_failures: u32) -> Duration {
    if consecutive_failures >= FAST_RETRY_LIMIT {
        SLOW_RETRY_DELAY
    } else {
        FAST_RETRY_DELAY
    }
}

fn error_type_name(error: &(dyn std::error::Error + 'static)) -> &'static str {
    if let Some(error) = error.downcast_ref::<ashpd::Error>() {
        match error {
            ashpd::Error::Response(_) => "ashpd::Error::Response",
            ashpd::Error::Portal(_) => "ashpd::Error::Portal",
            ashpd::Error::Zbus(_) => "ashpd::Error::Zbus",
            ashpd::Error::NoResponse => "ashpd::Error::NoResponse",
            ashpd::Error::ParseError(_) => "ashpd::Error::ParseError",
            ashpd::Error::IO(_) => "ashpd::Error::IO",
            ashpd::Error::InvalidAppID => "ashpd::Error::InvalidAppID",
            ashpd::Error::NulTerminated(_) => "ashpd::Error::NulTerminated",
            ashpd::Error::RequiresVersion(_, _) => "ashpd::Error::RequiresVersion",
            ashpd::Error::PortalNotFound(_) => "ashpd::Error::PortalNotFound",
            ashpd::Error::UnexpectedIcon => "ashpd::Error::UnexpectedIcon",
            _ => "ashpd::Error",
        }
    } else if error.is::<zbus::Error>() {
        "zbus::Error"
    } else if error.is::<zbus::fdo::Error>() {
        "zbus::fdo::Error"
    } else if error.is::<std::io::Error>() {
        "std::io::Error"
    } else {
        "unknown"
    }
}

fn format_error_chain(mut error: &(dyn std::error::Error + 'static)) -> String {
    let mut chain = error.to_string();

    while let Some(source) = error.source() {
        chain.push_str(": ");
        chain.push_str(&source.to_string());
        error = source;
    }

    chain
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

    use super::{
        activation_token, retry_delay_for, FAST_RETRY_DELAY, FAST_RETRY_LIMIT, SLOW_RETRY_DELAY,
    };

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

    #[test]
    fn uses_fast_retry_before_limit() {
        assert_eq!(retry_delay_for(FAST_RETRY_LIMIT - 1), FAST_RETRY_DELAY);
    }

    #[test]
    fn uses_slow_retry_at_limit() {
        assert_eq!(retry_delay_for(FAST_RETRY_LIMIT), SLOW_RETRY_DELAY);
    }
}
