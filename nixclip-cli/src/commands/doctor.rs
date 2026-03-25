//! `nixclip doctor` — run diagnostic checks and report system health.

use std::path::PathBuf;

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
    // 3. Config file validity
    // -----------------------------------------------------------------------
    checks.push(check_config_file());

    // -----------------------------------------------------------------------
    // 4. Database file
    // -----------------------------------------------------------------------
    checks.push(check_db_file());

    // -----------------------------------------------------------------------
    // 5. Blob directory permissions
    // -----------------------------------------------------------------------
    checks.push(check_blob_dir());

    // -----------------------------------------------------------------------
    // 6. System info (informational, always Ok)
    // -----------------------------------------------------------------------
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "(not set)".into());
    let wayland_display =
        std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "(not set)".into());

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
            println!("One or more checks failed. Run `journalctl --user -u nixclipd` for daemon logs.");
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
                        detail: Some(format!(
                            "not writable at {}: {e}",
                            blob_dir.display()
                        )),
                    },
                }
            } else {
                Check {
                    label: "Blob directory".into(),
                    status: CheckStatus::Error,
                    detail: Some(format!("{} exists but is not a directory", blob_dir.display())),
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
