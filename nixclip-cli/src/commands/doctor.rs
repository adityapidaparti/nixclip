//! `nixclip doctor` — run diagnostic checks and report system health.

use std::path::PathBuf;
use std::process::Command;

use nixclip_core::config::Config;
use nixclip_core::Result;

/// Result of a single diagnostic check.
#[derive(Debug)]
struct Check {
    label: String,
    status: CheckStatus,
    detail: Option<String>,
}

#[derive(Debug)]
enum CheckStatus {
    Ok,
    Warning,
    Error,
}

impl CheckStatus {
    fn symbol(&self) -> &'static str {
        match self {
            CheckStatus::Ok => "OK",
            CheckStatus::Warning => "WARN",
            CheckStatus::Error => "FAIL",
        }
    }
}

pub async fn run(json: bool) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // -----------------------------------------------------------------------
    // 1. Daemon connectivity
    // -----------------------------------------------------------------------
    let socket_path = Config::socket_path();
    let daemon_check = check_daemon(&socket_path).await;
    checks.push(daemon_check);

    // -----------------------------------------------------------------------
    // 2. XDG_RUNTIME_DIR
    // -----------------------------------------------------------------------
    checks.push(check_runtime_dir());

    // -----------------------------------------------------------------------
    // 3. GNOME version / support tier
    // -----------------------------------------------------------------------
    checks.push(check_gnome_version());

    // -----------------------------------------------------------------------
    // 4. Wayland clipboard protocol support
    // -----------------------------------------------------------------------
    checks.push(check_wayland_protocols());

    // -----------------------------------------------------------------------
    // 5. GlobalShortcuts portal availability
    // -----------------------------------------------------------------------
    checks.push(check_global_shortcuts_portal());

    // -----------------------------------------------------------------------
    // 6. Config file validity
    // -----------------------------------------------------------------------
    checks.push(check_config_file());

    // -----------------------------------------------------------------------
    // 7. Database file
    // -----------------------------------------------------------------------
    checks.push(check_db_file());

    // -----------------------------------------------------------------------
    // 8. Blob directory permissions
    // -----------------------------------------------------------------------
    checks.push(check_blob_dir());

    // -----------------------------------------------------------------------
    // 9. System info (informational, always Ok)
    // -----------------------------------------------------------------------
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "(not set)".into());
    let wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "(not set)".into());

    checks.push(Check {
        label: "XDG_SESSION_TYPE".into(),
        status: CheckStatus::Ok,
        detail: Some(session_type),
    });
    checks.push(Check {
        label: "WAYLAND_DISPLAY".into(),
        status: CheckStatus::Ok,
        detail: Some(wayland_display),
    });

    // -----------------------------------------------------------------------
    // Output
    // -----------------------------------------------------------------------
    if json {
        let arr: Vec<serde_json::Value> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "check": c.label,
                    "status": c.status.symbol(),
                    "detail": c.detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr).unwrap_or_default());
    } else {
        println!("nixclip doctor");
        println!("{}", "─".repeat(50));
        for check in &checks {
            let detail = check
                .detail
                .as_deref()
                .map(|d| format!("  {}", d))
                .unwrap_or_default();
            println!("[{:4}] {}{}", check.status.symbol(), check.label, detail);
        }
        println!();

        let has_errors = checks
            .iter()
            .any(|c| matches!(c.status, CheckStatus::Error));
        let has_warnings = checks
            .iter()
            .any(|c| matches!(c.status, CheckStatus::Warning));

        if has_errors {
            println!(
                "One or more checks failed. Run `journalctl --user -u nixclipd` for daemon logs."
            );
        } else if has_warnings {
            println!("Some warnings detected. nixclip may work with reduced functionality.");
        } else {
            println!("All checks passed.");
        }
    }

    Ok(())
}

async fn check_daemon(socket_path: &std::path::Path) -> Check {
    match tokio::net::UnixStream::connect(socket_path).await {
        Ok(_) => Check {
            label: "Daemon".into(),
            status: CheckStatus::Ok,
            detail: Some(format!("running ({})", socket_path.display())),
        },
        Err(e) => Check {
            label: "Daemon".into(),
            status: CheckStatus::Error,
            detail: Some(format!(
                "not running at {} — {e}\n       Start with: nixclipd",
                socket_path.display()
            )),
        },
    }
}

fn check_runtime_dir() -> Check {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) => {
            let path = PathBuf::from(&dir);
            if path.exists() {
                Check {
                    label: "XDG_RUNTIME_DIR".into(),
                    status: CheckStatus::Ok,
                    detail: Some(dir),
                }
            } else {
                Check {
                    label: "XDG_RUNTIME_DIR".into(),
                    status: CheckStatus::Warning,
                    detail: Some(format!("set to '{}' but path does not exist", dir)),
                }
            }
        }
        Err(_) => Check {
            label: "XDG_RUNTIME_DIR".into(),
            status: CheckStatus::Warning,
            detail: Some("not set; falling back to /tmp".into()),
        },
    }
}

