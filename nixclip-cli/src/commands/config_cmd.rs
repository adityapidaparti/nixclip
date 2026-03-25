//! `nixclip config` — view or modify daemon configuration.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::ipc_client::IpcClient;

/// Possible sub-actions for the config command.
#[derive(Debug, Clone)]
pub enum ConfigAction {
    Set { key: String, value: String },
}

pub async fn run(
    client: &mut IpcClient,
    action: Option<ConfigAction>,
    json: bool,
) -> Result<()> {
    match action {
        None => {
            // Retrieve and display the current config.
            let msg = ClientMessage::get_config();
            match client.request(&msg).await? {
                ServerMessage::ConfigValue { config, .. } => {
                    if json {
                        // Serialize config to JSON.
                        let json_val = serde_json::to_string_pretty(&config).map_err(|e| {
                            nixclip_core::NixClipError::Serialization(format!(
                                "failed to serialize config: {e}"
                            ))
                        })?;
                        println!("{}", json_val);
                    } else {
                        // Serialize config to TOML for human-readable output.
                        let toml_val = toml::to_string_pretty(&config).map_err(|e| {
                            nixclip_core::NixClipError::Config(format!(
                                "failed to serialize config: {e}"
                            ))
                        })?;
                        println!("{}", toml_val);
                    }
                }
                ServerMessage::Error { message, .. } => {
                    eprintln!("Error from daemon: {}", message);
                    std::process::exit(1);
                }
                other => {
                    eprintln!("Unexpected response from daemon: {:?}", other);
                    std::process::exit(1);
                }
            }
        }
        Some(ConfigAction::Set { key, value }) => {
            // Build a minimal TOML patch from the key/value pair.
            // Keys may be dotted (e.g. "general.max_entries"), so we need to
            // construct a nested TOML table.
            let patch = build_patch(&key, &value)?;
            let msg = ClientMessage::set_config(patch);

            match client.request(&msg).await? {
                ServerMessage::ConfigValue { config, .. } => {
                    if json {
                        let json_val = serde_json::to_string_pretty(&config).map_err(|e| {
                            nixclip_core::NixClipError::Serialization(format!(
                                "failed to serialize config: {e}"
                            ))
                        })?;
                        println!("{}", json_val);
                    } else {
                        println!("Configuration updated.");
                        let toml_val = toml::to_string_pretty(&config).map_err(|e| {
                            nixclip_core::NixClipError::Config(format!(
                                "failed to serialize config: {e}"
                            ))
                        })?;
                        println!("{}", toml_val);
                    }
                }
                ServerMessage::Ok { .. } => {
                    println!("Configuration updated.");
                }
                ServerMessage::Error { message, .. } => {
                    eprintln!("Error from daemon: {}", message);
                    std::process::exit(1);
                }
                other => {
                    eprintln!("Unexpected response from daemon: {:?}", other);
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

/// Build a TOML patch `Value` from a dotted key string and a string value.
///
/// For example, `"general.max_entries"` and `"500"` becomes:
/// ```toml
/// [general]
/// max_entries = 500
/// ```
fn build_patch(key: &str, value: &str) -> nixclip_core::Result<toml::Value> {
    let parts: Vec<&str> = key.splitn(2, '.').collect();

    // Try to parse the value as different TOML scalar types.
    let scalar = parse_scalar(value);

    if parts.len() == 2 {
        // Nested key: build a table with the section.
        let mut inner = toml::map::Map::new();
        inner.insert(parts[1].to_string(), scalar);
        let mut outer = toml::map::Map::new();
        outer.insert(parts[0].to_string(), toml::Value::Table(inner));
        Ok(toml::Value::Table(outer))
    } else {
        // Top-level key.
        let mut table = toml::map::Map::new();
        table.insert(key.to_string(), scalar);
        Ok(toml::Value::Table(table))
    }
}

/// Parse a string into the most appropriate TOML scalar type.
fn parse_scalar(s: &str) -> toml::Value {
    if let Ok(i) = s.parse::<i64>() {
        return toml::Value::Integer(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return toml::Value::Float(f);
    }
    if s == "true" {
        return toml::Value::Boolean(true);
    }
    if s == "false" {
        return toml::Value::Boolean(false);
    }
    toml::Value::String(s.to_string())
}
