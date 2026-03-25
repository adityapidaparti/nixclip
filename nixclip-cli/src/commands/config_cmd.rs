//! `nixclip config` — view or modify daemon configuration.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum ConfigAction {
    Set { key: String, value: String },
}

pub async fn run(client: &mut IpcClient, action: Option<ConfigAction>, json: bool) -> Result<()> {
    match action {
        None => {
            let msg = ClientMessage::get_config();
            match client.request(&msg).await? {
                ServerMessage::ConfigValue { config, .. } => print_config(&config, json)?,
                ServerMessage::Error { message, .. } => daemon_error(message),
                other => unexpected_response(other),
            }
        }
        Some(ConfigAction::Set { key, value }) => {
            let patch = build_patch(&key, &value)?;
            let msg = ClientMessage::set_config(patch);

            match client.request(&msg).await? {
                ServerMessage::ConfigValue { config, .. } => {
                    if !json {
                        println!("Configuration updated.");
                    }
                    print_config(&config, json)?;
                }
                ServerMessage::Ok { .. } => {
                    println!("Configuration updated.");
                }
                ServerMessage::Error { message, .. } => daemon_error(message),
                other => unexpected_response(other),
            }
        }
    }

    Ok(())
}

fn print_config<T: Serialize>(config: &T, json: bool) -> nixclip_core::Result<()> {
    if json {
        let json_val = serde_json::to_string_pretty(config).map_err(|e| {
            nixclip_core::NixClipError::Serialization(format!("failed to serialize config: {e}"))
        })?;
        println!("{}", json_val);
    } else {
        let toml_val = toml::to_string_pretty(config).map_err(|e| {
            nixclip_core::NixClipError::Config(format!("failed to serialize config: {e}"))
        })?;
        println!("{}", toml_val);
    }

    Ok(())
}

fn build_patch(key: &str, value: &str) -> nixclip_core::Result<toml::Value> {
    let scalar = parse_scalar(value);

    if let Some((section, leaf)) = key.split_once('.') {
        let mut outer = toml::map::Map::new();
        let mut inner = toml::map::Map::new();
        inner.insert(leaf.to_string(), scalar);
        outer.insert(section.to_string(), toml::Value::Table(inner));
        Ok(toml::Value::Table(outer))
    } else {
        let mut table = toml::map::Map::new();
        table.insert(key.to_string(), scalar);
        Ok(toml::Value::Table(table))
    }
}

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