fn check_gnome_version() -> Check {
    if !cfg!(target_os = "linux") {
        return Check {
            label: "GNOME version".into(),
            status: CheckStatus::Warning,
            detail: Some("GNOME version probing is only available on Linux".into()),
        };
    }

    let current_desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !current_desktop.to_ascii_lowercase().contains("gnome") {
        return Check {
            label: "GNOME version".into(),
            status: CheckStatus::Warning,
            detail: Some(format!(
                "XDG_CURRENT_DESKTOP='{}'; NixClip targets GNOME Wayland",
                if current_desktop.is_empty() {
                    "(not set)"
                } else {
                    current_desktop.as_str()
                }
            )),
        };
    }

    match command_stdout("gnome-shell", &["--version"]) {
        Ok(output) => match parse_gnome_shell_major(&output) {
            Some(version) if version >= 48 => Check {
                label: "GNOME version".into(),
                status: CheckStatus::Ok,
                detail: Some(format!("GNOME Shell {version} detected (full-support floor met)")),
            },
            Some(version) => Check {
                label: "GNOME version".into(),
                status: CheckStatus::Warning,
                detail: Some(format!(
                    "GNOME Shell {version} detected; GlobalShortcuts is expected to work best on GNOME 48+"
                )),
            },
            None => Check {
                label: "GNOME version".into(),
                status: CheckStatus::Warning,
                detail: Some(format!("could not parse `gnome-shell --version`: {output}")),
            },
        },
        Err(e) => Check {
            label: "GNOME version".into(),
            status: CheckStatus::Warning,
            detail: Some(format!("could not run `gnome-shell --version`: {e}")),
        },
    }
}

fn check_wayland_protocols() -> Check {
    if !cfg!(target_os = "linux") {
        return Check {
            label: "Wayland clipboard protocols".into(),
            status: CheckStatus::Warning,
            detail: Some("clipboard protocol probing is only available on Linux".into()),
        };
    }

    if std::env::var("XDG_SESSION_TYPE")
        .map(|value| value != "wayland")
        .unwrap_or(true)
    {
        return Check {
            label: "Wayland clipboard protocols".into(),
            status: CheckStatus::Warning,
            detail: Some("not running under a Wayland session".into()),
        };
    }

    match command_stdout("wayland-info", &[]) {
        Ok(output) => {
            let protocols = advertised_data_control_protocols(&output);
            if protocols.is_empty() {
                Check {
                    label: "Wayland clipboard protocols".into(),
                    status: CheckStatus::Error,
                    detail: Some(
                        "neither ext-data-control nor wlr-data-control is advertised by the compositor"
                            .into(),
                    ),
                }
            } else {
                Check {
                    label: "Wayland clipboard protocols".into(),
                    status: CheckStatus::Ok,
                    detail: Some(format!("advertised: {}", protocols.join(", "))),
                }
            }
        }
        Err(e) => Check {
            label: "Wayland clipboard protocols".into(),
            status: CheckStatus::Warning,
            detail: Some(format!(
                "could not run `wayland-info` to probe ext-data-control / wlr-data-control: {e}"
            )),
        },
    }
}

fn check_global_shortcuts_portal() -> Check {
    if !cfg!(target_os = "linux") {
        return Check {
            label: "GlobalShortcuts portal".into(),
            status: CheckStatus::Warning,
            detail: Some("portal probing is only available on Linux".into()),
        };
    }

    match command_stdout(
        "gdbus",
        &[
            "introspect",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
        ],
    ) {
        Ok(output) => {
            if output.contains("org.freedesktop.portal.GlobalShortcuts") {
                Check {
                    label: "GlobalShortcuts portal".into(),
                    status: CheckStatus::Ok,
                    detail: Some("org.freedesktop.portal.GlobalShortcuts is exported".into()),
                }
            } else {
                Check {
                    label: "GlobalShortcuts portal".into(),
                    status: CheckStatus::Warning,
                    detail: Some(
                        "org.freedesktop.portal.Desktop is reachable, but GlobalShortcuts was not found in introspection output"
                            .into(),
                    ),
                }
            }
        }
        Err(e) => Check {
            label: "GlobalShortcuts portal".into(),
            status: CheckStatus::Warning,
            detail: Some(format!(
                "could not introspect org.freedesktop.portal.Desktop with `gdbus`: {e}"
            )),
        },
    }
}

