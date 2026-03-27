mod app;
mod ipc_client;
mod settings;
mod widgets;
mod window;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;
use libadwaita as adw;

fn plain_mode_from_args(args: &[std::ffi::OsString]) -> bool {
    args.iter().skip(1).any(|arg| arg == "--plain")
}

fn activation_token_from_args(args: &[std::ffi::OsString]) -> Option<String> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--activation-token" {
            return iter
                .next()
                .and_then(|value| value.to_str().map(str::to_owned));
        }
    }
    None
}

fn activation_token_from_env() -> Option<String> {
    std::env::var("NIXCLIP_ACTIVATION_TOKEN")
        .ok()
        .or_else(|| std::env::var("XDG_ACTIVATION_TOKEN").ok())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let application = adw::Application::builder()
        .application_id("com.nixclip.NixClip")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    application.add_main_option(
        "activation-token",
        0.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        "XDG activation token passed by the portal or daemon",
        Some("TOKEN"),
    );

    application.add_main_option(
        "plain",
        0.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Default paste action uses plain text instead of original format",
        None,
    );

    let state = Rc::new(RefCell::new(None));

    application.connect_activate({
        let state = state.clone();
        move |app| {
            let activation_token = activation_token_from_env();
            app::activate(app, &state, activation_token.as_deref(), false);
        }
    });

    application.connect_command_line({
        let state = state.clone();
        move |app, command_line| {
            let plain = command_line
                .options_dict()
                .contains("plain")
                || {
                    let args = command_line.arguments();
                    plain_mode_from_args(&args)
                };
            let activation_token = command_line
                .options_dict()
                .lookup::<String>("activation-token")
                .ok()
                .flatten()
                .or_else(|| {
                    let args = command_line.arguments();
                    activation_token_from_args(&args)
                })
                .or_else(activation_token_from_env);
            app::activate(app, &state, activation_token.as_deref(), plain);
            0
        }
    });

    application.run();
}
