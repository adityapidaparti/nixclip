pub mod clear;
pub mod config_cmd;
pub mod delete;
pub mod doctor;
pub mod list;
pub mod paste;
pub mod pin;
pub mod search;
pub mod show;
pub mod stats;

use nixclip_core::ipc::ServerMessage;

pub(crate) fn daemon_error(message: String) -> ! {
    eprintln!("Error from daemon: {}", message);
    std::process::exit(1);
}

pub(crate) fn unexpected_response(response: ServerMessage) -> ! {
    eprintln!("Unexpected response from daemon: {:?}", response);
    std::process::exit(1);
}