fn check_config_file() -> Check {
    let config_path = Config::config_path();
    if !config_path.exists() {
        return Check {
            label: "Config file".into(),
            status: CheckStatus::Ok,
            detail: Some(format!(
                "not found at {} (defaults will be used)",
                config_path.display()
            )),
        };
    }

    match Config::load(&config_path) {
        Ok(_) => Check {
            label: "Config file".into(),
            status: CheckStatus::Ok,
            detail: Some(format!("valid ({})", config_path.display())),
        },
        Err(e) => Check {
            label: "Config file".into(),
            status: CheckStatus::Error,
            detail: Some(format!("invalid TOML at {}: {e}", config_path.display())),
        },
    }
}

fn check_db_file() -> Check {
    let db_path = Config::db_path();
    if !db_path.exists() {
        return Check {
            label: "Database".into(),
            status: CheckStatus::Ok,
            detail: Some(format!(
                "not found at {} (will be created on first run)",
                db_path.display()
            )),
        };
    }

    match std::fs::File::open(&db_path) {
        Ok(_) => Check {
            label: "Database".into(),
            status: CheckStatus::Ok,
            detail: Some(format!("readable ({})", db_path.display())),
        },
        Err(e) => Check {
            label: "Database".into(),
            status: CheckStatus::Error,
            detail: Some(format!("cannot open {}: {e}", db_path.display())),
        },
    }
}

fn check_blob_dir() -> Check {
    let blob_dir = Config::blob_dir();
    if !blob_dir.exists() {
        return Check {
            label: "Blob directory".into(),
            status: CheckStatus::Ok,
            detail: Some(format!(
                "not found at {} (will be created on first run)",
                blob_dir.display()
            )),
        };
    }

    // Check we can read + write by inspecting metadata.
    match std::fs::metadata(&blob_dir) {
        Ok(meta) => {
            if meta.is_dir() {
                // Try to check write permission by attempting to create a temp entry.
                let test_path = blob_dir.join(".nixclip_write_test");
                match std::fs::write(&test_path, b"") {
                    Ok(_) => {
                        let _ = std::fs::remove_file(&test_path);
                        Check {
                            label: "Blob directory".into(),
                            status: CheckStatus::Ok,
                            detail: Some(format!("readable + writable ({})", blob_dir.display())),
                        }
                    }
                    Err(e) => Check {
                        label: "Blob directory".into(),
                        status: CheckStatus::Warning,
                        detail: Some(format!("not writable at {}: {e}", blob_dir.display())),
                    },
                }
            } else {
                Check {
                    label: "Blob directory".into(),
                    status: CheckStatus::Error,
                    detail: Some(format!(
                        "{} exists but is not a directory",
                        blob_dir.display()
                    )),
                }
            }
        }
        Err(e) => Check {
            label: "Blob directory".into(),
            status: CheckStatus::Error,
            detail: Some(format!("cannot stat {}: {e}", blob_dir.display())),
        },
    }
}

fn command_stdout(program: &str, args: &[&str]) -> std::io::Result<String> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stderr
        };
        Err(std::io::Error::other(detail))
    }
}

fn parse_gnome_shell_major(output: &str) -> Option<u32> {
    output
        .split_whitespace()
        .find_map(|token| token.split('.').next()?.parse::<u32>().ok())
}

fn advertised_data_control_protocols(output: &str) -> Vec<&'static str> {
    let mut protocols = Vec::new();

    if output.contains("ext_data_control_manager_v1") {
        protocols.push("ext-data-control");
    }

    if output.contains("zwlr_data_control_manager_v1") {
        protocols.push("wlr-data-control");
    }

    protocols
}

#[cfg(test)]
mod tests {
    use super::{advertised_data_control_protocols, parse_gnome_shell_major};

    #[test]
    fn parses_gnome_shell_major_version() {
        assert_eq!(parse_gnome_shell_major("GNOME Shell 48.1"), Some(48));
        assert_eq!(parse_gnome_shell_major("GNOME Shell 47"), Some(47));
    }

    #[test]
    fn ignores_unparseable_gnome_version_output() {
        assert_eq!(parse_gnome_shell_major("not gnome output"), None);
    }

    #[test]
    fn detects_data_control_protocols_from_wayland_info() {
        let output = "\
interface: 'ext_data_control_manager_v1'\n\
interface: 'zwlr_data_control_manager_v1'\n";
        assert_eq!(
            advertised_data_control_protocols(output),
            vec!["ext-data-control", "wlr-data-control"]
        );
    }

    #[test]
    fn returns_empty_when_no_supported_protocols_are_present() {
        assert!(advertised_data_control_protocols("wl_compositor").is_empty());
    }
}
