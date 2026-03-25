use thiserror::Error;

#[derive(Debug, Error)]
pub enum NixClipError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("pipeline error: {0}")]
    Pipeline(String),

    #[error("Wayland error: {0}")]
    Wayland(String),

    #[error("image error: {0}")]
    Image(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}

pub type Result<T> = std::result::Result<T, NixClipError>;
