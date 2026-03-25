//! IPC client for communicating with the nixclipd daemon over a Unix socket.

use std::path::Path;

use serde::de::DeserializeOwned;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

use nixclip_core::ipc::{self, ClientMessage, ServerMessage};
use nixclip_core::Result;

pub struct IpcClient {
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
}

impl IpcClient {
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).await.map_err(|e| {
            nixclip_core::NixClipError::Ipc(format!(
                "Could not connect to nixclipd at {}: {e}\n\
                 Is the daemon running? Try: nixclip doctor",
                socket_path.display()
            ))
        })?;

        let (reader, writer) = stream.into_split();
        Ok(Self { reader, writer })
    }

    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        ipc::send_message(&mut self.writer, msg).await
    }

    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        ipc::recv_message(&mut self.reader).await
    }

    pub async fn request(&mut self, msg: &ClientMessage) -> Result<ServerMessage> {
        self.send(msg).await?;
        self.recv::<ServerMessage>().await
    }
}
